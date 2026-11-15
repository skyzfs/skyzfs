#!/usr/bin/env python3
"""#100 at 59d09bf: the three tests of `refused_copy.py` under further plants of `plants.py`.

    refused_copy_more.py CHECKOUT TARGET_DIR PLANT [PLANT ...]

PLANT is a plant's name in `plants.py` (A7, A9, ...), in the cut state's file. For each, the
three tests are appended, the plant applied, the tests run, and both files restored.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402
import refused_copy as rc  # noqa: E402

NAMES = sys.argv[3:]


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=rc.TARGET)
    state, tests = os.path.join(rc.ROOT, rc.STATE), os.path.join(rc.ROOT, rc.TESTS)
    orig_state, orig_tests = open(state).read(), open(tests).read()
    chosen = [p for p in plants.PLANTS if p[0].split()[0] in NAMES]
    assert len(chosen) == len(NAMES), [p[0] for p in chosen]
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + rc.TEST)
        for name, path, old, new in chosen:
            assert path == rc.STATE, (name, path)
            assert orig_state.count(old) == 1, name
            print(name + ':', flush=True)
            with open(state, 'w') as f:
                f.write(orig_state.replace(old, new))
            rc.run(env)
            with open(state, 'w') as f:
                f.write(orig_state)
    finally:
        with open(state, 'w') as f:
            f.write(orig_state)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()
