// Scratch tests of the look at #146 (cd3a67c), appended to
// `engine/tests.rs` in a scratch worktree with `fake146.py`'s change to
// the fake (an uploader that stops as skyzfsd's does: every publish
// pending ends `refused stopped`, kept for its name, and every `publish`
// after is refused `stopped` at once and not kept). They print what they
// find and pass where the head behaves as the look describes; they are
// not proposed as the PR's tests.

use skyzfs_control::Code;

/// The fake's daemon, counting the requests the agent sends.
struct Counting {
    fake: FakeDaemon,
    asks: Rc<Cell<usize>>,
    withdraws: Rc<Cell<usize>>,
}

impl Daemon for Counting {
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

type Counted = Agent<FakeZfs, Counting, MemKeep, FakeClock>;

fn review5_agent(
    world: &Rc<RefCell<World>>,
    keep: &MemKeep,
    age_max: Duration,
) -> (Counted, Rc<Cell<usize>>, Rc<Cell<usize>>) {
    let asks = Rc::new(Cell::new(0));
    let withdraws = Rc::new(Cell::new(0));
    let agent = Agent::new(
        FakeZfs(world.clone()),
        Counting {
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

fn secs(world: &World) -> u64 {
    (world.now_ms - fake::EPOCH_MS) / 1000
}

/// N6 as built: the uploader stops just before the ended arm's first
/// withdraw (the fourth look's `review4_stopped_waits_and_version_gives_up`,
/// now with the fake's own stop): the publish is looked up, its hold
/// released, given up as too old, and `refused stopped` reported once.
#[test]
fn review5_stopped_too_old_is_given_up_and_reported() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, _, withdraws) = review5_agent(&world, &keep, Duration::from_secs(60));
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
        "stopped, too old: settled {:?}, outcomes {:?}, reports {:?}, holds {:?}, withdraws {}, at {} s",
        settled.as_ref().map_err(|e| e.to_string()),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        withdraws.get(),
        secs(&w)
    );
    assert!(settled.is_ok());
    assert_eq!(agent.outcomes(), [(t.clone(), Outcome::Abandoned(Why::TooOld))]);
    assert_eq!(agent.reports(), [(t, Report::Refused(Code::Stopped))]);
    assert!(holds(&w).is_empty());
    assert!(w.refs.is_empty());
}

/// The conservative reading, asked: the uploader stops just after the
/// publish is accepted and recorded. Not too old, the publish waits; its
/// hold goes at `held_until`; it is given up at the age bound, and the
/// code reported then. Nothing is asked meanwhile.
#[test]
fn review5_stopped_asked_waits_until_the_bound() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let age_max = Duration::from_secs(3600);
    let (agent, asks, withdraws) = review5_agent(&world, &keep, age_max);
    let hook = {
        let world = world.clone();
        let done = Cell::new(false);
        move |point: Point| {
            if point == ASKED && !done.replace(true) {
                world.borrow_mut().stop_uploader();
            }
            false
        }
    };
    let mut agent = agent.with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let early = agent.settle(Duration::from_secs(5 * 60));
    let (held_early, at_early) = {
        let w = world.borrow();
        (holds(&w), secs(&w))
    };
    let mid = agent.settle(Duration::from_secs(20 * 60));
    let (held_mid, at_mid, asks_mid) = {
        let w = world.borrow();
        (holds(&w), secs(&w), asks.get())
    };
    let settled = agent.settle(LIMIT);
    let w = world.borrow();
    println!(
        "stopped, asked: at {at_early} s {:?} holds {held_early:?}; at {at_mid} s {:?} holds {held_mid:?} asks {asks_mid}; \
         end {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, withdraws {}",
        early.as_ref().map_err(|e| e.to_string()),
        mid.as_ref().map_err(|e| e.to_string()),
        settled.as_ref().map_err(|e| e.to_string()),
        secs(&w),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        asks.get(),
        withdraws.get()
    );
    assert!(matches!(early, Err(Error::Unsettled(_))));
    assert!(!held_early.is_empty());
    assert!(matches!(mid, Err(Error::Unsettled(_))));
    assert!(held_mid.is_empty(), "released at held_until");
    assert_eq!(asks_mid, 1);
    assert!(settled.is_ok());
    assert!(secs(&w) >= 3600);
    assert_eq!(agent.outcomes(), [(t.clone(), Outcome::Abandoned(Why::TooOld))]);
    assert_eq!(agent.reports(), [(t, Report::Refused(Code::Stopped))]);
}

/// The conservative reading, held: the first agent stops after the
/// daemon accepted and before `Asked` is saved; the uploader stops; the
/// next agent, not too old, keeps the hold until the bound, then looks,
/// releases and gives up, the code reported.
#[test]
fn review5_stopped_held_keeps_its_hold_until_the_bound() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let age_max = Duration::from_secs(3600);
    let t = {
        let stop = |point: Point| {
            point
                == Point {
                    effect: Effect::Asked,
                    after: false,
                }
        };
        let (first, _, _) = review5_agent(&world, &keep, age_max);
        let mut first = first.with_points(Box::new(stop));
        let t = first.begin().expect("begun");
        assert!(matches!(first.settle(LIMIT), Err(Error::Stopped)));
        t
    };
    world.borrow_mut().stop_uploader();
    let (mut next, asks, withdraws) = review5_agent(&world, &keep, age_max);
    let early = next.settle(Duration::from_secs(30 * 60));
    let (held_early, at_early) = {
        let w = world.borrow();
        (holds(&w), secs(&w))
    };
    let settled = next.settle(LIMIT);
    let w = world.borrow();
    println!(
        "stopped, held: at {at_early} s {:?} holds {held_early:?}; end {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, withdraws {}",
        early.as_ref().map_err(|e| e.to_string()),
        settled.as_ref().map_err(|e| e.to_string()),
        secs(&w),
        next.outcomes(),
        next.reports(),
        holds(&w),
        asks.get(),
        withdraws.get()
    );
    assert!(matches!(early, Err(Error::Unsettled(_))));
    assert!(!held_early.is_empty(), "the hold kept while not too old");
    assert_eq!(asks.get(), 0);
    assert!(settled.is_ok());
    assert_eq!(next.outcomes(), [(t.clone(), Outcome::Abandoned(Why::TooOld))]);
    assert_eq!(next.reports(), [(t, Report::Refused(Code::Stopped))]);
    assert!(holds(&w).is_empty());
}

