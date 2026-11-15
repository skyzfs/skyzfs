#!/usr/bin/env python3
"""#107 at b6aaad8 (M2.2d): a failed run in the middle of a part, with an older cut
that stays. Two scenarios the head's tests do not have, each on the three held cuts of
`three_held` (cut 2 uploading with block 20; cut 3 waiting with 30 to 32, 100, 170;
cut 4's flush under way with 40; block 50 in the open map).

X  change(20, 21): runs [20] for cut 2, [30..32] for cut 3, [40] for cut 4. The second
   run's write fails. Cuts 3 and 4 go; cut 2 stays and must read block 20 from its
   copy, and the next cut's map must hold every block of the change.
Y  change(30, 141): runs [30..32] for cut 3, [40] for cut 4, [100], [170] for cut 3.
   The second run's write fails. Cut 4 goes alone; cut 3 stays and must read 100 and
   170 from copies made when the rest is planned again.

The script appends one test to inplace/state/tests.rs of CHECKOUT and runs it as the
head has it, then under each plant named (A2, A3, A4 of plants.py), and restores the files.

    failed_run_scenarios.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/state/tests.rs'
ST = 'crates/skyzfs-local/src/inplace/state.rs'
PLANTS = [
    ('as the head has it', []),
    ('A2 after a failed run, the copies not made are recorded too',
     [("        for copy in &plan.copies[..made] {\n            let base = self.record_base(copy.slot);\n",
       "        for copy in &plan.copies {\n            let base = self.record_base(copy.slot);\n"),
      ("        for copy in plan.copies[made..].iter().rev() {\n            self.free_records.push(copy.slot);\n        }\n", "")]),
    ('A3 after a failed run, the blocks before it get no bits',
     [("        let block = plan.copies[made].block;\n        self.set_bits(plan.from, block);\n",
       "        let block = plan.copies[made].block;\n")]),
    ("A4 after a failed run, its first block gets no bit",
     [("        let open = self.open();\n        self.maps.insert(&mut self.region, open.first, block);\n        Ok(block + 1)\n",
       "        Ok(block + 1)\n")]),
]
TEST = r'''
/// The review's copier: the `nth` write of copies fails, once.
struct ReviewFailNth {
    disk: Disk,
    nth: u64,
    writes: u64,
    calls: Vec<(u32, usize, bool)>,
}

impl Copier for ReviewFailNth {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        self.disk.read_image(first, buf)
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        self.writes += 1;
        let ok = self.writes != self.nth;
        self.calls.push((first, buf.len() / BLOCK_SIZE as usize, ok));
        if !ok {
            return Err(io::Error::other("the review's failure"));
        }
        self.disk.write_copies(first, buf)
    }
}

#[test]
fn review_a_failed_run_with_an_older_cut_that_stays() {
    for (name, first, count, cut, blocks) in [
        ("X", 20u64, 21u64, 2u64, vec![20u64]),
        ("Y", 30, 141, 3, vec![30, 31, 32, 100, 170]),
    ] {
        let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
        let (mut state, io) = three_held(&region);
        let mut io = ReviewFailNth {
            disk: io.disk,
            nth: 2,
            writes: 0,
            calls: Vec::new(),
        };
        for block in 0..BLOCKS {
            io.disk.image[block as usize] = 1000 + block;
        }
        state.change(first, count, &mut io).expect("the change goes on");
        for block in first..first + count {
            io.disk.image[block as usize] = 9000 + block;
        }
        let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
        eprintln!("{name}: writes of copies (slot, blocks, ok): {:?}", io.calls);
        eprintln!("{name}: held after the change: {held:?}; copied {} blocks", state.figures().copied / BLOCK_SIZE);
        let mut wrong = Vec::new();
        for &block in &blocks {
            let got = match state.read(cut, block) {
                Some(Read::Copy(slot)) => io.disk.copies.get(&slot).copied(),
                Some(Read::Image) => Some(io.disk.image[block as usize]),
                _ => None,
            };
            if got != Some(1000 + block) {
                wrong.push((block, state.read(cut, block), got));
            }
        }
        eprintln!("{name}: cut {cut} reads wrongly (block, how, content): {wrong:?}");
        let next = state.freeze(&marked(5), true);
        let map = state.map(next);
        let lacks: Vec<u64> = (first..first + count).filter(|b| !map.contains(b)).collect();
        eprintln!("{name}: the next cut's map lacks of the change's blocks: {lacks:?}");
    }
}
'''


def run(env):
    p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks',
                        'review_a_failed_run', '--', '--nocapture'],
                       cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
    for ln in (p.stdout + p.stderr).splitlines():
        if ln.startswith(('test inplace', 'X:', 'Y:', 'error', 'thread')) \
                or ('test result' in ln and ' 0 passed; 0 failed' not in ln):
            print('  ' + ln[:400])
    print('  exit', p.returncode, flush=True)


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    tests, st = os.path.join(ROOT, TESTS), os.path.join(ROOT, ST)
    orig_t, orig_s = open(tests).read(), open(st).read()
    try:
        with open(tests, 'w') as f:
            f.write(orig_t + TEST)
        for name, edits in PLANTS:
            text = orig_s
            for old, new in edits:
                assert text.count(old) == 1, old[:60]
                text = text.replace(old, new)
            with open(st, 'w') as f:
                f.write(text)
            print(name, flush=True)
            run(env)
    finally:
        with open(tests, 'w') as f:
            f.write(orig_t)
        with open(st, 'w') as f:
            f.write(orig_s)
    print('finished')


if __name__ == '__main__':
    main()
