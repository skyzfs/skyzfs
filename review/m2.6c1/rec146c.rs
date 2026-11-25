// Scratch engine tests of the next look at #146, head 0213a6c (N8's fix,
// 26e83c5). Appended to `engine/tests.rs` in a scratch worktree by
// `search146.py`; they use the head's own stopped uploader
// (`World::stop_uploader`). They print what they find and pass where the
// head behaves as the note says; they are not proposed as the PR's tests.

/// The fake's daemon, counting the requests the agent sends.
struct R7Count {
    fake: FakeDaemon,
    asks: Rc<Cell<usize>>,
    withdraws: Rc<Cell<usize>>,
    progress: Rc<Cell<usize>>,
}

impl Daemon for R7Count {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        match request {
            Request::Publish(_) => self.asks.set(self.asks.get() + 1),
            Request::Withdraw(_) => self.withdraws.set(self.withdraws.get() + 1),
            Request::Progress(_) => self.progress.set(self.progress.get() + 1),
            _ => {}
        }
        self.fake.ask(request, manifest)
    }

    fn device(&mut self) -> io::Result<u64> {
        self.fake.device()
    }
}

type R7Agent = Agent<FakeZfs, R7Count, MemKeep, FakeClock>;

/// What a run did: the agent's effects (before each), and the daemon's
/// requests.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct R7Did {
    holds: usize,
    releases: usize,
    looks: usize,
    saves: usize,
    asks: usize,
    withdraws: usize,
    progress: usize,
    /// Requests (`publish`, `withdraw`) sent between a release and the
    /// give-up of the same publish (L5).
    asked_after_release: usize,
}

struct R7Run {
    agent: R7Agent,
    asks: Rc<Cell<usize>>,
    withdraws: Rc<Cell<usize>>,
    progress: Rc<Cell<usize>>,
    did: Rc<RefCell<R7Did>>,
}

/// An agent on `world` and `keep` whose points count what it does and
/// then call `then` (true stops the agent there).
fn r7_agent(
    world: &Rc<RefCell<World>>,
    keep: &MemKeep,
    then: impl FnMut(Point) -> bool + 'static,
) -> R7Run {
    let asks = Rc::new(Cell::new(0));
    let withdraws = Rc::new(Cell::new(0));
    let progress = Rc::new(Cell::new(0));
    let did = Rc::new(RefCell::new(R7Did::default()));
    let mut then = then;
    let hook = {
        let (did, asks, withdraws) = (did.clone(), asks.clone(), withdraws.clone());
        let released = Cell::new(None::<usize>);
        move |point: Point| {
            {
                let mut d = did.borrow_mut();
                let sent = asks.get() + withdraws.get();
                if !point.after {
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
                        | Effect::Done
                        | Effect::Abandon
                        | Effect::Forget => d.saves += 1,
                        _ => {}
                    }
                }
                match (point.effect, point.after) {
                    (Effect::Released, true) => released.set(Some(sent)),
                    (Effect::Abandon, false) => {
                        if let Some(at) = released.take() {
                            d.asked_after_release += sent - at;
                        }
                    }
                    (Effect::Hold, false) => released.set(None),
                    _ => {}
                }
            }
            then(point)
        }
    };
    let agent = Agent::new(
        FakeZfs(world.clone()),
        R7Count {
            fake: FakeDaemon(world.clone()),
            asks: asks.clone(),
            withdraws: withdraws.clone(),
            progress: progress.clone(),
        },
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_points(Box::new(hook));
    R7Run {
        agent,
        asks,
        withdraws,
        progress,
        did,
    }
}

impl R7Run {
    fn did(&self) -> R7Did {
        let mut d = self.did.borrow().clone();
        d.asks = self.asks.get();
        d.withdraws = self.withdraws.get();
        d.progress = self.progress.get();
        d
    }
}

fn r7_world() -> Rc<RefCell<World>> {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    world
}

fn r7_secs(world: &World) -> u64 {
    (world.now_ms - fake::EPOCH_MS) / 1000
}

