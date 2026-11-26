//! Review scratch for #153 (M2.6c-2b), sixth look at 291746c: D2 across
//! restarts. Copied in as `engine/review_scratch.rs` with a `mod` line
//! appended to `engine/tests.rs` (`scratch153f_patch.py`); never
//! committed to the product. Run with `cargo test -p skyzfs-agent
//! --release --lib review_scratch -- --nocapture --test-threads=1`.
//!
//! One publish, run as a chain of processes: process `p` stops at its
//! own point number `sched[p]` (or, in the loops, at the same point in
//! every process), a new agent starts after `DOWN`, and the first
//! process past the schedule runs with no stop. The cases:
//!
//! - `Never`: `pool/vol` skipped by every take (the take never whole);
//! - `Held(n)`: as `Never`, and a person's hold on the n-th take landed,
//!   so that its destroy is refused;
//! - `NotAsBefore`: every take whole, a property set between the reading
//!   and each take (each take not as read before it);
//! - `FirstWhole`: the first take whole and as before, `pool/vol`
//!   skipped from the second take on.
//!
//! Judged per run: the end (given up as the case allows, or published
//! where it may be), the takes against `CHECKS_FAILED_MAX` plus the
//! uncounted retakes, nothing held, under way, kept or left as a
//! manifest, and the reports.

use std::collections::BTreeMap;

use super::*;
use crate::record::Record;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Case {
    Never,
    Held(usize),
    NotAsBefore,
    FirstWhole,
}

const CASES: [Case; 6] = [
    Case::Never,
    Case::Held(1),
    Case::Held(2),
    Case::Held(3),
    Case::NotAsBefore,
    Case::FirstWhole,
];

/// How a process is stopped.
#[derive(Debug, Clone)]
enum Stops {
    /// Process `p` at its own point number `sched[p]`; none after.
    Sched(Vec<usize>),
    /// Every process at its own point number `k`, up to `MAX_PROCS`.
    Same(usize),
    /// Every process at the `nth` reach (from 1) of `point` in it.
    SamePoint(Point, usize),
}

const MAX_PROCS: usize = 400;

#[derive(Debug, Clone)]
struct Knobs {
    case: Case,
    stops: Stops,
    /// The daemon away for `ms` from the global point number `at`.
    away: Option<(usize, u64)>,
}

struct Out {
    procs: usize,
    /// Points reached by the last process.
    last_points: usize,
    takes_asked: usize,
    takes_landed: usize,
    uncounted: usize,
    outcomes: Vec<(String, Outcome)>,
    reports: Vec<(String, Report)>,
    settled: Result<(), String>,
    begun: bool,
    age_ms: u64,
    problems: Vec<String>,
}

fn vol() -> String {
    format!("{POOL}/vol")
}

