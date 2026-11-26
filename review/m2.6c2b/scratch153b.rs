//! Review scratch tests for #153 (M2.6c-2b), next look at 3d40045.
//! `scratch153.rs` (the searches S1-S3 and the probes PA, PB, PM, R154
//! of the review at 65d72ee) with: the knobs `skip` (the head's own
//! `World::skip`: `pool/vol` is skipped by every `zfs snapshot -r`, a
//! take never whole), `every_take` (a person's action between the
//! reading before each take and the take, in every process) and
//! `loop_stop` (each process stopped at its own point number k, a new
//! one started, up to `MAX_PROCS`); and the searches SD1 (a take never
//! whole crossed with a stop at every point, a person's action at every
//! point and the destroy's answers) and SL (restart loops), judged by
//! `judge_d1`. Copied in by `scratch153b_patch.py`; never committed to
//! the product. Run with `cargo test -p skyzfs-agent --release --lib
//! review_scratch -- --nocapture --test-threads=1`.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::*;
use crate::record::Record;

#[derive(Debug, Clone, Copy)]
struct Knobs {
    mode: Mode,
    late: Option<usize>,
    on_land: Option<User>,
    snap_fail: bool,
    ds_fail: Option<usize>,
    destroy_mode: u8,
    act: Option<(usize, User)>,
    stop: Option<usize>,
    skip: bool,
    every_take: Option<User>,
    loop_stop: Option<usize>,
}

/// The most processes a restart loop runs (`Knobs::loop_stop`).
const MAX_PROCS: usize = 400;

fn base(mode: Mode) -> Knobs {
    Knobs {
        mode,
        late: None,
        on_land: None,
        snap_fail: false,
        ds_fail: None,
        destroy_mode: 0,
        act: None,
        stop: None,
        skip: false,
        every_take: None,
        loop_stop: None,
    }
}

/// The publish's name as the record holds it, else the last take's.
fn current_name(keep: &MemKeep, world: &World) -> String {
    keep.0
        .borrow()
        .record
        .as_deref()
        .and_then(|b| Record::decode(b).ok())
        .and_then(|r| r.under_way.first().map(|p| p.name.clone()))
        .or_else(|| world.programs.keys().last().cloned())
        .unwrap_or_default()
}

struct Ran2 {
    ran: Ran,
    /// The committed oracle's verdict.
    check: Result<(), String>,
    /// The GUID oracle's verdict.
    strict: Result<(), String>,
    /// `pool@t`s of the agent's left after the late landing.
    orphans: Vec<String>,
    /// Each take asked: the process's number and the world's time.
    takes: Vec<(usize, u64)>,
    /// The processes run.
    procs: usize,
}

