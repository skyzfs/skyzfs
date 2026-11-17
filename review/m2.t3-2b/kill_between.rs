
// ---- review of #111 (not part of the pull request): where a death falls
// between the request log's lines and the cut state's count ----

/// Group 7 is cut (cut 1). Group 8's uberblocks are applied, so a mark
/// waits. The flush that carries its cut is received; the daemon dies
/// `when` the watcher (the request log) is told of the barrier or of the
/// cut. What the watcher heard before the death, the count the next
/// instance starts with, and what it hears and counts when the kernel
/// hands it the flush again.
fn zz_death(at_cut: bool) -> (Vec<Seen>, (u64, Option<u64>), Vec<Seen>, (u64, Option<u64>)) {
    state::tests::quiet_deaths();
    let disk = created(if at_cut { 91 } else { 92 }, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    for label in 0..LABELS {
        assert!(run(&mut store, 200 + label as u64, uberblock_write(label, 8, 80)).result.is_ok());
    }
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    store.watch(Box::new({
        let heard = heard.clone();
        let region = region.clone();
        move |seen| {
            heard.lock().expect("lock").push(seen);
            match seen {
                // kill -9 right after the `barrier` line is written
                Seen::Barrier(_) if !at_cut => std::panic::panic_any(state::Lost),
                // kill -9 right after the `cut` line is written: no store
                // into the state is made from here on
                Seen::Cut(..) if at_cut => region.die_at(0),
                _ => {}
            }
        }
    }));
    let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut store, 300, Op::Flush)));
    region.revive();
    match lived {
        Ok(_) => panic!("the flush lived"),
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => {}
        Err(payload) => std::panic::resume_unwind(payload),
    }
    let before = heard.lock().expect("lock").clone();
    drop(store);
    disk.kill();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    let at_start = counted(&store);
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    store.watch(Box::new({
        let heard = heard.clone();
        move |seen| heard.lock().expect("lock").push(seen)
    }));
    // The flush was never answered: the kernel hands it over again.
    assert!(run(&mut store, 300, Op::Flush).result.is_ok());
    let after = heard.lock().expect("lock").clone();
    (before, at_start, after, counted(&store))
}

#[test]
fn zz_review_a_death_after_the_cut_line() {
    let (before, at_start, after, end) = zz_death(true);
    eprintln!("REVIEW after the cut line: heard before the death {before:?}; the next instance starts at {at_start:?}; handed the flush again it hears {after:?} and counts {end:?}");
}

#[test]
fn zz_review_a_death_after_the_barrier_line() {
    let (before, at_start, after, end) = zz_death(false);
    eprintln!("REVIEW after the barrier line: heard before the death {before:?}; the next instance starts at {at_start:?}; handed the flush again it hears {after:?} and counts {end:?}");
}
