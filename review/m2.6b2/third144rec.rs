// Scratch tests of the third look at #144 (1b5e5f9), appended to
// `engine/tests.rs` in a scratch worktree. They print what they find and
// pass where the head behaves as this look describes; they are not
// proposed as the PR's tests.

/// A world in which the uploader never uploads a cut (L2's case: an
/// uplink too slow), so each request waits; and a hook that rewinds the
/// pool with `-F` to `back` just before the agent's first `withdraw`.
fn review3_rewind_at_withdraw(back: Back) -> (Rc<RefCell<World>>, impl FnMut(Point) -> bool) {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let hook = {
        let world = world.clone();
        let done = Cell::new(false);
        move |point: Point| {
            if point.effect == Effect::Withdraw && !point.after && !done.replace(true) {
                world.borrow_mut().restart(Restart::RewindF(back), None);
            }
            false
        }
    };
    (world, hook)
}

fn review3_tell(world: &Rc<RefCell<World>>, agent: &FakeAgent, t: &str, settled: &Result<(), Error>) {
    let progress = {
        let mut daemon = FakeDaemon(world.clone());
        daemon.ask(&Request::Progress(t.to_string()), &[])
    };
    let w = world.borrow();
    println!(
        "  settled: {:?}\n  outcomes: {:?}\n  reports: {:?}\n  under way: {:?}\n  holds: {:?}\n  progress now: {:?}\n  world time: {} s",
        settled.as_ref().map_err(|e| e.to_string()),
        agent.outcomes(),
        agent.reports(),
        agent
            .record()
            .under_way
            .iter()
            .map(|p| (p.name.clone(), format!("{:?}", p.phase).chars().take(5).collect::<String>(), p.held))
            .collect::<Vec<_>>(),
        holds(&w),
        progress,
        (w.now_ms - fake::EPOCH_MS) / 1000
    );
    for line in w.log.iter().filter(|l| l.contains("import") || l.contains("user")) {
        println!("  log: {line}");
    }
}

/// N4: a publish too old whose request a rewind ends `refused rewound`
/// is never given up while the daemon keeps that answer; not too old,
/// the same world gives it up as `Rewound` at once.
#[test]
fn review3_n4_a_too_old_publish_whose_request_ended_rewound_never_ends() {
    // Too old (a bound of a minute): the first withdraw is the ended
    // arm's; the rewind takes `pool@t` with it (below its creation).
    let (world, hook) = review3_rewind_at_withdraw(Back::BelowCreated);
    let keep = MemKeep::default();
    let mut agent = self::agent(&world, &keep)
        .with_age_max(Duration::from_secs(60))
        .with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    println!("N4, too old, pool@t lost by the rewind:");
    review3_tell(&world, &agent, &t, &settled);
    assert!(matches!(settled, Err(Error::Unsettled(_))));
    assert!(agent.outcomes().is_empty());
    assert_eq!(agent.record().under_way.len(), 1);

    // The same, the rewind back to after the hold: the snapshot and its
    // hold stay; the hold goes at `held_until`, the publish stays.
    let (world, hook) = review3_rewind_at_withdraw(Back::AfterHold);
    let keep = MemKeep::default();
    let mut agent = self::agent(&world, &keep)
        .with_age_max(Duration::from_secs(60))
        .with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    println!("N4, too old, pool@t kept by the rewind:");
    review3_tell(&world, &agent, &t, &settled);
    assert!(matches!(settled, Err(Error::Unsettled(_))));
    assert_eq!(agent.record().under_way.len(), 1);

    // Control: not too old. The rewind at the second acceptance's
    // point (the request pends, its cut never uploaded).
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let hook = {
        let world = world.clone();
        let n = Cell::new(0);
        move |point: Point| {
            if point == ASKED {
                n.set(n.get() + 1);
                if n.get() == 1 {
                    world
                        .borrow_mut()
                        .restart(Restart::RewindF(Back::BelowCreated), None);
                }
            }
            false
        }
    };
    let keep = MemKeep::default();
    let mut agent = self::agent(&world, &keep).with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    println!("N4 control, not too old, pool@t lost by the rewind:");
    review3_tell(&world, &agent, &t, &settled);
    assert!(settled.is_ok());
    assert!(matches!(agent.outcomes(), [(_, Outcome::Abandoned(Why::Rewound))]));
}

/// N4 at Held: a request accepted and not recorded (the agent stopped
/// before `Asked`), the next agent too old at its first round, the
/// rewind back to after the hold just before its withdraw. The hold is
/// kept for as long as the daemon answers `refused rewound`.
#[test]
fn review3_n4_held_keeps_its_hold_without_end() {
    let (world, hook) = review3_rewind_at_withdraw(Back::AfterHold);
    let keep = MemKeep::default();
    let t = {
        let stop = |point: Point| {
            point
                == Point {
                    effect: Effect::Asked,
                    after: false,
                }
        };
        let mut first = agent(&world, &keep)
            .with_age_max(Duration::from_secs(60))
            .with_points(Box::new(stop));
        let t = first.begin().expect("begun");
        assert!(matches!(first.settle(LIMIT), Err(Error::Stopped)));
        t
    };
    world.borrow_mut().advance(Duration::from_secs(120));
    let mut next = agent(&world, &keep)
        .with_age_max(Duration::from_secs(60))
        .with_points(Box::new(hook));
    let settled = next.settle(LIMIT);
    println!("N4 at Held:");
    review3_tell(&world, &next, &t, &settled);
    assert!(matches!(settled, Err(Error::Unsettled(_))));
    // A hold on each member's `@t` (a recursive hold).
    assert_eq!(holds(&world.borrow()).len(), 3);
}

/// Item 2: the withdrawn flag across a restart. The agent stops just
/// after the withdraw is answered `withdrawn` (the flag set, then lost);
/// the next agent withdraws again before it gives up: one more withdraw,
/// no give-up without one.
#[test]
fn review3_flag_lost_at_a_restart_withdraws_again() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let t = {
        let stop = |point: Point| {
            point
                == Point {
                    effect: Effect::Withdraw,
                    after: true,
                }
        };
        let mut first = agent(&world, &keep)
            .with_age_max(Duration::from_secs(60))
            .with_points(Box::new(stop));
        let t = first.begin().expect("begun");
        assert!(matches!(first.settle(LIMIT), Err(Error::Stopped)));
        t
    };
    let before = world.borrow().calls.iter().filter(|c| **c == "withdraw").count();
    let mut next = agent(&world, &keep).with_age_max(Duration::from_secs(60));
    let settled = next.settle(LIMIT);
    let after = world.borrow().calls.iter().filter(|c| **c == "withdraw").count();
    println!("flag lost at a restart: withdraws before {before}, after {after}");
    review3_tell(&world, &next, &t, &settled);
    assert!(settled.is_ok());
    assert!(matches!(next.outcomes(), [(_, Outcome::Abandoned(Why::TooOld))]));
    assert_eq!(after, before + 1);
}