/// Settles, catching the fake's livelock check: Ok(settled) or
/// Err(the panic's words).
fn r7_settle(agent: &mut R7Agent, limit: Duration) -> Result<Result<(), String>, String> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| agent.settle(limit)));
    std::panic::set_hook(prev);
    match ran {
        Ok(settled) => Ok(settled.map_err(|e| e.to_string())),
        Err(panic) => Err(panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()),
    }
}

/// Stops the uploader at the `nth` (from 1) point equal to `at`.
fn r7_stop_at(world: &Rc<RefCell<World>>, at: Point, nth: usize) -> impl FnMut(Point) -> bool {
    let world = world.clone();
    let mut seen = 0;
    move |point: Point| {
        if point == at {
            seen += 1;
            if seen == nth {
                let mut w = world.borrow_mut();
                w.stop_uploader();
                let at = w.now_ms - fake::EPOCH_MS;
                w.log.push(format!("{at}: r7 stop"));
            }
        }
        false
    }
}

const R7_ASKED: Point = Point {
    effect: Effect::Asked,
    after: true,
};
const R7_HELD: Point = Point {
    effect: Effect::Held,
    after: true,
};
const R7_COUNT: Point = Point {
    effect: Effect::Count,
    after: false,
};

/// The livelock scenarios of N8 at the head: each given up as `Stopped`
/// at once, reported once, one hold and one release, and nothing asked
/// between the release and the give-up.
#[test]
fn review7_n8_scenarios_end_at_once() {
    use skyzfs_control::Code;
    // (what, stop before begin, stop point, publishes, (holds, asks, releases))
    let cases: Vec<(&str, bool, Option<(Point, usize)>, usize)> = vec![
        ("begun after the stop", true, None, 1),
        ("two begun after the stop", true, None, 2),
        ("held, the ask refused", false, Some((R7_HELD, 1)), 1),
        ("asked anew after the stop (expired before it)", false, Some((R7_COUNT, 1)), 1),
        ("asked: the request pended at the stop", false, Some((R7_ASKED, 1)), 1),
        ("two asked, one stop", false, Some((R7_ASKED, 2)), 2),
    ];
    for (what, before, at, publishes) in cases {
        let world = r7_world();
        if before {
            world.borrow_mut().stop_uploader();
        }
        let keep = MemKeep::default();
        let hook: Box<dyn FnMut(Point) -> bool> = match at {
            Some((p, nth)) => Box::new(r7_stop_at(&world, p, nth)),
            None => Box::new(|_| false),
        };
        let mut run = r7_agent(&world, &keep, hook);
        let mut names = Vec::new();
        for i in 0..publishes {
            if i > 0 {
                world.borrow_mut().advance(Duration::from_secs(1));
            }
            names.push(run.agent.begin().expect("begun"));
        }
        let settled = r7_settle(&mut run.agent, LIMIT);
        let w = world.borrow();
        let did = run.did();
        // From the stop (r7_stop_at logs it; 0 where it came before).
        let t0 = w
            .log
            .iter()
            .find_map(|l| l.strip_suffix(": r7 stop")?.parse::<u64>().ok())
            .map_or(0, |ms| ms / 1000);
        println!(
            "R7N8 {what}: {settled:?} after {} s from the stop; outcomes {:?}; reports {:?}; holds left {:?}; {did:?}",
            r7_secs(&w) - t0,
            run.agent.outcomes(),
            run.agent.reports(),
            holds(&w)
        );
        assert_eq!(settled, Ok(Ok(())), "{what}");
        for t in &names {
            assert!(
                run.agent.outcomes().contains(&(t.clone(), Outcome::Abandoned(Why::Stopped))),
                "{what}"
            );
            assert_eq!(
                run.agent
                    .reports()
                    .iter()
                    .filter(|(n, r)| n == t && *r == Report::Refused(Code::Stopped))
                    .count(),
                1,
                "{what}"
            );
        }
        assert!(holds(&w).is_empty(), "{what}");
        assert_eq!(did.holds, publishes, "{what}: one hold each");
        assert_eq!(did.releases, publishes, "{what}: one release each");
        assert_eq!(did.asked_after_release, 0, "{what}: L5");
        assert!(r7_secs(&w) - t0 < 60, "{what}: at once");
    }
}

