#!/usr/bin/env python3
"""Judge a run of the daemon rig (tests/kernel/daemon-rig.sh).

    daemon_check.py OUT [--checks "a b c d e"] > summary.md

Reads OUT/results.tsv and compares every pin and every control with the
table EXPECTED below. Exit status 0 if they are equal, 1 if a pin or a
control differs from the table in either direction, 2 if the rig itself
failed: a line is missing, a stage that judges nothing went wrong
(ERROR), the run names a check or wrote a pin this table does not know,
a pin was written twice with two results, an observation says it did
not run, or nothing was checked at all. A rig error is never a verdict
on the server.

When ZFS on the machine got stuck (the line `zfs_stuck`, M1.T32), the
rig ran no course after it, and a line missing then reads ERROR "not
run: ZFS on this machine was stuck since STEP". The lines written before
are judged as always: the mark changes no verdict and no exit status.

Changing what the rig catches means changing EXPECTED on purpose.
"""
import argparse
import os
import re
import sys

# Where a result comes from when the rig runs on the stand-in
# (skyzfsd-ref.sh on the reference ublk server).
KERNEL = 'measured: a property of the kernel and of ZFS, whichever server is behind the device'
SERVER = 'measured on the reference server; to be measured again on the daemon'
WAITING = 'waiting for the daemon: on the stand-in this checks the stand-in, and shows only that the check works'