fn run(k: &Knobs) -> Out {
    let world = World::new(Mode::Busy);
    if matches!(k.case, Case::Never | Case::Held(_)) {
        world.borrow_mut().skip = Some(vol());
    }
    let start_ms = world.borrow().now_ms;
    let keep = MemKeep::default();
    let takes_asked = Rc::new(Cell::new(0usize));
    let takes_after = Rc::new(Cell::new(0usize));
    let uncounted = Rc::new(Cell::new(0usize));
    let global = Rc::new(Cell::new(0usize));
    let held_name: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let (mut outcomes, mut reports) = (Vec::new(), Vec::new());
    let mut procs = 0;
    let mut last_points = 0;
    let mut settled: Result<(), String> = Ok(());
    loop {
        if procs >= MAX_PROCS {
            settled = Err(format!("{MAX_PROCS} processes"));
            break;
        }
        let p = procs;
        procs += 1;
        let own = Rc::new(Cell::new(0usize));
        let prev: Rc<Cell<Option<Point>>> = Rc::new(Cell::new(None));
        let reached: Rc<RefCell<BTreeMap<Point, usize>>> = Rc::new(RefCell::new(BTreeMap::new()));
        let stop_at: Option<usize> = match &k.stops {
            Stops::Sched(s) => s.get(p).copied(),
            Stops::Same(i) => Some(*i),
            Stops::SamePoint(..) => None,
        };
        let points: Points = Box::new({
            let (world, takes_asked, takes_after, uncounted, global, held_name) = (
                world.clone(),
                takes_asked.clone(),
                takes_after.clone(),
                uncounted.clone(),
                global.clone(),
                held_name.clone(),
            );
            let (own, prev, reached, case, stops, away) =
                (own.clone(), prev.clone(), reached.clone(), k.case, k.stops.clone(), k.away);
            move |pt: Point, name: &str| {
                let i = own.get();
                own.set(i + 1);
                let g = global.get();
                global.set(g + 1);
                let nth = {
                    let mut r = reached.borrow_mut();
                    let n = r.entry(pt).or_insert(0);
                    *n += 1;
                    *n
                };
                let stop = match stops {
                    Stops::SamePoint(sp, sn) => sp == pt && sn == nth,
                    _ => stop_at == Some(i),
                };
                // A take is asked only where the process goes on past
                // the point before it.
                if pt == at(Effect::Take, false) && !stop {
                    takes_asked.set(takes_asked.get() + 1);
                    match case {
                        Case::NotAsBefore => world.borrow_mut().user(User::SetProperty, ""),
                        Case::FirstWhole if takes_asked.get() == 2 => {
                            world.borrow_mut().skip = Some(vol());
                        }
                        _ => {}
                    }
                }
                if pt == at(Effect::Take, true) {
                    takes_after.set(takes_after.get() + 1);
                    if case == Case::Held(takes_after.get()) {
                        let mut w = world.borrow_mut();
                        if let Some(snap) = w
                            .pool
                            .as_mut()
                            .and_then(|p| p.snaps.get_mut(&format!("{POOL}@{name}")))
                        {
                            snap.holds.insert("theirs".into());
                            *held_name.borrow_mut() = Some(name.to_string());
                        }
                    }
                }
                if pt == at(Effect::Destroy, false) && prev.get() != Some(at(Effect::Failed, true)) {
                    uncounted.set(uncounted.get() + 1);
                }
                prev.set(Some(pt));
                if let Some((a, ms)) = away {
                    if a == g {
                        let mut w = world.borrow_mut();
                        w.away_until = w.now_ms + ms;
                    }
                }
                stop
            }
        });
        let mut agent = agent(&world, &keep).with_points(points);
        let s = if p == 0 {
            agent.begin().and_then(|_| agent.settle(LIMIT))
        } else {
            agent.settle(LIMIT)
        };
        outcomes.extend(agent.outcomes().iter().cloned());
        reports.extend(agent.reports().iter().cloned());
        drop(agent);
        last_points = own.get();
        match s {
            Ok(()) => break,
            Err(Error::Stopped) => world.borrow_mut().advance(DOWN),
            // `begin` refused while the daemon is away: nothing begun.
            Err(Error::Daemon(_))
                if p == 0
                    && keep.0.borrow().record.as_deref().map_or(true, |b| {
                        Record::decode(b).is_ok_and(|r| r.under_way.is_empty())
                    }) =>
            {
                break
            }
            // Settled for its whole limit: the next process goes on.
            Err(Error::Unsettled(_)) => world.borrow_mut().advance(DOWN),
            Err(e) => {
                settled = Err(format!("process {p}: {e}"));
                break;
            }
        }
    }
    let w = world.borrow();
    let age_ms = w.now_ms - start_ms;
    let files = keep.0.borrow();
    let record = files.record.as_deref().map(Record::decode);
    let mut problems = Vec::new();
    let record = match record {
        None => None,
        Some(Ok(r)) => Some(r),
        Some(Err(e)) => {
            problems.push(format!("record corrupt: {e:?}"));
            None
        }
    };
    let begun = record.as_ref().is_some_and(|r| !r.under_way.is_empty() || !r.abandoned.is_empty())
        || takes_asked.get() > 0;
    let mut out = Out {
        procs,
        last_points,
        takes_asked: takes_asked.get(),
        takes_landed: w.programs.len(),
        uncounted: uncounted.get(),
        outcomes,
        reports,
        settled,
        begun,
        age_ms,
        problems,
    };
    judge(&mut out, k, &w, record.as_ref(), &files, held_name.borrow().clone());
    out
}

