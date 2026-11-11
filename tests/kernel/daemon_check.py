#!/usr/bin/env python3
"""Judge a run of the daemon rig (tests/kernel/daemon-rig.sh).

    daemon_check.py OUT [--checks "a b c d e"] > summary.md

Reads OUT/results.tsv and compares every pin and every control with the
table EXPECTED below. Exit status 0 if they are equal, 1 if a pin or a
control differs from the table in either direction, 2 if the rig itself
failed: a line is missing, or a stage that judges nothing went wrong
(ERROR). A rig error is never a verdict on the server.

Changing what the rig catches means changing EXPECTED on purpose.
"""
import argparse
import os
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
    'pin_no_fua': ('a', 'PASS', SERVER, 'forced-unit-access writes are not advertised (§4.1)'),
    'pin_label_at_byte_0': ('a', 'PASS', KERNEL, 'the pool is on the bare device, its first label at byte 0 (§4.2)'),
    'pin_flush_counted': ('a', 'PASS', SERVER, 'flushes reach the server during zpool sync, by its own count (§4.2)'),
    'pin_unstored_zero_a': ('a', 'PASS', WAITING, 'no completion waits for the acknowledged position to be stored (status: unstored 0)'),

    'pin_c_max_request': ('c', 'PASS', SERVER,
                          'every device of the conformance suite takes requests of 1 MiB: without this, its '
                          'cases about a 1 MiB request and a discard across the device test only the block layer'),

    'pin_kill_under_load': ('b', 'PASS', SERVER,
                            'kill -9 with writes in flight: no application write fails, no vdev error, '
                            'a clean scrub, every file as the writer recorded it (§4.1, M1 exit)'),
    'pin_second_serve_refused': ('b', 'PASS', WAITING, 'a second serve exits with an error and disturbs nothing (§4.2)'),
    'pin_orderly_stop_keeps_device': ('b', 'PASS', WAITING,
                                      'after SIGTERM the device waits for a server and the pool carries on (§4.2); '
                                      'the stand-in stops by being killed'),
    'pin_serve_after_kill_accepts_log': ('b', 'PASS', WAITING,
                                         'no serve after a kill is refused by the check of what was acknowledged '
                                         '(§4.2): a false alarm would suspend a healthy pool'),
    'ctl_recovered_log_too_short': ('b', 'FAIL', WAITING,
                                    'control: a recovery that drops the last record (serve --recover-short 1) is '
                                    'refused with status 4 and the pool suspends', 'WAITING'),
    'pin_unstored_zero_b': ('b', 'PASS', WAITING, 'the same after the kills, read from the instance serving at the end'),
    'ctl_kill_without_reissue': ('b', 'FAIL', KERNEL,
                                 'control: recovery that fails the requests in flight must show in the same check'),

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
    'pin_failure_note_under_run': ('d', 'PASS', WAITING, 'the note of a failure is under /run by default (§4.2)'),
    'pin_failed_unheld_volume_serves_when_acknowledged': ('d', 'PASS', WAITING,
                                                          'a volume that failed with no client attached is served '
                                                          'again only when the operator acknowledges (§4.2)'),

    'pin_held_request_completes': ('e', 'PASS', SERVER,
                                   'a request held for minutes completes; the kernel neither times it out nor '
                                   'reports an error (§4.2, a pin for M3)'),
    'pin_unstored_zero_e': ('e', 'PASS', WAITING, 'the same after a request was held for minutes'),
    'ctl_hold_too_short': ('e', 'FAIL', SERVER, 'control: a request held 5 s is not accepted as one held for minutes'),
}
# Observations: recorded, not judged, but the run is incomplete without them.
OBSERVED = {
    'obs_device_back_without_clear': ('d', 'does zed bring the vdev back when the device reappears, with no zpool clear?'),
    'obs_returned_device_with_older_content': ('d', 'the device comes back holding less than ZFS was told'),
    'obs_scrub_in_progress_at_failure': ('d', 'what a scrub in progress records before the pool suspends'),
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


def read_results(path):
    """key -> value, the last line for a key winning."""
    out = {}
    with open(path, encoding='utf-8', errors='replace') as f:
        for line in f:
            key, _, value = line.rstrip('\n').partition('\t')
            if key:
                out[key] = value
    return out


def judge(results, checks, impl='reference'):
    """-> (rows, differs, rig_errors). A row is (name, expected, got, detail, basis, claim)."""
    rows, differs, errors = [], [], []
    for name, (check, want, basis, claim, *on_stand_in) in EXPECTED.items():
        if check not in checks:
            continue
        if impl == 'reference' and on_stand_in:
            want = on_stand_in[0]
        if impl != 'reference':
            if name in REFERENCE_ONLY:
                rows.append((name, want, 'not run', 'needs the reference server', KERNEL, claim))
                continue
            basis = 'measured on the daemon' if basis in (SERVER, WAITING) else basis
        value = results.get(name)
        if value is None:
            errors.append(f'{name}: no result')
            rows.append((name, want, 'missing', '', basis, claim))
            continue
        got, _, detail = value.partition(': ')
        if got == 'ERROR' or got not in ('PASS', 'FAIL', 'WAITING') or (got == 'WAITING' and want != 'WAITING'):
            errors.append(f'{name}: {value}')
        elif got != want:
            differs.append(f'{name}: expected {want}, got {got}: {detail}')
        rows.append((name, want, got, detail, basis, claim))
    for name, (check, _) in OBSERVED.items():
        if check in checks and name not in results:
            errors.append(f'{name}: no observation')
    if 'c' in checks:
        judge_cases(results, rows, differs, errors, impl)
    return rows, differs, errors


def judge_cases(results, rows, differs, errors, impl='reference'):
    """Check c: each case of the conformance suite against what the suite declares."""
    basis = SERVER if impl == 'reference' else 'measured on the daemon'
    run = results.get('c_run')
    if run is None or not run.startswith('ok'):
        errors.append(f"c_run: {run or 'the suite did not run'}")
    declared = results.get('c_declared')
    if declared is None:
        errors.append('c_declared: missing')
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


def render(results, rows, differs, errors, checks):
    out = [f"## Daemon rig: {results.get('impl', 'unknown server')}", '',
           f"Kernel {results.get('kernel', '?')}, {results.get('os', '?')}, ZFS {results.get('zfs', '?')}, "
           f"zed {results.get('zed', '?')}; {results.get('ublk_server', '')}. Checks run: {' '.join(sorted(checks))}.", '']
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
            out.append(f"- **{question}** {results.get(name, 'no observation')}")
    context = [k for k in results if not k.startswith(('pin_', 'ctl_', 'obs_', 'case_'))]
    out += ['', '<details><summary>Everything else the rig wrote down</summary>', '']
    out += [f'- `{k}`: {results[k]}' for k in context] + ['', '</details>', '']
    return '\n'.join(out)


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument('out')
    ap.add_argument('--checks', default='a b c d e')
    args = ap.parse_args(argv)
    checks = set(args.checks.split())
    path = os.path.join(args.out, 'results.tsv')
    try:
        results = read_results(path)
    except OSError as e:
        print(f'The rig left no results: {e}')
        return 2
    impl = 'reference' if results.get('impl', '').startswith('reference') else 'daemon'
    rows, differs, errors = judge(results, checks, impl)
    if results.get('setup') != 'ok':
        errors.insert(0, f"setup: {results.get('setup', 'did not finish')}")
    print(render(results, rows, differs, errors, checks))
    return 2 if errors else 1 if differs else 0


if __name__ == '__main__':
    sys.exit(main())