/// A publish begun after the uploader stopped: each `publish` is refused
/// `stopped` at once and not kept, so `progress` answers `unknown`, and
/// a publish held and not asked is asked again. Not too old, it is asked
/// about once a tick, not waited on; too old, it is withdrawn (answered
/// `unknown`), looked up and given up. What is reported is printed.
#[test]
fn review5_a_publish_begun_after_the_uploader_stopped() {
    for age in [600, 601] {
        let world = World::new(Mode::Busy);
        world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
        world.borrow_mut().stop_uploader();
        let keep = MemKeep::default();
        let (mut agent, asks, withdraws) =
            review5_agent(&world, &keep, Duration::from_secs(age));
        let t = agent.begin().expect("begun");
        let settled = agent.settle(LIMIT);
        let w = world.borrow();
        println!(
            "begun after the stop, bound {age} s: settled {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, withdraws {}",
            settled.as_ref().map_err(|e| e.to_string()),
            secs(&w),
            agent.outcomes(),
            agent.reports(),
            holds(&w),
            asks.get(),
            withdraws.get()
        );
        assert!(settled.is_ok());
        assert_eq!(agent.outcomes(), [(t, Outcome::Abandoned(Why::TooOld))]);
        assert!(holds(&w).is_empty());
        assert!(asks.get() > 100, "asked {} times", asks.get());
    }
}

/// Two publishes pending when the uploader stops, both too old: each is
/// given up and each reported `refused stopped` (one stop, one code, two
/// names).
#[test]
fn review5_two_publishes_ended_by_one_stop_are_each_reported() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, _, _) = review5_agent(&world, &keep, Duration::from_secs(120));
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
    let t1 = agent.begin().expect("begun");
    world.borrow_mut().advance(Duration::from_secs(1));
    let t2 = agent.begin().expect("begun");
    let settled = agent.settle(LIMIT);
    let w = world.borrow();
    println!(
        "two, one stop: settled {:?}, outcomes {:?}, reports {:?}, holds {:?}",
        settled.as_ref().map_err(|e| e.to_string()),
        agent.outcomes(),
        agent.reports(),
        holds(&w)
    );
    assert!(settled.is_ok());
    let reports = agent.reports();
    assert_eq!(reports.len(), 2, "{reports:?}");
    assert!(reports.contains(&(t1, Report::Refused(Code::Stopped))));
    assert!(reports.contains(&(t2, Report::Refused(Code::Stopped))));
    assert!(holds(&w).is_empty());
}

/// A publish whose request expired just before the uploader stopped:
/// asked anew, refused `stopped` at once; `progress` still answers the
/// old request's `expired`, so the publish is looked up and asked anew
/// again, a count saved in the record each time. Counted over ten
/// minutes of the wait, then run to the bound.
#[test]
fn review5_a_publish_asked_anew_after_the_stop() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let (agent, asks, withdraws) = review5_agent(&world, &keep, Duration::from_secs(7200));
    let saves = Rc::new(Cell::new(0usize));
    let looks = Rc::new(Cell::new(0usize));
    let hook = {
        let (world, saves, looks) = (world.clone(), saves.clone(), looks.clone());
        let done = Cell::new(false);
        move |point: Point| {
            let count = Point {
                effect: Effect::Count,
                after: false,
            };
            if point == count && !done.replace(true) {
                world.borrow_mut().stop_uploader();
            }
            if done.get() && point == count {
                saves.set(saves.get() + 1);
            }
            if done.get()
                && point
                    == (Point {
                        effect: Effect::Look,
                        after: false,
                    })
            {
                looks.set(looks.get() + 1);
            }
            false
        }
    };
    let mut agent = agent.with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let stopped_at = {
        // Until the request expires and is asked anew (the stop).
        let _ = agent.settle(Duration::from_secs(700));
        secs(&world.borrow())
    };
    let (asks0, saves0, looks0) = (asks.get(), saves.get(), looks.get());
    let _ = agent.settle(Duration::from_secs(600));
    let (asks1, saves1, looks1) = (asks.get(), saves.get(), looks.get());
    let settled = agent.settle(LIMIT);
    let w = world.borrow();
    println!(
        "asked anew after the stop: at {stopped_at} s asks {asks0} saves {saves0} looks {looks0}; \
         600 s later asks {asks1} saves {saves1} looks {looks1}; end {:?} at {} s, outcomes {:?}, reports {:?}, holds {:?}, asks {}, saves {}, looks {}, withdraws {}",
        settled.as_ref().map_err(|e| e.to_string()),
        secs(&w),
        agent.outcomes(),
        agent.reports(),
        holds(&w),
        asks.get(),
        saves.get(),
        looks.get(),
        withdraws.get()
    );
    assert!(settled.is_ok());
    assert_eq!(agent.outcomes(), [(t, Outcome::Abandoned(Why::TooOld))]);
    assert!(saves1 - saves0 > 100, "{} saves in 600 s", saves1 - saves0);
}