fn run_k(k: Knobs) -> Ran2 {
    let world = World::new(k.mode);
    {
        let mut w = world.borrow_mut();
        w.late_take = k.late;
        w.on_land = k.on_land;
        w.snap_fail_once = k.snap_fail;
        w.datasets_fail_at = k.ds_fail;
        w.destroy_mode = k.destroy_mode;
        if k.skip {
            w.skip = Some(format!("{POOL}/vol"));
        }
    }
    let keep = MemKeep::default();
    let count = Rc::new(Cell::new(0usize));
    let stopped_at = Rc::new(Cell::new(None));
    let gone = Rc::new(RefCell::new(BTreeSet::new()));
    let released = Rc::new(RefCell::new(Vec::new()));
    let takes: Rc<RefCell<Vec<(usize, u64)>>> = Rc::new(RefCell::new(Vec::new()));
    let proc_no = Rc::new(Cell::new(0usize));
    let on_take = {
        let (world, keep, takes, proc_no) = (world.clone(), keep.clone(), takes.clone(), proc_no.clone());
        move |point: Point| {
            if point == (Point { effect: Effect::Take, after: false }) {
                let now = world.borrow().now_ms;
                takes.borrow_mut().push((proc_no.get(), now));
                if let Some(user) = k.every_take {
                    let t = current_name(&keep, &world.borrow());
                    world.borrow_mut().user(user, &t);
                }
            }
        }
    };
    let hook = {
        let on_take = on_take.clone();
        let (count, stopped_at, world, keep) =
            (count.clone(), stopped_at.clone(), world.clone(), keep.clone());
        let (gone, released) = (gone.clone(), released.clone());
        move |point: Point| {
            let i = count.get();
            count.set(i + 1);
            on_take(point);
            note_gone(&world, &gone, point);
            note_released(&world, &released, point);
            if let Some((at, user)) = k.act {
                if at == i {
                    let t = current_name(&keep, &world.borrow());
                    world.borrow_mut().user(user, &t);
                }
            }
            if k.stop == Some(i) || k.loop_stop == Some(i) {
                stopped_at.set(Some(point));
                return true;
            }
            false
        }
    };
    let mut first = agent(&world, &keep).with_points(Box::new(hook));
    let mut outcomes = Vec::new();
    let mut reports = Vec::new();
    let settled = first.begin().and_then(|_| first.settle(LIMIT));
    outcomes.extend(first.outcomes().iter().cloned());
    reports.extend(first.reports().iter().cloned());
    drop(first);
    let mut settled = settled;
    let mut procs = 1;
    while matches!(settled, Err(Error::Stopped)) && procs < MAX_PROCS {
        world.borrow_mut().advance(DOWN);
        proc_no.set(procs);
        procs += 1;
        let own = Rc::new(Cell::new(0usize));
        let note = {
            let (world, gone, released) = (world.clone(), gone.clone(), released.clone());
            let on_take = on_take.clone();
            let stopped_at = stopped_at.clone();
            move |point: Point| {
                let i = own.get();
                own.set(i + 1);
                on_take(point);
                note_gone(&world, &gone, point);
                note_released(&world, &released, point);
                if k.loop_stop == Some(i) {
                    stopped_at.set(Some(point));
                    return true;
                }
                false
            }
        };
        let mut again = agent(&world, &keep).with_points(Box::new(note));
        settled = again.settle(LIMIT);
        outcomes.extend(again.outcomes().iter().cloned());
        reports.extend(again.reports().iter().cloned());
    }
    let cuts_read = world.borrow().cuts_read;
    let ran = Ran {
        world: world.clone(),
        files: keep.0.clone(),
        settled: settled.map_err(|e| e.to_string()),
        outcomes,
        reports,
        points: count.get(),
        cuts_read,
        stopped_at: stopped_at.get(),
        gone_at_done: gone.take(),
        released_pending: released.take(),
    };
    let checked = check(&ran);
    let strict = strict(&ran);
    // The kernel's sync task ends whatever the agent does.
    world.borrow_mut().land();
    let orphans = orphans(&ran);
    let takes = takes.borrow().clone();
    Ran2 {
        ran,
        check: checked,
        strict,
        orphans,
        takes,
        procs,
    }
}

/// The agent's takes left in the pool that no publish, no destroy of the
/// agent's and no give-up accounts for.
fn orphans(ran: &Ran) -> Vec<String> {
    let world = ran.world.borrow();
    let files = ran.files.borrow();
    let record = files.record.as_deref().and_then(|b| Record::decode(b).ok());
    let Some(pool) = world.pool.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (t, manifest) in &world.programs {
        if world.refs.contains_key(t) {
            continue;
        }
        let left = manifest.members.iter().any(|m| {
            pool.snaps
                .values()
                .any(|s| s.guid == m.snapshot_guid)
        });
        if !left {
            continue;
        }
        let given_up = record
            .as_ref()
            .and_then(|r| r.abandoned.iter().find(|a| a.name == *t))
            .map(|a| a.why);
        out.push(format!("{t} (given up {given_up:?})"));
    }
    out
}

/// `check`, with a published member matched to its cut by its snapshot's
/// GUID (a dataset renamed after the take keeps its `@t` under its new
/// name), and the rest as the committed oracle has it.
fn strict(ran: &Ran) -> Result<(), String> {
    let world = ran.world.borrow();
    ran.settled.clone()?;
    if !ran.released_pending.is_empty() {
        return Err(format!(
            "a hold released while a request pended: {:?}",
            ran.released_pending
        ));
    }
    for (t, r) in &world.refs {
        let manifest = Manifest::decode(&r.manifest).map_err(|e| format!("snap/{t}: {e}"))?;
        let commit = world
            .commits
            .get(&r.commit)
            .ok_or(format!("snap/{t} names no commit"))?;
        for m in &manifest.members {
            if !commit.cut.snaps.values().any(|g| *g == m.snapshot_guid) {
                return Err(format!(
                    "WRONG PUBLISH: snap/{t} names cut {} (group {}), which does not hold {}'s snapshot {:x}",
                    commit.cut.number, commit.cut.group, m.name, m.snapshot_guid
                ));
            }
        }
        if world.programs.get(t) != Some(&manifest) && world.retakes.get(t) != Some(&manifest) {
            return Err(format!(
                "WRONG MANIFEST: snap/{t}'s manifest is not the take's:\n  published {:?}\n  take      {:?}",
                manifest.members,
                world.programs.get(t).map(|m| &m.members)
            ));
        }
    }
    drop(world);
    // The rest of the committed oracle, where item 0 passed by GUID.
    match check(ran) {
        Err(why) if why.contains("which does not hold") => Ok(()),
        other => other,
    }
}

