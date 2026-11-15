#!/usr/bin/env python3
"""#100 at 59d09bf: three sequences after which a start takes up a cut state it should not.

    holes.py CHECKOUT TARGET_DIR

All go through the store with a region in memory, as the pull request's own trust tests do.
The daemon tells the store nothing more than these tests do: `serve` without --hold-cuts opens
the store with `hold_cuts: None`, and the lifecycle opens the store as acknowledging only when
a marker or a failure note is there.

1. A start that holds no cuts between two that do. Cut 1 is held (the whole volume; block 5
   holds 0x55). The daemon is lost. A daemon without --hold-cuts serves the volume and writes
   0x66 to block 5: no copy, no bit. It stops. A daemon with --hold-cuts starts: same boot,
   same volume, the copies sync. The state is taken up, and cut 1 reads block 5 as 0x66.

2. A start that acknowledges a failure and does not reach its second phase. A cut is held and
   the volume fails (a write of the image is refused). A start without the acknowledgement is
   refused. A start with it opens the store, which takes the marker away, and is lost before
   the device is its own (the lifecycle finds and attaches the device between the open and the
   start). The next start has nothing to acknowledge, and takes the state up.

3. A failed sync of the copies, heard by a start that is lost. The kernel reports a failed
   writeback to the first sync after it and to no sync of a file opened later. The start that
   hears it takes the state up first (`restore`, which finishes what a death interrupted) and
   only then begins a new one, so a start killed between the sync and its first store into the
   region leaves the state as it was, and the next start's sync succeeds. The simulated disk
   fails every sync until told otherwise, so the test clears the fault after the lost start.

The tests assert what the design asks for, so they fail at the head. The file is restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
TEST = r'''
/// The store's options with the cut state in `region`.
fn review_holding(region: &state::MemRegion, acknowledge: bool) -> OpenOptions {
    OpenOptions {
        io: ON_CALLER,
        acknowledge_failure: acknowledge,
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region.clone()),
            boot: state::tests::BOOT,
            bound: Some(TEST_BOUND),
        }),
        ..OpenOptions::default()
    }
}

#[test]
fn review_a_start_that_holds_no_cuts_between_two_that_do() {
    let disk = created(40, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    assert_eq!(cut_block(&mut store, 1, 5), vec![0x55; BLOCK_USIZE]);
    drop(store);
    disk.kill();

    // A daemon without --hold-cuts: the store is told of no region.
    let mut store = start_io(&disk, 0, false, ON_CALLER).expect("opens");
    assert_eq!(store.cuts_at_start(), None);
    write(&mut store, 5, 0x66).unwrap();
    assert!(run(&mut store, 300, Op::Flush).result.is_ok());
    drop(store);
    disk.kill();

    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    let held: Vec<u64> = store.held_cuts().iter().map(|c| c.number).collect();
    let five = if held.contains(&1) {
        cut_block(&mut store, 1, 5)[0]
    } else {
        0
    };
    println!(
        "review: after a start that held no cuts: {:?}, held {held:?}, cut 1 reads block 5 as {five:#x}",
        store.cuts_at_start()
    );
    assert!(
        held.is_empty() || five == 0x55,
        "cut 1 reads block 5 as {five:#x}; the volume held 0x55 at its instant"
    );
}

#[test]
fn review_an_acknowledged_failure_whose_start_is_lost_before_its_second_phase() {
    let disk = created(41, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    // The volume fails: a write of the image is refused.
    disk.fail_write_of(HOME_NAME);
    assert_eq!(write(&mut store, 6, 0x66), Err(BlkError::Failed));
    drop(store);
    disk.kill();
    disk.clear_faults();

    // Without the acknowledgement the start is refused.
    let refused = Opened::open(
        disk.clone(),
        lock(&disk).expect("the lock"),
        review_holding(&region, false),
    )
    .map_err(|(e, _)| e)
    .err();
    assert!(matches!(refused, Some(OpenError::MarkedFailed)), "{refused:?}");

    // With it the open takes the marker away, and the daemon is lost
    // before its second phase.
    let opened = Opened::open(
        disk.clone(),
        lock(&disk).expect("the lock"),
        review_holding(&region, true),
    )
    .map_err(|(e, _)| e)
    .expect("opens");
    assert!(opened.acknowledged());
    drop(opened);
    disk.kill();

    // The next start has nothing to acknowledge.
    let opened = Opened::open(
        disk.clone(),
        lock(&disk).expect("the lock"),
        review_holding(&region, false),
    )
    .map_err(|(e, _)| e)
    .expect("opens");
    assert!(!opened.acknowledged());
    let store = opened
        .start_with(None, Mutants(0))
        .expect("the in-place store");
    let held: Vec<u64> = store.held_cuts().iter().map(|c| c.number).collect();
    println!(
        "review: after a failure acknowledged by a start that was lost: {:?}, held {held:?}",
        store.cuts_at_start()
    );
    assert!(
        matches!(store.cuts_at_start(), Some(CutsTakenUp::Begun(_))),
        "the state of a failed volume was taken up, with cuts {held:?}"
    );
}

#[test]
fn review_a_failed_sync_of_the_copies_heard_by_a_start_that_is_lost() {
    state::tests::quiet_deaths();
    let disk = created(42, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    write(&mut store, 5, 0x66).unwrap();
    assert!(store.cut_figures().copied > 0);
    drop(store);
    disk.kill();

    // The writeback of the copies has failed. This start's sync hears
    // it, and the start is lost at its first store into the region.
    disk.fail_sync_of(COPIES_NAME);
    region.die_at(0);
    let lost = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        start_on(&disk, 0, &region, ON_CALLER)
    }));
    region.revive();
    match lost {
        Ok(_) => panic!("the start made no store into the region"),
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => {}
        Err(payload) => std::panic::resume_unwind(payload),
    }
    disk.kill();
    // The kernel reports a failed writeback once: the next sync succeeds.
    disk.clear_faults();

    let store = start_on(&disk, 0, &region, ON_CALLER);
    let held: Vec<u64> = store.held_cuts().iter().map(|c| c.number).collect();
    println!(
        "review: after a start that heard the copies' failed sync and was lost: {:?}, held {held:?}",
        store.cuts_at_start()
    );
    assert!(
        matches!(store.cuts_at_start(), Some(CutsTakenUp::Begun(_))),
        "a state whose copies failed to sync was taken up, with cuts {held:?}"
    );
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'review_', '--nocapture', '--test-threads=1']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    tests = os.path.join(ROOT, TESTS)
    orig = open(tests).read()
    try:
        with open(tests, 'w') as f:
            f.write(orig + TEST)
        p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        print('exit', p.returncode)
        for ln in out.splitlines():
            if (ln.startswith(('review:', 'error', 'test inplace')) or 'panicked' in ln or 'cut 1 reads' in ln
                    or 'was taken up' in ln or ln.lstrip().startswith(('left:', 'right:', '-->'))
                    or (ln.startswith('test result') and ' 0 passed; 0 failed' not in ln)):
                print('   ', ln.strip()[:400])
    finally:
        with open(tests, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
