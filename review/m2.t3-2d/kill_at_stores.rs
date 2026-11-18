
// ---- review of #115 (not part of the pull request): what a death leaves
// at each store of the state after a cut's line, and after the `applied`
// line of an uberblock write that marks ----

/// Group 7 is cut (cut 1) and group 8's uberblocks are applied. The
/// flush that carries cut 2 reaches its barrier; the daemon dies at the
/// `n`th store into the state after the watcher (the request log) was
/// told of the cut. -> None where the flush lived (no such store), else
/// the count and newest group the next instance starts with, what it
/// hears when the kernel hands it the flush again, and its count then.
type Death = Option<((u64, Option<u64>), Vec<Seen>, (u64, Option<u64>))>;
fn zz_death_at_store(n: u64) -> Death {
    state::tests::quiet_deaths();
    let disk = created(300 + n, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    for label in 0..LABELS {
        assert!(run(&mut store, 200 + label as u64, uberblock_write(label, 8, 80)).result.is_ok());
    }
    store.watch(Box::new({
        let region = region.clone();
        move |seen| {
            if let Seen::Cut(..) = seen {
                region.die_at(n);
            }
        }
    }));
    let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut store, 300, Op::Flush)));
    region.revive();
    match lived {
        Ok(_) => return None,
        Err(payload) if payload.downcast_ref::<state::Lost>().is_some() => {}
        Err(payload) => std::panic::resume_unwind(payload),
    }
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
    Some((at_start, after, counted(&store)))
}

#[test]
fn zz_review_a_death_at_each_store_after_the_cut_line() {
    for n in 0..200 {
        match zz_death_at_store(n) {
            None => {
                eprintln!("REVIEW store {n}: the flush lived (no such store before its answer)");
                break;
            }
            Some((at_start, after, end)) => eprintln!(
                "REVIEW store {n}: the next instance starts at {at_start:?}; handed the flush again it hears {after:?} and counts {end:?}"
            ),
        }
    }
}

/// Group 7 is cut (cut 1). The first uberblock write of group 8 is
/// applied and the watcher is told (the request log's `applied` line);
/// the daemon dies there, before the state stores the mark. The next
/// instance is handed another flush first (one that was in flight),
/// then the write again and the three others, then the group's flush.
#[test]
fn zz_review_a_death_after_the_applied_line() {
    state::tests::quiet_deaths();
    let disk = created(299, 0);
    let region = region();
    let mut store = start_on(&disk, 0, &region, ON_CALLER);
    group(&mut store, 100, 7, 70);
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    store.watch(Box::new({
        let heard = heard.clone();
        move |seen| {
            heard.lock().expect("lock").push(seen);
            if let Seen::Applied(_) = seen {
                std::panic::panic_any(state::Lost);
            }
        }
    }));
    let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut store, 200, uberblock_write(0, 8, 80))
    }));
    match lived {
        Ok(_) => panic!("the write lived"),
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
    assert!(run(&mut store, 400, Op::Flush).result.is_ok());
    let other = heard.lock().expect("lock").clone();
    let after_other = counted(&store);
    for label in 0..LABELS {
        assert!(run(&mut store, 200 + label as u64, uberblock_write(label, 8, 80)).result.is_ok());
    }
    assert!(run(&mut store, 300, Op::Flush).result.is_ok());
    let all = heard.lock().expect("lock").clone();
    eprintln!("REVIEW after the applied line: heard before the death {before:?}; the next instance starts at {at_start:?}; handed another flush first it hears {other:?} and counts {after_other:?}; then the write again, the other three and the group's flush: it hears {:?} and counts {:?}", &all[other.len()..], counted(&store));
}