const USERS_X: [User; 10] = [
    User::SetProperty,
    User::NewDs,
    User::RmDs,
    User::RenameDs,
    User::Retake,
    User::DestroyChild,
    User::HoldTheirs,
    User::ReleaseChild,
    User::Attach,
    User::Destroy,
];

#[derive(Default)]
struct Tally {
    runs: usize,
    published: usize,
    given_up: BTreeMap<String, usize>,
    retakes: usize,
    panics: Vec<String>,
    check_fail: BTreeMap<String, Vec<String>>,
    strict_fail: BTreeMap<String, Vec<String>>,
    orphans: BTreeMap<String, Vec<String>>,
}

/// A failure's kind: its words without names and numbers.
fn kind_of(why: &str) -> String {
    let first = why.lines().next().unwrap_or("");
    first
        .split_whitespace()
        .map(|w| {
            if w.chars().any(|c| c.is_ascii_digit()) {
                "#"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(140)
        .collect()
}

impl Tally {
    fn add(&mut self, k: Knobs, r: std::thread::Result<Ran2>) {
        self.runs += 1;
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                let why = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                if self.panics.len() < 400 {
                    self.panics.push(format!("{k:?}: {why}"));
                }
                return;
            }
        };
        for (_, o) in &r.ran.outcomes {
            match o {
                Outcome::Published(_) => self.published += 1,
                Outcome::Abandoned(why) => {
                    *self.given_up.entry(format!("{why:?}")).or_default() += 1
                }
                Outcome::Stopped(_) => {}
            }
        }
        if r.ran
            .world
            .borrow()
            .log
            .iter()
            .any(|l| l.starts_with("agent destroy -r ") && !l.contains("refused"))
        {
            self.retakes += 1;
        }
        let ex = |r: &Ran2, why: &str| {
            format!(
                "{k:?}: {why}\n    outcomes {:?}\n    log: {}",
                r.ran.outcomes,
                r.ran.world.borrow().log.join(" | ")
            )
        };
        if let Err(why) = &r.check {
            let e = self.check_fail.entry(kind_of(why)).or_default();
            if e.len() < 3 {
                e.push(ex(&r, why));
            } else {
                e.push(String::new());
            }
        }
        if let Err(why) = &r.strict {
            let e = self.strict_fail.entry(kind_of(why)).or_default();
            if e.len() < 3 {
                e.push(ex(&r, why));
            } else {
                e.push(String::new());
            }
        }
        if !r.orphans.is_empty() {
            let key = format!(
                "{:?} late={:?} act={:?}",
                r.orphans
                    .iter()
                    .map(|o| o.split_once(' ').map_or("", |x| x.1).to_string())
                    .collect::<Vec<_>>(),
                k.late.map(|l| l.min(9)),
                k.act.map(|a| a.1)
            );
            let e = self.orphans.entry(key).or_default();
            if e.len() < 1 {
                e.push(ex(&r, &r.orphans.join(", ")));
            } else {
                e.push(String::new());
            }
        }
    }

    fn print(&self, what: &str, secs: f64) {
        println!(
            "{what} ({secs:.0} s): {} runs, {} published, given up {:?}, {} with a destroy of the agent's, {} panics",
            self.runs,
            self.published,
            self.given_up,
            self.retakes,
            self.panics.len()
        );
        for p in self.panics.iter().take(5) {
            println!("  PANIC {p}");
        }
        for (label, map) in [
            ("check (committed oracle)", &self.check_fail),
            ("strict (GUID oracle)", &self.strict_fail),
            ("orphans after the late landing", &self.orphans),
        ] {
            let n: usize = map.values().map(Vec::len).sum();
            println!("  {label}: {n} runs");
            for (kind, ex) in map {
                println!("    {} x {kind}", ex.len());
                for e in ex.iter().filter(|e| !e.is_empty()).take(1) {
                    let e: String = e.chars().take(2500).collect();
                    println!("      e.g. {e}");
                }
            }
        }
    }
}

fn quiet<T>(f: impl FnOnce() -> T) -> std::thread::Result<T> {
    catch_unwind(AssertUnwindSafe(f))
}

fn points_of(k: Knobs) -> usize {
    quiet(|| run_k(Knobs { act: None, stop: None, loop_stop: None, ..k }).ran.points).unwrap_or(80)
}

const LATES: [Option<usize>; 7] = [None, Some(0), Some(1), Some(2), Some(3), Some(5), Some(usize::MAX)];

/// S1: the take's answers crossed with a person's action at every point.
#[test]
fn s1_take_answers_by_a_persons_action_at_every_point() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let t0 = std::time::Instant::now();
    let mut tally = Tally::default();
    for mode in [Mode::Busy, Mode::Idle] {
        for late in LATES {
            for destroy_mode in [0u8, 1, 2] {
                let k = Knobs {
                    late,
                    destroy_mode,
                    ..base(mode)
                };
                tally.add(k, quiet(|| run_k(k)));
                let points = points_of(k);
                for user in USERS_X {
                    for i in 0..points {
                        let k = Knobs {
                            act: Some((i, user)),
                            ..k
                        };
                        tally.add(k, quiet(|| run_k(k)));
                    }
                }
            }
        }
    }
    std::panic::set_hook(hook);
    tally.print("S1", t0.elapsed().as_secs_f64());
}