fn judge(
    out: &mut Out,
    k: &Knobs,
    w: &World,
    record: Option<&Record>,
    files: &crate::engine::fake::MemFiles,
    held: Option<String>,
) {
    let mut bad = |s: String| out.problems.push(s);
    if let Err(e) = &out.settled {
        bad(format!("not settled: {e}"));
        return;
    }
    if !out.begun {
        return;
    }
    let Some(record) = record else {
        bad("no record".into());
        return;
    };
    if !record.under_way.is_empty() {
        bad(format!("under way: {:?}", record.under_way));
    }
    let mut ends: Vec<(String, Outcome)> = out
        .outcomes
        .iter()
        .filter(|(_, o)| !matches!(o, Outcome::Stopped(_)))
        .cloned()
        .collect();
    if ends.is_empty() {
        // The end committed, and the process stopped before it was told
        // (at the unlink after the commit): read it from the record, or
        // from object storage.
        if let Some(a) = record.abandoned.last() {
            ends.push((a.name.clone(), Outcome::Abandoned(a.why)));
        } else if let Some((t, r)) = w.refs.iter().next() {
            ends.push((t.clone(), Outcome::Published(r.commit)));
        }
    }
    if ends.len() != 1 {
        bad(format!("ends: {:?}", out.outcomes));
        return;
    }
    let (name, end) = ends[0].clone();
    let too_old = out.age_ms >= ms(plan::AGE_MAX);
    let allowed = match (k.case, &end) {
        (_, Outcome::Abandoned(Why::TooOld)) => too_old,
        (Case::Never, Outcome::Abandoned(Why::NotWhole)) => true,
        (Case::Held(_), Outcome::Abandoned(Why::NotWhole | Why::ManifestLost)) => true,
        (Case::NotAsBefore, Outcome::Abandoned(Why::NotWhole)) => true,
        (Case::FirstWhole, Outcome::Abandoned(Why::NotWhole) | Outcome::Published(_)) => true,
        _ => false,
    };
    if !allowed {
        bad(format!("end {end:?}"));
    }
    if record.abandoned.iter().filter(|a| a.name == name).count()
        != usize::from(matches!(end, Outcome::Abandoned(_)))
    {
        bad(format!("abandoned in the record: {:?}", record.abandoned));
    }
    // The takes.
    let bound = plan::CHECKS_FAILED_MAX as usize + out.uncounted;
    if out.takes_asked > bound || out.takes_landed > bound {
        bad(format!(
            "takes {} asked, {} landed, bound {bound}",
            out.takes_asked, out.takes_landed
        ));
    }
    if matches!(k.case, Case::Never | Case::Held(_)) && out.uncounted != 0 {
        bad(format!("{} uncounted retakes of a take never whole", out.uncounted));
    }
    if out.uncounted > out.procs.saturating_sub(1) {
        bad(format!("{} uncounted in {} processes", out.uncounted, out.procs));
    }
    // What is left.
    let pool = w.pool.as_ref().expect("the pool");
    let left: Vec<&String> = pool.snaps.keys().collect();
    let published = matches!(end, Outcome::Published(_));
    let kept_ok = |s: &String| match (&end, &held) {
        (Outcome::Abandoned(Why::ManifestLost), Some(h)) => s.ends_with(&format!("@{h}")),
        (Outcome::Published(_), _) => s.ends_with(&format!("@{name}")),
        // A publish too old at Intent keeps its take (`plan`, from
        // before this pull request).
        (Outcome::Abandoned(Why::TooOld), _) => s.ends_with(&format!("@{name}")),
        _ => false,
    };
    if !left.iter().all(|s| kept_ok(s)) {
        bad(format!("left in the pool: {left:?}"));
    }
    if matches!(end, Outcome::Abandoned(Why::ManifestLost)) && left.is_empty() {
        bad("given up kept, and nothing kept".into());
    }
    if !holds(w).is_empty() {
        bad(format!("held: {:?}", holds(w)));
    }
    if !files.manifests.is_empty() {
        bad(format!("manifests: {:?}", files.manifests.keys()));
    }
    if published != w.refs.contains_key(&name) || w.refs.len() != usize::from(published) {
        bad(format!("refs {:?}", w.refs.keys()));
    }
    // The reports.
    if !out
        .reports
        .iter()
        .all(|(_, r)| matches!(r, Report::NotWhole(_)) || (k.away.is_some() && *r == Report::HeldAway))
    {
        bad(format!("reports {:?}", out.reports));
    }
    if matches!(end, Outcome::Abandoned(Why::NotWhole)) && out.reports.is_empty() {
        bad("given up not whole with no report".into());
    }
    if published && out.reports.iter().any(|(_, r)| matches!(r, Report::NotWhole(_))) {
        bad(format!("published after {:?}", out.reports));
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Default)]
struct Tally {
    runs: usize,
    bad: usize,
    kinds: BTreeMap<String, (usize, String)>,
    ends: BTreeMap<String, usize>,
    takes: BTreeMap<usize, usize>,
    uncounted: BTreeMap<usize, usize>,
    procs_max: usize,
    report_kinds: BTreeMap<String, usize>,
    not_begun: usize,
}

