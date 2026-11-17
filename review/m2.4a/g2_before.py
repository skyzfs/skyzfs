#!/usr/bin/env python3
"""#110, the next look: plant G2 of plants2.py (the commit PUTs its own segments after
the head swap) against the tests of the head it is given, to compare dcf2df3 with
4a51efc: which tests catch it at each. The file is restored afterwards.

    g2_before.py CHECKOUT TARGET_DIR
"""
import os
import subprocess

import plants
import plants2


def main():
    root = plants.ROOT
    head = subprocess.run(['git', 'rev-parse', '--short', 'HEAD'], cwd=root, capture_output=True, text=True).stdout.strip()
    name, path, edits = next(p for p in plants2.NEW if p[0].startswith('G2'))
    full = os.path.join(root, path)
    orig = open(full).read()
    text = orig
    for old, new in edits:
        assert text.count(old) == 1, old
        text = text.replace(old, new)
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=plants.TARGET)
    try:
        open(full, 'w').write(text)
        failed, broken = plants.tests(env)
    finally:
        open(full, 'w').write(orig)
    print(f'at {head}, {name}: failed = {failed} {broken or ""}', flush=True)
    st = subprocess.run(['git', 'status', '--short'], cwd=root, capture_output=True, text=True)
    print(f'git status --short in the checkout: {st.stdout.strip() or "(clean)"}', flush=True)


if __name__ == '__main__':
    main()
