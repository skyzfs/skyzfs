//! Review scratch tests for #153 (M2.6c-2b) at 65d72ee. Copied into a
//! scratch worktree as `crates/skyzfs-agent/src/engine/review_scratch.rs`
//! by `scratch153_patch.py`, which also patches the scratch copy of
//! `engine/fake.rs` with the knobs used here; never committed to the
//! product. Run with `cargo test -p skyzfs-agent --release --lib
//! review_scratch -- --nocapture --test-threads=1`.
//!
//! The searches cross the take's answers (`zfs snapshot -r` exits 0, is
//! killed at its limit and lands after k more of the agent's calls to
//! ZFS, or fails before its ioctl; a `datasets` read that fails; a
//! destroy that goes through, is refused, or goes through with an error)
//! with a person's action at every point of the agent's, a person's
//! action just as a late take lands, and an agent stopped at every
//! point. Each run is judged by the committed oracle (`check`), and by a
//! copy of it that matches a published member to its cut by snapshot GUID
//! rather than by name (a dataset renamed after the take keeps its `@t`
//! under its new name). After the judgement any take still waiting in the
//! kernel lands (the ioctl ends whatever the agent does), and a `pool@t`
//! of the agent's that is then neither published nor destroyed by the
//! agent nor in a give-up record is tallied as an orphan.

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
}

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
    }
    let keep = MemKeep::default();
    let count = Rc::new(Cell::new(0usize));
    let stopped_at = Rc::new(Cell::new(None));
    let gone = Rc::new(RefCell::new(BTreeSet::new()));
    let released = Rc::new(RefCell::new(Vec::new()));
    let hook = {
        let (count, stopped_at, world, keep) =
            (count.clone(), stopped_at.clone(), world.clone(), keep.clone());
        let (gone, released) = (gone.clone(), released.clone());
        move |point: Point| {
            let i = count.get();
            count.set(i + 1);
            note_gone(&world, &gone, point);
            note_released(&world, &released, point);
            if let Some((at, user)) = k.act {
                if at == i {
                    let t = current_name(&keep, &world.borrow());
                    world.borrow_mut().user(user, &t);
                }
            }
            if k.stop == Some(i) {
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
    let settled = match settled {
        Err(Error::Stopped) => {
            world.borrow_mut().advance(DOWN);
            let note = {
                let (world, gone, released) = (world.clone(), gone.clone(), released.clone());
                move |point: Point| {
                    note_gone(&world, &gone, point);
                    note_released(&world, &released, point);
                    false
                }
            };
            let mut again = agent(&world, &keep).with_points(Box::new(note));
            let settled = again.settle(LIMIT);
            outcomes.extend(again.outcomes().iter().cloned());
            reports.extend(again.reports().iter().cloned());
            settled
        }
        other => other,
    };
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
    Ran2 {
        ran,
        check: checked,
        strict,
        orphans,
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
    quiet(|| run_k(Knobs { act: None, stop: None, ..k }).ran.points).unwrap_or(80)
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
