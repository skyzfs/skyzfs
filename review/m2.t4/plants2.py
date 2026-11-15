#!/usr/bin/env python3
"""#108 at 775814c: the answer's fixes planted back, one at a time, against the tests
that are to hold them: test_cut_release.py, test_bracket.py, test_embedded_python.py
(tests/bench) and tests/lib/test_skz_store.sh. The file is restored after each.

    plants2.py CHECKOUT
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
REL = 'tests/bench/cut_release.py'
BR = 'tests/bench/bracket.py'
SH = 'tests/bench/topology-bench.sh'
PLANTS = [
    ('Q1 a refused release always counts as a cut gone', REL,
     "            if any(c['number'] == n and c['phase'] == 'uploading' for c in self.held()):\n                raise ch.HolderError(f'release {n} refused while cut {n} is still being uploaded: {last}')\n",
     "            self.held()\n"),
    ('Q2 a refused release always ends the releaser', REL,
     "            if any(c['number'] == n and c['phase'] == 'uploading' for c in self.held()):",
     "            if True:"),
    ('Q3 a cut gone is written as a release is, with no # before it', REL,
     "            self.out.write(f'# {time.monotonic_ns()} cut {n} gone after its release was refused ({last})\\n')",
     "            self.out.write(f'{time.monotonic_ns()} cut {n} gone after its release was refused ({last})\\n')"),
    ('Q4 a cut gone is not counted', REL, "            self.gone += 1\n", ""),
    ('Q5 a refused held is taken as nothing held', REL,
     "        if not ok:\n            raise ch.HolderError(f'held refused: {last}')\n        return ch.parse_held(lines)",
     "        return ch.parse_held(lines if ok else [])"),
    ('Q6 after a cut gone the step says a cut was released', REL,
     "            self.out.flush()\n            return False\n        self.released += 1",
     "            self.out.flush()\n            return True\n        self.released += 1"),
    ('B1 a row whose releaser ended gets a verdict', BR,
     "    if ended:\n        out['verdict']", "    if False:\n        out['verdict']"),
    ('S1 TB_SKZ_RELEASE without --hold-cuts is not refused', SH,
     "                [ -n \"$_hold\" ] || { echo \"TB_SKZ_RELEASE needs --hold-cuts",
     "                true || { echo \"TB_SKZ_RELEASE needs --hold-cuts"),
    ('S2 TB_SKZ_RELEASE with a build without +hooks is not refused', SH,
     "                [[ ${TB_SKYZFSD_REF:-} == *+hooks ]] || {", "                true || {"),
    ('S3 --hold-cuts is looked for as a part of a word', SH,
     "for a in \"${SKZ_CHECK_ARGS[@]}\"; do [ \"$a\" != --hold-cuts ] || _hold=1; done",
     "for a in \"${SKZ_CHECK_ARGS[@]}\"; do [[ $a != --hold-cuts* ]] || _hold=1; done"),
    ('E1 the releases in the window are the count at the second reading', SH,
     "'in_window': int(ra[1]) - int(rb[1]) if ok else None,", "'in_window': int(ra[1]) if ok else None,"),
    ('E2 a releaser that ended in the window counts as running', SH,
     "'running': ok and rb[2] == '1' and ra[2] == '1',", "'running': ok and rb[2] == '1',"),
    ('E3 a releaser that ended before the window counts as running', SH,
     "'running': ok and rb[2] == '1' and ra[2] == '1',", "'running': ok and ra[2] == '1',"),
    ('E4 a row with no line from the releaser counts as running', SH,
     "'running': ok and rb[2] == '1' and ra[2] == '1',", "'running': not ok or (rb[2] == '1' and ra[2] == '1'),"),
    ('E5 the row of an ended releaser does not say so in its serve arguments', SH,
     "            k['serve_args'] += f\" (cuts released every {every}: THE RELEASER ENDED, {k['release']['ended']})\"\n", ""),
    ('E6 the row of an ended releaser carries no reason for the verdict', SH,
     "            k['release']['ended'] = k['release']['last'] or 'no line from the releaser'\n",
     "            k['release']['ended'] = ''\n"),
    ('H1 the reading counts every line of the releases file as a release', SH,
     "awk '/^[0-9]/ { n++ } END { print n + 0 }'", "awk '{ n++ } END { print n + 0 }'"),
    ('H2 the reading says the releaser runs whatever became of it', SH,
     "{ [ -n \"$SKZ_RELEASER\" ] && kill -0 \"$SKZ_RELEASER\" 2>/dev/null; } && echo 1 || echo 0)",
     "true && echo 1 || echo 0)"),
]
TESTS = [['python3', 'tests/bench/test_cut_release.py'], ['python3', 'tests/bench/test_bracket.py'],
         ['python3', 'tests/bench/test_embedded_python.py'], ['bash', 'tests/lib/test_skz_store.sh']]
for name, path, old, new in PLANTS:
    full = os.path.join(ROOT, path)
    orig = open(full).read()
    if orig.count(old) != 1:
        print(f'{name}: NOT APPLIED (found {orig.count(old)} times)', flush=True)
        continue
    try:
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
        failed = []
        for cmd in TESTS:
            try:
                p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=180)
            except subprocess.TimeoutExpired:
                subprocess.run(['pkill', '-f', 'cut_release.py'], check=False)
                failed.append(f'{os.path.basename(cmd[1])} (did not end)')
                continue
            if p.returncode != 0:
                names = [ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))]
                names += [ln[7:60] for ln in p.stdout.splitlines() if ln.startswith('NOT OK') or ln.startswith('not ok')]
                failed += names or [os.path.basename(cmd[1])]
        print(f'{name}: ' + (f'KILLED by {len(failed)}: {"; ".join(sorted(set(failed))[:4])}' if failed else 'SURVIVED'), flush=True)
    finally:
        with open(full, 'w') as f:
            f.write(orig)
print('finished')
