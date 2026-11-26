//! Review scratch probes for #149's fifth look (head 6ec5909). Copied
//! into a scratch worktree as
//! `crates/skyzfs-agent/src/engine/review_scratch_e.rs` with
//! `#[path = "review_scratch_e.rs"] mod review_scratch_e;` appended to
//! `crates/skyzfs-agent/src/engine/tests.rs` (so that it sees that
//! module's private helpers: `holds`, `pending_now`); never committed to
//! the product. Run with `cargo test -p skyzfs-agent --release --lib
//! review_scratch_e -- --nocapture --test-threads=1`.
//!
//! E1: the fourth look's D3 at this head, widened: the first asking save
//! slowed by 0 to 1000 s, the answer of the first request sent lost, the
//! daemon away for 900 s, 27 upload lags. The strict check: at each
//! release, does the fake daemon have a request of `t` pending?
//!
//! E2: stops and steps of the clocks at every point of the course (every
//! point up to the 120th, then every 7th): at the k-th point the engine
//! reaches, the world runs on for `stop` (a
//! stop of the agent's process, the daemon and the destroyer going on)
//! and the agent's wall clock steps by `step`. Answer lost or not, a
//! person's give-up or not, uploads that end or never do. No release
//! while a request pends, no wrong publish, no hold and no record left.
//!
//! E3: the gap after the check: the first request reaches the daemon
//! `late` after the slack's check (a stop of the agent between the check
//! and the write; no point is there), its answer lost. The threshold on
//! the fake, which accepts at the send.
//!
//! E4: a restart after the round that sent nothing: the first asking save
//! slowed past the slack, the agent stopped at its next point, and a new
//! agent on the same record, with the daemon away, a step of the wall
//! clock, a give-up, or a take-over of the daemon. The next round asks
//! afresh; the record and the hold do not leak; what the record counts.
//!
//! E5: every asking save slower than the slack: nothing is ever sent, and
//! the publish ends by the planner's own bounds.
//!
//! E6: the author's C1 test with two rows' worth more: within the slack
//! every `asking` record is sent (`askings == sent`), and the same rows
//! again with the agent's wall clock stepped back by the save's length
//! as it is saved. Run at the head, and with the plants C1d (`>=`) and
//! C1e (the check on the wall clock).

use std::io;

use skyzfs_control::{Code, Reply, Request};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loss {
    None,
    Err,
    NoAnswer,
}

/// The fake daemon; the first `publish` it is sent reaches it `late_s`
/// after the agent called `ask` (the world runs on meanwhile), and its
/// answer is lost as `loss` says, the daemon then away for 900 s.
struct D {
    world: Rc<RefCell<World>>,
    loss: Loss,
    late_s: u64,
    first_done: bool,
}

impl Daemon for D {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let first = matches!(request, Request::Publish(_)) && !self.first_done;
        if first && self.late_s > 0 {
            self.world
                .borrow_mut()
                .advance(Duration::from_secs(self.late_s));
        }
        let answer = FakeDaemon(self.world.clone()).ask(request, manifest);
        if first {
            self.first_done = true;
            if self.loss != Loss::None {
                let mut w = self.world.borrow_mut();
                w.away_until = w.now_ms + 900_000;
                w.log.push(format!("publish answered {answer:?}; lost"));
                return if self.loss == Loss::NoAnswer {
                    Ok(Reply::Refused(Code::NoAnswer, String::new()))
                } else {
                    Err(io::Error::new(io::ErrorKind::ConnectionReset, "lost"))
                };
            }
        }
        answer
    }

    fn device(&mut self) -> io::Result<u64> {
        FakeDaemon(self.world.clone()).device()
    }
}

#[derive(Debug, Clone, Copy)]
struct Cfg {
    mode: Mode,
    loss: Loss,
    give_up: bool,
    stuck: bool,
    /// The first asking save takes this long (s).
    slow_s: u64,
    /// Every asking save takes `slow_s`, not only the first.
    every: bool,
    /// The first request reaches the daemon this late after `ask` (s).
    late_s: u64,
    /// At the point of this index (0-based, over the whole run), the
    /// world runs on `stop_s` and the agent's wall clock steps `step_ms`.
    at: Option<usize>,
    stop_s: u64,
    step_ms: i64,
    /// The world's upload lag, where not the default.
    lag_ms: Option<u64>,
    /// The agent's wall clock steps back by the slow save's length as it
    /// is saved, so that the wall clock reads no time passed.
    slow_step_back: bool,
}

