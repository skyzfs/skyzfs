#!/usr/bin/env python3
"""#106 at ab2d7f2: the one plant of plants2.py that passed (D2c), by a run.

D2c takes `self.run_deferred()` out of the drop's loop. No test of the head fails
without it. This script shows the case that needs the line: a store with no I/O
threads, a write into a clean region waits for the map's sync, a read of its block
waits behind it, and the volume fails (the test hook `fail_now`). The failure answers
the write; the read stays deferred, with nothing left that waits for the map. The
disk still holds the map's sync when the store is dropped.

The script appends one test to inplace/tests/cuts.rs of CHECKOUT, runs it as the head
has it and then with D2c planted, and restores both files.

    failed_while_deferred.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
PLANT = ("            self.collect(Duration::from_millis(10));\n            self.run_deferred();\n        }\n        self.pool = None;\n",
         "            self.collect(Duration::from_millis(10));\n        }\n        self.pool = None;\n")
TEST = r'''
#[test]
fn review_a_store_that_failed_with_a_read_deferred_is_dropped() {
    for io in [IoMode::default(), ON_CALLER] {
        let disk = created(47, 0);
        let mut store = committed_one(&disk, 0, io);
        disk.hold_syncs_of(intent::INTENT_NAME);
        submit(&mut store, 1, write_op(150, 0x66));
        submit(&mut store, 2, read_op(150));
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(50));
        assert!(done.is_empty(), "both wait while the disk holds the sync");
        store.fail_now();
        polled(&mut store, &mut done, 2, Duration::from_millis(200));
        eprintln!(
            "{io:?}: answered within 200 ms of the failure, the map's sync still held: {:?}",
            ended(&done)
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(store);
            let _ = tx.send(());
        });
        let ended = rx.recv_timeout(Duration::from_secs(15)).is_ok();
        eprintln!("{io:?}: the drop ended in 15 s, the map's sync still held: {ended}");
        disk.release_syncs();
    }
}
'''


def run(label, env):
    p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks',
                        'review_a_store_that_failed', '--', '--nocapture'],
                       cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
    print(label)
    for ln in (p.stdout + p.stderr).splitlines():
        if ln.startswith(('test inplace', 'IoMode', 'error', 'thread')) \
                or ('test result' in ln and ' 0 passed; 0 failed' not in ln):
            print('  ' + ln[:300])
    print('  exit', p.returncode, flush=True)


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    tests, inp = os.path.join(ROOT, TESTS), os.path.join(ROOT, INP)
    orig_t, orig_i = open(tests).read(), open(inp).read()
    assert orig_i.count(PLANT[0]) == 1
    try:
        with open(tests, 'w') as f:
            f.write(orig_t + TEST)
        run('as the head has it', env)
        with open(inp, 'w') as f:
            f.write(orig_i.replace(PLANT[0], PLANT[1]))
        run('with D2c planted (no run_deferred in the drop)', env)
    finally:
        with open(tests, 'w') as f:
            f.write(orig_t)
        with open(inp, 'w') as f:
            f.write(orig_i)
    print('finished')


if __name__ == '__main__':
    main()
