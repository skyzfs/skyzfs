#!/usr/bin/env python3
"""#98 at e905c2e: a write-zeroes over a block of a held cut, through the store.

    zeroes.py CHECKOUT TARGET_DIR

The store notes a write-zeroes in the cut state before it zeroes (`inplace.rs`, `Op::WriteZeroes`).
With that call removed (plants.py I1) every test of the pull request passes: the model test
zeroes through `CutState::change` itself, the store tests only write, and the conformance suite
holds no cut.

The test: block 5 holds 0x55 and block 6 0x66; group 7 is cut; blocks 5 and 6 are zeroed; the
held cut still reads 0x55 and 0x66, and the volume reads zeros. Run as the head has it and with
I1. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
INP = 'crates/skyzfs-local/src/inplace.rs'
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
TEST = r'''
#[test]
fn review_a_held_cut_reads_as_the_volume_at_its_instant_after_a_write_zeroes() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let disk = created(36, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, io);
        write(&mut store, 5, 0x55).unwrap();
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 100, 7, 70);
        let zeroed = run(
            &mut store,
            300,
            Op::WriteZeroes {
                offset: 5 * BLOCK,
                len: 2 * BLOCK,
            },
        );
        assert!(zeroed.result.is_ok(), "{io:?}");
        assert_eq!(read(&mut store, 5).unwrap(), vec![0; BLOCK_USIZE], "{io:?}");
        assert_eq!(cut_block(&mut store, 1, 5), vec![0x55; BLOCK_USIZE], "{io:?}");
        assert_eq!(cut_block(&mut store, 1, 6), vec![0x66; BLOCK_USIZE], "{io:?}");
    }
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--',
       'review_a_held_cut_reads_as_the_volume_at_its_instant_after_a_write_zeroes']


def plant():
    src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'plants.py')).read()
    ns = {}
    exec(src.split('def main')[0].replace('ROOT, TARGET = sys.argv[1], sys.argv[2]', 'ROOT = TARGET = ""')
         .replace('ONLY = sys.argv[3:]', 'ONLY = []'), ns)
    for name, _path, old, new in ns['PLANTS']:
        if name.startswith('I1 '):
            return old, new
    raise SystemExit('no I1')


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    print('    exit', p.returncode, flush=True)
    for ln in out.splitlines():
        if (ln.startswith('error') or 'assertion' in ln or ln.lstrip().startswith(('left:', 'right:'))
                or (ln.startswith('test result') and ' 0 passed; 0 failed' not in ln)):
            print('   ', ln.strip()[:200], flush=True)


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    inp, tests = os.path.join(ROOT, INP), os.path.join(ROOT, TESTS)
    orig_inp, orig_tests = open(inp).read(), open(tests).read()
    old, new = plant()
    assert orig_inp.count(old) == 1
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + TEST)
        print('as the head has it:', flush=True)
        run(env)
        with open(inp, 'w') as f:
            f.write(orig_inp.replace(old, new))
        print('with a write-zeroes not noted (I1):', flush=True)
        run(env)
    finally:
        with open(inp, 'w') as f:
            f.write(orig_inp)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()