# name -> (check, expected result, what it rests on with the stand-in, what the design says[, expected on the
# stand-in]). The last is WAITING for a check the stand-in cannot carry at all: the rig then writes WAITING
# with the reason, and with the daemon behind the device the expected result applies.
EXPECTED = {
    'pin_block_size': ('a', 'PASS', SERVER, 'logical and physical block size 4096 (§4.1)'),
    'pin_max_request': ('a', 'PASS', SERVER, 'the largest request is 1 MiB (§4.1)'),
    'pin_write_cache': ('a', 'PASS', SERVER, 'a volatile write cache, so that flushes reach the server (§4.2)'),
    'pin_discard': ('a', 'PASS', SERVER, 'discard is offered (§4.1)'),
    'pin_write_zeroes': ('a', 'PASS', SERVER, 'write_zeroes is offered (§4.1)'),
    'pin_discard_carried_out': ('a', 'PASS', SERVER, 'a discard issued to the device reaches the server (its own count)'),
    'pin_write_zeroes_carried_out': ('a', 'PASS', SERVER, 'a write of zeros issued to the device reaches the server as one'),
    'pin_volume_on_larger_blocks': ('a', 'PASS', WAITING,
                                    'on an XFS with 16 KiB blocks: create makes a volume whose write unit is the '
                                    'block and says so; a volume made on 4 KiB blocks and copied there is refused '
                                    'by serve with exit 1, the reason, and its files untouched; the volume made '
                                    'there serves a pool that takes a synced write and scrubs clean', 'WAITING'),
    'pin_no_fua': ('a', 'PASS', SERVER, 'forced-unit-access writes are not advertised (§4.1)'),
    'pin_label_at_byte_0': ('a', 'PASS', KERNEL, 'the pool is on the bare device, its first label at byte 0 (§4.2)'),
    'pin_flush_counted': ('a', 'PASS', SERVER, 'flushes reach the server during zpool sync, by its own count (§4.2)'),
    'pin_commands_failed_zero_a': ('a', 'PASS', WAITING,
                                   'no command of the queues ended with a result other than OK or the driver\'s abort at '
                                   'a stop: no commit or fetch the driver refused (status: commands_failed 0, M1.3aa)', 'WAITING'),

    'pin_overlapping_writes_apply_whole': ('a', 'PASS', SERVER,
                                           'two writes at one range in flight together apply whole: after both, '
                                           'the range is exactly what one of them wrote, in every round of a '
                                           'minute of rounds, with and without the commit delay (§4.1). A run '
                                           'in which the two calls overlapped in time in fewer than half of the '
                                           'rounds is a rig error'),

    'ctl_torn_overlapping_writes': ('a', 'FAIL', KERNEL,
                                    'control: a server that applies only the first half of a write that follows '
                                    'another of the same range is caught by the same load, in every run'),

    'pin_c_commands_failed_zero': ('c', 'PASS', WAITING,
                                   'in the request logs of both passes of the suite, every instance of every volume, '
                                   'the killed ones too: no command ended with a result other than OK or the driver\'s '
                                   'abort (M1.3aa). A build without the ended lines is a rig error', 'WAITING'),
    'pin_c_max_request': ('c', 'PASS', SERVER,
                          'every device of the conformance suite takes requests of 1 MiB: without this, its '
                          'cases about a 1 MiB request and a discard across the device test only the block layer'),

    'pin_c_kill_with_requests_in_flight': ('c', 'PASS', SERVER,
                                           'the kills of the conformance suite find requests in flight at the server: '
                                           'at a third or more of the kills of a live server a call is outstanding '
                                           'when it dies and is left to the kernel to issue again. Below that the '
                                           'kill cases test little more than a restart, and the run is a rig error'),
    'pin_c_kill_with_applied_unacknowledged': ('c', 'PASS', SERVER,
                                               'with a server that acknowledges a write some milliseconds after '
                                               'applying it, the suite\'s kills leave writes applied and not '
                                               'acknowledged, by the next server\'s own count at its take-over; the '
                                               'kernel issues them again and they are applied a second time. A run '
                                               'where the count is zero, or is not given, is a rig error'),
    'pin_c_delay_cases': ('c', 'PASS', SERVER,
                          'every case of the suite ends with that delay as it does without it'),
    'pin_failed_volume_while_a_request_is_held': ('d', 'PASS', WAITING,
                                                  'a volume that fails while the server holds a request (--hold '
                                                  'with --fail-log-after) still ends: serve exits with the failed '
                                                  'status within a stated time and its device is gone or dead. The '
                                                  'driver\'s delete of a live device waits while that device\'s '
                                                  'server holds a request (control rig, '
                                                  'obs_delete_while_the_server_holds_a_request), and the daemon\'s '
                                                  'failure path deletes its own live device before it stops its '
                                                  'queue', 'WAITING'),
    'pin_kill_under_load': ('b', 'PASS', SERVER,
                            'kill -9 with writes in flight: no application write fails, no vdev error, '
                            'a clean scrub, every file as the writer recorded it (§4.1, M1 exit). Writes '
                            'must have been in flight just before at least nine kills in ten, or the run '
                            'is a rig error: a kill with none tests a restart, not the reissue'),
    'pin_second_serve_refused': ('b', 'PASS', WAITING, 'a second serve exits with an error and disturbs nothing (§4.2)'),
    'pin_orderly_stop_keeps_device': ('b', 'PASS', WAITING,
                                      'after SIGTERM the device waits for a server within 5 s of serve\'s end, the '
                                      'daemon\'s limit for a take-over, and the pool carries on (§4.2); '
                                      'the stand-in stops by being killed'),
    'pin_detach_refused_while_served': ('b', 'PASS', WAITING,
                                        'detach while a server holds the volume exits 5 and changes nothing: the '
                                        'server, the device, the attachment record and the notes are as before (§4.2)'),
    'pin_detach_frees_number': ('b', 'PASS', WAITING,
                                'with the server stopped and the pool exported, detach exits 0 and the device '
                                'number is free again (§4.2)'),
    'pin_serve_on_zfs_refused': ('b', 'PASS', WAITING,
                                 'serve with the volume\'s directory on a ZFS dataset, and serve with the run '
                                 'directory on one, each exits with an error that says so, before any device '
                                 'exists (§4.2: the daemon keeps its files off any pool)', 'WAITING'),
    'pin_orderly_stop_under_load': ('b', 'PASS', WAITING,
                                    'SIGTERM while a writer writes and syncs: serve exits 0, the next serve of the '
                                    'boot finds nothing applied and not acknowledged, no write fails and every '
                                    'file the writer was told is written reads back (§4.2). A stop that did not '
                                    'find the writer at work is a rig error', 'WAITING'),
    'pin_detach_of_held_says_6': ('b', 'PASS', WAITING,
                                  'detach of a stopped device that a process holds open deletes it and exits 6: '
                                  'the number stays taken while it is held, and an add under it succeeds within '
                                  '5 s of the holder closing it (§4.2)', 'WAITING'),
    'pin_commands_failed_zero_b': ('b', 'PASS', WAITING, 'the same after the kills, read from the instance serving at the end',
                                   'WAITING'),
    'pin_no_ublksrvd_entry_from_daemon': ('b', 'PASS', WAITING,
                                          'the daemon causes no file under /run/ublksrvd, the reference server\'s '
                                          'directory: after each of its serves there is no entry for the device\'s '
                                          'number newer than that serve, under the reference server\'s names '
                                          '(N.pid, ublk_N.json) or libublk\'s (the number padded, 0000.json) '
                                          '(§4.2, Library)', 'WAITING'),
    'ctl_kill_without_reissue': ('b', 'FAIL', KERNEL,
                                 'control: recovery that fails the requests in flight must show in the same check'),
    'pin_kill_under_load_applied_unacknowledged': ('b', 'PASS', SERVER,
                                                   'kills under a pool\'s load of a server that acknowledges a write '
                                                   'some milliseconds after applying it: the pool comes through as in '
                                                   'the plain kill test, and the servers that took over found writes '
                                                   'applied and not acknowledged (status: recovered_unacknowledged), '
                                                   'more than 0 over the run, or the run is a rig error'),
    'pin_device_waits_for_a_server': ('b', 'PASS', KERNEL,
                                      'a killed server\'s device with no server for 300 s under a pool: it reads '
                                      'as waiting at every look (a look every 10 s; a look that names no state, '
                                      'or fewer looks than one per 12 s, is a rig error), the kernel log of that time has no request of '
                                      'the device ended in an error of any status, no I/O error, request timed '
                                      'out or abort (no kernel log of that time is a rig error), and the '
                                      'take-over afterwards is as clean as after any kill '
                                      '(§4.2, the states of a device: waiting, no server comes)'),
    'ctl_delay_before_apply': ('b', 'FAIL', SERVER,
                               'control: with the same delay taken before the write is applied, the servers that '
                               'take over find nothing applied and unacknowledged, over five kills that all '
                               'happened with writes in flight and left the pool whole: the count above, '
                               'made in the same place in both servers, is not one that is always positive'),

    'pin_suspends_on_io_errors': ('d', 'PASS', KERNEL,
                                  'every request fails with a plain I/O error: the pool suspends under '
                                  'failmode=wait and no application sees an error (§4.1)'),
    'ctl_volume_that_does_not_fail': ('d', 'FAIL', KERNEL, 'control: a volume that does not fail does not suspend its pool'),
    'pin_suspends_on_deleted_device': ('d', 'PASS', KERNEL,
                                       'the device is deleted under the pool: the same (§4.2)'),
    'pin_failed_exit_status': ('d', 'PASS', WAITING, 'serve exits with the documented status when the volume fails'),
    'pin_failed_held_volume_refused': ('d', 'PASS', WAITING,
                                       'a volume that failed while a client held it is refused in this boot, '
                                       'with and without --acknowledge-failure (§4.2)'),
    'pin_clear_resumes_on_returned_device': ('d', 'PASS', KERNEL,
                                             'zpool clear resumes the pool when a device that was kept through the '
                                             'failure answers again: the reason a failed volume is not served '
                                             'again (§4.2). A device deleted under the pool does not come back: '
                                             'see tests/kernel/failed-volume-obs.sh'),
    'pin_failure_note_under_run': ('d', 'PASS', WAITING,
                                   'the note of a failure is /run/skyzfsd/v-<dev>-<ino>/failed by default, the '
                                   'device and inode numbers of the volume\'s directory in hex (§4.2)'),
    'pin_failed_unheld_volume_serves_when_acknowledged': ('d', 'PASS', WAITING,
                                                          'a volume that failed with no client attached is served '
                                                          'again only when the operator acknowledges, and a write to the '
                                                          'device then reaches the server: its own count of writes goes '
                                                          'up (§4.2)'),

    'pin_held_request_completes': ('e', 'PASS', SERVER,
                                   'a request held for minutes completes, by the reader\'s own clock and by the '
                                   'server\'s; the kernel neither times it out nor reports an error (§4.2, a pin '
                                   'for M3)'),
    'pin_commands_failed_zero_e': ('e', 'PASS', WAITING, 'the same after a request was held for minutes', 'WAITING'),
    'ctl_hold_too_short': ('e', 'FAIL', SERVER, 'control: a request held 5 s is not accepted as one held for minutes'),
    # M1.T35: the driver refuses one commit of the daemon's (its test hook, #83). The stand-in has no hooks.
    'pin_refused_read_commit_taken_over': ('e', 'PASS', WAITING,
                                           'a read whose commit the driver refused: serve ends with 8 and its line, '
                                           'the device waits for a server and the read with it, and the next serve '
                                           'is handed the read again and completes it with what was written (§4.2)',
                                           'WAITING'),
    'pin_refused_write_commit_taken_over': ('e', 'PASS', WAITING,
                                            'the same for a write: after the take-over the client\'s write returns, '
                                            'reads back through the device, and is in the home image after the '
                                            'next stop (§4.2)', 'WAITING'),
    'pin_refused_flush_commit_taken_over': ('e', 'PASS', WAITING,
                                            'the same for a flush: the client\'s fsync returns after the take-over, '
                                            'and its write reads back and is in the home image (§4.2)', 'WAITING'),
    # M1.T37: the driver refuses tag 0's first fetch (the daemon's test hook, #87). The stand-in has no hooks.
    'pin_refused_fetch_at_a_start_gives_up_9': ('e', 'PASS', WAITING,
                                                'a first fetch the driver refuses at a start: serve gives the start '
                                                'up with 9 and its line, the device it added is deleted and its '
                                                'number free (a delete answers a START_DEV that waits), the volume '
                                                'is not failed, and the next serve starts and reads (§4.2)', 'WAITING'),
    'pin_refused_fetch_at_a_take_over_gives_up_9': ('e', 'PASS', WAITING,
                                                    'the same at a take-over, with a pool on the device and its write '
                                                    'and sync held: serve gives the end of the recovery up with 9 '
                                                    '(the process\'s end answers an END_USER_RECOVERY that waits), '
                                                    'the device is left QUIESCED with the I/O still held, the volume '
                                                    'is not failed, and the next serve takes it over: the held I/O '
                                                    'completes, the pool ONLINE with no vdev error (§4.2)', 'WAITING'),
    'pin_refused_commit_at_a_stop_ends_8': ('e', 'PASS', WAITING,
                                            'a held write whose commit the driver refuses when SIGTERM stops the '
                                            'server during the hold: the stop\'s last commits are checked, so serve '
                                            'ends with 8 and not 0, and the take-over completes the write. A build '
                                            'whose stop drops what its last flush reaps (before 343dd01) ends 0: the '
                                            'negative control, by dispatch', 'WAITING'),

    # Check g (M1.T23): the disk under the home image fails for real (dm-error under an ext4 without a
    # journal), with the daemon's in-place store. The stand-in has no rule of its own here: WAITING.
    'pin_start_serves_what_the_disk_holds': ('g', 'PASS', WAITING,
                                             'after a sync of the home image failed and left pages in the cache that '
                                             'read as written (the rig\'s own write and fdatasync, the error then taken), '
                                             'the next start serves what the disk holds, not the cache: "every start, '
                                             'after its sync, drops the kernel\'s cache of the volume\'s file and checks '
                                             'that it is gone, and serves only what the disk holds" (§4.1). That the '
                                             'kernel left such pages is the premise; without it the line is the rig\'s '
                                             'error', 'WAITING'),
    'ctl_g_old_content_after_good_sync': ('g', 'FAIL', WAITING,
                                          'control of the pin above: the same write with no error syncs and the disk '
                                          'holds it; the pin\'s test (the start serves the content from before the '
                                          'write) then fails, because the start serves the write', 'WAITING'),
    'pin_failed_start_fails_volume': ('g', 'PASS', WAITING,
                                      'a start whose sync of the home image fails, a client holding the device, exits '
                                      'with status 3 and says the volume failed at the start of the in-place store '
                                      '(§4.1, §4.2)', 'WAITING'),
    'pin_failed_start_suspends_pool': ('g', 'PASS', WAITING,
                                       'the pool of that client then suspends, with no read or write failed to an '
                                       'application (§4.1, §4.2)', 'WAITING'),
    'pin_failed_start_volume_refused': ('g', 'PASS', WAITING,
                                        'with the disk working again, a serve of that volume is refused with status 4 '
                                        'while the client holds the device (§4.1)', 'WAITING'),
    'pin_sync_failure_fails_volume': ('g', 'PASS', WAITING,
                                      'the disk under the home image fails while the volume is served and written: '
                                      'serve exits with status 3 and says the volume failed while it was served (§4.1), '
                                      'on a failed write or sync of the image (the rig writes to the device itself right '
                                      'after the disk fails). Still serving with a request held, or with the rig\'s write '
                                      'answered, is a FAIL; only a rig write that could not be issued is a rig error',
                                      'WAITING'),
    'pin_sync_failure_suspends_pool': ('g', 'PASS', WAITING,
                                       'the pool then suspends, with no write failed to an application (§4.1); a '
                                       'failed read fails its request alone (§5.1): obs_g_reads_while_failing',
                                       'WAITING'),
    'pin_sync_failed_volume_refused': ('g', 'PASS', WAITING,
                                       'with the disk working again, a serve of that volume is refused with status 4 '
                                       'while the client holds the device (§4.1)', 'WAITING'),
    'pin_failure_reason_is_the_images': ('g', 'PASS', WAITING,
                                         'the reason the daemon gives, in the failed serve\'s line and in the refusal '
                                         'after it, at a start (g.3) and while served (g.1), is the home image\'s: "a '
                                         'write or sync of the volume\'s image failed" (M1.3x)', 'WAITING'),
    'pin_data_journal_refused': ('g', 'PASS', WAITING,
                                 'on ext4 mounted data=journal, whose statx reports no direct-I/O alignment for a '
                                 'journalled file, serve exits 1 with the reason ("no direct I/O") and "the volume has '
                                 'not failed", no ready line; home.img and volume byte-identical, no failure marker '
                                 'appears or goes, and no other file changes but the status file (every changed file '
                                 'named); the same file system '
                                 'mounted data=ordered serves the volume before and after (M1.3y, g.4)', 'WAITING'),
}
# Observations: recorded, not judged, but the run is incomplete without
# them: a missing one, or one that says "not run", is a rig error.
OBSERVED = {
    'obs_c_evidence': ('c', 'what check c kept beside the tool\'s trace (M1.T28): the server\'s request log, the '
                            'kernel\'s block trace and the count of udev\'s change events of a ublk device for each '
                            'pass, kept in the artifact when a case failed; without a block trace the run is the '
                            'rig\'s error'),
    'obs_g_failed_sync': ('g', 'the premise of pin_start_serves_what_the_disk_holds: what fdatasync returned when '
                                'the disk failed under the range, what the cache then read, and what the disk held'),
    'obs_g_new_descriptor_after_failed_sync': ('g', 'after one descriptor\'s fdatasync took the write-back error, '
                                                     'does a newly opened descriptor see it: fdatasync, then syncfs?'),
    'obs_g_reads_while_failing': ('g', 'the disk under the home image failing while served: how many reads failed '
                                       'to ZFS, and how many an application saw before the volume failed (§5.1)'),
    'obs_g_data_journal_cache': ('g', 'g.4 (M1.T30): after each serve of the volume on ext4, what the page cache holds '
                                      'of the home image once 16 MiB were written to the device with O_DIRECT and '
                                      'synced (cachestat, in pages): none for a store that writes past the cache'),
    'obs_g_chattr_journal': ('g', 'g.4 (M1.T30): what serve does when the image alone journals its data (chattr +j '
                                  'on ext4 data=ordered), and if it serves, what the cache then holds of the image'),
    'b_dead_server_live_ms': ('b', "how long a killed server's device is still reported LIVE"),
    'obs_device_back_without_clear': ('d', 'does zed bring the vdev back when the device reappears, with no zpool clear?'),
    'obs_returned_device_with_older_content': ('d', 'the device comes back holding less than ZFS was told'),
    'obs_scrub_in_progress_at_failure': ('d', 'what a scrub in progress records before the pool suspends'),
    'obs_a_unordered_overlapping_writes': ('a', 'how often the reference server without the ordering tears the same load, '
                                                'on this runner: the pin on overlapping writes means as much as this shows'),
    'obs_b_takeover_ms': ('b', 'how long each take-over of the kill test took, in order, and the medians of '
                               'the first and the last five'),
    'obs_b_takeover_starts': ('b', 'each take-over of the kill test by the new server\'s own account: how long '
                                   'its start took and how much log it replayed'),
    'obs_c_checked_reads': ('c', 'the reads that found a record of the log not yet durable and checked its payload '
                                 '(status version 6), summed over the suite\'s volumes in its three passes; a mismatch on '
                                 'a healthy volume would be a defect. Each volume\'s count is its last server\'s, read '
                                 'when the volume is destroyed: a killed server\'s count is lost. "Not recorded" (no '
                                 'status read at the end) is not a rig error. Not judged until a run has shown the keys'),
    'obs_a_overlap_checked_reads': ('a', 'the same for the two volumes of the overlap test'),
    'obs_b_device_waits_for_a_server': ('b', 'a killed server\'s device left waiting 300 s under a pool, then taken over'),
    'obs_c_delay_kills': ('c', 'calls in flight at a death beside what the next server found applied and unacknowledged'),
    'obs_e_kernel_log': ('e', 'the kernel log while a request was held'),
}
# Check c. skyzfs-blkcheck says for each case what the suite declares for
# a harness with its capabilities; a case must end as declared. How many
# cases the suite declares each way, for a harness with no power cut, no
# base, no read-only volume and no invalid requests, that can make the
# log fail. The stand-in can also reopen a failed volume; the daemon
# refuses one that failed while a client held its device, as the suite's
# client does, so with it those cases are not exercised. A change in the
# suite or in what the harness can do changes these on purpose.
C_DECLARED = {'reference': 'PASS 1, WEAKER 15, NOT_EXERCISED 10',
              'daemon': 'PASS 1, WEAKER 13, NOT_EXERCISED 12'}
