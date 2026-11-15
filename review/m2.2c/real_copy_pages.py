#!/usr/bin/env python3
"""#104 at 3251b30: on real files, does a copy leave a page of copies.img in the kernel's cache?

    real_copy_pages.py CHECKOUT TARGET_DIR

The pull request's test counts the simulated disk's pages. This asks the kernel (cachestat,
through the tests' own `skyzfs_ublk_sys::cached_pages`) on the file system the tests' temporary
directory is on, after three copies are made for a held cut and after the cut is read from one.
Run as the head has it, with a copy written through the cache (W) and with a copy read through
it (R). The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
INP = 'crates/skyzfs-local/src/inplace.rs'
TESTS = 'crates/skyzfs-local/tests/inplace_real.rs'
TEST = r'''
#[cfg(feature = "test-hooks")]
#[test]
fn review_a_copy_on_a_real_file_leaves_no_page() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        println!("review: skipped: the directory's file system does not do");
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
    let copies = std::fs::File::open(tmp.path().join("copies.img")).unwrap();
    let len = copies.metadata().unwrap().len();
    println!(
        "review: held {:?}; copies.img is {len} bytes, {} bytes copied; its pages in the cache after the copies: {:?}",
        store.held_cuts().iter().map(|c| c.number).collect::<Vec<_>>(),
        store.cut_figures().copied,
        skyzfs_ublk_sys::cached_pages(&copies)
    );
    let mut buf = Buf::filled(1, 0xEE);
    let read = store.read_cut(1, 6, &mut buf);
    println!(
        "review: cut 1 reads block 6 as {:#x} ({read:?}); pages in the cache after the read: {:?}",
        buf[0],
        skyzfs_ublk_sys::cached_pages(&copies)
    );
}
'''
WRITE = "        if self.direct {\n            file.write_direct_at(at, buf)\n        } else {\n            file.write_at(at, buf)\n        }\n"
READ = "                if self.io.direct {\n                    file.read_direct_at(at, buf)?;\n                } else {\n                    file.read_at(at, buf)?;\n                }\n"
CASES = [
    ('as the head has it', None, None),
    ('W a copy is written through the cache', WRITE, "        file.write_at(at, buf)\n"),
    ('R a copy is read through the cache', READ, "                file.read_at(at, buf)?;\n"),
]
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--test', 'inplace_real', '--',
       'review_a_copy_on_a_real_file', '--nocapture']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    inp, tests = os.path.join(ROOT, INP), os.path.join(ROOT, TESTS)
    orig_inp, orig_tests = open(inp).read(), open(tests).read()
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + TEST)
        for name, old, new in CASES:
            print(name + ':', flush=True)
            if old:
                assert orig_inp.count(old) == 1, name
                with open(inp, 'w') as f:
                    f.write(orig_inp.replace(old, new))
            p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
            print('    exit', p.returncode)
            for ln in (p.stdout + p.stderr).splitlines():
                s = ln.strip()
                if 'review: ' in s:
                    print('    ' + s[s.find('review: '):][:300])
                elif s.startswith('error') or (s.startswith('test ') and 'FAILED' in s):
                    print('    ' + s[:300])
            with open(inp, 'w') as f:
                f.write(orig_inp)
    finally:
        with open(inp, 'w') as f:
            f.write(orig_inp)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()