impl Tally {
    fn add(&mut self, k: &Knobs, o: &Out) {
        self.runs += 1;
        self.procs_max = self.procs_max.max(o.procs);
        if !o.begun {
            self.not_begun += 1;
        }
        let end = o
            .outcomes
            .iter()
            .filter(|(_, x)| !matches!(x, Outcome::Stopped(_)))
            .map(|(_, x)| match x {
                Outcome::Published(_) => "published".to_string(),
                Outcome::Abandoned(w) => format!("{w:?}"),
                Outcome::Stopped(_) => unreachable!(),
            })
            .collect::<Vec<_>>()
            .join("+");
        *self.ends.entry(if end.is_empty() { "none".into() } else { end }).or_default() += 1;
        *self.takes.entry(o.takes_asked).or_default() += 1;
        *self.uncounted.entry(o.uncounted).or_default() += 1;
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for (_, r) in &o.reports {
            let w = match r {
                Report::NotWhole(v) if v.is_empty() => "NW[]".to_string(),
                Report::NotWhole(_) => "NW[..]".to_string(),
                r => format!("{r:?}"),
            };
            *kinds.entry(w).or_default() += 1;
        }
        let rk = kinds
            .iter()
            .map(|(w, n)| if *n > 3 { format!("{w}x>3") } else { format!("{w}x{n}") })
            .collect::<Vec<_>>()
            .join(",");
        *self.report_kinds.entry(rk).or_default() += 1;
        if !o.problems.is_empty() {
            self.bad += 1;
            for p in &o.problems {
                let kind: String = p
                    .chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .take(90)
                    .collect();
                let e = self.kinds.entry(kind).or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() {
                    e.1 = format!("{k:?}: {p}");
                }
            }
        }
    }

    fn print(&self, what: &str) {
        println!(
            "{what}: {} runs, {} with a problem, {} not begun, at most {} processes",
            self.runs, self.bad, self.not_begun, self.procs_max
        );
        println!("  ends: {:?}", self.ends);
        println!("  takes asked: {:?}", self.takes);
        println!("  uncounted retakes: {:?}", self.uncounted);
        println!("  reports: {:?}", self.report_kinds);
        for (kind, (n, first)) in &self.kinds {
            let first: String = first.chars().take(400).collect();
            println!("  PROBLEM x{n}: {kind}\n    first: {first}");
        }
    }
}

