#!/usr/bin/env python3
"""#99 at 2c67c42: plant one change at a time in the daemon check and in the reference's
receipt rule, and run the rig's unit tests (test_cut_daemon_check, test_cut_ref, test_cut_check).

    plants.py CHECKOUT

The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
CDC = 'tests/kernel/cut_daemon_check.py'
REF = 'tests/kernel/cut_ref.py'
TESTS = ['tests/kernel/test_cut_daemon_check.py', 'tests/kernel/test_cut_ref.py', 'tests/kernel/test_cut_check.py']

PLANTS = [
    # records_of
    ('R1 a write is placed at its recv line, with no bytes', CDC,
     "                pending[ln['tag']] = (off, n)\n                data.pop(ln['tag'], None)",
     "                pending[ln['tag']] = (off, n)\n                data.pop(ln['tag'], None)\n"
     "                add({'flags': [], 'offset': off, 'bytes': n, 'pos': None, 'tag': ln['tag']})"),
    ('R2 a flush is one record at its barrier (no receipt)', CDC,
     "                add({'flags': ['receipt'], 'offset': 0, 'bytes': 0, 'tag': ln['tag']})\n                received.add(ln['tag'])",
     "                received.add(ln['tag'])",
     "                r['received'] = True", "                pass"),
    ('R3 a write-zeroes changes nothing', CDC,
     "                add({'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op']})", "                pass"),
    ('R4 a discard reads as zeros too', CDC,
     "            elif ln['op'] == 'write_zeroes':", "            elif ln['op'] in ('write_zeroes', 'discard'):"),
    ('R5 a write keeps no bytes of its label line', CDC,
     "                r['data'] = b", "                pass"),
    ('R6 the marks all come after the last line', CDC,
     "    for ln in lines:\n        marks_until(ln['ns'])", "    for ln in lines:"),
    ('R7 a cut\'s instant is the record after its barrier', CDC,
     "                          'prefix': last_barrier['i'], 'line': ln['line'], 'ns': ln['ns'],",
     "                          'prefix': last_barrier['i'] + 1, 'line': ln['line'], 'ns': ln['ns'],"),
    ('R8 a cut line need not follow its own barrier', CDC,
     "            if last_barrier is None or last_barrier['tag'] != ln['tag'] or last_barrier['op'] != ln['op']:",
     "            if last_barrier is None:"),
    ('R9 a mark at a line\'s own time comes after it', CDC,
     "        while mk < len(marks) and marks[mk][0] <= ns:", "        while mk < len(marks) and marks[mk][0] < ns:"),
    ('R10 a write is placed at its commit line, not where it was applied', CDC,
     "        elif ev == 'applied':", "        elif ev == 'commit' and ln['op'] == 'write' and ln['tag'] in pending:"),
    ('R11 a label line of another length is taken', CDC,
     "                if len(b) != n:", "                if False:"),
    ('R12 a barrier of a flush that was never received is taken', CDC,
     "                if ln['tag'] not in received:", "                if False:"),
    # parse_log
    ('P1 a line of any number of columns is read', CDC,
     "        if not 8 <= len(f) <= 10 or not f[0].isdigit():", "        if len(f) < 8 or not f[0].isdigit():"),
    ('P2 a line of a pid with no start line is dropped', CDC,
     "        if inst is None:\n            raise RigError(f'request log line {k + 1}: pid {rec[\"pid\"]} has no start line')",
     "        if inst is None:\n            continue"),
    # compare
    ('C1 the count is not compared', CDC,
     "    if len(dcuts) != len(ref):", "    if False:"),
    ('C2 the instant is not compared', CDC,
     "        key_d = (d['prefix'], d['txg'], d['itself'])\n        key_r = (r['prefix'], r['txg'], r['itself'])",
     "        key_d = (d['txg'], d['itself'])\n        key_r = (r['txg'], r['itself'])"),
    ('C3 the answer to itself is not compared', CDC,
     "        key_d = (d['prefix'], d['txg'], d['itself'])\n        key_r = (r['prefix'], r['txg'], r['itself'])",
     "        key_d = (d['prefix'], d['txg'])\n        key_r = (r['prefix'], r['txg'])"),
    ('C4 the group is not compared', CDC,
     "        key_d = (d['prefix'], d['txg'], d['itself'])\n        key_r = (r['prefix'], r['txg'], r['itself'])",
     "        key_d = (d['prefix'], d['itself'])\n        key_r = (r['prefix'], r['itself'])"),
    # the status
    ('T1 a status written before the last cut is held to its values alone', CDC,
     "        if hi + SLACK_NS < times[-1]:", "        if False:"),
    ('T2 no slack', CDC,
     "        if hi + SLACK_NS < times[-1]:", "        if hi < times[-1]:"),
    ('T3 the status\'s age is not taken off', CDC,
     "    return before - age_ns - 1_000_000, after - age_ns", "    return before - 1_000_000, after"),
    ('T4 cut_opens is not compared', CDC,
     "        want['cut_opens'] = 'itself' if ref[-1]['itself'] else 'rewind'", "        pass"),
    ('T5 cut_group is not compared', CDC,
     "        want['cut_group'] = str(ref[-1]['txg'])", "        pass"),
    ('T6 the count is not compared', CDC,
     "    want = {'cuts': str(len(ref))}", "    want = {}"),
    ('T7 a read that ended before it began is taken', CDC,
     "    if after < before:", "    if False:"),
    # the numbers
    ('N1 numbers that do not count from 1 pass', CDC,
     "        if n != str(k + 1):", "        if not n.isdigit():"),
    # main: what sets the exit code
    ('M1 a failed cuts line does not fail the check', CDC,
     "        put('cuts', 'PASS' if ok else 'FAIL', detail)\n        if not ok:\n            rc = 1",
     "        put('cuts', 'PASS' if ok else 'FAIL', detail)"),
    ('M2 a failed status line does not fail the check', CDC,
     "        put('status', 'PASS' if ok else 'FAIL', detail)\n        if not ok:\n            rc = 1",
     "        put('status', 'PASS' if ok else 'FAIL', detail)"),
    ('M3 a control that was missed does not fail the check', CDC,
     "            if not good:\n                rc = 1", "            pass"),
    ('M4 more than one instance is read as the first', CDC,
     "        if len(instances) != 1:", "        if not instances:"),
    ('M5 the rig\'s own error ends as a verdict', CDC,
     "        put('rig_error', 'ERROR', str(e))\n        rc = 2", "        put('rig_error', 'ERROR', str(e))\n        rc = 1"),
    ('M6 failed sessions do not fail the check', CDC,
     "            'increasing groups')\n        if bad:\n            rc = 1", "            'increasing groups')"),
    ('M7 wrong cut numbers do not fail the check', CDC,
     "        if verdict == 'FAIL':\n            rc = 1", "        pass"),
    ('M8 the early control compares the daemon with the design\'s reference', CDC,
     "        ecs, _ = cut_ref.cuts(records, ubs, 'early')", "        ecs, _ = cut_ref.cuts(records, ubs)"),
    # the controls' lists
    ('K1 the drop control drops nothing', CDC,
     "    out['drop'] = dcuts[:mid] + dcuts[mid + 1:]", "    out['drop'] = list(dcuts)"),
    # cut_ref: the receipt
    ('F1 a receipt leaves the mark where it was', REF,
     "            carried[r['tag']], mark = mark, None", "            carried[r['tag']] = mark"),
    ('F2 a received flush\'s barrier loses the mark made since its receipt', REF,
     "        if r.get('received'):\n            mark = kept", "        if r.get('received'):\n            pass"),
    ('F3 an uberblock that repeats a carried mark marks again', REF,
     "                    or any(c is not None and key == (c['txg'], c['rootbp']) for c in carried.values())):", "                    ):"),
    ('F4 a received flush\'s barrier takes the mark made since its receipt when it carries none', REF,
     "            mark, kept = carried.pop(r['tag'], None), mark",
     "            mark, kept = carried.pop(r['tag'], None), mark\n            if mark is None:\n                mark, kept = kept, None"),
    ('F5 a flush received twice before its barrier is taken', REF,
     "            if r['tag'] in carried:", "            if False:"),
]


def main():
    for name, path, *pairs in PLANTS:
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        counts = [orig.count(old) for old in pairs[0::2]]
        if counts != [1] * len(counts):
            print(f'{name}: NOT APPLIED (found {counts} times)', flush=True)
            continue
        changed = orig
        for old, new in zip(pairs[0::2], pairs[1::2]):
            changed = changed.replace(old, new)
        with open(full, 'w') as f:
            f.write(changed)
        try:
            failed = []
            for t in TESTS:
                p = subprocess.run([sys.executable, '-B', t], cwd=ROOT, capture_output=True, text=True)
                if p.returncode:
                    out = p.stdout + p.stderr
                    names = sorted({ln.split()[1] for ln in out.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))})
                    failed.append(os.path.basename(t) + ': ' + (', '.join(names) if names else 'did not run'))
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        print(f'{name}: ' + ('KILLED by ' + '; '.join(failed) if failed else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
