
// ---- The review's scenario for #120 (M2.4d) at 0b0789a, on real files ----
//
// Appended to crates/skyzfs-local/tests/inplace_real.rs by plants.py for
// a run with `--features test-hooks`, and taken out again.

/// r8. On real files the header's sync runs on the map's file's helper
/// thread, and its end calls the owner's waker with no request in
/// flight: the owner's next poll then finds the sync ended and counts
/// it. This is what the daemon's queue relies on (its thread sleeps
/// until woken), and the simulated disk calls no waker at a sync's end.
/// A second release-like header (a naming is not possible here) is not
/// made: one release, one sync, one wake.
#[cfg(feature = "test-hooks")]
#[test]
fn review_m24d_r8_the_end_of_a_header_sync_wakes_the_owner_on_real_files() {
    use skyzfs_local::inplace::{CutPhase, CutRegion, HoldCuts};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        eprintln!("review_m24d_r8: SKIPPED");
        return;
    };
    create(&dir, &config()).unwrap();
    let run_dir = tempfile::tempdir().unwrap();
    let region = run_dir.path().join("cuts.state");
    let wakes = Arc::new(AtomicU64::new(0));
    let waker = {
        let wakes = wakes.clone();
        skyzfs_local::store::Waker::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        })
    };
    let options = OpenOptions {
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Named(region.clone()),
            boot: BOOT,
            bound: None,
            intent_region: None,
            intent_header_sync: true,
        }),
        ..OpenOptions::default()
    };
    let opened = Opened::open(dir.clone(), lock(&dir).unwrap(), options)
        .map_err(|(e, _)| e)
        .unwrap();
    let mut store = opened.start(Some(waker)).expect("the in-place store");
    write(&mut store, 5, Buf::filled(1, 0x55));
    assert_eq!(run(&mut store, uberblock_write(7)).result, Ok(()));
    flush(&mut store);
    let held: Vec<(u64, CutPhase)> = store.held_cuts().iter().map(|c| (c.number, c.phase)).collect();
    assert_eq!(held, [(1, CutPhase::Uploading)]);
    // Everything before the release has ended; wakes that are still on
    // their way are let through.
    std::thread::sleep(Duration::from_millis(200));
    let figures = store.intent_figures().expect("an intent map");
    assert!(figures.header_sync);
    assert_eq!(figures.header_syncs.count, 0);
    let before = wakes.load(Ordering::SeqCst);
    assert!(store.release_cut(1));
    let began = Instant::now();
    while wakes.load(Ordering::SeqCst) <= before {
        assert!(began.elapsed() < TIMEOUT, "the end of the header's sync woke nobody");
        std::thread::yield_now();
    }
    let mut done = Vec::new();
    assert_eq!(store.poll(&mut done), 0);
    let after = store.intent_figures().expect("an intent map");
    assert_eq!(after.header_syncs.count, 1);
    assert_eq!(after.syncs, figures.syncs);
    assert!(!store.is_failed());
    eprintln!("review_m24d_r8: RAN");
}