impl Cfg {
    fn base(mode: Mode, loss: Loss, give_up: bool, stuck: bool) -> Self {
        Cfg {
            mode,
            loss,
            give_up,
            stuck,
            slow_s: 0,
            every: false,
            late_s: 0,
            at: None,
            stop_s: 0,
            step_ms: 0,
            lag_ms: None,
            slow_step_back: false,
        }
    }
}

#[derive(Debug, Default)]
struct Out {
    early: bool,
    wrong: bool,
    settled: bool,
    holds_left: usize,
    leaked: usize,
    stopped: bool,
    sent: usize,
    askings: u32,
    points: Vec<Point>,
    line: String,
}

impl Out {
    fn bad(&self) -> bool {
        self.early
            || self.wrong
            || !self.settled
            || (!self.stopped && (self.holds_left > 0 || self.leaked > 0))
    }
}

fn agent_for(
    world: &Rc<RefCell<World>>,
    keep: &MemKeep,
    daemon: D,
    hook: Box<dyn FnMut(Point, &str) -> bool>,
) -> Agent<FakeZfs, D, MemKeep, FakeClock> {
    Agent::new(
        FakeZfs(world.clone()),
        daemon,
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_points(hook)
}

/// The hook shared by the probes: the perturbation at `cfg.at`, the slow
/// asking saves, the count of `asking` records, the strict check at
/// each release, and the points seen.
#[allow(clippy::type_complexity)]
fn hook_for(
    world: &Rc<RefCell<World>>,
    name: &Rc<RefCell<String>>,
    cfg: Cfg,
    early: &Rc<RefCell<Vec<u64>>>,
    askings: &Rc<Cell<u32>>,
    points: &Rc<RefCell<Vec<Point>>>,
) -> Box<dyn FnMut(Point, &str) -> bool> {
    let (world, name, early, askings, points) = (
        world.clone(),
        name.clone(),
        early.clone(),
        askings.clone(),
        points.clone(),
    );
    Box::new(move |p: Point, _: &str| {
        let k = points.borrow().len();
        points.borrow_mut().push(p);
        if cfg.at == Some(k) {
            let mut w = world.borrow_mut();
            w.agent_skew_ms += cfg.step_ms;
            w.advance(Duration::from_secs(cfg.stop_s));
        }
        if p == (Point {
            effect: Effect::Asking,
            after: true,
        }) {
            askings.set(askings.get() + 1);
            if askings.get() == 1 || cfg.every {
                let mut w = world.borrow_mut();
                if cfg.slow_step_back {
                    w.agent_skew_ms -= i64::try_from(cfg.slow_s * 1000).unwrap_or(0);
                }
                w.advance(Duration::from_secs(cfg.slow_s));
            }
        }
        if p.effect == Effect::Release && !p.after {
            let t = name.borrow().clone();
            if let Some(now) = pending_now(&world, &t) {
                early.borrow_mut().push(now);
            }
        }
        false
    })
}

fn finish(
    world: &Rc<RefCell<World>>,
    keep: &MemKeep,
    t: &str,
    settled: &Result<(), Error>,
    outcomes: &[(String, Outcome)],
    early: &Rc<RefCell<Vec<u64>>>,
    askings: &Rc<Cell<u32>>,
    points: &Rc<RefCell<Vec<Point>>>,
    head: String,
) -> Out {
    world.borrow_mut().advance(Duration::from_secs(2 * 3600));
    let w = world.borrow();
    let wrong = w.refs.get(t).is_some_and(|r| {
        !w.commits[&r.commit]
            .cut
            .snaps
            .contains_key(&format!("{POOL}@{t}"))
    });
    let sent = w.calls.iter().filter(|c| **c == "publish").count();
    let leaked = match keep.load() {
        Ok(Loaded::Present(record)) => record.under_way.len(),
        _ => 0,
    };
    let stopped = outcomes
        .iter()
        .any(|(_, o)| matches!(o, Outcome::Stopped(_)));
    let out = Out {
        early: !early.borrow().is_empty(),
        wrong,
        settled: settled.is_ok(),
        holds_left: holds(&w).len(),
        leaked,
        stopped,
        sent,
        askings: askings.get(),
        points: points.borrow().clone(),
        line: String::new(),
    };
    let line = format!(
        "{head}: settled {settled:?}; outcomes {:?}; askings {}; sent {sent}; released while pending {}; snap/t on {:?}; wrong {wrong}; holds {:?}; record left {leaked}",
        outcomes.iter().map(|(n, o)| format!("{n} {o:?}")).collect::<Vec<_>>(),
        out.askings,
        out.early,
        w.refs.get(t).map(|r| r.number),
        holds(&w),
    );
    Out { line, ..out }
}

/// One run of the course `cfg` names, in one process.
fn run(cfg: Cfg) -> Out {
    let world = World::new(cfg.mode);
    if let Some(l) = cfg.lag_ms {
        world.borrow_mut().lag_ms = l;
    }
    if cfg.stuck {
        world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    }
    let keep = MemKeep::default();
    let daemon = D {
        world: world.clone(),
        loss: cfg.loss,
        late_s: cfg.late_s,
        first_done: false,
    };
    let name: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let early = Rc::new(RefCell::new(Vec::new()));
    let askings = Rc::new(Cell::new(0_u32));
    let points = Rc::new(RefCell::new(Vec::new()));
    let hook = hook_for(&world, &name, cfg, &early, &askings, &points);
    let mut a = agent_for(&world, &keep, daemon, hook);
    let t = a.begin().expect("begun");
    name.borrow_mut().clone_from(&t);
    world.borrow_mut().destroyer = Some(t.clone());
    if cfg.give_up {
        let _ = a.settle(Duration::from_secs(5));
        // A publish that has already left the record (a stop let it end
        // first) has nothing to give up.
        let _ = a.recover(&t, Choice::GiveUp);
    }
    let settled = a.settle(LIMIT);
    let outcomes = a.outcomes().to_vec();
    drop(a);
    finish(
        &world,
        &keep,
        &t,
        &settled,
        &outcomes,
        &early,
        &askings,
        &points,
        format!("{cfg:?}"),
    )
}

const MODES: [Mode; 3] = [Mode::Idle, Mode::Abandoned, Mode::Busy];
const LOSSES: [Loss; 3] = [Loss::None, Loss::Err, Loss::NoAnswer];

// ---------------------------------------------------------------------
// E1: D3 at this head, widened.
// ---------------------------------------------------------------------

#[test]
fn e1_a_slow_asking_save() {
    let mut table = std::collections::BTreeMap::<u64, (u32, u32, u32, u32)>::new();
    let mut bad = Vec::new();
    for mode in MODES {
        for loss in [Loss::Err, Loss::NoAnswer] {
            for give_up in [false, true] {
                for slow_s in [0, 9, 10, 11, 30, 41, 60, 79, 81, 91, 120, 300, 1000] {
                    let mut lags: Vec<Option<u64>> = vec![None, Some(365 * 24 * 3_600_000)];
                    lags.extend((0..=24).map(|i| Some(500_000 + i * 10_000)));
                    for lag in lags {
                        let mut cfg = Cfg::base(mode, loss, give_up, false);
                        cfg.slow_s = slow_s;
                        cfg.lag_ms = lag;
                        let o = run(cfg);
                        let e = table.entry(slow_s).or_default();
                        e.0 += 1;
                        e.1 += u32::from(o.early);
                        e.2 += u32::from(o.wrong);
                        e.3 += u32::from(o.bad());
                        if o.bad() {
                            bad.push(format!("lag {lag:?}: {}", o.line));
                        }
                    }
                }
            }
        }
    }
    for (d, (n, e, w, b)) in table {
        eprintln!("E1 save {d} s: {n} runs, {e} released while pending, {w} wrong publishes, {b} bad");
    }
    for l in bad.iter().take(20) {
        eprintln!("E1 BAD {l}");
    }
    assert!(bad.is_empty(), "{} bad", bad.len());
}

// ---------------------------------------------------------------------
// E2: stops and steps at every point.
// ---------------------------------------------------------------------

#[test]
fn e2_stops_and_steps_at_every_point() {
    let perturbs: [(u64, i64); 12] = [
        (5, 0),
        (11, 0),
        (41, 0),
        (51, 0),
        (91, 0),
        (700, 0),
        (0, 7_200_000),
        (0, -7_200_000),
        (41, 7_200_000),
        (41, -7_200_000),
        (91, -7_200_000),
        (700, -7_200_000),
    ];
    let (mut n, mut bad, mut sent_none) = (0_u32, Vec::new(), 0_u32);
    let mut most_points = 0;
    for mode in MODES {
        for loss in LOSSES {
            for give_up in [false, true] {
                for stuck in [false, true] {
                    let base = Cfg::base(mode, loss, give_up, stuck);
                    let dry = run(base);
                    assert!(!dry.bad(), "the dry run: {}", dry.line);
                    // Every point up to the 120th, then every 7th.
                    let len = dry.points.len();
                    most_points = most_points.max(len);
                    let ats = (0..len).filter(|&i| i < 120 || i % 7 == 0);
                    for at in ats {
                        for (stop_s, step_ms) in perturbs {
                            let mut cfg = base;
                            cfg.at = Some(at);
                            cfg.stop_s = stop_s;
                            cfg.step_ms = step_ms;
                            let o = run(cfg);
                            n += 1;
                            if o.sent == 0 {
                                sent_none += 1;
                            }
                            if o.bad() {
                                bad.push(format!("at {:?}: {}", dry.points.get(at), o.line));
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "E2: {n} runs (every point up to the 120th, then every 7th; the longest dry course had {most_points} points), {} bad, {sent_none} sent nothing",
        bad.len()
    );
    for l in bad.iter().take(30) {
        eprintln!("E2 BAD {l}");
    }
    assert!(bad.is_empty(), "{} bad", bad.len());
}

// ---------------------------------------------------------------------
// E3: the gap after the check (no point is there).
// ---------------------------------------------------------------------

#[test]
fn e3_a_stop_after_the_check() {
    let mut table = std::collections::BTreeMap::<(u64, u64), (u32, u32, u32)>::new();
    for mode in MODES {
        for loss in [Loss::Err, Loss::NoAnswer] {
            for give_up in [false, true] {
                for stuck in [false, true] {
                    for slow_s in [0, 10] {
                        for late_s in [0, 30, 41, 51, 61, 79, 80, 81, 89, 90, 91, 120, 300] {
                            let mut cfg = Cfg::base(mode, loss, give_up, stuck);
                            cfg.slow_s = slow_s;
                            cfg.late_s = late_s;
                            let o = run(cfg);
                            let e = table.entry((slow_s, late_s)).or_default();
                            e.0 += 1;
                            e.1 += u32::from(o.early);
                            e.2 += u32::from(o.wrong);
                            if o.early && e.1 == 1 {
                                eprintln!("E3 first early at save {slow_s} s, late {late_s} s: {}", o.line);
                            }
                        }
                    }
                }
            }
        }
    }
    for ((s, l), (n, e, w)) in table {
        eprintln!("E3 save {s} s, sent {l} s after the check: {n} runs, {e} released while pending, {w} wrong");
    }
}

// ---------------------------------------------------------------------
// E4: a restart after the round that sent nothing.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtRestart {
    Plain,
    Away,
    StepForward,
    StepBack,
    GiveUp,
    TakeOver,
}

#[test]
fn e4_a_restart_after_the_round_that_sent_nothing() {
    let (mut n, mut bad) = (0_u32, Vec::new());
    let mut lines = Vec::new();
    let mut unsettled = Vec::new();
    let mut asked = 0_u32;
    let mut stalls = std::collections::BTreeMap::<Option<u32>, u32>::new();
    let mut not_asked = std::collections::BTreeMap::<String, u32>::new();
    for mode in MODES {
        for loss in LOSSES {
            for stuck in [false, true] {
                for slow_s in [11_u64, 41, 200] {
                    for at_restart in [
                        AtRestart::Plain,
                        AtRestart::Away,
                        AtRestart::StepForward,
                        AtRestart::StepBack,
                        AtRestart::GiveUp,
                        AtRestart::TakeOver,
                    ] {
                        let (o, notes) = e4_run(mode, loss, stuck, slow_s, at_restart);
                        n += 1;
                        let mut why = Vec::new();
                        // Safety, whatever the course.
                        if o.early || o.wrong {
                            why.push("released while pending, or a wrong publish");
                        }
                        if o.settled && !o.stopped && (o.holds_left > 0 || o.leaked > 0) {
                            why.push("settled with a hold or the record left");
                        }
                        if !o.settled {
                            unsettled.push(format!("{mode:?} {loss:?} stuck={stuck} slow={slow_s} {at_restart:?}"));
                        }
                        // The set-up.
                        if notes.not_sent_first != Some(true) {
                            why.push("the first record was sent");
                        }
                        if notes.stopped_with_asked != Some(true) {
                            why.push("not stopped with the record asked");
                        }
                        // The course after the restart: an ask, where there is
                        // one, from a fresh time, with no attempt counted.
                        if o.askings > 0 {
                            asked += 1;
                            if notes.fresh != Some(true) {
                                why.push("the next asking record is not fresh");
                            }
                            if notes.attempts_after != Some(0) {
                                why.push("the round that sent nothing counted as an attempt");
                            }
                            *stalls.entry(notes.stalls_after).or_default() += 1;
                        } else {
                            *not_asked.entry(format!("{at_restart:?} {:?}", o.line.split("outcomes ").nth(1).and_then(|r| r.split(';').next()))).or_default() += 1;
                        }
                        if at_restart == AtRestart::GiveUp && o.sent != 0 {
                            why.push("sent after a give-up");
                        }
                        let line = format!(
                            "{mode:?} {loss:?} stuck={stuck} slow={slow_s} {at_restart:?}: {notes:?}; {}",
                            o.line
                        );
                        if !why.is_empty() {
                            bad.push(format!("{why:?} {line}"));
                        }
                        if stuck && loss == Loss::Err {
                            lines.push(line);
                        }
                    }
                }
            }
        }
    }
    for l in &lines {
        eprintln!("E4 {l}");
    }
    eprintln!("E4: {n} runs, {} bad; {asked} asked anew after the restart, stalls recorded then {stalls:?}", bad.len());
    for (k, v) in &not_asked {
        eprintln!("E4 not asked after the restart: {k}: {v}");
    }
    eprintln!("E4 unsettled within {} h after the restart: {unsettled:?}", 2 * LIMIT.as_secs() / 3600);
    for l in bad.iter().take(30) {
        eprintln!("E4 BAD {l}");
    }
    assert!(bad.is_empty(), "{} bad", bad.len());
}

#[derive(Debug, Default, Clone, Copy)]
struct Notes {
    /// The first asking record's request was not sent.
    not_sent_first: Option<bool>,
    /// The agent was stopped with the publish recorded asked.
    stopped_with_asked: Option<bool>,
    /// The first asking record after the restart counts from a time at
    /// or after the restart.
    fresh: Option<bool>,
    /// What that record says of attempts and stalls.
    attempts_after: Option<u32>,
    stalls_after: Option<u32>,
}

fn e4_run(mode: Mode, loss: Loss, stuck: bool, slow_s: u64, at_restart: AtRestart) -> (Out, Notes) {
    let world = World::new(mode);
    if stuck {
        world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    }
    let keep = MemKeep::default();
    let name: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let notes = Rc::new(RefCell::new(Notes::default()));
    // The first process: the first asking save slowed by `slow_s`; the
    // agent stopped at the first point after the round that sent nothing.
    let hook1 = {
        let (world, notes) = (world.clone(), notes.clone());
        let mut phase = 0_u8;
        let publishes_before = Rc::new(Cell::new(0_usize));
        Box::new(move |p: Point, _: &str| {
            match phase {
                0 if p
                    == (Point {
                        effect: Effect::Asking,
                        after: true,
                    }) =>
                {
                    publishes_before.set(
                        world
                            .borrow()
                            .calls
                            .iter()
                            .filter(|c| **c == "publish")
                            .count(),
                    );
                    world.borrow_mut().advance(Duration::from_secs(slow_s));
                    phase = 1;
                    false
                }
                1 if p
                    == (Point {
                        effect: Effect::Ask,
                        after: false,
                    }) =>
                {
                    phase = 2;
                    false
                }
                2 => {
                    // The round after: was anything sent?
                    let sent = world
                        .borrow()
                        .calls
                        .iter()
                        .filter(|c| **c == "publish")
                        .count();
                    notes.borrow_mut().not_sent_first = Some(sent == publishes_before.get());
                    phase = 3;
                    true
                }
                _ => false,
            }
        }) as Box<dyn FnMut(Point, &str) -> bool>
    };
    let d1 = D {
        world: world.clone(),
        loss,
        late_s: 0,
        first_done: false,
    };
    let mut a = agent_for(&world, &keep, d1, hook1);
    let t = a.begin().expect("begun");
    name.borrow_mut().clone_from(&t);
    world.borrow_mut().destroyer = Some(t.clone());
    let first = a.settle(LIMIT);
    drop(a);
    let asked = matches!(
        keep.load(),
        Ok(Loaded::Present(r)) if matches!(r.publish(&t).map(|p| &p.phase), Some(Phase::Asked { .. }))
    );
    notes.borrow_mut().stopped_with_asked = Some(matches!(first, Err(Error::Stopped)) && asked);
    // The restart.
    let restart_ms = {
        let mut w = world.borrow_mut();
        match at_restart {
            AtRestart::Away => w.away_until = w.now_ms + 900_000,
            AtRestart::StepForward => w.agent_skew_ms += 2 * 3_600_000,
            AtRestart::StepBack => w.agent_skew_ms -= 2 * 3_600_000,
            AtRestart::TakeOver => w.restart(Restart::TakeOver, None),
            AtRestart::Plain | AtRestart::GiveUp => {}
        }
        w.now_ms.saturating_add_signed(w.agent_skew_ms)
    };
    let early = Rc::new(RefCell::new(Vec::new()));
    let askings = Rc::new(Cell::new(0_u32));
    let points = Rc::new(RefCell::new(Vec::new()));
    let hook2 = {
        let inner = hook_for(&world, &name, Cfg::base(mode, loss, false, stuck), &early, &askings, &points);
        let (keep, notes, t) = (keep.clone(), notes.clone(), t.clone());
        let mut inner = inner;
        Box::new(move |p: Point, at: &str| {
            let stop = inner(p, at);
            if p == (Point {
                effect: Effect::Asking,
                after: true,
            }) && notes.borrow().fresh.is_none()
            {
                if let Ok(Loaded::Present(r)) = keep.load() {
                    if let Some(pu) = r.publish(&t) {
                        if let Phase::Asked { asked_ms, .. } = pu.phase {
                            let mut n = notes.borrow_mut();
                            n.fresh = Some(asked_ms >= restart_ms);
                            n.attempts_after = Some(pu.attempts);
                            n.stalls_after = Some(pu.stalls);
                        }
                    }
                }
            }
            stop
        }) as Box<dyn FnMut(Point, &str) -> bool>
    };
    let d2 = D {
        world: world.clone(),
        loss,
        late_s: 0,
        first_done: false,
    };
    let mut b = agent_for(&world, &keep, d2, hook2);
    if at_restart == AtRestart::GiveUp {
        b.recover(&t, Choice::GiveUp).expect("recovered");
    }
    let settled = b.settle(2 * LIMIT);
    let outcomes = b.outcomes().to_vec();
    drop(b);
    let out = finish(
        &world,
        &keep,
        &t,
        &settled,
        &outcomes,
        &early,
        &askings,
        &points,
        String::from("after the restart"),
    );
    let notes = *notes.borrow();
    (out, notes)
}

// ---------------------------------------------------------------------
// E5: every asking save slower than the slack.
// ---------------------------------------------------------------------

#[test]
fn e5_every_asking_save_slow() {
    let mut bad = Vec::new();
    for mode in MODES {
        for give_up in [false, true] {
            for slow_s in [11_u64, 60] {
                let mut cfg = Cfg::base(mode, Loss::None, give_up, false);
                cfg.slow_s = slow_s;
                cfg.every = true;
                let o = run(cfg);
                eprintln!("E5 {}", o.line);
                if o.bad() || o.sent != 0 {
                    bad.push(o.line);
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

// ---------------------------------------------------------------------
// E6: the C1 test, tightened.
// ---------------------------------------------------------------------

#[test]
fn e6_the_c1_test_tightened() {
    let slack = plan::ASKING_SLACK.as_secs();
    let mut found = Vec::new();
    let mut n = 0;
    for step_back in [false, true] {
        for slow_s in [0, slack, slack + 1, 41, 81, 200] {
            for mode in MODES {
                for loss in [Loss::Err, Loss::NoAnswer] {
                    for (give_up, stuck) in [(false, false), (false, true), (true, false), (true, true)] {
                        let mut cfg = Cfg::base(mode, loss, give_up, stuck);
                        cfg.slow_s = slow_s;
                        cfg.slow_step_back = step_back;
                        let o = run(cfg);
                        n += 1;
                        let mut bad = o.bad();
                        if slow_s <= slack {
                            bad |= o.sent == 0 || o.sent != o.askings as usize;
                        } else if give_up {
                            bad |= o.sent != 0;
                        } else {
                            bad |= o.sent == 0 || o.sent + 1 > o.askings as usize;
                        }
                        if bad {
                            found.push(format!("step_back={step_back} slow={slow_s}: {}", o.line));
                        }
                    }
                }
            }
        }
    }
    let mut by = std::collections::BTreeMap::<String, u32>::new();
    for l in &found {
        let key = l.split(':').next().unwrap_or("").to_string();
        *by.entry(key).or_default() += 1;
    }
    eprintln!("E6: {n} rows, {} failing, by row: {by:?}", found.len());
    assert!(found.is_empty(), "{} failing", found.len());
}
