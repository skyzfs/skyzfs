#!/usr/bin/env python3
"""#98 at e905c2e: what the next instance says of the newest cut once that cut is let go.

    newest_itself.py CHECKOUT TARGET_DIR

`take_up_cut_state` (inplace.rs) reads whether the newest cut opens as itself from the held cut
whose number is the count. The region keeps the newest cut's state beside the count, but not
that flag, so once the newest cut is no longer held (uploaded and let go, or abandoned at the
bound) a next instance says it does not open as itself.

The test: group 7 is cut and opens as itself; the cut is let go as uploaded; the daemon is
lost; the next instance is asked for the newest cut. Appended to tests/cuts.rs, run, restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
TEST = r'''
#[test]
fn review_the_newest_cut_after_it_is_let_go_and_the_daemon_is_lost() {
    for let_go in [false, true] {
        let disk = created(34, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, ON_CALLER);
        group(&mut store, 100, 7, 70);
        assert_eq!(newest(&store), Some((7, true)));
        if let_go {
            assert!(store.release_cut(1));
        }
        drop(store);
        disk.kill();
        let store = start_on(&disk, 0, &region, ON_CALLER);
        println!(
            "review: let go before the death: {let_go}; the next instance: count {}, newest {:?}",
            store.cuts().count,
            newest(&store)
        );
        assert_eq!(store.cuts().count, 1);
        assert_eq!(newest(&store), Some((7, true)), "let go: {let_go}");
    }
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--',
       'review_the_newest_cut_after_it_is_let_go_and_the_daemon_is_lost', '--nocapture']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    tests = os.path.join(ROOT, TESTS)
    orig = open(tests).read()
    try:
        with open(tests, 'w') as f:
            f.write(orig + TEST)
        p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    finally:
        with open(tests, 'w') as f:
            f.write(orig)
    out = p.stdout + p.stderr
    print('exit', p.returncode)
    for ln in out.splitlines():
        if (ln.startswith(('review:', 'error')) or 'assertion' in ln or ln.lstrip().startswith(('left:', 'right:'))
                or (ln.startswith('test result') and ' 0 passed; 0 failed' not in ln)):
            print('   ', ln.strip()[:300])


if __name__ == '__main__':
    main()