/// skyzfsd restarts (a take-over: an uploader that has not stopped) at
/// `at` in the case begun after the stop. Where the answer `stopped` is
/// kept (the ask's, across the look and the release), the publish is
/// given up as `Stopped` though the new uploader would take it; where
/// a round drops it, the next ask is accepted and published.
#[test]
fn review7_a_restart_while_the_answer_is_kept() {
    let points = [
        ("after the refused ask", Point { effect: Effect::Ask, after: true }),
        ("after the look", Point { effect: Effect::Look, after: true }),
        ("after the release", Point { effect: Effect::Released, after: true }),
    ];
    for (what, at) in points {
        let world = r7_world();
        world.borrow_mut().stop_uploader();
        let keep = MemKeep::default();
        let hook = {
            let world = world.clone();
            let mut done = false;
            move |point: Point| {
                if point == at && !done {
                    done = true;
                    let mut w = world.borrow_mut();
                    w.lag_ms = 7_000;
                    w.restart(Restart::TakeOver, None);
                }
                false
            }
        };
        let mut run = r7_agent(&world, &keep, hook);
        run.agent.begin().expect("begun");
        let settled = r7_settle(&mut run.agent, LIMIT);
        let w = world.borrow();
        println!(
            "R7RS restart {what}: {settled:?} at {} s; outcomes {:?}; reports {:?}; holds left {:?}; {:?}",
            r7_secs(&w),
            run.agent.outcomes(),
            run.agent.reports(),
            holds(&w),
            run.did()
        );
        assert_eq!(settled, Ok(Ok(())));
    }
}

/// The request pended at the stop (`progress` answers `stopped`, not
/// kept by the engine); skyzfsd restarts after the release: `progress`
/// answers `unknown`, and the publish is held and asked anew and made
/// (the note at 4ff4549, unchanged).
#[test]
fn review7_pended_then_restart_after_the_release() {
    let world = r7_world();
    let keep = MemKeep::default();
    let hook = {
        let world = world.clone();
        let (mut stopped, mut restarted) = (false, false);
        move |point: Point| {
            if point == R7_ASKED && !stopped {
                stopped = true;
                world.borrow_mut().stop_uploader();
            }
            let released = Point { effect: Effect::Released, after: true };
            if point == released && stopped && !restarted {
                restarted = true;
                let mut w = world.borrow_mut();
                w.lag_ms = 7_000;
                w.restart(Restart::TakeOver, None);
            }
            false
        }
    };
    let mut run = r7_agent(&world, &keep, hook);
    run.agent.begin().expect("begun");
    let settled = r7_settle(&mut run.agent, LIMIT);
    let w = world.borrow();
    println!(
        "R7PR pended, restart after the release: {settled:?} at {} s; outcomes {:?}; reports {:?}; {:?}",
        r7_secs(&w),
        run.agent.outcomes(),
        run.agent.reports(),
        run.did()
    );
    assert_eq!(settled, Ok(Ok(())));
}

/// The daemon away for a minute from `at`, in the case begun after the
/// stop, and restarted (not stopped) while away: does the kept answer
/// outlast the absence?
#[test]
fn review7_away_while_the_answer_is_kept() {
    let points = [
        ("after the refused ask", Point { effect: Effect::Ask, after: true }),
        ("after the look", Point { effect: Effect::Look, after: true }),
        ("after the release", Point { effect: Effect::Released, after: true }),
    ];
    for (what, at) in points {
        for restart in [false, true] {
            let world = r7_world();
            world.borrow_mut().stop_uploader();
            let keep = MemKeep::default();
            let hook = {
                let world = world.clone();
                let mut done = false;
                move |point: Point| {
                    if point == at && !done {
                        done = true;
                        let mut w = world.borrow_mut();
                        w.away_until = w.now_ms + 60_000;
                        if restart {
                            w.lag_ms = 7_000;
                            w.restart(Restart::TakeOver, None);
                        }
                    }
                    false
                }
            };
            let mut run = r7_agent(&world, &keep, hook);
            run.agent.begin().expect("begun");
            let settled = r7_settle(&mut run.agent, LIMIT);
            let w = world.borrow();
            println!(
                "R7AW away {what}, restarted {restart}: {settled:?} at {} s; outcomes {:?}; reports {:?}; holds left {:?}; {:?}",
                r7_secs(&w),
                run.agent.outcomes(),
                run.agent.reports(),
                holds(&w),
                run.did()
            );
            assert_eq!(settled, Ok(Ok(())));
        }
    }
}