/// S2: a late take landing after k calls, a person's action as it lands,
/// and the agent stopped at every point.
#[test]
fn s2_late_landing_with_an_action_and_a_stop_at_every_point() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let t0 = std::time::Instant::now();
    let mut tally = Tally::default();
    for mode in [Mode::Busy, Mode::Idle] {
        for late in [0, 1, 2, 3, 4, 5, 6, usize::MAX] {
            for on_land in [
                None,
                Some(User::SetProperty),
                Some(User::NewDs),
                Some(User::RmDs),
                Some(User::RenameDs),
                Some(User::DestroyChild),
                Some(User::Retake),
            ] {
                let k = Knobs {
                    late: Some(late),
                    on_land,
                    ..base(mode)
                };
                tally.add(k, quiet(|| run_k(k)));
                let points = points_of(k);
                for i in 0..points {
                    let k = Knobs { stop: Some(i), ..k };
                    tally.add(k, quiet(|| run_k(k)));
                }
            }
        }
    }
    std::panic::set_hook(hook);
    tally.print("S2", t0.elapsed().as_secs_f64());
}

/// S3: a take that fails before its ioctl, and a `datasets` read that
/// fails, crossed with a late take and a person's action at every point.
#[test]
fn s3_failed_take_and_read_by_late_and_action() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let t0 = std::time::Instant::now();
    let mut tally = Tally::default();
    for snap_fail in [false, true] {
        for ds_fail in [None, Some(0), Some(1), Some(2), Some(3)] {
            for late in [None, Some(0), Some(2), Some(usize::MAX)] {
                let k = Knobs {
                    snap_fail,
                    ds_fail,
                    late,
                    ..base(Mode::Busy)
                };
                tally.add(k, quiet(|| run_k(k)));
                let points = points_of(k);
                for user in [User::SetProperty, User::NewDs, User::RenameDs, User::RmDs] {
                    for i in 0..points {
                        let k = Knobs {
                            act: Some((i, user)),
                            ..k
                        };
                        tally.add(k, quiet(|| run_k(k)));
                    }
                }
                for i in 0..points {
                    let k = Knobs { stop: Some(i), ..k };
                    tally.add(k, quiet(|| run_k(k)));
                }
            }
        }
    }
    std::panic::set_hook(hook);
    tally.print("S3", t0.elapsed().as_secs_f64());
}

/// Probe A: the surviving plants' scenarios, each judged by the GUID
/// oracle: a take that lands at the agent's reading before its next take,
/// with a property set, a dataset created, destroyed or renamed just as
/// it lands; and a person's action at every point.
#[test]
fn pa_scenarios_for_the_surviving_plants() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut bad = Vec::new();
    let mut runs = 0;
    for late in 0..8usize {
        for on_land in [User::SetProperty, User::NewDs, User::RmDs, User::RenameDs] {
            let k = Knobs {
                late: Some(late),
                on_land: Some(on_land),
                ..base(Mode::Busy)
            };
            runs += 1;
            match quiet(|| run_k(k)) {
                Ok(r) => {
                    if let Err(why) = &r.strict {
                        bad.push(format!("{k:?}: {}", why.lines().next().unwrap_or("")));
                    }
                }
                Err(_) => bad.push(format!("{k:?}: panic")),
            }
        }
    }
    std::panic::set_hook(hook);
    println!("PA: {runs} runs, {} judged wrong", bad.len());
    for b in bad.iter().take(12) {
        println!("  {b}");
    }
}

