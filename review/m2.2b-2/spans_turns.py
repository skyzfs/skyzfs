#!/usr/bin/env python3
"""#101 at 7e4b7b8: a `write` that takes several turns on the in-place store, while the client
writes between its slices.

    spans_turns.py CHECKOUT TARGET_DIR

The pull request's test of the commands over the in-place store has a volume of 1 MiB: 256
blocks, which is one slice. So the first cut, the whole volume, is written in the turn that read
the command, and no test has the store serve a write between two slices of a `write`. The
stand-in store of the other tests does not change under a write.

The test here: a volume of 4 MiB (four slices). Blocks 5, 300, 700 and 1000 hold 0x11 at cut
1's instant. `write 1` begins (its first slice, blocks 0 to 255). Then the client overwrites
all four with 0x22: block 5 is already in the file, the others are not. The write ends, and the
file must hold 0x11 in all four; the volume reads 0x22.

Run as the head has it, and with the store making no copy before a write changes a block a held
cut reads (which the pull request's own store test notices too: it overwrites block 5 before
the command). The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'bins/skyzfsd/src/cut_commands/tests.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
TEST = r'''
/// Review: a volume of `size_mib` MiB on a simulated disk, started with
/// held cuts in memory.
fn review_store(size_mib: u64) -> Store {
    let disk = SimDisk::new(12);
    let config = VolumeConfig {
        volume: *b"held cuts' rig 2",
        size_mib,
        base: Base::Empty,
        write_unit: 4096,
    };
    inplace::create(&disk, &config).expect("create");
    let lock = inplace::lock(&disk).expect("the lock");
    let options = OpenOptions {
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(MemRegion::new(1)),
            boot: [7; 16],
            bound: None,
        }),
        ..OpenOptions::default()
    };
    Opened::open(disk, lock, options)
        .map_err(|(e, _)| e)
        .expect("open")
        .start(None)
        .expect("start")
}

/// Review: group `txg`'s uberblock in label 0 of a volume of `size_mib`
/// MiB, sealed.
fn review_uberblock(size_mib: u64, txg: u64) -> (u64, Buf) {
    let ring = Labels::for_size(size_mib << 20)
        .rings()
        .next()
        .expect("four labels")
        .start;
    let offset = ring + (txg % (skyzfs_uberblock::RING_SIZE / BLOCK_SIZE)) * BLOCK_SIZE;
    let mut slot = Buf::zeroed(1);
    let mut put = |at: usize, word: u64| slot[at..at + 8].copy_from_slice(&word.to_le_bytes());
    put(0, MAGIC);
    put(8, 5000);
    put(16, txg);
    for n in 0..16 {
        put(40 + 8 * n, txg.rotate_left(n as u32) ^ 0x5a5a);
    }
    seal(&mut slot, offset, ByteOrder::Little);
    (offset, slot)
}

#[test]
fn review_a_write_over_several_turns_reads_each_block_as_the_cut_does() {
    const MIB: u64 = 4;
    const CHANGED: [u64; 4] = [5, 300, 700, 1000];
    let blocks = MIB << 8;
    let mut store = review_store(MIB);
    let mut rig = Rig::new();
    for block in CHANGED {
        write(&mut store, block * BLOCK_SIZE, Buf::filled(1, 0x11));
    }
    let (offset, slot) = review_uberblock(MIB, 7);
    write(&mut store, offset, slot);
    flush(&mut store);
    let image = rig.image(blocks as usize);
    assert!(
        rig.send(&format!("w write 1 {}", image.display()), &mut store),
        "the write of {blocks} blocks takes more than one turn"
    );
    assert_eq!(rig.reply(), "", "no reply before the write is done");
    // Between two slices the client overwrites a block the write has
    // put into the file and three it has not come to.
    for block in CHANGED {
        write(&mut store, block * BLOCK_SIZE, Buf::filled(1, 0x22));
    }
    let turns = rig.finish(&mut store);
    assert_eq!(rig.reply(), format!("w ok {blocks} blocks\n"));
    let written = std::fs::read(&image).expect("the image");
    let got: Vec<u8> = CHANGED
        .iter()
        .map(|&block| written[block as usize * BLOCK_USIZE])
        .collect();
    println!("review: {turns} more turns; blocks {CHANGED:?} of cut 1 in the file: {got:02x?}");
    for block in CHANGED {
        let at = block as usize * BLOCK_USIZE;
        assert!(
            written[at..at + BLOCK_USIZE].iter().all(|&b| b == 0x11),
            "block {block} of cut 1 is {:#x} in the file; the volume held 0x11 at its instant",
            written[at]
        );
    }
    let now = run(
        &mut store,
        1,
        Op::Read {
            offset: 700 * BLOCK_SIZE,
            buf: Buf::filled(1, 0xEE),
        },
    );
    assert!(now.result.is_ok());
    assert!(now.buf.expect("the read's bytes").iter().all(|&b| b == 0x22));
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfsd', '--features', 'test-hooks', '--lib', '--', 'review_', '--nocapture']
OLD = "        state.change(first, end - first, &mut files)\n"
NEW = "        let _ = (first, end, &mut files, &state);\n        Ok(())\n"


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    print('    exit', p.returncode)
    for ln in out.splitlines():
        s = ln.strip()
        if (s.startswith(('review:', 'error', 'block ')) or 'review: ' in s
                or (s.startswith('test result') and ' 0 passed; 0 failed' not in s)):
            print('   ', s[:300])


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    tests, inp = os.path.join(ROOT, TESTS), os.path.join(ROOT, INP)
    orig_tests, orig_inp = open(tests).read(), open(inp).read()
    assert orig_inp.count(OLD) == 1
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + TEST)
        print('as the head has it:', flush=True)
        run(env)
        with open(inp, 'w') as f:
            f.write(orig_inp.replace(OLD, NEW))
        print('with no copy made before a write changes a block a held cut reads:', flush=True)
        run(env)
    finally:
        with open(inp, 'w') as f:
            f.write(orig_inp)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()
