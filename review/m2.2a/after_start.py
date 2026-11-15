#!/usr/bin/env python3
"""#98 at e905c2e: what the start leaves, where the model test adopts it unchecked.

    after_start.py CHECKOUT TARGET_DIR

After a death the model test's rules take the held cuts the restart left (`World::adopt`), with
their phases, and the step is done again at once. Three plants in `settle` pass every test of
the pull request for that reason (plants.py S7, S21; plants2.py P1). Two additions notice them:

1. In `World::restart`, after the start: no held cut's flush is still under way, and where a
   cut is held one is being uploaded. (S7: a cut left flushing is never the one that waits or
   is uploaded, its flush's completion being no longer known to the store; P1: the cut that
   waits is never uploaded.)
2. A fixed case for "a cut recorded but not counted is none": the daemon is lost in cut 3's
   freeze after the first word of its record (slot 2), and before the flush comes again cut 1
   is uploaded and let go, which frees slot 0. The freeze done again takes slot 0, and the
   count's store then makes the stale record in slot 2 a held cut too. (S21.)

Each is run as the head has it and with the plant. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
HERE = os.path.dirname(os.path.abspath(__file__))
ST = 'crates/skyzfs-local/src/inplace/state.rs'
TESTS = 'crates/skyzfs-local/src/inplace/state/tests.rs'

CHECK_OLD = "        let after = self.state.held();\n"
CHECK_NEW = CHECK_OLD + r'''        if after.iter().any(|c| c.phase == Phase::Flushing) {
            return Err(format!("review: after a start a cut's flush is still under way: {after:?}"));
        }
        if !after.is_empty() && !after.iter().any(|c| c.phase == Phase::Uploading) {
            return Err(format!("review: after a start no cut is being uploaded: {after:?}"));
        }
'''
SLOT_TEST = r'''
/// Review: a cut recorded and not counted is none, whatever comes before the freeze is done
/// again.
#[test]
fn review_a_cut_recorded_and_not_counted_is_none() {
    quiet_deaths();
    let bound = 64 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let mut state = CutState::begin(region.clone(), SIZE, bound, Mutants(0));
    let mut disk = Disk {
        image: vec![0; BLOCKS as usize],
        copies: BTreeMap::new(),
    };
    state.change(0, 1, &mut disk).unwrap();
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    state.change(1, 1, &mut disk).unwrap();
    assert_eq!(state.freeze(&marked(2), true), 2);
    state.flushed(2);
    state.change(2, 1, &mut disk).unwrap();
    // Lost in cut 3's freeze, after the first word of its record.
    region.die_at(1);
    let lost = catch_unwind(AssertUnwindSafe(|| state.freeze(&marked(3), true)));
    assert!(lost.is_err());
    region.revive();
    let mut state = CutState::restore(region.clone(), SIZE, bound, Mutants(0)).unwrap();
    // Cut 1 is uploaded before the flush comes again.
    assert!(state.release(1));
    state.change(3, 1, &mut disk).unwrap();
    assert_eq!(state.freeze(&marked(3), true), 3);
    let held: Vec<(u64, Phase)> = state.held().iter().map(|c| (c.number, c.phase)).collect();
    assert_eq!(held, [(2, Phase::Uploading), (3, Phase::Flushing)]);
    assert_eq!(state.map(3), [2, 3]);
}
'''


def plant(prefix):
    for script in ('plants.py', 'plants2.py'):
        src = open(os.path.join(HERE, script)).read()
        ns = {}
        exec(src.split('def main')[0].replace('ROOT, TARGET = sys.argv[1], sys.argv[2]', 'ROOT = TARGET = ""')
             .replace('ONLY = sys.argv[3:]', 'ONLY = []'), ns)
        for name, _path, *pairs in ns['PLANTS']:
            if name.startswith(prefix + ' '):
                return name, pairs[0], pairs[1]
    raise SystemExit(f'no plant {prefix}')


def run(env, what):
    p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', what],
                       cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out.splitlines()
                     if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    said = [ln for ln in out.splitlines() if 'review:' in ln or ln.startswith('error[')
            or 'assertion `left == right` failed' in ln or ln.lstrip().startswith(('left:', 'right:'))]
    return p.returncode, failed, said


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    st, tests = os.path.join(ROOT, ST), os.path.join(ROOT, TESTS)
    orig_st, orig_tests = open(st).read(), open(tests).read()
    assert orig_tests.count(CHECK_OLD) == 1
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests.replace(CHECK_OLD, CHECK_NEW) + SLOT_TEST)
        code, failed, said = run(env, 'inplace::state')
        print('as the head has it, with both additions: exit', code, failed, flush=True)
        for prefix, what in (('S7', 'inplace::state'), ('P1', 'inplace::state'),
                             ('S21', 'review_a_cut_recorded_and_not_counted_is_none')):
            name, old, new = plant(prefix)
            assert orig_st.count(old) == 1, name
            with open(st, 'w') as f:
                f.write(orig_st.replace(old, new))
            try:
                code, failed, said = run(env, what)
            finally:
                with open(st, 'w') as f:
                    f.write(orig_st)
            print(f'{name}: ' + ('NOTICED by ' + ', '.join(failed) if code else 'not noticed'), flush=True)
            for ln in said[:3]:
                print('   ', ln.strip()[:330], flush=True)
    finally:
        with open(st, 'w') as f:
            f.write(orig_st)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()
