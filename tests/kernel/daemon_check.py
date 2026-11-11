#!/usr/bin/env python3
"""Judge a run of the daemon rig (tests/kernel/daemon-rig.sh).

    daemon_check.py OUT [--checks "a b d e"] > summary.md

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

# name -> (check, expected result, what it rests on with the stand-in, what the design says)
EXPECTED = {
    'pin_block_size': ('a', 'PASS', SERVER, 'logical and physical block size 4096 (§4.1)'),
    'pin_max_request': ('a', 'PASS', SERVER, 'the largest request is 1 MiB (§4.1)'),
    'pin_write_cache': ('a', 'PASS', SERVER, 'a volatile write cache, so that flushes reach the server (§4.2)'),
    'pin_discard': ('a', 'PASS', SERVER, 'discard is offered (§4.1)'),
    'pin_write_zeroes': ('a', 'PASS', SERVER, 'write_zeroes is offered (§4.1)'),
    'pin_no_fua': ('a', 'PASS', SERVER, 'forced-unit-access writes are not advertised (§4.1)'),
    'pin_label_at_byte_0': ('a', 'PASS', KERNEL, 'the pool is on the bare device, its first label at byte 0 (§4.2)'),
    'pin_flush_counted': ('a', 'PASS', SERVER, 'flushes reach the server during zpool sync, by its own count (§4.2)'),

    'pin_kill_under_load': ('b', 'PASS', SERVER,
                            'kill -9 with writes in flight: no application write fails, no vdev error, '
                            'a clean scrub, every file as the writer recorded it (§4.1, M1 exit)'),
    'pin_second_serve_refused': ('b', 'PASS', WAITING, 'a second serve exits with an error and disturbs nothing (§4.2)'),
    'pin_orderly_stop_keeps_device': ('b', 'PASS', WAITING,
                                      'after SIGTERM the device waits for a server and the pool carries on (§4.2); '
                                      'the stand-in stops by being killed'),
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
                                             'zpool clear resumes the pool on a device that came back under the '
                                             'old name: the reason a failed volume is not served again (§4.2)'),
    'pin_failure_note_under_run': ('d', 'PASS', WAITING, 'the note of a failure is under /run by default (§4.2)'),
    'pin_failed_unheld_volume_serves_when_acknowledged': ('d', 'PASS', WAITING,
                                                          'a volume that failed with no client attached is served '
                                                          'again only when the operator acknowledges (§4.2)'),

    'pin_held_request_completes': ('e', 'PASS', SERVER,
                                   'a request held for minutes completes; the kernel neither times it out nor '
                                   'reports an error (§4.2, a pin for M3)'),
    'ctl_hold_too_short': ('e', 'FAIL', SERVER, 'control: a request held 5 s is not accepted as one held for minutes'),
}
# Observations: recorded, not judged, but the run is incomplete without them.
OBSERVED = {
    'obs_device_back_without_clear': ('d', 'does zed bring the vdev back when the device reappears, with no zpool clear?'),
    'obs_returned_device_with_older_content': ('d', 'the device comes back holding less than ZFS was told'),
    'obs_scrub_in_progress_at_failure': ('d', 'what a scrub in progress records before the pool suspends'),
    'obs_e_kernel_log': ('e', 'the kernel log while a request was held'),
}
# With the daemon behind the device these cannot be taken: they bring a
# device back from the reference server's flat image.
REFERENCE_ONLY = {'pin_clear_resumes_on_returned_device'}


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
    for name, (check, want, basis, claim) in EXPECTED.items():
        if check not in checks:
            continue
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
        if got == 'ERROR' or got not in ('PASS', 'FAIL'):
            errors.append(f'{name}: {value}')
        elif got != want:
            differs.append(f'{name}: expected {want}, got {got}: {detail}')
        rows.append((name, want, got, detail, basis, claim))
    for name, (check, _) in OBSERVED.items():
        if check in checks and name not in results:
            errors.append(f'{name}: no observation')
    return rows, differs, errors


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
    context = [k for k in results if not k.startswith(('pin_', 'ctl_', 'obs_'))]
    out += ['', '<details><summary>Everything else the rig wrote down</summary>', '']
    out += [f'- `{k}`: {results[k]}' for k in context] + ['', '</details>', '']
    return '\n'.join(out)


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument('out')
    ap.add_argument('--checks', default='a b d e')
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