C_RESULTS = ('PASS', 'WEAKER', 'NOT_EXERCISED', 'FAIL')

# Pins that cannot be taken with the daemon behind the device. None now:
# the pin that brings a kept device back is always taken on the
# stand-in, whichever server is under test, because the daemon deletes
# its device when the volume fails.
REFERENCE_ONLY = set()

# Observations only the daemon can make: on the stand-in the rig writes
# no line for them and none is asked for. A line that says "not run"
# is the rig's error whatever the server.
DAEMON_ONLY_OBSERVED = {'obs_g_failed_sync', 'obs_g_new_descriptor_after_failed_sync',
                        'obs_g_reads_while_failing', 'obs_g_data_journal_cache', 'obs_g_chattr_journal'}


# f is asked for by name: it is not among the default of a run.
CHECKS = ('a', 'b', 'c', 'd', 'e', 'g')
DEFAULT_CHECKS = 'a b c d e g'
JUDGED = ('pin_', 'ctl_', 'case_')


def read_results(path):
    """-> (key -> value, the last line for a key winning; rig errors).

    A pin, a control or a case written twice with two results is a rig
    error: the last line would otherwise decide which one counts.
    """
    out, twice = {}, []
    with open(path, encoding='utf-8', errors='replace') as f:
        for line in f:
            key, _, value = line.rstrip('\n').partition('\t')
            if not key:
                continue
            if key.startswith(JUDGED) and key in out and out[key].partition(':')[0] != value.partition(':')[0]:
                twice.append(f"{key}: written twice, as {out[key].partition(':')[0]} and as {value.partition(':')[0]}")
            out[key] = value
    return out, twice


