#!/usr/bin/env python3
"""#105 at ccd17dd: three checks around the defect of two_writes_one_sync.py. Each
changes files of CHECKOUT, runs the intent map's tests, and restores the files.

1. `fix`: a page written while a sync runs takes from that sync the bits the write
   clears (one way to mend the defect). The reproducer's test then passes, and so do
   the pull request's own tests.
2. `sharper`: the pull request's test for a bit cleared by a later write, with the
   region set again after the sync ends. With plant A1 (a sync makes durable what it
   covered, whatever was written since) the test as it is passes; this one fails.
3. `sector`: the checksum made and checked over a page's first sector only, to see
   whether the model's torn writes are noticed outside the first sector.

    fix_check.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
INT = 'crates/skyzfs-local/src/inplace/intent.rs'
TESTS = 'crates/skyzfs-local/src/inplace/intent/tests.rs'
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import two_writes_one_sync as repro  # noqa: E402

FIX = ("                self.unsynced.insert(page, words);\n",
       "                if let Some(covered) = self.syncing.as_mut().and_then(|s| s.get_mut(&page)) {\n"
       "                    for (c, word) in covered.iter_mut().zip(&words) {\n"
       "                        *c &= word;\n"
       "                    }\n"
       "                }\n"
       "                self.unsynced.insert(page, words);\n")
A1 = ("self.durable[from + i] |= word & self.written[from + i];", "self.durable[from + i] |= word;")
SECTOR = [("    crc32c::crc32c(&page[..SUM_AT]) == sum\n", "    crc32c::crc32c(&page[..512]) == sum\n"),
          ("    let sum = crc32c::crc32c(&page[..SUM_AT]);\n    page[SUM_AT..]", "    let sum = crc32c::crc32c(&page[..512]);\n    page[SUM_AT..]")]
SHARPER = r'''
#[test]
fn review_a_bit_cleared_by_a_later_write_and_set_again() {
    let mut map = IntentMap::new(shape(), T1, Mutants::default());
    let mut file = settled(&mut map);
    map.commit(BASE);
    map.clear(|first, _| first < 3 * 64);
    write_out(&mut map, &mut file);
    map.sync_begin();
    // While that sync runs, region 2 is let go and the page written.
    map.clear(|first, _| first < 2 * 64);
    write_out(&mut map, &mut file);
    map.synced(true);
    // A write into region 2 arrives after the sync has ended.
    map.set(2 * 64, 1);
    assert!(
        !map.covers(2 * 64, 1),
        "the disk holds the page with region 2's bit clear, and the map lets a write begin"
    );
    assert!(map.has_dirty(), "the page is to be written again");
}
'''


def run(changes, extra_test, title):
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    paths = {INT, TESTS}
    orig = {p: open(os.path.join(ROOT, p)).read() for p in paths}
    try:
        text = orig[INT]
        for old, new in changes:
            assert text.count(old) == 1, old
            text = text.replace(old, new)
        with open(os.path.join(ROOT, INT), 'w') as f:
            f.write(text)
        with open(os.path.join(ROOT, TESTS), 'w') as f:
            f.write(orig[TESTS] + extra_test)
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast', 'intent'],
                           cwd=ROOT, capture_output=True, text=True, env=env)
        out = (p.stdout + p.stderr).splitlines()
        failed = sorted({ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
        passed = sum(1 for ln in out if ln.startswith('test ') and ln.rstrip().endswith('ok'))
        errors = [ln for ln in out if ln.startswith('error')][:3]
        print(f'{title}: {passed} passed, failed {failed}' + (f', errors {errors}' if errors and not failed else ''), flush=True)
    finally:
        for p in paths:
            with open(os.path.join(ROOT, p), 'w') as f:
                f.write(orig[p])


def main():
    run([], repro.TEST + SHARPER, 'the head, with the two tests of this review')
    run([FIX], repro.TEST + SHARPER, 'fix: a write during a sync takes its cleared bits from that sync')
    run([A1], '', 'A1 with the pull request\'s tests')
    run([A1], SHARPER, 'A1 with the sharper test')
    run(SECTOR, '', 'sector: the checksum over the first sector only')


if __name__ == '__main__':
    main()