/// Probe B: a dataset that `zfs snapshot -r` skips (inconsistent: a full
/// receive under way or interrupted with `-s`) but the members program
/// lists. Every take is not whole: how often does the agent take and
/// destroy, and does time pass between?
#[test]
fn pb_an_inconsistent_dataset() {
    let world = World::new(Mode::Busy);
    {
        let mut w = world.borrow_mut();
        let pool = w.pool.as_mut().expect("the pool");
        pool.datasets.push(fake::Ds {
            name: format!("{POOL}/recv"),
            kind: crate::manifest::Kind::Filesystem,
            guid: 0x5ec0,
            createtxg: 1,
            local: Default::default(),
        });
        pool.datasets
            .sort_by(|a, b| (a.name != POOL, &a.name).cmp(&(b.name != POOL, &b.name)));
        w.inconsistent.insert(format!("{POOL}/recv"));
    }
    let keep = MemKeep::default();
    let t0 = world.borrow().now_ms;
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut a = agent(&world, &keep);
        let begun = a.begin();
        let settled = begun.and_then(|_| a.settle(Duration::from_secs(600)));
        (settled.map_err(|e| e.to_string()), a.outcomes().to_vec())
    }));
    let w = world.borrow();
    let destroys = w.log.iter().filter(|l| l.starts_with("agent destroy -r")).count();
    let takes = w.calls.iter().filter(|c| **c == "snapshot").count();
    println!(
        "PB: {destroys} destroys, {takes} takes in {} ms of the world's time; result {:?}",
        w.now_ms - t0,
        r.as_ref().map(|x| &x.0).map_err(|e| e
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default())
    );
    println!("  last log lines: {:?}", &w.log[w.log.len().saturating_sub(6)..]);
}

/// PM: the bracket's scenarios only (a property set, a dataset created,
/// destroyed or renamed, at every point and as a late take lands), for
/// the plants that survived the committed tests: the number of runs the
/// GUID oracle judges wrong.
#[test]
fn pm_the_brackets_scenarios() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let t0 = std::time::Instant::now();
    let mut tally = Tally::default();
    let users = [User::SetProperty, User::NewDs, User::RmDs, User::RenameDs];
    for late in LATES {
        let k = Knobs {
            late,
            ..base(Mode::Busy)
        };
        let points = points_of(k);
        for user in users {
            for i in 0..points {
                let k = Knobs {
                    act: Some((i, user)),
                    ..k
                };
                tally.add(k, quiet(|| run_k(k)));
            }
            if late.is_some() {
                let k = Knobs {
                    on_land: Some(user),
                    ..k
                };
                tally.add(k, quiet(|| run_k(k)));
            }
        }
    }
    std::panic::set_hook(hook);
    tally.print("PM", t0.elapsed().as_secs_f64());
}

