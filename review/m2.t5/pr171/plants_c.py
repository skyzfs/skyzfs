#!/usr/bin/env python3
"""Plants for the signal rule of #171 at 3b9dc1c (run_rounds_then_term,
round_left, pin_ID_no_round_after_sigterm): each applied alone to a
worktree (argv[1]), its suite run under a 600 s limit, the file
restored with `git checkout`. Run with python3 -I. argv[2:]: plant ids
(all where none)."""
import subprocess
import sys

WT = sys.argv[1]
ONLY = set(sys.argv[2:])
PY = ['python3', '-I', 'tests/kernel/test_agent_check.py']
AR = ['bash', 'tests/kernel/test_agent_record.sh']
AC = 'tests/kernel/agent_check.py'
AS = 'tests/kernel/agent-record.sh'

PLANTS = [
    ('C01', 'signal after the N-th line whatever is left', AS,
     '        [ "$left" -lt 3000 ] && [ "$k" -lt $(($2 + 3)) ] || break\n', '        break\n', AR),
    ('C02', 'no cap on the rounds waited for', AS,
     '        [ "$left" -lt 3000 ] && [ "$k" -lt $(($2 + 3)) ] || break\n',
     '        [ "$left" -lt 3000 ] || break\n', AR),
    ('C03', 'cap of four more rounds', AS, '[ "$k" -lt $(($2 + 3)) ]', '[ "$k" -lt $(($2 + 4)) ]', AR),
    ('C04', 'signal even with no round with 3 s left', AS,
     '    if [ "$have" -ge "$2" ] && [ "$left" -ge 3000 ] && kill -0', '    if [ "$have" -ge "$2" ] && kill -0', AR),
    ('C05', 'no stamping after the signal', AS, '        await_rounds "$1" 1000000 60 || true\n', '', AR),
    ('C06', 'round_left ignores late_ms', AS, 'l = e * 1000 - v["took_ms"] - v["late_ms"]',
     'l = e * 1000 - v["took_ms"]', AR),
    ('C07', 'margin 1 s, not 3', AS, '        [ "$left" -lt 3000 ] && [ "$k"', '        [ "$left" -lt 1000 ] && [ "$k"', AR),
    ('C08', 'pin not judged', AC, "        if term and what == 'sigterm':\n", '        if False:\n', PY),
    ('C09', 'rounds_after: a line at most 100 s late passes', AC, 'late = [k for k, a in enumerate(arrivals, 1) if a > t]',
     'late = [k for k, a in enumerate(arrivals, 1) if a > t + 100]', PY),
    ('C10', 'rounds_after: a bad term-at taken as 0', AC,
     "        raise RigError(f'term-at: not a time: {term!r}') from None", '        t = 0.0', PY),
    ('C11', 'pin judged on the second signal too', AC, "        if term and what == 'sigterm':\n", '        if term:\n', PY),
]


def run(cmd):
    try:
        p = subprocess.run(cmd, cwd=WT, capture_output=True, text=True, timeout=600)
        return p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return 'timeout', ''


def failing(out):
    return [ln for ln in out.splitlines() if ln.startswith(('FAIL', 'ERROR:', 'AssertionError', 'FAILED'))][:3]


for pid, what, path, old, new, suite in PLANTS:
    if ONLY and pid not in ONLY:
        continue
    full = f'{WT}/{path}'
    with open(full) as f:
        text = f.read()
    n = text.count(old)
    if n != 1:
        print(f'{pid}\t{what}\tNOT APPLIED: the text occurs {n} times', flush=True)
        continue
    with open(full, 'w') as f:
        f.write(text.replace(old, new))
    try:
        rc, out = run(suite)
        passed = rc == 0 and ('all passed' in out if suite is AR else True)
        print(f'{pid}\t{what}\t{"SURVIVES" if passed else "killed"}\t{" | ".join(failing(out))[:400]}', flush=True)
    finally:
        subprocess.run(['git', 'checkout', '--', path], cwd=WT, check=True)
        d = subprocess.run(['git', 'status', '--porcelain'], cwd=WT, capture_output=True, text=True).stdout
        if d.strip():
            print(f'{pid}\tNOT RESTORED: {d}', flush=True)
            sys.exit(1)
