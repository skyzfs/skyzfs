#!/usr/bin/env python3
"""#97 at its next head (75bf72b): plants in tests/kernel/nodeb_check.py for what the head added
(the stages, the comparison, the digest control, the flat detail), and the first round's
survivors again where their code is still there.

    plants2.py CHECKOUT

The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
PATH = os.path.join(ROOT, 'tests/kernel/nodeb_check.py')
LATER = "        if u['record'] >= cut['prefix'] and u['txg'] > cut['txg']:"
FAILED = "        return next((st for st in STAGES if st in self.stages and not self.stages[st][0]), None)"

PLANTS = [
    # The first round's survivors that still apply.
    ('L1 later_uberblock: an uberblock before the instant counts', LATER, "        if u['txg'] > cut['txg']:"),
    ('C8 choose: no check that a cut comes before',
     "if k == 0 or not ok(span[k - 1]) or span[k - 1]['txg'] >= group:", "if not ok(span[k - 1]) or span[k - 1]['txg'] >= group:"),
    ('C9 choose: the early cut may be of the snapshot\'s group or later',
     "if k == 0 or not ok(span[k - 1]) or span[k - 1]['txg'] >= group:", "if k == 0 or not ok(span[k - 1]):"),
    # The comparison.
    ('N1 compare: nothing differs', "    if missing or extra or differ:", "    if False:"),
    ('N2 compare: the digests are not compared',
     "    differ = sorted(k for k in set(want) & set(got) if want[k] != got[k])", "    differ = []"),
    ('N3 compare: files the source did not hold pass', "    if missing or extra or differ:", "    if missing or differ:"),
    ('N4 compare: files node B lacks pass', "    if missing or extra or differ:", "    if extra or differ:"),
    # The stages.
    ('N5 failed: the last stage that failed', FAILED,
     "        return next((st for st in reversed(STAGES) if st in self.stages and not self.stages[st][0]), None)"),
    ('N6 failed: the first that failed in the order of the calls', FAILED,
     "        return next((st for st in self.stages if not self.stages[st][0]), None)"),
    ('N7 control_verdict: caught at any stage is as expected', "    return verdict, st in want", "    return verdict, True"),
    ('N8 control_verdict: a control that is missed is as expected', "        return 'missed', False", "        return 'missed', True"),
    ('N9 flat: the text as it is', "    return ' '.join(str(text).split())", "    return str(text)"),
    ('N10 Seen.put: the detail is not made one line',
     "        self.stages[stage] = (ok, flat(detail))", "        self.stages[stage] = (ok, detail)"),
    ('N11 Seen.ok: a stage that did not run is ok',
     "        return self.stages.get(stage, (False, ''))[0]", "        return self.stages.get(stage, (True, ''))[0]"),
    ('N12 EXPECTED: early may be caught at the import', "    'early': ('snapshot',),", "    'early': ('import', 'snapshot'),"),
    ('N13 EXPECTED: digest may be caught at the copies', "    'digest': ('files',),", "    'digest': ('files', 'copies'),"),
    ('N14 EXPECTED: hole may be caught at the refs',
     "    'hole': ('import', 'files', 'copies'),", "    'hole': ('refs', 'import', 'files', 'copies'),"),
    ('N15 EXPECTED: lacks may be caught at the snapshot',
     "    'lacks': ('import', 'files', 'copies'),", "    'lacks': ('import', 'snapshot', 'files', 'copies'),"),
    # The parts with no test that needs no kernel.
    ('M1 main: early need not be otherwise clean',
     "                if ctl == 'early' and not (cseen.ok('import') and cseen.ok('copies')):", "                if False:"),
    ('M2 main: the digest control holds the cut against the unchanged digests',
     "        got['digest'] = (nb.verify(f'{name}-cut', ref, snap, bad, damaged=False),",
     "        got['digest'] = (nb.verify(f'{name}-cut', ref, snap, want, damaged=False),"),
    ('M3 main: a multihost pool is not refused',
     "        if any(u.get('mmp_delay') for u in ubs):", "        if False:"),
    ('M4 main: any other exception is not the rig\'s error',
     "    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict\n"
     "        put('rig_error', 'ERROR', f'{type(e).__name__}: {e}')\n        rc = 2\n", ""),
    ('M5 verify: zdb\'s abort on a control is the rig\'s error',
     "        if failed and failed.startswith('rig:') and not damaged:", "        if failed and failed.startswith('rig:'):"),
    ('M6 verify: zdb\'s abort on the published cut is a failed stage',
     "        if failed and failed.startswith('rig:') and not damaged:", "        if False:"),
    ('M7 verify: a ref whose content cannot be read is the rig\'s error',
     "        if rc == 4:\n            seen.put('content', False, f'{ref} is listed and its content cannot be read: {out.strip()[-300:]}')\n"
     "            return seen\n", ""),
    ('M8 main: a control that differs does not fail the run',
     "            if not ok:\n                rc = 1\n    except RigError as e:", "    except RigError as e:"),
    ('M9 main: the published cut\'s failure does not fail the run',
     "        if seen.failed():\n            rc = 1\n", ""),
    ('M10 NodeB: zdb is not given the log device',
     "        if logdev:\n            rc, out, _ = run(['cp', '--sparse=always', logdev, os.path.join(self.img, 'log-device.img')])",
     "        if False:\n            rc, out, _ = run(['cp', '--sparse=always', logdev, os.path.join(self.img, 'log-device.img')])"),
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
            p = subprocess.run([sys.executable, '-B', os.path.join(ROOT, 'tests/kernel/test_nodeb_check.py')],
                               capture_output=True, text=True)
            failed = sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))})
        finally:
            with open(PATH, 'w') as f:
                f.write(orig)
        print(f"{name}: " + ('KILLED by ' + ', '.join(failed) if p.returncode else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