def parse_checks(text):
    """-> (set of checks, rig errors): names the rig knows, and at least one."""
    names = text.split()
    unknown = [n for n in names if n not in CHECKS]
    errors = [f"checks: '{n}' is not a check; the checks are {' '.join(CHECKS)}" for n in unknown]
    if not names:
        errors.append('checks: none was asked for, so nothing was checked')
    return {n for n in names if n in CHECKS}, errors


# An observation that says it was not made, wherever and however it
# says so ("Not run", "was not run"); "not running" is something else.
NOT_RUN = re.compile(r'\bnot run\b', re.IGNORECASE)


# Lines the stand-in carries and the daemon cannot: the daemon's store
# (in place, M1.3r; the only one since M1.3t) keeps no record of what it
# applied, so a new server has no count of writes applied and not
# acknowledged. On the daemon the rig writes NA and why, and NA is
# expected of them there and nowhere else. The pins that rested on the
# log store alone (its checkpoints, its bound, its recovery) went with
# it.
NOT_ON_THE_DAEMON = {
    'ctl_delay_before_apply': 'the control of the count of writes a new server found applied and not acknowledged: '
                              'no such record, so pin_kill_under_load_applied_unacknowledged is judged without the count',
    'pin_c_kill_with_applied_unacknowledged': 'as pin_kill_under_load_applied_unacknowledged, through the suite',
}


