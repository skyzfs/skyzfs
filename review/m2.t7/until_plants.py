#!/usr/bin/env python3
"""Plants on d169139's status_until and its use in rec_pchannel (the
review of #139). Each plant edits a copy of tests/ (never the worktree)
and runs the pull request's tests/kernel/test_cut_record.sh there:
KILLED (it failed), SURVIVED, or NOT APPLIED.
Usage: until_plants.py TESTS_DIR SCRATCH_DIR
"""
import os
import shutil
import subprocess
import sys

PLANTS = [
    ('U1', 'pchannel reads the status once, as before',
     '    status_until channel-status.txt control_peer 1 10 || true\n',
     '    ustatus channel-status.txt\n'),
    ('U2', 'status_until returns 0 at once, whatever the count',
     '    while :; do\n        ustatus "$1"; v=$(skey "$1" "$2")\n        [[ $v =~ ^[0-9]+$ ]] && [ "$v" -ge "$3" ] && return 0\n',
     '    while :; do\n        ustatus "$1"; v=$(skey "$1" "$2")\n        return 0\n'),
    ('U3', 'status_until wants more than N (gt for ge)',
     '    while :; do\n        ustatus "$1"; v=$(skey "$1" "$2")\n        [[ $v =~ ^[0-9]+$ ]] && [ "$v" -ge "$3" ] && return 0\n',
     '    while :; do\n        ustatus "$1"; v=$(skey "$1" "$2")\n        [[ $v =~ ^[0-9]+$ ]] && [ "$v" -gt "$3" ] && return 0\n'),
    ('U4', 'status_until never gives up (no limit)',
     '        [ $((SECONDS - t0)) -lt "$4" ] || break\n        sleep 0.2\n    done\n    log "$NAME: the status\'s $2 was ${v:-nothing}, not at least $3, $4 s on"\n    return 1\n}\n# The volume',
     '        sleep 0.2\n    done\n    log "$NAME: the status\'s $2 was ${v:-nothing}, not at least $3, $4 s on"\n    return 1\n}\n# The volume'),
    ('U5', 'pchannel waits for control_malformed instead of control_peer',
     '    status_until channel-status.txt control_peer 1 10 || true\n',
     '    status_until channel-status.txt control_malformed 2 10 || true\n'),
]


def main():
    tests, scratch = sys.argv[1], sys.argv[2]
    src_path = os.path.join(tests, 'kernel', 'cut-record.sh')
    src = open(src_path).read()
    for pid, what, old, new in PLANTS:
        n = src.count(old)
        if n != 1:
            print(f'{pid}: NOT APPLIED ({n} matches): {what}')
            continue
        d = os.path.join(scratch, f'until-{pid}')
        shutil.rmtree(d, ignore_errors=True)
        shutil.copytree(tests, os.path.join(d, 'tests'), ignore=shutil.ignore_patterns('__pycache__'))
        with open(os.path.join(d, 'tests', 'kernel', 'cut-record.sh'), 'w') as f:
            f.write(src.replace(old, new))
        try:
            p = subprocess.run(['bash', os.path.join(d, 'tests', 'kernel', 'test_cut_record.sh')], cwd=d,
                               capture_output=True, text=True, timeout=120)
            failed = [ln for ln in p.stdout.splitlines() if ln.startswith('FAIL')]
            res = 'KILLED' if p.returncode else 'SURVIVED'
        except subprocess.TimeoutExpired:
            failed, res = ['the test did not end in 120 s'], 'KILLED (timeout)'
        print(f'{pid}: {res} {[f[:90] for f in failed]}: {what}')
        shutil.rmtree(d, ignore_errors=True)


if __name__ == '__main__':
    main()
