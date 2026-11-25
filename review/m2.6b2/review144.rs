// Scratch scenarios of the review of #144 (M2.6b2) at b5063b2. Appended
// to the end of `crates/skyzfs-agent/src/engine/tests.rs` in a scratch
// worktree only; each passes at the head, and shows the behaviour as
// built.

/// F1. A publish done with `pool@t` gone is reported `SnapshotGone` after
/// the record says Done. An agent stopped between the two leaves no
/// report, and the agent that takes over goes Done, Release, Forget and
/// never makes it: the report is at most once, not at least once.
#[test]
fn review_f1_a_snapshot_gone_report_is_lost_to_a_stop_after_the_done_commit() {
    for stop in [false, true] {
        let world = World::new(Mode::Busy);
        world.borrow_mut().die = Some((
            DaemonPoint::Landed,
            Restart::RewindT(fake::Back::BelowCreated),
        ));
        let keep = MemKeep::default();
        let done_after = Point {
            effect: Effect::Done,
            after: true,
        };
        let mut first = agent(&world, &keep).with_points(Box::new(move |p| stop && p == done_after));
        let t = first.begin().expect("begun");
        let settled = first.settle(LIMIT);
        let mut reports = first.reports().to_vec();
        drop(first);
        if stop {
            assert!(matches!(settled, Err(Error::Stopped)), "{settled:?}");
            let mut again = agent(&world, &keep);
            again.settle(LIMIT).expect("settled after the stop");
            assert!(
                matches!(again.outcomes(), [(_, Outcome::Published(_))]),
                "{:?}",
                again.outcomes()
            );
            reports.extend(again.reports().iter().cloned());
        } else {
            settled.expect("settled");
        }
        let w = world.borrow();
        assert!(w.refs.contains_key(&t), "snap/{t} published");
        let pool = w.pool.as_ref().expect("the pool");
        assert!(
            !pool.snaps.contains_key(&format!("{POOL}@{t}")),
            "pool@{t} is gone"
        );
        if stop {
            assert!(reports.is_empty(), "stopped: {reports:?}");
        } else {
            assert_eq!(reports, [(t.clone(), Report::SnapshotGone)]);
        }
    }
}

/// D1. A publish whose cuts are counted and never uploaded (an uplink
/// too slow for the copies' bound, or an uploader stuck): each request
/// expires with cuts counted, so no stall is counted; the publish is
/// asked anew for ever, is neither reported nor given up, keeps its hold,
/// and its sync task runs about once in ten seconds.
#[test]
fn review_d1_a_publish_whose_cuts_never_upload_is_never_bounded() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    world.borrow_mut().lag_ms = u64::MAX / 4;
    let mut agent = agent(&world, &keep);
    agent.begin().expect("begun");
    let limit = Duration::from_secs(8 * 3600);
    assert!(
        matches!(agent.settle(limit), Err(Error::Unsettled(_))),
        "settled"
    );
    let w = world.borrow();
    assert!(agent.reports().is_empty(), "{:?}", agent.reports());
    assert!(agent.outcomes().is_empty(), "{:?}", agent.outcomes());
    let publish = &agent.record().under_way[0];
    assert!(publish.stalls < plan::STALLS_REPORTED, "{publish:?}");
    assert_eq!(holds(&w).len(), 3, "still held");
    eprintln!(
        "review_d1: 8 h: {} requests, stalls {}, {} sync tasks, {} cuts counted",
        publish.attempts,
        publish.stalls,
        w.assists.len(),
        w.daemon.cuts.last().map_or(0, |c| c.number)
    );
    assert!(publish.attempts >= 40, "{}", publish.attempts);
}

/// N2. A daemon taken over as each request is accepted, on an idle pool,
/// with the sync task as built (it writes a group, but no request pends
/// ten seconds, so none runs): each `unknown` with the count where it was
/// asked is a stall, so a crash loop of the daemon gives the publish up
/// as `NoProgress` within seconds (a take-over keeps the attachment and
/// the state, which the stalls' reset reads).
#[test]
fn review_n2_take_overs_with_no_cut_count_as_stalls() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let hook = {
        let world = world.clone();
        move |point: Point| {
            if point == ASKED {
                world.borrow_mut().restart(Restart::TakeOver, None);
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    assert!(
        matches!(agent.outcomes(), [(_, Outcome::Abandoned(Why::NoProgress))]),
        "{:?}",
        agent.outcomes()
    );
    let took = (world.borrow().now_ms - fake::EPOCH_MS) / 1000;
    eprintln!("review_n2: given up after {took} s");
}
