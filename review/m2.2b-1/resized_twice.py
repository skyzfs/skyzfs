#!/usr/bin/env python3
"""#100 at 5fd84fb: Codex's P2 on `skyzfs-mapped/src/linux.rs:95`, at the store.

    resized_twice.py CHECKOUT TARGET_DIR [open]

With `open` the two lengths are given through `Mapped::open` itself (which is not refused here,
and does to the file what a refused open has done by the time the lock fails), so that a fix in
that open is seen. Without it the file's length is set directly, as the open did at 5fd84fb.

`Mapped::open` gives a region's file the length asked before it allocates, maps and locks it,
and the store's open maps the region before it writes its start token. So an open with another
bound that is refused at the lock has changed the file's length and nothing else, and a second
one with the first bound, refused too, gives the length back, with zeros where the records of
the copies were. The third open finds the length it asks for (`Opened::Kept`), the header and
the token as they were.

The test here, on real files: cut 1 is held and blocks 5, 6 and 7 were copied for it (a bound of
one block keeps the first record). The file is then
given the length a bound of one block asks for and the first length again, which is all the two
refused opens do to it (the lock cannot be made to fail here: the tests run with the capability
that lifts its limit). The next start is run as the head has it. The file is restored.

Whether `serve` can get there: it passes no bound (`bound: None`), so a volume's region has one
length as long as the default bound and the layout stay as they are.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
THROUGH_OPEN = sys.argv[3:] == ['open']
TESTS = 'crates/skyzfs-local/tests/inplace_real.rs'
TEST = r'''
#[cfg(feature = "test-hooks")]
#[test]
fn review_a_region_shrunk_and_regrown_by_two_refused_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        return;
    };
    create(&dir, &config()).unwrap();
    let run_dir = tempfile::tempdir().unwrap();
    let region = run_dir.path().join("cuts.state");
    let mut store = start_holding(&dir, &region, BOOT, false);
    for block in 5..8 {
        write(&mut store, block, Buf::filled(1, 0x55));
    }
    assert_eq!(run(&mut store, uberblock_write(7)).result, Ok(()));
    flush(&mut store);
    for block in 5..8 {
        write(&mut store, block, Buf::filled(1, 0x66));
    }
    let before: Vec<u8> = (5..8)
        .map(|block| {
            let mut buf = Buf::filled(1, 0xEE);
            assert!(store.read_cut(1, block, &mut buf).unwrap());
            buf[0]
        })
        .collect();
    println!("review: before: cut 1 reads blocks 5, 6, 7 as {before:x?}; copied {} bytes", store.cut_figures().copied);
    drop(store);

    // What two refused opens do to the file: the length a bound of one
    // block asks for (the default bound keeps 65536 records of two
    // words), then the first length again.
    let len = std::fs::metadata(&region).unwrap().len();
    let small = len - 65535 * 16;
    if std::env::var_os("REVIEW_THROUGH_OPEN").is_some() {
        // Through the region's own open, which here is not refused:
        // it does to the file what a refused one has done by the lock.
        for bytes in [small, len] {
            let (mapped, opened) = skyzfs_mapped::Mapped::open(&region, (bytes / 8) as usize).unwrap();
            println!("review: an open with {bytes} bytes: {opened:?}");
            drop(mapped);
        }
    } else {
        let file = std::fs::OpenOptions::new().write(true).open(&region).unwrap();
        file.set_len(small).unwrap();
        file.set_len(len).unwrap();
        drop(file);
    }
    println!("review: the region's file: {len} bytes, then {small}, then {len}");

    let mut store = start_holding(&dir, &region, BOOT, true);
    let held: Vec<(u64, u64)> = store.held_cuts().iter().map(|c| (c.number, c.state.txg)).collect();
    println!("review: the next start: {:?}, held {held:?}", store.cuts_at_start());
    if !held.is_empty() {
        let after: Vec<(u8, bool)> = (5..8)
            .map(|block| {
                let mut buf = Buf::filled(1, 0xEE);
                let read = store.read_cut(1, block, &mut buf);
                (buf[0], read.is_ok())
            })
            .collect();
        println!("review: cut 1 reads blocks 5, 6, 7 as {after:x?}; at its instant each held 0x55; copied {} bytes", store.cut_figures().copied);
        assert!(after.iter().all(|&(byte, _)| byte == 0x55), "cut 1 reads what was written after its instant");
    }
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--test', 'inplace_real', '--',
       'review_a_region_shrunk', '--nocapture']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if THROUGH_OPEN:
        env['REVIEW_THROUGH_OPEN'] = '1'
    path = os.path.join(ROOT, TESTS)
    orig = open(path).read()
    try:
        with open(path, 'w') as f:
            f.write(orig + TEST)
        p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
        print('exit', p.returncode)
        for ln in (p.stdout + p.stderr).splitlines():
            s = ln.strip()
            if ('review: ' in s or s.startswith(('error', 'cut 1 reads', 'thread '))
                    or (s.startswith('test ') and ('FAILED' in s or '... ok' in s))
                    or (s.startswith('test result') and ' 0 passed; 0 failed' not in s)):
                print(s[s.find('review: '):][:300] if 'review: ' in s else s[:300])
    finally:
        with open(path, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
