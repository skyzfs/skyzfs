#!/usr/bin/env python3
"""#105 at ccd17dd: two writes of a bit page while one sync runs.

A page is written with a bit cleared and then again with the bit set, both
while a sync of an earlier write of the page runs. When that sync ends,
the map calls the bit durable, though no sync has covered the write that
set it again: after a power cut the disk may hold the write between, in
which the bit is clear.

The script appends one test to intent/tests.rs of CHECKOUT, runs it, and
restores the file.

    two_writes_one_sync.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/intent/tests.rs'
TEST = r'''
#[test]
fn review_two_writes_of_a_page_during_one_sync() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    let mut file = settled(&mut map);
    map.commit(BASE);
    // Cut 3 is committed: regions 0, 1 and 2 changed after it.
    map.clear(|first, _| first < 3 * 64);
    write_out(&mut map, &mut file); // W1: 0, 1, 2
    map.sync_begin(); // S1 covers W1
    // While S1 runs, the next cut is committed: region 2 is let go, and
    // its page is written (a clear needs no sync).
    map.clear(|first, _| first < 2 * 64);
    write_out(&mut map, &mut file); // W2: 0, 1
    let after_w2 = file.clone();
    assert!(!map.covers(2 * 64, 1), "W2 took the bit");
    // A write into region 2 arrives: the bit is set and its page written.
    map.set(2 * 64, 1);
    write_out(&mut map, &mut file); // W3: 0, 1, 2
    eprintln!("before S1 ends: covers region 2 = {}", map.covers(2 * 64, 1));
    map.synced(true); // S1 ends: it covered W1, and neither W2 nor W3
    let covers = map.covers(2 * 64, 1);
    eprintln!("after S1 ends:  covers region 2 = {covers}");
    // The power cut now: S1 made W1 certain; W2 is kept and W3 is lost.
    let read = IntentMap::read(&after_w2, shape(), Mutants::default()).expect("whole");
    eprintln!(
        "the disk with W2 kept and W3 lost: trust {:?}, names {:?}",
        read.trust(Some(T1)),
        read.named()
    );
    assert!(
        !covers,
        "the map lets a write into region 2 begin, and no sync has covered the write that set \
         its bit: the disk may hold W2, which names {:?}",
        read.named()
    );
}
'''


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    full = os.path.join(ROOT, TESTS)
    orig = open(full).read()
    try:
        with open(full, 'w') as f:
            f.write(orig + TEST)
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks',
                            'review_two_writes', '--', '--nocapture'],
                           cwd=ROOT, capture_output=True, text=True, env=env)
        for ln in (p.stdout + p.stderr).splitlines():
            if ln.startswith(('test ', 'before', 'after', 'the disk', 'the map lets', 'error', 'thread')) \
                    or 'names [' in ln or 'test result' in ln and ' 0 passed; 0 failed' not in ln:
                print(ln)
        print('exit', p.returncode)
    finally:
        with open(full, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