/// At least once, across a stop of the agent: in each N8 case, the agent
/// stops at its `k`-th point, for every `k`; a second agent (nothing in
/// memory) settles. The publish ends `Abandoned(Stopped)` in one of the
/// two, `refused stopped` is reported in a process at or before the
/// give-up, no hold is left, and nothing livelocks.
#[test]
fn review7_n8_stop_matrix() {
    use skyzfs_control::Code;
    let cases: Vec<(&str, bool, Option<(Point, usize)>)> = vec![
        ("begun after the stop", true, None),
        ("held, the ask refused", false, Some((R7_HELD, 1))),
        ("asked anew after the stop", false, Some((R7_COUNT, 1))),
        ("asked: pended at the stop", false, Some((R7_ASKED, 1))),
    ];
    for (what, before, at) in cases {
        let mut k = 1;
        let mut summary = Vec::new();
        loop {
            let world = r7_world();
            if before {
                world.borrow_mut().stop_uploader();
            }
            let keep = MemKeep::default();
            let reached = Rc::new(Cell::new(0_usize));
            let armed = Rc::new(Cell::new(false));
            let hook = {
                let (reached, armed) = (reached.clone(), armed.clone());
                let mut stop = at.map(|(p, nth)| r7_stop_at(&world, p, nth));
                move |point: Point| {
                    if let Some(stop) = &mut stop {
                        stop(point);
                    }
                    if point.effect == Effect::Taken && point.after {
                        armed.set(true);
                        return false;
                    }
                    if !armed.get() {
                        return false;
                    }
                    reached.set(reached.get() + 1);
                    reached.get() == k
                }
            };
            let mut first = r7_agent(&world, &keep, hook);
            let t = first.agent.begin().expect("begun");
            let one = r7_settle(&mut first.agent, LIMIT);
            let finished = one == Ok(Ok(()));
            // Make sure the uploader is stopped before the second agent.
            {
                let mut w = world.borrow_mut();
                if !w.log.iter().any(|l| l == "the uploader stops") {
                    w.stop_uploader();
                }
            }
            let mut reports = first.agent.reports().to_vec();
            // The record says how it ended (a stop after the give-up's
            // save leaves no outcome in either process).
            let mut record = first.agent.record().clone();
            if !finished {
                let mut second = r7_agent(&world, &keep, |_| false);
                let two = r7_settle(&mut second.agent, LIMIT);
                assert_eq!(two, Ok(Ok(())), "{what}, k {k}: the second agent");
                reports.extend(second.agent.reports().iter().cloned());
                record = second.agent.record().clone();
            }
            let w = world.borrow();
            assert!(
                record.under_way.is_empty()
                    && record.abandoned.iter().any(|a| a.name == t && a.why == Why::Stopped),
                "{what}, k {k}: {record:?}"
            );
            assert!(
                reports.contains(&(t.clone(), Report::Refused(Code::Stopped))),
                "{what}, k {k}: not reported"
            );
            assert!(holds(&w).is_empty(), "{what}, k {k}: a hold left");
            summary.push(reports.len());
            if finished {
                break;
            }
            k += 1;
        }
        println!(
            "R7SM {what}: {} stop points, each ended Abandoned(Stopped) and reported (reports per run {:?})",
            k - 1,
            summary
        );
    }
}
