//! Review scratch probes for #149's fourth look (head 5259bf5). Copied
//! into a scratch worktree as
//! `crates/skyzfs-agent/src/engine/review_scratch_d.rs` with
//! `#[path = "review_scratch_d.rs"] mod review_scratch_d;` appended to
//! `crates/skyzfs-agent/src/engine/tests.rs` (so that it sees that
//! module's private helpers: `LoseNth`, `holds`); never committed to the
//! product. Run with `cargo test -p skyzfs-agent --lib review_scratch_d
//! -- --nocapture --test-threads=1`.
//!
//! D1: each of the 8 rows of the author's N6 test reaches the course it
//! names (the answer lost; where `moved`, two publishes and a withdraw of
//! the first answered `withdrawn` before the loss), and ends settled
//! with no hold left. The test itself asserts none of this.
//!
//! D2: N6's course widened: the first withdraw's answer as `withdrawn`,
//! `unknown` or `expired` (each sets `memory.withdrawn`), and the
//! person's give-up 0, 30 or 90 s after the loss. No release while a
//! request pends, no wrong publish, settled, no hold left.

use std::io;

use skyzfs_control::{Reply, Request};

use super::*;

/// Passes everything to `inner`; records each request's kind and
/// answer, and rewrites the first answer `withdrawn` to a withdraw into
/// `first_withdraw`.
struct Rec<D: Daemon> {
    inner: D,
    seen: Rc<RefCell<Vec<(&'static str, String)>>>,
    first_withdraw: Option<Reply>,
}

impl<D: Daemon> Daemon for Rec<D> {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let mut a = self.inner.ask(request, manifest);
        let kind = match request {
            Request::Cuts => "cuts",
            Request::Publish(_) => "publish",
            Request::Progress(_) => "progress",
            Request::Withdraw(_) => "withdraw",
            Request::Look(_) => "look",
        };
        if kind == "withdraw" && matches!(a, Ok(Reply::Withdrawn)) {
            if let Some(r) = self.first_withdraw.take() {
                a = Ok(r);
            }
        }
        self.seen.borrow_mut().push((
            kind,
            match &a {
                Ok(r) => format!("{r:?}"),
                Err(e) => format!("Err {e}"),
            },
        ));
        a
    }

    fn device(&mut self) -> io::Result<u64> {
        self.inner.device()
    }
}

struct Out {
    lost: bool,
    publishes_before: usize,
    withdrawn_before: bool,
    early: bool,
    wrong: bool,
    settled: bool,
    holds_left: bool,
    line: String,
}

