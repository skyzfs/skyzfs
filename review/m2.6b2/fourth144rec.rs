// Scratch tests of the fourth look at #144 (cf53e94), appended to
// `engine/tests.rs` in a scratch worktree. They print what they find and
// pass where the head behaves as this look describes; they are not
// proposed as the PR's tests.

use skyzfs_control::Code;

/// A world in which the uploader never uploads a cut, and a hook that
/// rewinds the pool with `-F` to `back` just before the agent's first
/// `withdraw` (the third look's N4 scenario).
fn review4_rewind_at_withdraw(back: Back) -> (Rc<RefCell<World>>, impl FnMut(Point) -> bool) {
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

/// N4's three runs of the third look, at the head: each now ends, the
/// hold released and `refused rewound` reported once.
#[test]
fn review4_n4_runs_of_the_third_look_now_end() {
    for (back, why) in [
        (Back::BelowCreated, Why::Rewound),
        (Back::AfterHold, Why::TooOld),
        (Back::BelowHold, Why::TooOld),
    ] {
        let (world, hook) = review4_rewind_at_withdraw(back);
        let keep = MemKeep::default();
        let mut agent = self::agent(&world, &keep)
            .with_age_max(Duration::from_secs(60))
            .with_points(Box::new(hook));
        let t = agent.begin().expect("begun");
        let settled = agent.settle(LIMIT);
        let w = world.borrow();
        println!(
            "too old, {back:?}: settled {:?}, outcomes {:?}, reports {:?}, holds {:?}, at {} s",
            settled.as_ref().map_err(|e| e.to_string()),
            agent.outcomes(),
            agent.reports(),
            holds(&w),
            (w.now_ms - fake::EPOCH_MS) / 1000
        );
        assert!(settled.is_ok());
        assert_eq!(agent.outcomes(), [(t.clone(), Outcome::Abandoned(why))]);
        assert_eq!(agent.reports(), [(t, Report::Refused(Code::Rewound))]);
        assert!(holds(&w).is_empty());
    }

    // Held, accepted and not recorded: the first agent stops before
    // `Asked`; the next is too old at its first round.
    let (world, hook) = review4_rewind_at_withdraw(Back::AfterHold);
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
    let w = world.borrow();
    println!(
        "held, accepted and not recorded: settled {:?}, outcomes {:?}, reports {:?}, holds {:?}",
        settled.as_ref().map_err(|e| e.to_string()),
        next.outcomes(),
        next.reports(),
        holds(&w)
    );
    assert!(settled.is_ok());
    assert_eq!(next.outcomes(), [(t.clone(), Outcome::Abandoned(Why::TooOld))]);
    assert_eq!(next.reports(), [(t, Report::Refused(Code::Rewound))]);
    assert!(holds(&w).is_empty());
}

/// The report is made at least once whatever point the agent stops at
/// after the rewind: the first agent stops at its `k`-th point after it,
/// and the next settles. Each `k` until the first agent no longer stops.
#[test]
fn review4_a_stop_at_each_point_of_the_give_up_reports_at_least_once() {
    for (back, why) in [(Back::BelowCreated, Why::Rewound), (Back::AfterHold, Why::TooOld)] {
        review4_stop_at_each_point(back, why);
    }
}

fn review4_stop_at_each_point(back: Back, why: Why) {
    let mut k = 0;
    loop {
        let (world, mut rewind) = review4_rewind_at_withdraw(back);
        let keep = MemKeep::default();
        let seen = Rc::new(Cell::new(None::<usize>));
        let stopped_at = Rc::new(RefCell::new(None::<Point>));
        let hook = {
            let seen = seen.clone();
            let stopped_at = stopped_at.clone();
            move |point: Point| {
                let _ = rewind(point);
                if seen.get().is_none() && point.effect == Effect::Withdraw && !point.after {
                    seen.set(Some(0));
                }
                match seen.get() {
                    Some(n) if n == k => {
                        *stopped_at.borrow_mut() = Some(point);
                        true
                    }
                    Some(n) => {
                        seen.set(Some(n + 1));
                        false
                    }
                    None => false,
                }
            }
        };
        let mut first = agent(&world, &keep)
            .with_age_max(Duration::from_secs(60))
            .with_points(Box::new(hook));
        let t = first.begin().expect("begun");
        let one = first.settle(LIMIT);
        let first_reports = first.reports().to_vec();
        let first_outcomes = first.outcomes().to_vec();
        if one.is_ok() {
            println!("{back:?}, k = {k}: the first agent ended without a stop; done");
            assert_eq!(first_outcomes, [(t.clone(), Outcome::Abandoned(why))]);
            assert_eq!(first_reports, [(t.clone(), Report::Refused(Code::Rewound))]);
            break;
        }
        assert!(matches!(one, Err(Error::Stopped)), "k = {k}: {one:?}");
        let mut next = agent(&world, &keep).with_age_max(Duration::from_secs(60));
        let two = next.settle(LIMIT);
        let w = world.borrow();
        println!(
            "{back:?}, k = {k}, stopped at {:?}: first {:?} {:?}; next {:?} {:?} {:?}; holds {:?}",
            stopped_at.borrow(),
            first_outcomes,
            first_reports,
            two.as_ref().map_err(|e| e.to_string()),
            next.outcomes(),
            next.reports(),
            holds(&w)
        );
        assert!(two.is_ok(), "k = {k}");
        let all: Vec<_> = first_reports.iter().chain(next.reports()).cloned().collect();
        assert!(
            all.contains(&(t.clone(), Report::Refused(Code::Rewound))),
            "k = {k}: no report"
        );
        assert!(next.record().under_way.is_empty(), "k = {k}");
        // How it ended is the record's: a stop after the give-up's save
        // leaves no outcome in either process.
        let given_up: Vec<_> = next
            .record()
            .abandoned
            .iter()
            .map(|a| (a.name.clone(), a.why))
            .collect();
        assert_eq!(given_up, [(t.clone(), why)], "k = {k}");
        assert!(holds(&w).is_empty(), "k = {k}");
        k += 1;
        assert!(k < 200);
    }
}

/// A daemon whose `progress` and `withdraw` answer `refused <code>` once
/// `on` holds one, the rest as the fake's.
struct Override {
    fake: FakeDaemon,
    on: Rc<Cell<Option<Code>>>,
}

impl Daemon for Override {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        match (request, self.on.get()) {
            (Request::Progress(_) | Request::Withdraw(_), Some(code)) => {
                Ok(Reply::Refused(code, String::new()))
            }
            _ => self.fake.ask(request, manifest),
        }
    }

    fn device(&mut self) -> io::Result<u64> {
        self.fake.device()
    }
}

