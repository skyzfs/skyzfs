#!/usr/bin/env python3
"""#113 at ae36c2f, the next look: the plants of plants.py again on the
new head (those whose text is gone say NOT APPLIED), and new ones in what
the three commits since 98024d5 changed, one at a time, against the four
unit test files and the new shell test (tests/kernel/test_cut_record.sh):

  M1-M6  numbers_verdict: a new state counts from 1 again (f5afb60)
  Q1-Q3  the status's count, held to the last state's cuts (f5afb60)
  W1-W8  cut-record.sh: the holder's end goes through holder_ended (e805d10)
  X1-X6  an old state's cut written after the failure is a verdict, and
         the instances' lines come first (ae36c2f)

The file is restored after each, and each result is written to OUT as it
comes.

    next_plants.py CHECKOUT OUT [PREFIX ...]
"""
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plants  # noqa: E402  (reads CHECKOUT, OUT and the prefixes from sys.argv)

ROOT, OUT, ONLY = plants.ROOT, plants.OUT, plants.ONLY
J, H = plants.J, plants.H
C = 'tests/kernel/cut-record.sh'
SHELL_TEST = 'tests/kernel/test_cut_record.sh'

FIRST = ("            if n != '1':\n"
         "                return 'FAIL', (f\"state {e + 1}'s first cut line (request log line {dcuts[k]['line']}) \"\n"
         "                                f\"carries the number {n!r}, not 1: a new state counts from 1 again\")\n"
         "            k0 = k\n"
         "            said.append(f'state {e + 1} counts from 1 again')\n")
SINCE = "        since = starts[-1]['i'] if starts else None\n"
STALE = ("        if n is None and any((b, str(w['number'])) in place for b in range(e)):\n"
         "            stale.append((w['number'], e + 1))\n"
         "            continue\n")
ORDER = ("            rc = max(rc, instance_lines(put, rdir, instances, dcuts, records, begun))\n"
         "            if name == 'failed':\n"
         "                rc = max(rc, failure_line(put, rdir))\n"
         "            v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)\n"
         "            rc, errored = max(rc, v), errored or e\n")
VERDICT = ("    if [ \"$rc\" = 1 ] && grep -q \"$(printf '\\tVERDICT ')\" \"$RDIR/holder.log\"; then\n")

NEW = [
    # ---- numbers_verdict at f5afb60: a new state counts from 1 again ----
    ('M1 a new state may count on, as before the commit (from 1 again or on, the line says which)', J,
     FIRST, "            if n == '1':\n                k0 = k\n                said.append(f'state {e + 1} counts from 1 again')\n"
            "            else:\n                said.append(f'state {e + 1} counts on')\n"),
    ('M2 a new state must count on (from 1 again is refused)', J,
     FIRST, "            if n == '1':\n"
            "                return 'FAIL', f\"state {e + 1}'s first cut line carries the number 1\"\n"
            "            said.append(f'state {e + 1} counts on')\n"),
    ('M3 the new state\'s own refusal is gone (the line then fails by the rule for every line, in its words)', J,
     "            if n != '1':\n                return 'FAIL', (f\"state", "            if False:\n                return 'FAIL', (f\"state"),
    ('M4 the count is not begun again at a new state (its first line must be 1, its later lines go on)', J,
     "            k0 = k\n            said.append(f'state {e + 1} counts from 1 again')\n",
     "            said.append(f'state {e + 1} counts from 1 again')\n"),
    ('M5 a new state whose first line is not 1 passes', J,
     "            if n != '1':\n                return 'FAIL', (f\"state", "            if n != '1':\n                return 'PASS', (f\"state"),
    ('M6 a new state is not seen (every line is of the first state)', J,
     "        if e != epoch:\n            epoch = e\n            if n != '1':", "        if False:\n            epoch = e\n            if n != '1':"),
    # ---- the status's count at f5afb60 ----
    ('Q1 the status is held to the whole log\'s count', J, SINCE, "        since = None\n"),
    ('Q2 the status is held to the cuts since the first new state, not the last', J,
     SINCE, "        since = starts[0]['i'] if starts else None\n"),
    ('Q3 the status is held to the last state\'s cuts only where its first line carries 1, as before the commit', J,
     SINCE, "        since = None\n        if starts:\n            first = [d for d in dcuts if d['epoch'] == len(starts)]\n"
            "            if first and first[0]['local'] == '1':\n                since = starts[-1]['i']\n"),
    # ---- cut-record.sh at e805d10: the holder's end ----
    ('W1 the bare wait is back in end_daemon (D1 as it was)', C,
     "        holder_ended \"at the stop\"\n", "        wait \"$HOLD_PID\" 2>/dev/null; HOLD_PID=\"\"\n"),
    ('W2 the bare wait is back inside holder_ended', C,
     "    wait \"$HOLD_PID\" 2>/dev/null || rc=$?\n", "    wait \"$HOLD_PID\" 2>/dev/null; rc=$?\n"),
    ('W3 the holder\'s 1 is a verdict without a VERDICT line in its log', C,
     VERDICT, "    if [ \"$rc\" = 1 ]; then\n"),
    ('W4 a VERDICT line is a verdict whatever the holder\'s status', C,
     VERDICT, "    if grep -q \"$(printf '\\tVERDICT ')\" \"$RDIR/holder.log\"; then\n"),
    ('W5 a verdict is the rig\'s error (the recording does not go on)', C,
     "        return 0\n    fi\n    [ \"$rc\" = 0 ] || rig_error \"the holder ended with $rc $1:",
     "        :\n    fi\n    [ \"$rc\" = 0 ] || rig_error \"the holder ended with $rc $1:"),
    ('W6 the holder\'s own error (2) is let through', C,
     "    [ \"$rc\" = 0 ] || rig_error \"the holder ended with $rc $1:", "    [ \"$rc\" != 3 ] || rig_error \"the holder ended with $rc $1:"),
    ('W7 the wait after the kill is bare again in end_daemon', C,
     "wait \"$HOLD_PID\" 2>/dev/null || true; HOLD_PID=\"\"\n            rig_error \"the holder did not end 720 s",
     "wait \"$HOLD_PID\" 2>/dev/null; HOLD_PID=\"\"\n            rig_error \"the holder did not end 720 s"),
    ('W8 holder_leave takes the holder\'s status itself, any status', C,
     "    holder_ended \"when asked to leave\"\n", "    wait \"$HOLD_PID\" 2>/dev/null || true; HOLD_PID=\"\"\n"),
    # ---- cut_daemon_check.py at ae36c2f: an old state's cut held after the failure ----
    ('X1 the branch is gone: such a write is the rig\'s error again (D2 as it was)', J, STALE, ""),
    ('X2 a written cut with no line in any state is a verdict too', J,
     "        if n is None and any((b, str(w['number'])) in place for b in range(e)):\n", "        if n is None:\n"),
    ('X3 held_lines does not look at such writes (the content is judged without them)', J,
     "        if stale:\n            # The content after", "        if False:\n            # The content after"),
    ('X4 the line fails and is no verdict (rc 0)', J,
     "                'still held after it, though that start trusts none of the cut state')\n            return 1, errored\n",
     "                'still held after it, though that start trusts none of the cut state')\n            return 0, errored\n"),
    ('X5 the held cuts are judged before the instances\' lines again', J,
     ORDER,
     "            v, e = held_lines(put, rdir, instances, records, dcuts, status, dev, name, begun)\n"
     "            rc, errored = max(rc, v), errored or e\n"
     "            rc = max(rc, instance_lines(put, rdir, instances, dcuts, records, begun))\n"
     "            if name == 'failed':\n"
     "                rc = max(rc, failure_line(put, rdir))\n"),
    ('X6 a cut of this state with a line counts as an old state\'s where an earlier state has the number too', J,
     "        if n is None and any((b, str(w['number'])) in place for b in range(e)):\n",
     "        if any((b, str(w['number'])) in place for b in range(e)):\n"),
]