fn dfs(case: Case, sched: &mut Vec<usize>, depth: usize, t: &mut Tally) {
    let k = Knobs {
        case,
        stops: Stops::Sched(sched.clone()),
        away: None,
    };
    let o = run(&k);
    t.add(&k, &o);
    if o.procs != sched.len() + 1 && o.settled.is_ok() {
        // A stop past the points the process reached: none happened.
        return;
    }
    if sched.len() < depth {
        for i in 0..o.last_points {
            sched.push(i);
            dfs(case, sched, depth, t);
            sched.pop();
        }
    }
}

/// SX1: every schedule of stops, up to `SX1_DEPTH` processes stopped
/// (default 3), each at any point it reaches.
#[test]
fn sx1_every_schedule_of_stops() {
    let depth: usize = std::env::var("SX1_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let only = std::env::var("SX1_CASES").unwrap_or_default();
    for case in CASES {
        if !only.is_empty() && !only.split(',').any(|c| c == format!("{case:?}")) {
            continue;
        }
        let mut t = Tally::default();
        dfs(case, &mut Vec::new(), depth, &mut t);
        t.print(&format!("SX1 {case:?} depth {depth}"));
    }
}

/// SX2: restart loops: every process stopped at its own point number k,
/// and every process stopped at the nth reach of one point, up to
/// `MAX_PROCS` processes.
#[test]
fn sx2_restart_loops() {
    for case in CASES {
        let mut t = Tally::default();
        for i in 0..80 {
            let k = Knobs {
                case,
                stops: Stops::Same(i),
                away: None,
            };
            let o = run(&k);
            t.add(&k, &o);
        }
        t.print(&format!("SX2 {case:?} same k"));
        let mut t = Tally::default();
        for effect in Effect::ALL {
            for after in [false, true] {
                for nth in 1..=3 {
                    let k = Knobs {
                        case,
                        stops: Stops::SamePoint(at(effect, after), nth),
                        away: None,
                    };
                    let o = run(&k);
                    t.add(&k, &o);
                }
            }
        }
        t.print(&format!("SX2 {case:?} same point"));
    }
}

/// SX3: random schedules of up to 20 stops, each at a point number below
/// 60, with the daemon away for a while from a random point in a third
/// of them; `SX3_RUNS` runs a case (default 20000).
#[test]
fn sx3_random_schedules_and_daemon_away() {
    let runs: usize = std::env::var("SX3_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for case in CASES {
        let mut t = Tally::default();
        for _ in 0..runs {
            let len = 1 + next(20) as usize;
            let sched: Vec<usize> = (0..len).map(|_| next(60) as usize).collect();
            let away = if next(3) == 0 {
                let ms = [5_000, 20_000, 120_000, 3 * 3_600_000][next(4) as usize];
                Some((next(200) as usize, ms))
            } else {
                None
            };
            let k = Knobs {
                case,
                stops: Stops::Sched(sched),
                away,
            };
            let o = run(&k);
            t.add(&k, &o);
        }
        t.print(&format!("SX3 {case:?} random"));
    }
}

/// SX0: the four cases with no stop, in detail.
#[test]
fn sx0_no_stop() {
    for case in CASES {
        let k = Knobs {
            case,
            stops: Stops::Sched(Vec::new()),
            away: None,
        };
        let o = run(&k);
        println!(
            "SX0 {case:?}: {} points, takes {}/{}, uncounted {}, ends {:?}, reports {:?}, problems {:?}",
            o.last_points, o.takes_asked, o.takes_landed, o.uncounted, o.outcomes, o.reports, o.problems
        );
    }
}