/// R154: the full mode as testing's recorder recorded it with this head's
/// Lua (#154's dispatch), read by this head's parser and judged by
/// `zfs::taken`. The directory of the unpacked artifact is in
/// `REC154` (one sub-directory per record, its `stdout`).
#[test]
fn r154_the_full_mode_as_recorded() {
    let Ok(dir) = std::env::var("REC154") else {
        println!("R154: REC154 not set");
        return;
    };
    for name in [
        "rig-members-full",
        "mirror-members-full",
        "rig-members-full-odd",
        "rig-members-full-odd-utf8",
        "mirror-members-full-odd",
        "rig-members-full-partial",
        "mirror-members-full-partial",
        "rig-members-full-absent",
        "mirror-members-full-absent",
    ] {
        let path = format!("{dir}/{name}/stdout");
        let Ok(text) = std::fs::read_to_string(&path) else {
            println!("R154 {name}: not recorded");
            continue;
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let returned = value["return"].as_object().expect("return").clone();
        let pool = if name.starts_with("rig") { "skzq" } else { "skzqm" };
        match crate::zfs::cli::parse_datasets(&returned) {
            Ok(datasets) => {
                let taken = crate::zfs::taken(&datasets, pool);
                let summary: Vec<String> = datasets
                    .iter()
                    .map(|d| {
                        format!(
                            "{} {:?} txg {} local {:?} at {:?}",
                            d.name,
                            d.kind,
                            d.createtxg,
                            d.local.keys().collect::<Vec<_>>(),
                            d.at.map(|a| a.createtxg)
                        )
                    })
                    .collect();
                let verdict = match &taken {
                    crate::zfs::Taken::Whole(p) => format!(
                        "Whole: group {} members {:?}",
                        p.createtxg,
                        p.members.iter().map(|m| &m.name).collect::<Vec<_>>()
                    ),
                    other => format!("{other:?}"),
                };
                println!("R154 {name}: {verdict}\n    {}", summary.join("\n    "));
            }
            Err(why) => println!("R154 {name}: parse error {why}"),
        }
    }
}

// ---------------------------------------------------------------------
// The next look at 3d40045: the bound on failed checks (D1), searched.
// ---------------------------------------------------------------------

/// `strict`, where a `Report::NotWhole` is allowed for a publish given
/// up `NotWhole`, or `ManifestLost` with its take kept (the give-up's
/// destroy failed), and nothing else is relaxed: no hold, nothing under
/// way, no manifest's file, no take left that no give-up accounts for.
fn judge_d1(r: &Ran2) -> Result<(), String> {
    let ran = &r.ran;
    // How each name ended: the outcomes, else the record's give-ups (a
    // process stopped between the give-up's commit and its outcome).
    let given_up = |t: &str| -> Option<Why> {
        ran.files
            .borrow()
            .record
            .as_deref()
            .and_then(|b| Record::decode(b).ok())
            .and_then(|rec| rec.abandoned.iter().find(|a| a.name == t).map(|a| a.why))
    };
    for (t, report) in &ran.reports {
        if let Report::NotWhole(names) = report {
            let ended = ran
                .outcomes
                .iter()
                .find(|(n, _)| n == t)
                .map(|(_, o)| o.clone())
                .or_else(|| given_up(t).map(Outcome::Abandoned));
            match ended {
                Some(Outcome::Abandoned(Why::NotWhole | Why::ManifestLost)) => {}
                other => {
                    return Err(format!(
                        "REPORT NotWhole({names:?}) for {t}, which ended {other:?}"
                    ))
                }
            }
        }
    }
    let mut ends: Vec<(String, Outcome)> = ran.outcomes.clone();
    if let Some(rec) = ran.files.borrow().record.as_deref().and_then(|b| Record::decode(b).ok()) {
        for a in &rec.abandoned {
            if !ends.iter().any(|(n, _)| *n == a.name) {
                ends.push((a.name.clone(), Outcome::Abandoned(a.why)));
            }
        }
    }
    for (t, o) in &ends {
        if *o == Outcome::Abandoned(Why::NotWhole) {
            let world = ran.world.borrow();
            let left: Vec<&String> = world
                .pool
                .iter()
                .flat_map(|p| p.snaps.keys())
                .filter(|k| k.ends_with(&format!("@{t}")))
                .collect();
            if !left.is_empty() {
                return Err(format!("NOTWHOLE KEPT: {t} given up NotWhole with {left:?} left"));
            }
            if !ran.reports.iter().any(|(n, rep)| n == t && matches!(rep, Report::NotWhole(_))) {
                return Err(format!("NOT REPORTED: {t} given up NotWhole with no report"));
            }
        }
    }
    let world = ran.world.borrow();
    let held = holds(&world);
    if !held.is_empty() {
        return Err(format!("still held: {held:?}"));
    }
    drop(world);
    let files = ran.files.borrow();
    let record = files.record.as_deref().map(Record::decode).transpose().map_err(|e| format!("the record: {e}"))?;
    if let Some(record) = &record {
        if !record.under_way.is_empty() {
            return Err(format!("still under way: {:?}", record.under_way));
        }
    }
    if !files.manifests.is_empty() {
        return Err(format!("manifests left: {:?}", files.manifests.keys().collect::<Vec<_>>()));
    }
    drop(files);
    // Takes left after the late landing that no give-up accounts for.
    for o in &r.orphans {
        if o.ends_with("(given up None)") {
            return Err(format!("LEAK: {o}"));
        }
    }
    // The published ones, by GUID; the reports' check is the one above.
    let stripped = Ran {
        world: ran.world.clone(),
        files: ran.files.clone(),
        settled: ran.settled.clone(),
        outcomes: ran.outcomes.clone(),
        reports: ran
            .reports
            .iter()
            .filter(|(_, rep)| !matches!(rep, Report::NotWhole(_)))
            .cloned()
            .collect(),
        points: ran.points,
        cuts_read: ran.cuts_read,
        stopped_at: ran.stopped_at,
        gone_at_done: ran.gone_at_done.clone(),
        released_pending: ran.released_pending.clone(),
    };
    match strict(&stripped) {
        // A take the agent gave up NotWhole was destroyed: `check` reads
        // the give-up as unconfirmed only where it was not.
        Err(why) if why.contains("given up as NotWhole") => Err(format!("CHECK: {why}")),
        other => other,
    }
}

#[derive(Default)]
struct D1Tally {
    runs: usize,
    outcomes: BTreeMap<String, usize>,
    takes: BTreeMap<usize, usize>,
    procs_max: usize,
    /// The least gap between two takes in one process, where the first
    /// was destroyed for a failed check of the agent's (no person's act).
    least_gap_ms: Option<u64>,
    least_gap_eg: String,
    bad: BTreeMap<String, Vec<String>>,
    panics: Vec<String>,
}

impl D1Tally {
    fn add(&mut self, k: Knobs, r: std::thread::Result<Ran2>) {
        self.runs += 1;
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                let why = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                if self.panics.len() < 50 {
                    self.panics.push(format!("{k:?}: {why}"));
                }
                return;
            }
        };
        let last = r
            .ran
            .outcomes
            .last()
            .map(|(_, o)| match o {
                Outcome::Published(_) => "Published".to_string(),
                o => format!("{o:?}"),
            })
            .or_else(|| {
                r.ran
                    .files
                    .borrow()
                    .record
                    .as_deref()
                    .and_then(|b| Record::decode(b).ok())
                    .and_then(|rec| rec.abandoned.last().map(|a| format!("Abandoned({:?}) (record)", a.why)))
            })
            .unwrap_or("none".into());
        *self.outcomes.entry(last).or_default() += 1;
        *self.takes.entry(r.takes.len()).or_default() += 1;
        self.procs_max = self.procs_max.max(r.procs);
        if k.act.is_none() && k.every_take.is_none() && k.late.is_none() && k.destroy_mode == 0 {
            for w in r.takes.windows(2) {
                if w[0].0 == w[1].0 {
                    let gap = w[1].1 - w[0].1;
                    if self.least_gap_ms.is_none_or(|g| gap < g) {
                        self.least_gap_ms = Some(gap);
                        self.least_gap_eg = format!("{k:?}: takes {:?}", r.takes);
                    }
                }
            }
        }
        if let Err(why) = judge_d1(&r) {
            let key = format!(
                "{} | destroy_mode {} act {:?} late {:?}",
                kind_of(&why),
                k.destroy_mode,
                k.act.map(|a| a.1),
                k.late.map(|l| l.min(9))
            );
            let e = self.bad.entry(key).or_default();
            if e.len() < 2 {
                e.push(format!(
                    "{k:?}: {why}\n    outcomes {:?}\n    reports {:?}\n    takes {:?}\n    log: {}",
                    r.ran.outcomes,
                    r.ran.reports,
                    r.takes,
                    r.ran.world.borrow().log.join(" | ")
                ));
            } else {
                e.push(String::new());
            }
        }
    }

    fn print(&self, what: &str, secs: f64) {
        println!(
            "{what} ({secs:.0} s): {} runs; last outcomes {:?}; takes per run {:?}; most processes {}; least gap between takes in a process (no person, no late take) {:?} ms; {} panics",
            self.runs, self.outcomes, self.takes, self.procs_max, self.least_gap_ms, self.panics.len()
        );
        if !self.least_gap_eg.is_empty() {
            println!("  least gap e.g. {}", self.least_gap_eg);
        }
        for p in self.panics.iter().take(5) {
            println!("  PANIC {p}");
        }
        let n: usize = self.bad.values().map(Vec::len).sum();
        println!("  judged wrong (judge_d1): {n} runs");
        for (kind, ex) in &self.bad {
            println!("    {} x {kind}", ex.len());
            for e in ex.iter().filter(|e| !e.is_empty()).take(1) {
                let e: String = e.chars().take(3000).collect();
                println!("      e.g. {e}");
            }
        }
    }
}

