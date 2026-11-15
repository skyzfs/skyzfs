#!/usr/bin/env python3
"""#100 at 4331a90: plants in the fix for a region's file of another length (N1), against the
tests of skyzfs-mapped.

    plants_next4.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
LINUX = 'crates/skyzfs-mapped/src/linux.rs'
OLD = "            file.set_len(0)\n                .and_then(|()| file.set_len(len as u64))\n"
PLANTS = [
    ('E1 a file of another length is only cut or grown', OLD, "            file.set_len(len as u64)\n"),
    ('E2 the file is emptied only where it is cut short', OLD,
     "            (if had > len as u64 { file.set_len(0) } else { Ok(()) })\n"
     "                .and_then(|()| file.set_len(len as u64))\n"),
    ('E3 the file is emptied only where it grows', OLD,
     "            (if had < len as u64 { file.set_len(0) } else { Ok(()) })\n"
     "                .and_then(|()| file.set_len(len as u64))\n"),
]
RUN = ['cargo', 'test', '-p', 'skyzfs-mapped', '--no-fail-fast']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    path = os.path.join(ROOT, LINUX)
    orig = open(path).read()
    try:
        for name, old, new in [('as the head has it', None, None)] + PLANTS:
            if old:
                if orig.count(old) != 1:
                    print(f'{name}: NOT APPLIED', flush=True)
                    continue
                with open(path, 'w') as f:
                    f.write(orig.replace(old, new))
            p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
            out = p.stdout + p.stderr
            failed = sorted({ln.split()[1].split('::')[-1] for ln in out.splitlines()
                             if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
            results = [ln.strip() for ln in out.splitlines() if ln.startswith('test result') and ' 0 passed; 0 failed' not in ln]
            if p.returncode == 0:
                print(f'{name}: {"passes" if not old else "SURVIVED"}: {"; ".join(results)[:160]}', flush=True)
            elif failed:
                print(f'{name}: KILLED by {len(failed)}: {", ".join(failed)}', flush=True)
            else:
                print(f'{name}: DID NOT BUILD OR RUN: ' + ' | '.join(l for l in out.splitlines() if l.startswith('error'))[:300], flush=True)
            with open(path, 'w') as f:
                f.write(orig)
    finally:
        with open(path, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