# Pins the daemon cannot carry yet: expected WAITING on the daemon too.
WAITS_FOR_THE_DAEMON = {
    'pin_failed_volume_while_a_request_is_held': 'M1.T8: the rig\'s code for it, once M1.3d says what the failure '
                                                 'path does while the pump holds a request',
}


def stuck_since(results):
    """'not run: ZFS on this machine was stuck since "STEP"' when the run has the line zfs_stuck, else None."""
    line = results.get('zfs_stuck')
    if line is None:
        return None
    m = re.match(r'since "([^"]*)"', line)
    return f'not run: ZFS on this machine was stuck since "{m.group(1) if m else line}"'


def judge(results, checks, impl='reference'):
    """-> (rows, differs, rig_errors). A row is (name, expected, got, detail, basis, claim)."""
    rows, differs, errors = [], [], []
    stuck = stuck_since(results)
    for name, (check, want, basis, claim, *on_stand_in) in EXPECTED.items():
        if check not in checks:
            continue
        if impl == 'reference' and on_stand_in:
            want = on_stand_in[0]
        if impl != 'reference' and name in WAITS_FOR_THE_DAEMON:
            want = 'WAITING'
        if impl != 'reference':
            if name in REFERENCE_ONLY:
                rows.append((name, want, 'not run', 'needs the reference server', KERNEL, claim))
                continue
            basis = 'measured on the daemon' if basis in (SERVER, WAITING) else basis
            if name in NOT_ON_THE_DAEMON:
                want, basis = 'NA', f"not taken with the daemon's store: {NOT_ON_THE_DAEMON[name]}"
        value = results.get(name)
        if value is None and stuck:
            errors.append(f'{name}: {stuck}')
            rows.append((name, want, 'ERROR', stuck, basis, claim))
            continue
        if value is None:
            errors.append(f'{name}: no result')
            rows.append((name, want, 'missing', '', basis, claim))
            continue
        got, _, detail = value.partition(': ')
        if (got == 'ERROR' or got not in ('PASS', 'FAIL', 'WAITING', 'NA') or (got == 'WAITING' and want != 'WAITING')
                or (got == 'NA' and want != 'NA')):
            errors.append(f'{name}: {value}')
        elif got != want:
            differs.append(f'{name}: expected {want}, got {got}: {detail}')
        rows.append((name, want, got, detail, basis, claim))
    for name, (check, _) in OBSERVED.items():
        if check not in checks:
            continue
        if name not in results and impl == 'reference' and name in DAEMON_ONLY_OBSERVED:
            continue
        if name not in results:
            errors.append(f'{name}: {stuck or "no observation"}')
        elif NOT_RUN.search(results[name]):
            errors.append(f'{name}: {results[name]}')
    # A pin or a control the table does not know is not judged by
    # anything: the rig and the table have come apart.
    for name in results:
        if name.startswith(('pin_', 'ctl_')) and name not in EXPECTED:
            errors.append(f'{name}: not in the table EXPECTED')
        # A line of a check that was not asked for should not exist, and
        # nothing above has looked at what it says.
        elif name.startswith(('pin_', 'ctl_')) and EXPECTED[name][0] not in checks:
            errors.append(f"{name}: a result of check {EXPECTED[name][0]}, which was not asked for: {results[name]}")
    if 'c' in checks:
        judge_cases(results, rows, differs, errors, impl, stuck)
    if not rows and not errors:
        errors.append('the run has no row to judge: nothing was checked')
    return rows, differs, errors