/// SD1: a take never whole (`pool/vol` skipped by every take) crossed
/// with: the agent stopped at every point (one restart); a person's
/// action at every point; the destroy's three answers; a take killed at
/// its limit and landing late. Every run must end given up (NotWhole,
/// or ManifestLost with its take kept where a destroy failed, or
/// published where a person's action made the pool whole), with nothing
/// held, under way, or left without a give-up.
#[test]
fn sd1_a_take_never_whole_by_stops_actions_and_destroys() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let t0 = std::time::Instant::now();
    let mut tally = D1Tally::default();
    for mode in [Mode::Busy, Mode::Idle] {
        for late in [None, Some(0), Some(1), Some(3), Some(usize::MAX)] {
            for destroy_mode in [0u8, 1, 2] {
                let k = Knobs {
                    late,
                    destroy_mode,
                    skip: true,
                    ..base(mode)
                };
                tally.add(k, quiet(|| run_k(k)));
                let points = points_of(k);
                for i in 0..points {
                    let k = Knobs { stop: Some(i), ..k };
                    tally.add(k, quiet(|| run_k(k)));
                }
                if destroy_mode == 0 {
                    for user in USERS_X {
                        for i in 0..points {
                            let k = Knobs {
                                act: Some((i, user)),
                                ..k
                            };
                            tally.add(k, quiet(|| run_k(k)));
                        }
                    }
                }
            }
        }
    }
    std::panic::set_hook(hook);
    tally.print("SD1", t0.elapsed().as_secs_f64());
}

