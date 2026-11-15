#!/usr/bin/env python3
"""#98 at e905c2e: the newest cut's state, given to the next instance for the repeat rule.

    repeat_after_start.py CHECKOUT TARGET_DIR

ZFS writes a group's uberblock to labels 0 and 2, flushes, writes it to labels 1 and 3, and
flushes again. The first flush makes the cut; the writes to labels 1 and 3 are repeats of the
newest cut's state and mark nothing. If the daemon is lost between the two halves, the next
instance knows that state only from the cut state (`take_up_cut_state` gives it to the rule).

The test: group 7 is cut; group 8's uberblock goes to labels 0 and 2 and the flush cuts it;
the daemon is lost; the next instance gets the writes to labels 1 and 3 and the flush. The
count must stay 2.

Run as the head has it, and with the next instance not given the newest cut (plants.py I6),
which the pull request's tests pass with. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
INP = 'crates/skyzfs-local/src/inplace.rs'
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
TEST = r'''
#[test]
fn review_a_repeat_of_the_newest_cut_marks_nothing_after_a_lost_daemon() {
    let disk = created(35, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    for (i, label) in [0usize, 2].into_iter().enumerate() {
        let done = run(&mut store, 200 + i as u64, uberblock_write(label, 8, 80));
        assert!(done.result.is_ok());
    }
    assert!(run(&mut store, 210, Op::Flush).result.is_ok());
    assert_eq!(store.cuts().count, 2);
    drop(store);
    disk.kill();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    for (i, label) in [1usize, 3].into_iter().enumerate() {
        let done = run(&mut store, 220 + i as u64, uberblock_write(label, 8, 80));
        assert!(done.result.is_ok());
    }
    assert!(run(&mut store, 230, Op::Flush).result.is_ok());
    let held: Vec<(u64, u64)> = store
        .held_cuts()
        .iter()
        .map(|c| (c.number, c.state.txg))
        .collect();
    println!("review: the next instance: count {}, held {held:?}", store.cuts().count);
    assert_eq!(store.cuts().count, 2, "a repeat of the newest cut's uberblock was cut: {held:?}");
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--',
       'review_a_repeat_of_the_newest_cut_marks_nothing_after_a_lost_daemon', '--nocapture']
OLD = "            .restore(cut_state.count(), newest, cut_state.mark());"
NEW = "            .restore(cut_state.count(), None, cut_state.mark());"


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    print('    exit', p.returncode)
    for ln in out.splitlines():
        if (ln.startswith(('review:', 'error')) or 'assertion' in ln or ln.lstrip().startswith(('left:', 'right:'))
                or (ln.startswith('test result') and ' 0 passed; 0 failed' not in ln)):
            print('   ', ln.strip()[:300])


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    inp, tests = os.path.join(ROOT, INP), os.path.join(ROOT, TESTS)
    orig_inp, orig_tests = open(inp).read(), open(tests).read()
    assert orig_inp.count(OLD) == 1
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + TEST)
        print('as the head has it:', flush=True)
        run(env)
        with open(inp, 'w') as f:
            f.write(orig_inp.replace(OLD, NEW))
        print('with the next instance not given the newest cut (I6):', flush=True)
        run(env)
    finally:
        with open(inp, 'w') as f:
            f.write(orig_inp)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()
