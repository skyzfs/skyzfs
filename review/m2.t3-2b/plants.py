#!/usr/bin/env python3
"""#111 at 8210332: plants in the reference (tests/kernel/cut_ref.py), the judge
(tests/kernel/cut_daemon_check.py), the holder (tests/kernel/cut_hold.py) and the bench's
releaser line (tests/bench/topology-bench.sh), one at a time, against their unit tests. The file is restored after each.

    plants.py CHECKOUT [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
ONLY = tuple(sys.argv[2:])
R = 'tests/kernel/cut_ref.py'
J = 'tests/kernel/cut_daemon_check.py'
H = 'tests/kernel/cut_hold.py'
B = 'tests/bench/topology-bench.sh'
TESTS = ['tests/kernel/test_cut_ref.py', 'tests/kernel/test_cut_daemon_check.py',
         'tests/kernel/test_cut_daemon_main.py', 'tests/kernel/test_cut_hold.py', 'tests/kernel/test_cut_check.py']
SH_TESTS = ['tests/lib/test_skz_store.sh']
INST = ("            keep = {'mark-dropped': False, 'mark-kept': True}.get(rule, r['taken_over'])\n"
        "            if not keep:\n                stored = None\n            mark = stored\n")

PLANTS = [
    # ---- the reference: the stored mark (question 1) ----
    ('R1 a flush received before the instance ended still carries its mark to a barrier', R,
     "        if 'instance' in flags:\n            carried = {}\n", "        if 'instance' in flags:\n"),
    ('R2 a take-over drops the mark (the design rule is mark-dropped)', R,
     INST, INST.replace("r['taken_over'])", "False)")),
    ('R3 a created device keeps the mark (the design rule is mark-kept)', R,
     INST, INST.replace("r['taken_over'])", "True)")),
    ('R4 the rule mark-dropped is the design', R,
     INST, INST.replace("'mark-dropped': False, ", "")),
    ('R5 the rule mark-kept is the design', R,
     INST, INST.replace(", 'mark-kept': True", "")),
    ('R6 a cut does not clear the stored mark', R,
     "            if stored is not None and (stored['txg'], stored['rootbp']) == newest:\n                stored = None\n", ""),
    ('R7 any cut clears the stored mark, whatever state it holds', R,
     "            if stored is not None and (stored['txg'], stored['rootbp']) == newest:\n", "            if stored is not None:\n"),
    ('R8 a mark is not stored', R,
     "                stored = mark\n            taken.append(u)", "            taken.append(u)"),
    ('R9 at a take-over the instance keeps the mark it had in memory, not the state\'s', R,
     INST, INST.replace("            mark = stored\n", "            mark = stored if not keep else mark\n")),
    ('R10 a created device forgets the newest cut as well', R,
     INST, INST.replace("                stored = None\n", "                stored = None\n                newest = None\n")),
    ('R11 the stored mark is the first made, not the newest', R,
     "                stored = mark\n            taken.append(u)", "                stored = stored or mark\n            taken.append(u)"),
    # ---- the judge: the instances' lines and the boundary (question 2) ----
    ('C1 no record `instance` where an instance starts', J,
     "            add({'flags': ['instance'], 'offset': 0, 'bytes': 0, 'taken_over': ln['taken_over'], 'pid': ln['pid'],\n"
     "                 'ns': ln['ns']})\n            continue\n", "            continue\n"),
    ('C2 a flush received and not at its barrier is kept across the boundary, with what waits behind it', J,
     "            received.clear()\n            pending.clear()", "            pending.clear()"),
    ('C3 a write received and not applied is kept across the boundary', J,
     "            received.clear()\n            pending.clear()", "            received.clear()"),
    ('C4 the last barrier is kept across the boundary', J,
     "            zeroing.clear()\n            last_barrier = None\n", "            zeroing.clear()\n"),
    ('C5 every later instance is read as a take-over', J,
     "                        'taken_over': kinds[inst['pid']]})", "                        'taken_over': True})"),
    ('C6 every later instance is read as a created device', J,
     "                        'taken_over': kinds[inst['pid']]})", "                        'taken_over': False})"),
    ('C7 two instances with no instances.tsv are read as take-overs', J,
     "    if len(instances) > 1 and kinds is None:\n        raise RigError(f'{len(instances)} instances of the daemon in the request log and no instances.tsv '\n"
     "                       'to say which took the device over')\n",
     "    if kinds is None:\n        kinds = {i['pid']: True for i in instances}\n"),
    ('C8 a pid instances.tsv does not name took the device over', J,
     "            if inst['pid'] not in kinds:\n                raise RigError(f\"instances.tsv does not say whether pid {inst['pid']} took the device over\")\n",
     "            kinds.setdefault(inst['pid'], True)\n"),
    ('C9 an instance that starts before the one before it wrote its last line is no error', J,
     "            if prev and prev[-1]['line'] > inst['line']:", "            if False and prev and prev[-1]['line'] > inst['line']:"),
    ('C10 a second start line of one pid begins another instance', J,
     "            if rec['pid'] in by_pid:\n                raise RigError(f'request log line {k + 1}: a second start line of pid {rec[\"pid\"]}')\n", ""),
    ('C11 instances.tsv may say anything: what is not `taken-over` is a created device', J,
     "        if len(f) != 2 or not f[0].isdigit() or f[1] not in ('taken-over', 'added'):", "        if len(f) != 2:"),
    ('C12 a write-zeroes behind a flush with no barrier is placed at the boundary', J,
     "            received.clear()\n            pending.clear()",
     "            for waiting in received.values():\n                for x in waiting:\n                    add(x)\n"
     "            received.clear()\n            pending.clear()"),
    # ---- lost_at_kill (question 3) ----
    ('K1 lost_at_kill never finds a lost line', J,
     "    bounds = [r['i'] for r in records if 'instance' in r['flags']]\n    for b in bounds:\n        before = sum(",
     "    bounds = []\n    for b in bounds:\n        before = sum("),
    ('K2 lost_at_kill: the next instance\'s number is not looked at', J,
     "        if len(rbefore) == before + 1 and rbefore[-1]['prefix'] == last and after \\\n                and after[0]['number'] == str(before + 2):",
     "        if len(rbefore) == before + 1 and rbefore[-1]['prefix'] == last and after:"),
    ('K3 lost_at_kill: the reference\'s extra cut may be at any barrier before the kill', J,
     "        if len(rbefore) == before + 1 and rbefore[-1]['prefix'] == last and after \\", "        if len(rbefore) == before + 1 and after \\"),
    ('K4 lost_at_kill: any number of cuts may be missing before the kill', J,
     "        if len(rbefore) == before + 1 and rbefore[-1]['prefix'] == last and after \\", "        if len(rbefore) > before and rbefore[-1]['prefix'] == last and after \\"),
    ('K5 main does not ask lost_at_kill: a lost line is a verdict', J,
     "            lost = lost_at_kill(dcuts, ref, records)\n", "            lost = None\n"),
    ('K6 main asks lost_at_kill and goes on: the line is no error', J,
     "            if lost:\n                raise RigError(lost)\n", "            if lost:\n                ok = True\n"),
    # ---- the instances' lines ----
    ('I1 every start is to say begun', J,
     "        want = 'begun' if k == 0 else 'taken_up'\n        before =", "        want = 'begun'\n        before ="),
    ('I2 every start is to say taken_up', J,
     "        want = 'begun' if k == 0 else 'taken_up'\n        before =", "        want = 'taken_up'\n        before ="),
    ('I3 an instance\'s cuts line in the log is not looked at by instance_K', J,
     "        if said != [want]:\n            bad.append(f\"its cuts line says {said or 'nothing'}\")\n", ""),
    ('I4 the status\'s cut_state at a start is not looked at', J,
     "        for key in ('cut_state', 'intent_state'):\n            if st.get(key) != want:", "        for key in ('intent_state',):\n            if st.get(key) != want:"),
    ('I5 the status\'s intent_state at a start is not looked at', J,
     "        for key in ('cut_state', 'intent_state'):\n            if st.get(key) != want:", "        for key in ('cut_state',):\n            if st.get(key) != want:"),
    ('I6 a count at a start below the cuts counted before it is no finding', J,
     "        if not n.isdigit() or int(n) < before:", "        if not n.isdigit():"),
    ('I7 a failed instance line is no verdict (main drops instance_lines\' result)', J,
     "            rc = max(rc, instance_lines(put, rdir, instances, dcuts, records))", "            instance_lines(put, rdir, instances, dcuts, records)"),
    ('I8 cut_state: only the first start\'s cuts line is looked at', J,
     "    ok = said == want and st == want[-1]\n", "    ok = said[:1] == want[:1] and st == want[-1]\n"),
    ('I9 cut_state: the last status may say begun after a take-up', J,
     "    ok = said == want and st == want[-1]\n", "    ok = said == want and st in want\n"),
    ('I10 several instances with no holder are no error', J,
     "        elif len(instances) > 1:\n            raise RigError(", "        elif False:\n            raise RigError("),
    ('I11 a held stage on a log with no hashes is judged', J,
     "    if bare:\n        raise RigError(f'{len(bare)} writes applied with no hashes", "    if False:\n        raise RigError(f'{len(bare)} writes applied with no hashes"),
    ('I12 the last cut written is held to the count of cut lines, not the last line\'s number', J,
     "    last = int(dcuts[-1]['number']) if dcuts and dcuts[-1]['number'] != '-' else len(dcuts)", "    last = len(dcuts)"),
    # ---- noflush_kstats (question 4) ----
    ('N1 every group written in a noflush span is left out, cut or not', J,
     "    left = {u['txg'] for u in taken if any(a < u['record'] < b for a, b in spans)} - cut",
     "    left = {u['txg'] for u in taken if any(a < u['record'] < b for a, b in spans)}"),
    ('N2 every group the reference does not cut is left out, in a noflush span or not', J,
     "    left = {u['txg'] for u in taken if any(a < u['record'] < b for a, b in spans)} - cut",
     "    left = {u['txg'] for u in taken} - cut"),
    ('N3 a noflush span reaches to the end of the record', J,
     "    left = {u['txg'] for u in taken if any(a < u['record'] < b for a, b in spans)} - cut",
     "    left = {u['txg'] for u in taken if any(a < u['record'] for a, b in spans)} - cut"),
    ('N4 a noflush-begin with no end is no error', J,
     "    if lo is not None:\n        raise RigError('a noflush-begin mark with no noflush-end after it')\n", ""),
    ('N5 main does not take the groups out of the kstats', J,
     "        kstats, left = noflush_kstats(kstats, records, taken, cs)", "        _, left = noflush_kstats(kstats, records, taken, cs)"),
    ('N6 nothing is left out of the kstats', J,
     "    return {k: [g for g in v if g not in left] for k, v in kstats.items()}, sorted(left)", "    return kstats, sorted(left)"),
    # ---- the table ----
    ('E1 takeover expects mark-dropped the same', J,
     "    'mark-dropped': dict({r: 'same' for r in RECS}, takeover='caught'),", "    'mark-dropped': dict({r: 'same' for r in RECS}),"),
    ('E2 takeover expects mark-kept same or caught', J,
     "    'mark-kept': dict({r: 'same' for r in RECS}, takeover='caught'),", "    'mark-kept': dict({r: 'same' for r in RECS}, takeover=('same', 'caught')),"),
    ('E3 life expects mark-dropped same or caught', J,
     "    'mark-dropped': dict({r: 'same' for r in RECS}, takeover='caught'),", "    'mark-dropped': dict({r: ('same', 'caught') for r in RECS}, takeover='caught'),"),
    # ---- the holder's leave ----
    ('H1 the leave file is not looked at', H,
     "        if leave and os.path.exists(leave):\n", "        if False:\n"),
    ('H2 the holder leaves and says nothing in its log', H,
     "            h.d.log.write(f'{time.monotonic_ns()}\\tleft\\n')\n            return True", "            return True"),
    ('H3 the loop says it was stopped when it left: main goes on to record and drain', H,
     "            h.d.log.write(f'{time.monotonic_ns()}\\tleft\\n')\n            return True",
     "            h.d.log.write(f'{time.monotonic_ns()}\\tleft\\n')\n            return False"),
    ('H4 main drains after a leave (it does not exit there)', H,
     "            if loop(h, stop, os.environ.get('CUT_HOLD_PAUSE', ''), os.environ.get('CUT_HOLD_LEAVE', '')):\n                sys.exit(0)\n",
     "            loop(h, stop, os.environ.get('CUT_HOLD_PAUSE', ''), os.environ.get('CUT_HOLD_LEAVE', ''))\n"),
    ('H5 the holder steps while paused', H,
     "                paused = True\n            sleep(0.2)\n            continue\n", "                paused = True\n            sleep(0.2)\n"),
    ('H6 the holder leaves only when it is not paused', H,
     "        if leave and os.path.exists(leave):\n", "        if leave and os.path.exists(leave) and not (pause and os.path.exists(pause)):\n"),
    # ---- the bench's releaser line (run by tests/lib/test_skz_store.sh) ----
    ('B1 skz_release_line counts every line, comments too', B,
     "    n=$(awk '/^[0-9]/ { n++ } END { print n + 0 }' \"$1\" 2>/dev/null) || n=0", "    n=$(awk '{ n++ } END { print n + 0 }' \"$1\" 2>/dev/null) || n=0"),
    ('B2 skz_release_line says the releaser runs whenever it was given a pid', B,
     "$( { [ -n \"$2\" ] && kill -0 \"$2\" 2>/dev/null; } && echo 1 || echo 0) $last\"", "$( { [ -n \"$2\" ]; } && echo 1 || echo 0) $last\""),
    ('B3 skz_release_line does not cut the last line', B,
     "tr -s ' \\t' ' ' | cut -c1-300) || last=\"\"", "tr -s ' \\t' ' ') || last=\"\""),
    ('B4 skz_release_line does not squeeze the last line\'s blanks', B,
     "last=$(tail -n 1 \"$1\" 2>/dev/null | tr -s ' \\t' ' ' | cut -c1-300)", "last=$(tail -n 1 \"$1\" 2>/dev/null | cut -c1-300)"),
    ('B5 the snapshot does not call skz_release_line', B,
     "            skz_release_line \"$OUT/skz-releases.tsv\" \"$SKZ_RELEASER\"\n", "            :\n"),
    ('H7 a pause does not write what is held', H,
     "                os.rename(pause + '.held.tmp', pause + '.held')\n                paused = True", "                paused = True"),
]


def main():
    cache = {}
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    for name, path, *pairs in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = cache.setdefault(full, open(full).read())
        counts = [orig.count(old) for old in pairs[0::2]]
        if any(c != 1 for c in counts) or any(old == new for old, new in zip(pairs[0::2], pairs[1::2])):
            print(f'{name}: NOT APPLIED (found {counts} times)', flush=True)
            continue
        text = orig
        for old, new in zip(pairs[0::2], pairs[1::2]):
            text = text.replace(old, new)
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed = []
            for t in (SH_TESTS if path == B else TESTS):
                limit = 40 if t.endswith('test_cut_hold.py') else 300
                try:
                    p = subprocess.run(['bash', t] if path == B else [sys.executable, '-B', t], cwd=ROOT, env=env,
                                       capture_output=True, text=True, timeout=limit)
                except subprocess.TimeoutExpired:
                    failed.append(f'{os.path.basename(t)} did not end in {limit} s')
                    continue
                failed += sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL: ', 'ERROR: '))})
                failed += [ln.strip()[:70] for ln in p.stdout.splitlines() if ln.startswith(('FAIL', 'not ok'))]
                if p.returncode != 0 and not any(ln.startswith(('FAIL: ', 'ERROR: ')) for ln in p.stderr.splitlines()):
                    failed.append(f'{os.path.basename(t)} exit {p.returncode}')
            if failed:
                print(f'{name}: KILLED by {len(failed)}: {", ".join(failed[:6])}', flush=True)
            else:
                print(f'{name}: SURVIVED', flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)


if __name__ == '__main__':
    main()
