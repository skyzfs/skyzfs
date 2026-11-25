// Scratch tests of the look at #146's head 4ff4549, appended to
// `engine/tests.rs` in a scratch worktree with `scratch146.py`'s change
// to the fake (an uploader that stops as skyzfsd's does: every publish
// pending ends `refused stopped`, kept for its name, and every `publish`
// after is refused `stopped` at once and not kept). They print what they
// find and pass where the head behaves as the look describes; they are
// not proposed as the PR's tests.

use skyzfs_control::Code;

/// The fake's daemon, counting the requests the agent sends.
struct Counting6 {
    fake: FakeDaemon,
    asks: Rc<Cell<usize>>,
    withdraws: Rc<Cell<usize>>,
}

impl Daemon for Counting6 {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        match request {
            Request::Publish(_) => self.asks.set(self.asks.get() + 1),
            Request::Withdraw(_) => self.withdraws.set(self.withdraws.get() + 1),
            _ => {}
        }
        self.fake.ask(request, manifest)
    }

    fn device(&mut self) -> io::Result<u64> {
        self.fake.device()
    }
}

type Counted6 = Agent<FakeZfs, Counting6, MemKeep, FakeClock>;

/// What a run did, counted at the points before each effect.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct Did {
    holds: usize,
    releases: usize,
    looks: usize,
    saves: usize,
}

fn review6_agent(
    world: &Rc<RefCell<World>>,
    keep: &MemKeep,
    age_max: Duration,
) -> (Counted6, Rc<Cell<usize>>, Rc<Cell<usize>>) {
    let asks = Rc::new(Cell::new(0));
    let withdraws = Rc::new(Cell::new(0));
    let agent = Agent::new(
        FakeZfs(world.clone()),
        Counting6 {
            fake: FakeDaemon(world.clone()),
            asks: asks.clone(),
            withdraws: withdraws.clone(),
        },
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_age_max(age_max);
    (agent, asks, withdraws)
}

fn secs6(world: &World) -> u64 {
    (world.now_ms - fake::EPOCH_MS) / 1000
}

/// Counts what is done at each point before an effect, and calls `then`
/// at each point (true stops the agent there).
fn counted(did: Rc<Cell<Did>>, then: impl Fn(Point) -> bool + 'static) -> Points {
    Box::new(move |point: Point| {
        if !point.after {
            let mut d = did.get();
            match point.effect {
                Effect::Hold => d.holds += 1,
                Effect::Release => d.releases += 1,
                Effect::Look => d.looks += 1,
                Effect::Holding
                | Effect::Held
                | Effect::Count
                | Effect::Stalled
                | Effect::Asked
                | Effect::Released
                | Effect::Done => d.saves += 1,
                _ => {}
            }
            did.set(d);
        }
        then(point)
    })
}

/// N6 as built, unchanged by 4ff4549: the uploader stops just before the
/// ended arm's first withdraw; the publish is looked up, its hold
/// released, given up as too old, and `refused stopped` reported once.
#[test]
fn review6_stopped_too_old_is_given_up_and_reported() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, _, withdraws) = review6_agent(&world, &keep, Duration::from_secs(60));
    let hook = {
        let world = world.clone();
        let done = Cell::new(false);
        move |point: Point| {
            if point.effect == Effect::Withdraw && !point.after && !done.replace(true) {
                world.borrow_mut().stop_uploader();
            }
            false
        }
    };
    let mut agent = agent.with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    let w = world.borrow();
    println!(
        "R6a stopped, too old: settled {:?}, outcomes {:?}, reports {:?}, holds {:?}, withdraws {}, at {} s",
        settled.as_ref().map_err(|e| e.to_string()),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        withdraws.get(),
        secs6(&w)
    );
    assert!(settled.is_ok());
    assert_eq!(agent.outcomes(), [(t.clone(), Outcome::Abandoned(Why::TooOld))]);
    assert_eq!(agent.reports(), [(t, Report::Refused(Code::Stopped))]);
    assert!(holds(&w).is_empty());
}