def tally(words):
    """'PASS 1, WEAKER 15, NOT_EXERCISED 10' from the declared word of each case."""
    return ', '.join(f'{w} {sum(1 for x in words if x == w)}' for w in C_RESULTS[:3])


def judge_cases(results, rows, differs, errors, impl='reference', stuck=None):
    """Check c: each case of the conformance suite against what the suite declares."""
    basis = SERVER if impl == 'reference' else f'measured on the {impl}'
    run = results.get('c_run')
    if run is None or not run.startswith('ok'):
        errors.append(f"c_run: {run or stuck or 'the suite did not run'}")
    declared = results.get('c_declared')
    if declared is None:
        errors.append(f"c_declared: {stuck or 'missing'}")
    elif declared != C_DECLARED[impl]:
        differs.append(f'the suite declares {declared}; the table C_DECLARED has {C_DECLARED[impl]}')
    cases = {k[len('case_'):]: v for k, v in results.items() if k.startswith('case_')}
    for name, value in cases.items():
        got, _, rest = value.partition(': declared ')
        want, _, detail = rest.partition(': ')
        if got not in C_RESULTS or want not in C_RESULTS[:3]:
            # UNSUPPORTED: the suite asked for what the harness said it cannot do.
            errors.append(f'case_{name}: {value}')
        elif got != want:
            differs.append(f'case_{name}: the suite declares {want}, got {got}: {detail}')
        rows.append((f'case_{name}', want, got, detail, basis, 'conformance suite of skyzfs-blk, through the kernel'))
    if run is not None and run.startswith('ok') and not cases:
        errors.append('check c ran and left no case')
    # The case lines must be the cases the suite declared, all of them:
    # a suite that stopped early, or a line lost, is not a shorter pass.
    wanted = [v.partition(': declared ')[2].partition(': ')[0] for v in cases.values()]
    if declared is not None and cases and tally(wanted) != declared:
        errors.append(f'check c left {len(cases)} cases declared {tally(wanted)}; the suite declared {declared}')


