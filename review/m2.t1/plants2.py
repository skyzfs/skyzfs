#!/usr/bin/env python3
"""Plant one change at a time in cut_ref.py / cut_check.py and run both test files.

    plants.py ROOT        (ROOT holds tests/kernel and tests/fixtures/pools)
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
K = os.path.join(ROOT, 'tests', 'kernel')

PLANTS = [
    # cut_ref.cuts: the repeat rule
    ('R1 repeat: newest cut not compared', 'cut_ref.py',
     "key == newest or (mark is not None", "False or (mark is not None"),
    ('R2 repeat: pending mark not compared', 'cut_ref.py',
     "key == newest or (mark is not None and key == (mark['txg'], mark['rootbp']))", "key == newest"),
    ('R3 repeat: keyed on txg alone in design', 'cut_ref.py',
     "key = (u['txg'], u['rootbp'])", "key = (u['txg'], None)"),
    # FUA and preflush
    ('F1 FUA: no barrier after the write', 'cut_ref.py',
     "            barrier(r, r['i'] + 1)\n", "            pass\n"),
    ('F2 FUA: barrier excludes the write', 'cut_ref.py',
     "barrier(r, r['i'] + 1)", "barrier(r, r['i'])"),
    ('F3 preflush: flush after its data', 'cut_ref.py',
     "        if 'flush' in flags:\n            # A flush that carries data flushes before its data.\n            barrier(r, r['i'])\n",
     "        pass\n"),
    ('F3b (with F3) the flush after the uberblocks', 'cut_ref.py',
     "        if 'fua' in flags and is_write(r):",
     "        if 'flush' in flags:\n            barrier(r, r['i'] + (1 if is_write(r) else 0))\n        if 'fua' in flags and is_write(r):"),
    ('G0 group zero marks', 'cut_ref.py', "if u['txg'] == 0:", "if False:"),
    # opens
    ('O1 opens: txg alone for the best', 'cut_ref.py',
     "key=lambda u: (u['txg'], u['timestamp'])", "key=lambda u: u['txg']"),
    ('O2 opens: itself by txg alone', 'cut_ref.py',
     "(best['txg'], best['rootbp']) == (c['txg'], c['rootbp'])", "best['txg'] == c['txg']"),
    ('O3 opens: a discard leaves the slot', 'cut_ref.py',
     "                    slots.pop(o, None)\n        elif is_write(r):", "                    pass\n        elif is_write(r):"),
    ('O4 opens: an empty write leaves the slot', 'cut_ref.py',
     "                    else:\n                        slots.pop(o, None)\n", "                    else:\n                        pass\n"),
    ('O5 opens: noted after the record', 'cut_ref.py',
     "    for r in records:\n        note(r['i'])\n", "    for r in records:\n        note(r['i'] - 1)\n"),
    ('S1 stale: consecutive equal states too', 'cut_ref.py',
     "first[key] < k - 1", "first[key] < k"),
    ('S2 stale: none', 'cut_ref.py', "            out[c['n']] = cs[first[key]]['n']\n", "            pass\n"),
    # cut_check
    ('C1 committed: groups that wrote nothing too', 'cut_check.py',
     "and int(f[5]) > 0:", "and int(f[5]) >= 0:"),
    ('C2 sessions: no range filter', 'cut_check.py',
     " and min(want) <= c['txg'] <= max(want)\n               and c['n'] not in skip]",
     "\n               and c['n'] not in skip]"),
    ('C3 sessions: stale cuts kept', 'cut_check.py', "and c['n'] not in skip]", "]"),
    ('C4 rule_same: txg alone', 'cut_check.py',
     "    key = [(c['txg'], c['prefix']) for c in rcs]\n    if key == [(c['txg'], c['prefix']) for c in cs]:",
     "    key = [(c['txg'], c['prefix']) for c in rcs]\n    if [k[0] for k in key] == [c['txg'] for c in cs]:"),
    ('C5 rule_same: stale leniency for any drop', 'cut_check.py',
     "    if key == fresh:", "    if len(key) >= len(fresh):"),
    ('C6 dropped_states: nothing', 'cut_check.py',
     "    have = {(c['txg'], c['prefix']) for c in rcs}", "    return []"),
    ('C7 dropped_states: rootbp ignored', 'cut_check.py',
     "(r['txg'], r['rootbp']) != (c['txg'], c['rootbp'])", "r['txg'] != c['txg']"),
    ('C8 dropped_states: not-itself cuts held too', 'cut_check.py',
     "in have or not o or not o['itself']:", "in have or not o:"),
    ('C9 synced_before_ready: begin not checked', 'cut_check.py',
     "if lo[k] < c['ub_record'] and c['flush_record'] < ready[k]]", "if c['flush_record'] < ready[k]]"),
    ('C10 pick_later: discards ignored', 'cut_check.py',
     "(cut_ref.is_write(r) or ('discard' in r['flags'] and r['bytes']))", "cut_ref.is_write(r)"),
    ('C11 pick_withheld: first, not last', 'cut_check.py',
     "for i in range(cut['ub_record'] - 1, -1, -1):", "for i in range(0, cut['ub_record']):"),
    ('C12 later_candidates: middle only', 'cut_check.py',
     "    for k in (4, 8, 16):", "    return out\n    for k in (4, 8, 16):"),
    ('C13 EXPECTED later: missed allowed', 'cut_check.py',
     "{r: ('not built', 'caught') for r in RECS}", "{r: ('not built', 'caught', 'missed') for r in RECS}"),
]

PLANTS += [
    ('N1 order: no timestamp', 'cut_ref.py',
     "return (u['txg'], u['timestamp'], u.get('mmp_seq', 0))", "return (u['txg'], 0, u.get('mmp_seq', 0))"),
    ('N2 order: no MMP sequence', 'cut_ref.py',
     "return (u['txg'], u['timestamp'], u.get('mmp_seq', 0))", "return (u['txg'], u['timestamp'], 0)"),
    ('N3 a tie is itself', 'cut_ref.py',
     "order(best_other) < order(best_own)", "order(best_other) <= order(best_own)"),
    ('N4 own state by group alone', 'cut_ref.py',
     "own = [u for u in slots.values() if (u['txg'], u['rootbp']) == (c['txg'], c['rootbp'])]",
     "own = [u for u in slots.values() if u['txg'] == c['txg']]"),
    ('N5 MMP sequence without its valid bit', 'cut_ref.py',
     "tail[1] == MMP_MAGIC and tail[3] & 0x02 else 0", "tail[1] == MMP_MAGIC else 0"),
    ('N6 MMP sequence without the MMP magic', 'cut_ref.py',
     "tail[1] == MMP_MAGIC and tail[3] & 0x02 else 0", "tail[3] & 0x02 else 0"),
    ('N7 version not checked', 'cut_ref.py',
     "    if version != str(FIXTURE_VERSION):", "    if False:"),
    ('L1 loaded: the first load, not the last', 'cut_check.py', "int(found[-1])", "int(found[0])"),
    ('L2 loaded: not held to the group', 'cut_check.py', "                if at != txg:", "                if False:"),
    ('X1 outside: not strictly increasing', 'cut_check.py', "if b['txg'] <= a['txg']:", "if b['txg'] < a['txg']:"),
    ('X2 outside: stale cuts kept', 'cut_check.py',
     "span = [c for c in cs if lo < c['ub_record'] < nxt and c['n'] not in skip]",
     "span = [c for c in cs if lo < c['ub_record'] < nxt]"),
    ('X3 outside: spans to the end mark only', 'cut_check.py',
     "span = [c for c in cs if lo < c['ub_record'] < nxt and", "span = [c for c in cs if lo < c['ub_record'] < (hi or nxt) and"),
    ('X4 outside: covered not counted', 'cut_check.py',
     "        covered += sum(", "        covered += 0 * sum("),
    ('E1 stale: any expected everywhere', 'cut_check.py',
     "EXPECTED_STALE = dict({r: 'none' for r in RECS}, multihost='any')", "EXPECTED_STALE = {r: 'any' for r in RECS}"),
    ('E2 stale: verdict ignored', 'cut_check.py',
     "stale_ok = bool(stale) <= (EXPECTED_STALE.get(name) == 'any')", "stale_ok = True"),
    ('Q1 rule_same: no root', 'cut_check.py', "return c['txg'], c['rootbp'], c['prefix']", "return c['txg'], c['prefix']"),
    ('Q2 rule_same: no prefix', 'cut_check.py', "return c['txg'], c['rootbp'], c['prefix']", "return c['txg'], c['rootbp']"),
]


def run_tests():
    out = []
    for t in ('test_cut_ref.py', 'test_cut_check.py'):
        p = subprocess.run([sys.executable, '-I', os.path.join(K, t)], cwd=ROOT, capture_output=True, text=True)
        fails = [ln for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))]
        out.append((t, p.returncode, fails))
    return out


def main():
    orig = {}
    for f in ('cut_ref.py', 'cut_check.py'):
        with open(os.path.join(K, f)) as fh:
            orig[f] = fh.read()
    base = run_tests()
    assert all(rc == 0 for _, rc, _ in base), base
    active = {}
    for name, f, old, new in PLANTS:
        text = orig[f]
        if name.startswith('F3b'):
            text = active['F3']
        n = text.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)')
            continue
        text = text.replace(old, new)
        active[name.split()[0]] = text
        with open(os.path.join(K, f), 'w') as fh:
            fh.write(text)
        try:
            res = run_tests()
        finally:
            with open(os.path.join(K, f), 'w') as fh:
                fh.write(orig[f])
        killed = [(t, fl) for t, rc, fl in res if rc != 0]
        if killed:
            print(f'{name}: KILLED by ' + '; '.join(f"{t}: {', '.join(x.split(' ')[1] for x in fl[:4]) or 'exit'}"
                                                    for t, fl in killed))
        else:
            print(f'{name}: SURVIVED')


if __name__ == '__main__':
    main()
