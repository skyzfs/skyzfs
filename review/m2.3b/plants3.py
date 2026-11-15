#!/usr/bin/env python3
"""#106 at 89ad21b: the fix of N1 planted back, and plant D2c of plants2.py again,
against the tests of skyzfs-local with its test hooks.

    plants3.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
INP = 'crates/skyzfs-local/src/inplace.rs'
PLANTS = [
    ('F1 collect does not run what is deferred when there are no threads', INP,
     [("            return self.run_deferred();\n        };\n        let mut done = Vec::new();\n",
       "            return;\n        };\n        let mut done = Vec::new();\n")]),
    ('D2c the drop does not run what was deferred itself', INP,
     [("            self.collect(Duration::from_millis(10));\n            self.run_deferred();\n        }\n        self.pool = None;\n",
       "            self.collect(Duration::from_millis(10));\n        }\n        self.pool = None;\n")]),
]


def tests(env):
    cmd = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=720)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', os.path.basename(TARGET) + '/debug/deps/skyzfs_local-'], check=False)
        return ['(did not end in 12 minutes)'], []
    out = (p.stdout + p.stderr).splitlines()
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    broken = [] if failed or p.returncode == 0 else [ln for ln in out if ln.startswith('error')][:3]
    return failed, broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if not ONLY:
        print(f'as the head has it: {tests(env)}', flush=True)
    for name, path, edits in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        text, ok = orig, True
        for old, new in edits:
            if text.count(old) != 1:
                print(f'{name}: NOT APPLIED ({text.count(old)} matches of {old[:50]!r})', flush=True)
                ok = False
                break
            text = text.replace(old, new)
        if not ok:
            continue
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed, broken = tests(env)
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:8])}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()
