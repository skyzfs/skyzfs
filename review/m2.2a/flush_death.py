#!/usr/bin/env python3
"""#98 at e905c2e: a death at each store of the flush that makes a cut, through the store.

    flush_death.py CHECKOUT TARGET_DIR

The pull request's store-level death test (`a_death_at_any_store_of_a_mark_leaves_the_group_cut_whole`)
kills the daemon at the stores of the uberblock WRITE. The stores of the cut's own flush (the
freeze: the cut's record, the count, the newest cut's state, the mark cleared) are reached by a
death only in state/tests.rs, whose world never sets a mark. So nothing checks that after a
death in the freeze the next instance cuts the group once.

The test below does: group 7 is cut; group 8's uberblock is written to the four labels and
completed; the daemon is lost at store `death` of the flush after them; the next instance gets
that flush again (its completion never went out), then one more. The group must be cut once:
the count is 2 and cuts 1 and 2 are held, of groups 7 and 8.

It is run as the head has it, and under plants of plants.py and plants2.py that the pull
request's tests pass with, one at a time. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ST = 'crates/skyzfs-local/src/inplace/state.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
TEST = r'''
/// Review: what a death at store `death` of group 8's flush leaves, after the next instance
/// has had the flush again: whether the run died, the count, and the held cuts' numbers and
/// groups.
fn review_lost_in_a_cut(death: u64) -> (bool, u64, Vec<(u64, u64)>) {
    state::tests::quiet_deaths();
    let disk = created(33, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    for label in 0..LABELS {
        let done = run(&mut store, 200 + label as u64, uberblock_write(label, 8, 80));
        assert!(done.result.is_ok());
    }
    let id = RequestId(210);
    let mut done = Vec::new();
    region.die_at(death);
    let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.submit(Request { id, op: Op::Flush }).unwrap();
        while done.is_empty() {
            store.wait(&mut done, WAIT);
        }
    }));
    region.revive();
    let died = match lived {
        Ok(()) => false,
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => true,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    if died {
        drop(store);
        disk.kill();
        store = start_on(&disk, 0, &region, ON_CALLER);
        // The flush's completion never went out: the kernel issues it again.
        assert!(run(&mut store, 211, Op::Flush).result.is_ok());
    }
    assert!(run(&mut store, 212, Op::Flush).result.is_ok());
    let held = store
        .held_cuts()
        .iter()
        .map(|c| (c.number, c.state.txg))
        .collect();
    (died, store.cuts().count, held)
}

#[test]
fn review_a_death_at_any_store_of_a_cuts_flush_cuts_the_group_once() {
    let mut death = 0;
    loop {
        let (died, count, held) = review_lost_in_a_cut(death);
        assert_eq!(
            (count, held.clone()),
            (2, vec![(1, 7), (2, 8)]),
            "death at store {death}: count {count}, held {held:?}"
        );
        if !died {
            break;
        }
        death += 1;
    }
    assert!(death > 20, "{death} stores");
    println!("review: {death} stores in the flush, each with a death");
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--',
       'inplace::tests::cuts::review_a_death_at_any_store_of_a_cuts_flush_cuts_the_group_once',
       '--nocapture']
PLANTS = [
    ('S19 settle: a mark the newest cut took is kept', ST,
     "        if self.mark() == self.newest() {\n            self.set_mark(None);\n        }\n        for map in 0..MAPS {",
     "        for map in 0..MAPS {"),
    ('S20 settle: the newest cut\'s state is not made good', ST,
     "            if newest.number == count && self.newest() != Some(newest.state) {", "            if false {"),
    ('S38 freeze: the mark the cut took is kept', ST,
     "        if self.mark().as_ref() == Some(state) {\n            self.set_mark(None);\n        }\n", ""),
    ('S7 settle: a cut whose flush was under way does not take the start\'s sync as its flush', ST,
     None, None),
    ('P8 settle: the start clears the mark whatever it is', ST,
     "        if self.mark() == self.newest() {\n            self.set_mark(None);",
     "        if true {\n            self.set_mark(None);"),
    ('I4 store: the next instance is not given the mark', INP,
     "            .restore(cut_state.count(), newest, cut_state.mark());",
     "            .restore(cut_state.count(), newest, None);"),
    ('I5 store: the next instance is not given the count', INP,
     "            .restore(cut_state.count(), newest, cut_state.mark());",
     "            .restore(0, newest, cut_state.mark());"),
    ('I6 store: the next instance is not given the newest cut', INP,
     "            .restore(cut_state.count(), newest, cut_state.mark());",
     "            .restore(cut_state.count(), None, cut_state.mark());"),
]


def s7():
    """Plant S7's text, taken from plants.py beside this file."""
    src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'plants.py')).read()
    ns = {}
    exec(src.split('def main')[0].replace('ROOT, TARGET = sys.argv[1], sys.argv[2]', 'ROOT = TARGET = ""')
         .replace('ONLY = sys.argv[3:]', 'ONLY = []'), ns)
    for name, _path, old, new in ns['PLANTS']:
        if name.startswith('S7 '):
            return old, new
    raise SystemExit('no S7 in plants.py')


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    said = [ln for ln in out.splitlines()
            if 'death at store' in ln or ln.startswith('review:') or ln.startswith('error[')
            or (ln.startswith('test result') and ' 0 passed; 0 failed' not in ln)]
    return p.returncode, said


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    tests = os.path.join(ROOT, TESTS)
    orig_tests = open(tests).read()
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + TEST)
        code, said = run(env)
        print('as the head has it: exit', code, flush=True)
        for ln in said:
            print('   ', ln[:300], flush=True)
        for name, path, old, new in PLANTS:
            if old is None:
                old, new = s7()
            full = os.path.join(ROOT, path)
            orig = open(full).read()
            if orig.count(old) != 1:
                print(f'{name}: NOT APPLIED', flush=True)
                continue
            with open(full, 'w') as f:
                f.write(orig.replace(old, new))
            try:
                code, said = run(env)
            finally:
                with open(full, 'w') as f:
                    f.write(orig)
            print(f'{name}: ' + ('NOTICED' if code else 'not noticed'), flush=True)
            for ln in said[:2]:
                print('   ', ln[:300], flush=True)
    finally:
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()