/// SL: restart loops. Each process is stopped at its own point number k
/// (every k a run reaches), a new one started, up to `MAX_PROCS`: with a
/// take never whole, and with a person's property set between the
/// reading and every take (each take not as before), with and without a
/// late take. The tally gives the takes and processes each needed.
#[test]
fn sl_restart_loops() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for (label, skip, every) in [
        ("never whole", true, None),
        ("not as before at every take", false, Some(User::SetProperty)),
        ("a dataset created before every take", false, Some(User::NewDs)),
    ] {
        for late in [None, Some(0), Some(2)] {
            let t0 = std::time::Instant::now();
            let mut tally = D1Tally::default();
            let k0 = Knobs {
                late,
                skip,
                every_take: every,
                ..base(Mode::Busy)
            };
            let points = points_of(k0);
            let mut longest: Vec<(usize, usize, usize, String)> = Vec::new();
            for i in 0..points.max(1) {
                let k = Knobs { loop_stop: Some(i), ..k0 };
                let r = quiet(|| run_k(k));
                if let Ok(r) = &r {
                    let last = r.ran.outcomes.last().map(|(_, o)| format!("{o:?}")).unwrap_or("none".into());
                    longest.push((r.takes.len(), r.procs, i, format!("{last} stopped at {:?}", r.ran.stopped_at)));
                }
                tally.add(k, r);
            }
            longest.sort();
            tally.print(&format!("SL {label}, late {late:?}, {points} points"), t0.elapsed().as_secs_f64());
            for l in longest.iter().rev().take(3) {
                println!("    most takes: {} takes in {} processes, k={} ({})", l.0, l.1, l.2, l.3);
            }
        }
    }
    std::panic::set_hook(hook);
}

/// SN: the base runs of SL with no stop: how many takes each needs.
#[test]
fn sn_no_stop() {
    for (label, skip, every) in [
        ("never whole", true, None),
        ("not as before at every take", false, Some(User::SetProperty)),
        ("a dataset created before every take", false, Some(User::NewDs)),
    ] {
        let k = Knobs { skip, every_take: every, ..base(Mode::Busy) };
        let r = run_k(k);
        println!(
            "SN {label}: outcomes {:?} reports {:?} takes {:?} judge {:?}",
            r.ran.outcomes,
            r.ran.reports,
            r.takes,
            judge_d1(&r).map_err(|e| e.chars().take(300).collect::<String>())
        );
    }
}

/// SLP: one restart loop in detail (`SLP_K`, `SLP_CASE`: 0 never whole,
/// 1 a property set before every take).
#[test]
fn slp_one_restart_loop() {
    let k_stop: usize = std::env::var("SLP_K").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    let case: usize = std::env::var("SLP_CASE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let k = Knobs {
        skip: case == 0,
        every_take: if case == 1 { Some(User::SetProperty) } else { None },
        loop_stop: Some(k_stop),
        ..base(Mode::Busy)
    };
    let r = run_k(k);
    let files = r.ran.files.borrow();
    let rec = files.record.as_deref().and_then(|b| Record::decode(b).ok());
    let w = r.ran.world.borrow();
    let agent_lines: Vec<&String> = w.log.iter().filter(|l| l.starts_with("agent") || l.starts_with("user")).collect();
    println!(
        "SLP case {case} k {k_stop}: settled {:?}; procs {}; takes {}; first takes {:?}; world time {} s; outcomes {:?}; reports {}; stopped at {:?}",
        r.ran.settled,
        r.procs,
        r.takes.len(),
        &r.takes[..r.takes.len().min(6)],
        (w.now_ms - r.takes.first().map_or(w.now_ms, |t| t.1)) / 1000,
        r.ran.outcomes,
        r.ran.reports.len(),
        r.ran.stopped_at
    );
    println!("  record under way {:?}", rec.as_ref().map(|r| r.under_way.iter().map(|p| (p.name.clone(), p.attempts, format!("{:?}", p.phase).chars().take(20).collect::<String>())).collect::<Vec<_>>()));
    println!("  record abandoned {:?}", rec.as_ref().map(|r| r.abandoned.iter().map(|a| (a.name.clone(), a.why)).collect::<Vec<_>>()));
    println!("  first agent/user lines {:?}", &agent_lines[..agent_lines.len().min(8)]);
    println!("  last agent/user lines {:?}", &agent_lines[agent_lines.len().saturating_sub(4)..]);
}
