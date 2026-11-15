#!/usr/bin/env python3
"""#106 at 7729360: Codex's P2 (inplace.rs, the store's drop), by a run.

With I/O threads, a write into a clean region waits for the map's sync and a read of
its block waits behind it. The disk lets the sync end, and the store is dropped
without another poll. The drop loops while a request is deferred and only collects
the threads' completions; nothing in it looks at the map's sync, so the write stays
waiting, the read stays behind it, and the drop does not end.

The script appends one test to inplace/tests/cuts.rs of CHECKOUT, runs it, and
restores the file.

    drop_while_waiting.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
TEST = r'''
#[test]
fn review_a_store_dropped_while_a_write_waits_for_the_map() {
    for with_read in [false, true] {
        let disk = created(43, 0);
        let mut store = committed_one(&disk, 0, IoMode::default());
        let home_writes = disk.writes_of(crate::store::HOME_NAME);
        disk.hold_syncs_of(intent::INTENT_NAME);
        submit(&mut store, 1, write_op(150, 0x66));
        if with_read {
            submit(&mut store, 2, read_op(150));
        }
        let mut done = Vec::new();
        polled(&mut store, &mut done, 1, Duration::from_millis(100));
        assert!(done.is_empty(), "both wait while the disk holds the sync");
        // The sync may end now; the store is dropped without a poll.
        disk.release_syncs();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(store);
            let _ = tx.send(());
        });
        let ended = rx.recv_timeout(Duration::from_secs(15)).is_ok();
        let written = disk.writes_of(crate::store::HOME_NAME) - home_writes;
        eprintln!(
            "a read behind the write: {with_read}; the drop ended in 15 s: {ended}; \
             writes of the image: {written}"
        );
        assert!(ended, "the store's drop did not end in 15 s");
    }
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
                            'review_a_store_dropped', '--', '--nocapture'],
                           cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
        for ln in (p.stdout + p.stderr).splitlines():
            if ln.startswith(('test inplace', 'a read behind', 'error', 'thread')) or 'did not end' in ln \
                    or ('test result' in ln and ' 0 passed; 0 failed' not in ln):
                print(ln[:300])
        print('exit', p.returncode)
    finally:
        with open(full, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
