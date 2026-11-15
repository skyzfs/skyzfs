#!/usr/bin/env python3
"""#104 at 3251b30: plants in the copies' direct I/O, one at a time, against the tests of
skyzfs-local (with its test hooks) and skyzfs-mapped. The file is restored after each.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
INP = 'crates/skyzfs-local/src/inplace.rs'
LINUX = 'crates/skyzfs-mapped/src/linux.rs'
RUNS = [
    ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast'],
    ['cargo', 'test', '-p', 'skyzfs-mapped', '--no-fail-fast'],
]
WRITE = "        if self.direct {\n            file.write_direct_at(at, buf)\n        } else {\n            file.write_at(at, buf)\n        }\n"
READ = "                if self.io.direct {\n                    file.read_direct_at(at, buf)?;\n                } else {\n                    file.read_at(at, buf)?;\n                }\n"
PLANTS = [
    ('W a copy is written through the cache', INP, WRITE, "        file.write_at(at, buf)\n"),
    ('R a copy is read through the cache', INP, READ, "                file.read_at(at, buf)?;\n"),
    ('W2 a copy is written by direct I/O whatever the store\'s I/O is', INP, WRITE, "        let _ = self.direct;\n        file.write_direct_at(at, buf)\n"),
    ('R2 a copy is read by direct I/O whatever the store\'s I/O is', INP, READ, "                file.read_direct_at(at, buf)?;\n"),
    ('W3 a copy that takes no direct write is written through the cache', INP, WRITE,
     "        if self.direct {\n            file.write_direct_at(at, buf).or_else(|_| file.write_at(at, buf))\n        } else {\n            file.write_at(at, buf)\n        }\n"),
    ('T4 the copies file is opened and not synced by the start', INP,
     "                let synced = file.sync();", "                let synced: io::Result<()> = Ok(());"),
    ('E3 a region\'s file is emptied only where it grows', LINUX,
     "            file.set_len(0)\n                .and_then(|()| file.set_len(len as u64))\n",
     "            (if had < len as u64 { file.set_len(0) } else { Ok(()) })\n"
     "                .and_then(|()| file.set_len(len as u64))\n"),
]


def tests(env):
    failed, broken = [], []
    for cmd in RUNS:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        failed += [ln.split()[1].split('::')[-1] for ln in out.splitlines()
                   if ln.startswith('test ') and ln.rstrip().endswith('FAILED')]
        if p.returncode != 0 and not any(ln.startswith('test ') and ln.rstrip().endswith('FAILED') for ln in out.splitlines()):
            broken += [ln for ln in out.splitlines() if ln.startswith('error')][:3]
    return sorted(set(failed)), broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if not ONLY or ONLY == ('none',):
        failed, broken = tests(env)
        print(f'as the head has it: failed {failed}, broken {broken}', flush=True)
        if ONLY == ('none',):
            return
    for name, path, old, new in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        if orig.count(old) != 1:
            print(f'{name}: NOT APPLIED (found {orig.count(old)} times)', flush=True)
            continue
        try:
            with open(full, 'w') as f:
                f.write(orig.replace(old, new))
            failed, broken = tests(env)
            if failed:
                print(f'{name}: KILLED by {len(failed)}: {", ".join(failed[:6])}', flush=True)
            elif broken:
                print(f'{name}: DID NOT BUILD OR RUN: ' + ' | '.join(broken)[:300], flush=True)
            else:
                print(f'{name}: SURVIVED', flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)


if __name__ == '__main__':
    main()