fn d_run(
    mode: Mode,
    no_answer: bool,
    moved: bool,
    first_withdraw: Option<Reply>,
    delay_s: u64,
) -> Out {
    let world = World::new(mode);
    world.borrow_mut().lag_ms = 60_000;
    let keep = MemKeep::default();
    let lost = Rc::new(Cell::new(false));
    let seen: Rc<RefCell<Vec<(&'static str, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let daemon = Rec {
        inner: super::LoseNth {
            world: world.clone(),
            n: if moved { 2 } else { 1 },
            seen: 0,
            no_answer,
            lost: lost.clone(),
        },
        seen: seen.clone(),
        first_withdraw,
    };
    let name: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let early: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let hook = {
        let (world, name, early) = (world.clone(), name.clone(), early.clone());
        let mut rewound = false;
        move |p: Point, _: &str| {
            if moved
                && !rewound
                && p == (Point {
                    effect: Effect::Asked,
                    after: true,
                })
            {
                rewound = true;
                let t = name.borrow().clone();
                world
                    .borrow_mut()
                    .restart(Restart::RewindT(Back::AfterHold), Some(&t));
            }
            if p.effect == Effect::Release && !p.after {
                let t = name.borrow().clone();
                let (now, away, silent) = {
                    let mut w = world.borrow_mut();
                    let saved = (w.now_ms, w.away_until, w.silent_until);
                    w.away_until = 0;
                    w.silent_until = 0;
                    saved
                };
                let reply = FakeDaemon(world.clone()).ask(&Request::Progress(t), &[]);
                {
                    let mut w = world.borrow_mut();
                    w.away_until = away;
                    w.silent_until = silent;
                    w.calls.pop();
                }
                if matches!(reply, Ok(Reply::Pending)) {
                    early.borrow_mut().push(now);
                }
            }
            false
        }
    };
    let mut a = Agent::new(
        FakeZfs(world.clone()),
        daemon,
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_points(Box::new(hook));
    let t = a.begin().expect("begun");
    name.borrow_mut().clone_from(&t);
    world.borrow_mut().destroyer = Some(t.clone());
    let mut rounds = 0;
    while !lost.get() && rounds < 900 {
        let _ = a.settle(Duration::from_secs(1));
        rounds += 1;
    }
    let before = seen.borrow().clone();
    let publishes_before = before.iter().filter(|(k, _)| *k == "publish").count();
    let withdrawn_before = before.iter().any(|(k, r)| *k == "withdraw" && r != "Pending" && !r.starts_with("Err") && !r.contains("NoAnswer"));
    for _ in 0..delay_s {
        let _ = a.settle(Duration::from_secs(1));
    }
    let given_at = seen.borrow().len();
    // A publish that ended before the give-up (published, or given up
    // by the agent itself) is no longer under way: nothing to give up.
    let gave_up = match a.recover(&t, Choice::GiveUp) {
        Ok(_) => "given up",
        Err(Error::NotUnderWay(_)) => "ended before the give-up",
        Err(e) => panic!("recover: {e:?}"),
    };
    let settled = a.settle(LIMIT);
    let outcomes = a.outcomes().to_vec();
    drop(a);
    world.borrow_mut().advance(Duration::from_secs(2 * 3600));
    let w = world.borrow();
    let wrong = w.refs.get(&t).is_some_and(|r| {
        !w.commits[&r.commit]
            .cut
            .snaps
            .contains_key(&format!("{POOL}@{t}"))
    });
    let after: Vec<String> = seen.borrow()[given_at..]
        .iter()
        .filter(|(k, _)| *k != "cuts")
        .take(8)
        .map(|(k, r)| format!("{k}={r}"))
        .collect();
    let early_any = !early.borrow().is_empty();
    let line = format!(
        "{mode:?} no_answer={no_answer} moved={moved} delay={delay_s} ({gave_up}): lost {} after {rounds} s; publishes before {publishes_before}; withdraw answered before {withdrawn_before}; settled {settled:?}; outcomes {:?}; released while pending {}; wrong {wrong}; holds {:?}; after the give-up {after:?}",
        lost.get(),
        outcomes.iter().map(|(n, o)| format!("{n} {o:?}")).collect::<Vec<_>>(),
        early_any,
        holds(&w),
    );
    Out {
        lost: lost.get(),
        publishes_before,
        withdrawn_before,
        early: early_any,
        wrong,
        settled: settled.is_ok(),
        holds_left: !holds(&w).is_empty(),
        line,
    }
}

/// D1: the author's 8 rows reach their course.
#[test]
fn d1_the_n6_rows_reach_their_course() {
    let mut bad = Vec::new();
    for mode in [Mode::Idle, Mode::Busy] {
        for no_answer in [false, true] {
            for moved in [false, true] {
                let o = d_run(mode, no_answer, moved, None, 0);
                eprintln!("D1 {}", o.line);
                let course = o.lost
                    && o.publishes_before == if moved { 2 } else { 1 }
                    && o.withdrawn_before == moved;
                if !course || o.early || o.wrong || !o.settled || o.holds_left {
                    bad.push(o.line);
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// D2: N6's course widened.
#[test]
fn d2_n6_widened() {
    let mut bad = Vec::new();
    let mut n = 0;
    for mode in [Mode::Idle, Mode::Busy] {
        for no_answer in [false, true] {
            for first in [None, Some(Reply::Unknown), Some(Reply::Expired)] {
                for delay in [0, 30, 90] {
                    let o = d_run(mode, no_answer, true, first.clone(), delay);
                    n += 1;
                    eprintln!("D2 first={first:?} {}", o.line);
                    if !o.lost || o.early || o.wrong || !o.settled || o.holds_left {
                        bad.push(o.line);
                    }
                }
            }
        }
    }
    eprintln!("D2: {n} runs, {} bad", bad.len());
    assert!(bad.is_empty(), "{bad:#?}");
}

// ---------------------------------------------------------------------
// D3: Codex's P2 on 5259bf5 (engine.rs:1156). The asking record's
// `held_until` and the reading of the clock that does not step are taken
// before the asking save; the request is sent after it. A save that
// takes `d` (simulated: the world's clock advanced by `d` at the point
// after the asking save) sends the request `d` later than the budget
// assumes. Then the answer is lost (`Err` or `no-answer`) and the daemon
// is away for 900 s (N5's daemon `LostAnswer`, no skew). Does a release
// come while the request pends at the daemon (the strict check), and is
// `snap/t` made on a cut without `pool@t`?
// ---------------------------------------------------------------------

struct D3 {
    d_s: u64,
    early: bool,
    wrong: bool,
    line: String,
}

fn d3_run(mode: Mode, no_answer: bool, give_up: bool, d_s: u64, lag_ms: Option<u64>) -> D3 {
    let world = World::new(mode);
    if let Some(lag) = lag_ms {
        world.borrow_mut().lag_ms = lag;
    }
    let keep = MemKeep::default();
    let daemon = super::LostAnswer {
        world: world.clone(),
        lost: false,
        no_answer,
        away_s: 900,
        skew_ms: 0,
    };
    let name: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let early: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let sent_at: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let hook = {
        let (world, name, early, sent_at) =
            (world.clone(), name.clone(), early.clone(), sent_at.clone());
        let mut stalled = false;
        move |p: Point, _: &str| {
            if !stalled
                && p == (Point {
                    effect: Effect::Asking,
                    after: true,
                })
            {
                stalled = true;
                world.borrow_mut().advance(Duration::from_secs(d_s));
            }
            if p == (Point {
                effect: Effect::Ask,
                after: false,
            }) && sent_at.get() == 0
            {
                sent_at.set(world.borrow().now_ms);
            }
            if p.effect == Effect::Release && !p.after {
                let t = name.borrow().clone();
                let (now, away, silent) = {
                    let mut w = world.borrow_mut();
                    let saved = (w.now_ms, w.away_until, w.silent_until);
                    w.away_until = 0;
                    w.silent_until = 0;
                    saved
                };
                let reply = FakeDaemon(world.clone()).ask(&Request::Progress(t), &[]);
                {
                    let mut w = world.borrow_mut();
                    w.away_until = away;
                    w.silent_until = silent;
                    w.calls.pop();
                }
                if matches!(reply, Ok(Reply::Pending)) {
                    early.borrow_mut().push(now);
                }
            }
            false
        }
    };
    let mut a = Agent::new(
        FakeZfs(world.clone()),
        daemon,
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_points(Box::new(hook));
    let t = a.begin().expect("begun");
    name.borrow_mut().clone_from(&t);
    world.borrow_mut().destroyer = Some(t.clone());
    if give_up {
        let _ = a.settle(Duration::from_secs(5));
        let _ = a.recover(&t, Choice::GiveUp);
    }
    let settled = a.settle(LIMIT);
    let outcomes = a.outcomes().to_vec();
    drop(a);
    world.borrow_mut().advance(Duration::from_secs(2 * 3600));
    let w = world.borrow();
    let wrong = w.refs.get(&t).is_some_and(|r| {
        !w.commits[&r.commit]
            .cut
            .snaps
            .contains_key(&format!("{POOL}@{t}"))
    });
    let first_release = early.borrow().first().copied();
    let early_any = first_release.is_some();
    let line = format!(
        "{mode:?} no_answer={no_answer} give_up={give_up} save={d_s} s lag={lag_ms:?}: settled {settled:?}; outcomes {:?}; released while pending {early_any} (at {} s after the send); snap/t on {:?}; wrong {wrong}; holds {:?}",
        outcomes.iter().map(|(n, o)| format!("{n} {o:?}")).collect::<Vec<_>>(),
        first_release.map_or(-1, |at| (at as i64 - sent_at.get() as i64) / 1000),
        w.refs.get(&t).map(|r| r.number),
        holds(&w),
    );
    D3 {
        d_s,
        early: early_any,
        wrong,
        line,
    }
}

/// D3: a slow asking save. Prints every row; asserts nothing, so that
/// the table shows the threshold (it is a probe of a bot's finding).
#[test]
fn d3_a_slow_asking_save() {
    let mut early = std::collections::BTreeMap::<u64, (u32, u32, u32)>::new();
    for mode in [Mode::Idle, Mode::Busy] {
        for no_answer in [false, true] {
            for give_up in [false, true] {
                for d_s in [0, 30, 60, 79, 81, 120, 300] {
                    let mut lags: Vec<Option<u64>> = vec![None, Some(365 * 24 * 3_600_000)];
                    lags.extend((0..=24).map(|i| Some(500_000 + i * 10_000)));
                    for lag in lags {
                        let o = d3_run(mode, no_answer, give_up, d_s, lag);
                        let e = early.entry(o.d_s).or_default();
                        e.0 += 1;
                        e.1 += u32::from(o.early);
                        e.2 += u32::from(o.wrong);
                        if o.early || o.wrong || lag.is_none() {
                            eprintln!("D3 {}", o.line);
                        }
                    }
                }
            }
        }
    }
    for (d, (n, e, w)) in early {
        eprintln!("D3 save {d} s: {n} runs, {e} released while pending, {w} wrong publishes");
    }
}
