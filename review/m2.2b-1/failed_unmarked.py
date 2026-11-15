#!/usr/bin/env python3
"""#100 at 07a9f2d: a volume that failed, whose marker could not be written, and the next start.

    failed_unmarked.py CHECKOUT TARGET_DIR

Design §5.2: the state is trusted only if the volume has not failed since, by the next start
(the instance that saw the failure goes on reading its held cuts). At the store the next start
knows of a failure by the marker in the volume's directory, which the failed disk may not take,
or by the acknowledgement the lifecycle passes for its failure note.

The test here: cut 1 is held, with a copy made for it; the disk takes nothing more (a write
that needs no copy fails, the volume fails, and the marker cannot be written); the daemon is
lost; the disk is back. The next start, which is
told of nothing to acknowledge, is run as the head has it. The file is restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
TEST = r'''
#[test]
fn review_a_volume_that_failed_with_no_marker_written() {
    let disk = created(77, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    // Block 5 changes once, and its copy is made for cut 1.
    write(&mut store, 5, 0x66).unwrap();
    // The disk takes nothing more: the next write of block 5, which
    // needs no copy, fails; the volume fails, and its marker cannot be
    // written.
    disk.halt_after(0);
    let failed = write(&mut store, 5, 0x77);
    println!("review: the write: {failed:?}; the volume has failed: {}", store.is_failed());
    assert!(store.is_failed());
    drop(store);
    disk.kill();
    disk.clear_faults();
    let opened = Opened::open_with(
        disk.clone(),
        lock(&disk).expect("the lock"),
        holding(&region, false),
        Mutants(0),
    )
    .map_err(|(e, _)| e);
    match opened {
        Err(e) => println!("review: the next open is refused: {e:?}"),
        Ok(opened) => {
            let store = opened
                .start_with(None, Mutants(0))
                .expect("the in-place store");
            let (taken, held) = after_start(&store);
            println!("review: the next start: {taken:?}, held {held:?}");
            assert!(
                held.is_empty(),
                "the volume failed, and the next start holds {held:?}: {taken:?}"
            );
        }
    }
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'review_a_volume_that_failed', '--nocapture']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    path = os.path.join(ROOT, TESTS)
    orig = open(path).read()
    try:
        with open(path, 'w') as f:
            f.write(orig + TEST)
        p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
        print('exit', p.returncode)
        for ln in (p.stdout + p.stderr).splitlines():
            s = ln.strip()
            if ('review: ' in s or s.startswith(('error', 'the volume failed'))
                    or (s.startswith('test ') and 'FAILED' in s)
                    or (s.startswith('test result') and ' 0 passed; 0 failed' not in s)):
                print(s[s.find('review: '):][:300] if 'review: ' in s else s[:300])
    finally:
        with open(path, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
