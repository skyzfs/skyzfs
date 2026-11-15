#!/usr/bin/env python3
"""Plant one change at a time in tests/kernel/nodeb_check.py of #97 and run its unit tests.

    plants.py CHECKOUT

The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
PATH = os.path.join(ROOT, 'tests/kernel/nodeb_check.py')

PLANTS = [
    ('C1 choose: the cut after the snapshot\'s group, not at it',
     "at = next((c for c in span if c['txg'] >= group), None)", "at = next((c for c in span if c['txg'] > group), None)"),
    ('C2 choose: cuts of later sessions count',
     "span = [c for c in cs if lo < c['ub_record'] and (hi is None or c['ub_record'] < hi)]",
     "span = [c for c in cs if lo < c['ub_record']]"),
    ('C3 choose: cuts of earlier sessions count',
     "span = [c for c in cs if lo < c['ub_record'] and (hi is None or c['ub_record'] < hi)]",
     "span = [c for c in cs if (hi is None or c['ub_record'] < hi)]"),
    ('C4 choose: the early cut need not be usable',
     "if k == 0 or not ok(span[k - 1]) or span[k - 1]['txg'] >= group:", "if k == 0 or span[k - 1]['txg'] >= group:"),
    ('C5 choose: the published cut need not be usable',
     "if at is None or not ok(at):", "if at is None:"),
    ('C6 choose: the first that qualifies, not the middle',
     "return picks[len(picks) // 2] if picks else None", "return picks[0] if picks else None"),
    ('C7 choose: the last that qualifies',
     "return picks[len(picks) // 2] if picks else None", "return picks[-1] if picks else None"),
    ('C8 choose: no check that a cut comes before',
     "if k == 0 or not ok(span[k - 1]) or span[k - 1]['txg'] >= group:", "if not ok(span[k - 1]) or span[k - 1]['txg'] >= group:"),
    ('C9 choose: the early cut may be of the snapshot\'s group or later',
     "if k == 0 or not ok(span[k - 1]) or span[k - 1]['txg'] >= group:", "if k == 0 or not ok(span[k - 1]):"),
    ('C10 choose: the snapshot\'s session is not looked at',
     "        if session not in spans:\n            continue\n        lo, hi = spans[session]",
     "        lo, hi = min(b for b, _ in spans.values()), None"),
    ('F1 read_snaps: a bad line is passed over',
     "            raise RigError(f'snaps.tsv: a line that is not name, group, session: {ln.strip()!r}')", "            continue"),
    ('F2 read_sums: a bad line is passed over',
     "            raise RigError(f'a checksum line that is not sha256sum\\'s: {ln!r}')", "            continue"),
    ('F3 read_sums: the digest and the name swapped',
     "        out[m.group(2)] = m.group(1)", "        out[m.group(1)] = m.group(2)"),
    ('L1 later_uberblock: an uberblock before the instant counts',
     "        if u['record'] >= cut['prefix'] and u['txg'] > cut['txg']:", "        if u['txg'] > cut['txg']:"),
    ('L2 later_uberblock: the cut\'s own group counts',
     "        if u['record'] >= cut['prefix'] and u['txg'] > cut['txg']:", "        if u['record'] >= cut['prefix'] and u['txg'] >= cut['txg']:"),
    ('L3 later_uberblock: any group counts',
     "        if u['record'] >= cut['prefix'] and u['txg'] > cut['txg']:", "        if u['record'] >= cut['prefix']:"),
    ('B1 live_block: any level',
     "if bp['type'] == 'ZFS plain file' and bp['level'] == 0 and bp['dvas']:", "if bp['type'] == 'ZFS plain file' and bp['dvas']:"),
    ('B2 live_block: any type',
     "if bp['type'] == 'ZFS plain file' and bp['level'] == 0 and bp['dvas']:", "if bp['level'] == 0 and bp['dvas']:"),
    ('B3 live_block: the offset without the labels',
     "            return off + replay_check.DVA_OFFSET, asize", "            return off, asize"),
    ('B4 live_block: the last copy, not the first',
     "            _, off, asize = bp['dvas'][0]", "            _, off, asize = bp['dvas'][-1]"),
    ('E1 EXPECTED: early may be missed',
     "EXPECTED = {ctl: 'caught' for ctl in CONTROLS}", "EXPECTED = dict({ctl: 'caught' for ctl in CONTROLS}, early='missed')"),
    ('S1 spans_of: no session ends',
     "return {k: (b, next((x for x in order if x > b), None)) for k, b in begins.items()}",
     "return {k: (b, None) for k, b in begins.items()}"),
    # The parts with no test that needs no kernel: what node B does.
    ('V1 verify: a missing ref is not noticed',
     "        if ref not in names:\n            return 'refs', f'no ref {ref} among {names}'\n", ""),
    ('V2 snapshot_files: a missing snapshot is not noticed',
     "            if f'{POOL}@{snap}' not in have:", "            if False:"),
    ('V3 snapshot_files: the digests are not compared',
     "            differ = sorted(k for k in set(want) & set(got) if want[k] != got[k])", "            differ = []"),
    ('V4 snapshot_files: files the source did not hold are not noticed',
     "            extra = sorted(set(got) - set(want))", "            extra = []"),
    ('V5 verify: every copy is not checked',
     "        if failed:\n            return failed, d2\n", ""),
    ('V6 main: a control that is missed does not fail the run',
     "            if not ok:\n                rc = 1\n    except RigError as e:", "    except RigError as e:"),
    ('V7 main: the published cut\'s failure does not fail the run',
     "        if failed:\n            rc = 1\n\n        # The controls.", "\n        # The controls."),
]


def main():
    orig = open(PATH).read()
    for name, old, new in PLANTS:
        n = orig.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)', flush=True)
            continue
        with open(PATH, 'w') as f:
            f.write(orig.replace(old, new))
        try:
            p = subprocess.run([sys.executable, os.path.join(ROOT, 'tests/kernel/test_nodeb_check.py')],
                               capture_output=True, text=True)
            failed = sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))})
        finally:
            with open(PATH, 'w') as f:
                f.write(orig)
        print(f"{name}: " + ('KILLED by ' + ', '.join(failed) if p.returncode else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