def run_tests(env):
    """-> the names of what failed, [] when every test passes."""
    failed = []
    for t in plants.TESTS:
        try:
            p = subprocess.run([sys.executable, '-B', t], cwd=ROOT, env=env, capture_output=True, text=True,
                               timeout=300)
        except subprocess.TimeoutExpired:
            failed.append(f'{os.path.basename(t)} did not end in 300 s')
            continue
        failed += sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL: ', 'ERROR: '))})
        if p.returncode != 0 and not any(ln.startswith(('FAIL: ', 'ERROR: ')) for ln in p.stderr.splitlines()):
            failed.append(f'{os.path.basename(t)} exit {p.returncode}')
    if os.path.exists(os.path.join(ROOT, SHELL_TEST)):
        try:
            p = subprocess.run(['bash', SHELL_TEST], cwd=ROOT, env=env, capture_output=True, text=True, timeout=300)
            bad = [ln for ln in p.stdout.splitlines() if ln.startswith('FAIL ')]
            failed += [f'test_cut_record.sh: {ln[5:].split(":")[0]}' for ln in bad]
            if p.returncode != 0 and not bad:
                failed.append(f'test_cut_record.sh exit {p.returncode}')
        except subprocess.TimeoutExpired:
            failed.append('test_cut_record.sh did not end in 300 s')
    return failed


def main():
    cache = {}
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    with open(OUT, 'a') as out:
        def say(text):
            print(text, flush=True)
            out.write(text + '\n')
            out.flush()
            os.fsync(out.fileno())
        for name, path, *pairs in plants.PLANTS + NEW:
            if ONLY and not name.startswith(ONLY):
                continue
            full = os.path.join(ROOT, path)
            if full not in cache:
                with open(full) as f:
                    cache[full] = f.read()
            orig = cache[full]
            counts = [orig.count(old) for old in pairs[0::2]]
            if any(c != 1 for c in counts) or any(old == new for old, new in zip(pairs[0::2], pairs[1::2])):
                say(f'{name}: NOT APPLIED (found {counts} times)')
                continue
            text = orig
            for old, new in zip(pairs[0::2], pairs[1::2]):
                text = text.replace(old, new)
            try:
                with open(full, 'w') as f:
                    f.write(text)
                failed = run_tests(env)
            finally:
                with open(full, 'w') as f:
                    f.write(orig)
            say(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:6])}' if failed else 'SURVIVED'))


if __name__ == '__main__':
    main()