def render(results, rows, differs, errors, checks):
    out = [f"## Daemon rig: {results.get('impl', 'unknown server')}", '',
           f"Kernel {results.get('kernel', '?')}, {results.get('os', '?')}, ZFS {results.get('zfs', '?')}, "
           f"zed {results.get('zed', '?')}; {results.get('ublk_server', '')}. Checks run: {' '.join(sorted(checks))}.", '']
    if 'zfs_stuck' in results:
        out += [f"**ZFS on this machine got stuck**, and the courses after it were not run: {results['zfs_stuck']}", '']
    if errors:
        out += ['**The rig failed. This is not a verdict on the server.**', ''] + [f'- {e}' for e in errors] + ['']
    if differs:
        out += ['**Results differ from the table `EXPECTED`:**', ''] + [f'- {d}' for d in differs] + ['']
    if not errors and not differs:
        out += ['Every pin and every control is as the table `EXPECTED` in `tests/kernel/daemon_check.py` has it.', '']
    out += ['| | expected | got | what was seen | rests on | the claim |', '|---|---|---|---|---|---|']
    for name, want, got, detail, basis, claim in rows:
        mark = '' if got == want else ' **!**'
        out.append(f"| `{name}` | {want} | {got}{mark} | {detail.replace('|', '/')} | {basis} | {claim} |")
    out += ['', '### Observations (not judged)', '']
    for name, (check, question) in OBSERVED.items():
        if check in checks:
            none = 'made only with the daemon' if name in DAEMON_ONLY_OBSERVED else 'no observation'
            out.append(f"- **{question}** {results.get(name, none)}")
    context = [k for k in results if not k.startswith(('pin_', 'ctl_', 'obs_', 'case_'))]
    out += ['', '<details><summary>Everything else the rig wrote down</summary>', '']
    out += [f'- `{k}`: {results[k]}' for k in context] + ['', '</details>', '']
    return '\n'.join(out)


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument('out')
    ap.add_argument('--checks', default=DEFAULT_CHECKS)
    args = ap.parse_args(argv)
    checks, asked_wrong = parse_checks(args.checks)
    path = os.path.join(args.out, 'results.tsv')
    try:
        results, twice = read_results(path)
    except OSError as e:
        print(f'The rig left no results: {e}')
        return 2
    impl = 'reference' if results.get('impl', '').startswith('reference') else 'daemon'
    rows, differs, errors = judge(results, checks, impl)
    errors = asked_wrong + twice + errors
    # A regular file under a ublk device's name in /dev: the rig (or a
    # tool it ran) created a device's path, and writes "to the device"
    # after that went to the file. Whatever the pins say, the run is not
    # to be trusted. The line must be there, and say none.
    if results.get('setup') == 'ok' and results.get('dev_strays') != 'none':
        errors.append(f"dev_strays: {results.get('dev_strays', 'no result')}")
    if results.get('setup') != 'ok':
        errors.insert(0, f"setup: {results.get('setup', 'did not finish')}")
    print(render(results, rows, differs, errors, checks))
    return 2 if errors else 1 if differs else 0


if __name__ == '__main__':
    sys.exit(main())
