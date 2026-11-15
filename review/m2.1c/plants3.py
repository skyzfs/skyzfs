#!/usr/bin/env python3
"""#96 at 2c7a3ce: plants in the rule that follows a slot whose fields did not verify, and the
second round's one gap (R12) again.

    plants3.py CHECKOUT TARGET_DIR

Each plant runs skyzfs-uberblock's tests (U) and skyzfs-local's tests of the cut rule and the
recorded pools (L). The file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
UB = 'crates/skyzfs-uberblock/src/lib.rs'
RUNS = {
    'U': ['cargo', 'test', '-p', 'skyzfs-uberblock'],
    'L': ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'tests::cuts', 'tests::recorded'],
}
REACHED = "            .filter(|&(&at, &slot_end)| at < end && offset < slot_end)"

PLANTS = [
    ('A1 changed: a place known not to verify is not followed (as at 110d2b6)',
     "        for at in completed {\n            self.rejected.remove(&at);\n            self.unknown.insert(at);\n        }\n",
     "        let _ = completed;\n"),
    ('A2 largest_slot_end: the smallest slot that could begin there',
     "            .filter(|&size| (at - ring.start).is_multiple_of(size))\n            .max()",
     "            .filter(|&size| (at - ring.start).is_multiple_of(size))\n            .min()"),
    ('A3 changed: group 0 is remembered too',
     "            } else if uberblock.counts() {", "            } else if true {"),
    ('A4 changed: a remembered place whose fields are written again stays remembered',
     "                    self.held.remove(&at);\n                    self.rejected.remove(&at);\n",
     "                    self.held.remove(&at);\n"),
    ('A5 changed: any change that begins before the slot\'s end reaches it', REACHED,
     "            .filter(|&(_, &slot_end)| offset < slot_end)"),
    ('A6 changed: any change that ends after the place reaches it', REACHED,
     "            .filter(|&(&at, _)| at < end)"),
    ('A7 changed: a place reached is forgotten and left known',
     "            self.rejected.remove(&at);\n            self.unknown.insert(at);\n        }\n        for uberblock in found {",
     "            self.rejected.remove(&at);\n        }\n        for uberblock in found {"),
    ('A8 changed: nothing is remembered',
     "                if let Some(slot_end) = self.largest_slot_end(uberblock.at) {\n                    self.rejected.insert(uberblock.at, slot_end);\n                }",
     "                let _ = self.largest_slot_end(uberblock.at);"),
    ('R12 may_go_on: 8 KiB alone is asked',
     "                SLOT_SIZES\n                    .into_iter()\n                    .any(|size| (at - ring.start).is_multiple_of(size) && at + size > end)",
     "                [SLOT_MAX]\n                    .into_iter()\n                    .any(|size| (at - ring.start).is_multiple_of(size) && at + size > end)"),
]


def run(kind):
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    p = subprocess.run(RUNS[kind], cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out.splitlines()
                     if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    if p.returncode and not failed:
        errs = [ln for ln in out.splitlines() if ln.startswith('error')]
        return p.returncode, ['(build) ' + (errs[0][:160] if errs else 'failed')]
    return p.returncode, failed


def main():
    full = os.path.join(ROOT, UB)
    for name, old, new in PLANTS:
        orig = open(full).read()
        n = orig.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)', flush=True)
            continue
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
        try:
            got = []
            for k in 'UL':
                rc, failed = run(k)
                if rc:
                    got.append(f"{k}: {', '.join(failed[:5])}" + (f' (+{len(failed) - 5})' if len(failed) > 5 else ''))
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        print(f"{name}: " + ('KILLED by ' + '; '.join(got) if got else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