/// Not too old, asked: the uploader stops just after the publish is
/// accepted and recorded (the request pends, and ends `refused stopped`,
/// kept for its name). The ruling: looked up, released, given up as
/// `Stopped` and reported, at once.
#[test]
fn review6_stopped_asked_is_given_up_at_once() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, asks, withdraws) = review6_agent(&world, &keep, Duration::from_secs(7200));
    let did = Rc::new(Cell::new(Did::default()));
    let hook = {
        let world = world.clone();
        let done = Cell::new(false);
        counted(did.clone(), move |point: Point| {
            if point == ASKED && !done.replace(true) {
                world.borrow_mut().stop_uploader();
            }
            false
        })
    };
    let mut agent = agent.with_points(hook);
    let t = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    let w = world.borrow();
    println!(
        "R6b stopped, asked, not too old: settled {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, withdraws {}, {:?}",
        settled.as_ref().map_err(|e| e.to_string()),
        secs6(&w),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        asks.get(),
        withdraws.get(),
        did.get()
    );
    assert!(settled.is_ok());
    assert!(secs6(&w) < 60, "given up at once");
    assert_eq!(agent.outcomes(), [(t.clone(), Outcome::Abandoned(Why::Stopped))]);
    assert_eq!(agent.reports(), [(t, Report::Refused(Code::Stopped))]);
    assert!(holds(&w).is_empty());
    assert_eq!(asks.get(), 1);
}

/// Not too old, held: the first agent stops after the daemon accepted
/// and before `Asked` is saved; the uploader stops; the next agent reads
/// `refused stopped` from `progress` and gives the publish up at once.
#[test]
fn review6_stopped_held_is_given_up_at_once() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let age_max = Duration::from_secs(7200);
    let t = {
        let stop = |point: Point| {
            point
                == Point {
                    effect: Effect::Asked,
                    after: false,
                }
        };
        let (first, _, _) = review6_agent(&world, &keep, age_max);
        let mut first = first.with_points(Box::new(stop));
        let t = first.begin().expect("begun");
        assert!(matches!(first.settle(LIMIT), Err(Error::Stopped)));
        t
    };
    world.borrow_mut().stop_uploader();
    let (mut next, asks, withdraws) = review6_agent(&world, &keep, age_max);
    let settled = next.settle(LIMIT);
    let w = world.borrow();
    println!(
        "R6c stopped, held, not too old: settled {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, withdraws {}",
        settled.as_ref().map_err(|e| e.to_string()),
        secs6(&w),
        next.outcomes(),
        next.reports(),
        holds(&w),
        asks.get(),
        withdraws.get()
    );
    assert!(settled.is_ok());
    assert!(secs6(&w) < 60, "given up at once");
    assert_eq!(next.outcomes(), [(t.clone(), Outcome::Abandoned(Why::Stopped))]);
    assert_eq!(next.reports(), [(t, Report::Refused(Code::Stopped))]);
    assert!(holds(&w).is_empty());
    assert_eq!(asks.get(), 0);
}

/// Runs `settle` for `limit`, and says whether the fake called the agent
/// livelocked (its own check: [`fake::STILL_MAX`] questions with no time
/// passing), with the panic's words.
fn settle_or_livelock(agent: &mut Counted6, limit: Duration) -> Result<Result<(), String>, String> {
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| agent.settle(limit)));
    match ran {
        Ok(settled) => Ok(settled.map_err(|e| e.to_string())),
        Err(panic) => Err(panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()),
    }
}

/// A publish begun after the uploader stopped: its `publish` is refused
/// `stopped` at once and not kept. The ask's answer is kept one round:
/// looked up, released, reported. Then `progress` answers `unknown`, and
/// a publish held and not asked, with its hold gone, is held again and
/// asked again: each act moves, so no tick passes.
#[test]
fn review6_a_publish_begun_after_the_uploader_stopped() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    world.borrow_mut().stop_uploader();
    let keep = MemKeep::default();
    let (agent, asks, withdraws) = review6_agent(&world, &keep, Duration::from_secs(7200));
    let did = Rc::new(Cell::new(Did::default()));
    let mut agent = agent.with_points(counted(did.clone(), |_| false));
    let _t = agent.begin().expect("begun");
    let ran = settle_or_livelock(&mut agent, Duration::from_secs(600));
    let w = world.borrow();
    println!(
        "R6d begun after the stop: {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, withdraws {}, {:?}",
        ran,
        secs6(&w),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        asks.get(),
        withdraws.get(),
        did.get()
    );
    // As the look describes it: not given up as `Stopped`, but held,
    // asked, looked up and released again and again, with no time passing.
    let Err(livelock) = ran else {
        panic!("not a livelock: {ran:?}")
    };
    assert!(livelock.contains("livelock"), "{livelock}");
    assert_eq!(secs6(&w), 0);
    let d = did.get();
    assert!(d.holds > 100 && d.releases > 100 && d.looks > 100 && asks.get() > 100, "{d:?}");
    assert!(agent.outcomes().is_empty());
}