fn review4_override_run(code: Code) -> (Agent<FakeZfs, Override, MemKeep, FakeClock>, String, Rc<RefCell<World>>, Result<(), Error>) {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let on = Rc::new(Cell::new(None));
    let hook = {
        let on = on.clone();
        move |point: Point| {
            if point.effect == Effect::Withdraw && !point.after {
                on.set(Some(code));
            }
            false
        }
    };
    let keep = MemKeep::default();
    let mut agent = Agent::new(
        FakeZfs(world.clone()),
        Override {
            fake: FakeDaemon(world.clone()),
            on,
        },
        keep,
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_age_max(Duration::from_secs(60))
    .with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    (agent, t, world, settled)
}

/// `refused stopped` (the uploader's own end of every request pending,
/// `Publishing::stop`) from the ended arm's first withdraw on: the
/// publish asked keeps its hold until `held_until`, then stays under way
/// with no outcome and no report for as long as the daemon answers so.
/// And `refused version` (a protocol refusal) where the fake still holds
/// the request pending: given up, released and reported.
#[test]
fn review4_stopped_waits_and_version_gives_up() {
    let (agent, t, world, settled) = review4_override_run(Code::Stopped);
    let w = world.borrow();
    let pending = w.daemon.asked();
    println!(
        "stopped: settled {:?}, outcomes {:?}, reports {:?}, under way {:?}, holds {:?}, at {} s",
        settled.as_ref().map_err(|e| e.to_string()),
        agent.outcomes(),
        agent.reports(),
        agent.record().under_way.iter().map(|p| (p.name.clone(), p.held)).collect::<Vec<_>>(),
        holds(&w),
        (w.now_ms - fake::EPOCH_MS) / 1000
    );
    assert!(matches!(settled, Err(Error::Unsettled(_))));
    assert!(agent.outcomes().is_empty());
    assert!(agent.reports().is_empty());
    assert_eq!(agent.record().under_way.len(), 1);
    assert!(pending);
    drop(w);

    let (agent, t2, world, settled) = review4_override_run(Code::Version);
    let w = world.borrow();
    println!(
        "version: settled {:?}, outcomes {:?}, reports {:?}, holds {:?}, the fake's request still pending: {}",
        settled.as_ref().map_err(|e| e.to_string()),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        w.daemon.asked()
    );
    assert!(settled.is_ok());
    assert_eq!(agent.outcomes(), [(t2.clone(), Outcome::Abandoned(Why::TooOld))]);
    assert_eq!(agent.reports(), [(t2, Report::Refused(Code::Version))]);
    assert!(holds(&w).is_empty());
    let _ = t;
}