/// A publish whose request expired just before the uploader stopped:
/// asked anew, refused `stopped` at once and not kept; `progress` still
/// answers the old request's `expired`, which is looked up and asked
/// anew.
#[test]
fn review6_a_publish_asked_anew_after_the_stop() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, asks, withdraws) = review6_agent(&world, &keep, Duration::from_secs(7200));
    let did = Rc::new(Cell::new(Did::default()));
    let hook = {
        let world = world.clone();
        let done = Cell::new(false);
        counted(did.clone(), move |point: Point| {
            let count = Point {
                effect: Effect::Count,
                after: false,
            };
            if point == count && !done.replace(true) {
                world.borrow_mut().stop_uploader();
            }
            false
        })
    };
    let mut agent = agent.with_points(hook);
    let _t = agent.begin().expect("begun");
    let ran = settle_or_livelock(&mut agent, LIMIT);
    let w = world.borrow();
    println!(
        "R6e asked anew after the stop: {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, withdraws {}, {:?}",
        ran,
        secs6(&w),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        asks.get(),
        withdraws.get(),
        did.get()
    );
    let Err(livelock) = ran else {
        panic!("not a livelock: {ran:?}")
    };
    assert!(livelock.contains("livelock"), "{livelock}");
    let d = did.get();
    assert!(d.holds > 50 && d.releases > 50 && d.saves > 150, "{d:?}");
    assert!(agent.outcomes().is_empty());
}

/// Two publishes pending when the uploader stops, neither too old: each
/// is given up as `Stopped` and each reported.
#[test]
fn review6_two_publishes_ended_by_one_stop_are_each_given_up() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, _, _) = review6_agent(&world, &keep, Duration::from_secs(7200));
    let hook = {
        let world = world.clone();
        let asked = Cell::new(0);
        move |point: Point| {
            if point == ASKED {
                asked.set(asked.get() + 1);
                if asked.get() == 2 {
                    world.borrow_mut().stop_uploader();
                }
            }
            false
        }
    };
    let mut agent = agent.with_points(Box::new(hook));
    let t1 = agent.begin().expect("begun");
    world.borrow_mut().advance(Duration::from_secs(1));
    let t2 = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    let w = world.borrow();
    println!(
        "R6f two, one stop, not too old: settled {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}",
        settled.as_ref().map_err(|e| e.to_string()),
        secs6(&w),
        agent.outcomes(),
        agent.reports(),
        holds(&w)
    );
    assert!(settled.is_ok());
    let outcomes = agent.outcomes();
    assert!(outcomes.contains(&(t1.clone(), Outcome::Abandoned(Why::Stopped))));
    assert!(outcomes.contains(&(t2.clone(), Outcome::Abandoned(Why::Stopped))));
    let reports = agent.reports();
    assert_eq!(reports.len(), 2, "{reports:?}");
    assert!(reports.contains(&(t1, Report::Refused(Code::Stopped))));
    assert!(reports.contains(&(t2, Report::Refused(Code::Stopped))));
    assert!(holds(&w).is_empty());
}

/// skyzfsd restarts (a take-over, the uploader new) after the agent
/// released the hold on `refused stopped` and before it gave the publish
/// up: `progress` answers `unknown`, the publish is held and asked anew,
/// and published (the upload's lag made the fake's own at the restart),
/// with `refused stopped` reported beside it.
#[test]
fn review6_a_restart_between_the_release_and_the_give_up() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, asks, _) = review6_agent(&world, &keep, Duration::from_secs(7200));
    let hook = {
        let world = world.clone();
        let (stopped, restarted) = (Cell::new(false), Cell::new(false));
        move |point: Point| {
            if point == ASKED && !stopped.replace(true) {
                world.borrow_mut().stop_uploader();
            }
            let released = Point {
                effect: Effect::Released,
                after: true,
            };
            if point == released && stopped.get() && !restarted.replace(true) {
                let mut w = world.borrow_mut();
                w.uploader_stopped = false;
                w.lag_ms = 7_000;
                w.restart(Restart::TakeOver, None);
            }
            false
        }
    };
    let mut agent = agent.with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    let w = world.borrow();
    println!(
        "R6g a restart after the release: settled {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}",
        settled.as_ref().map_err(|e| e.to_string()),
        secs6(&w),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        asks.get()
    );
    assert!(settled.is_ok());
    assert!(matches!(agent.outcomes()[..], [(_, Outcome::Published(_))]));
    assert_eq!(agent.reports(), [(t, Report::Refused(Code::Stopped))]);
}
