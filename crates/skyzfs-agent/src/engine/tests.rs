//! The engine on the fake world ([`fake`]), and the crash matrix (the
//! plan of M2.6, §7): every loss at every point, then recovery to
//! quiescence, then the checks of [`check`].

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use super::fake::{
    holds, Back, DaemonPoint, FakeClock, FakeDaemon, FakeZfs, MemFiles, MemKeep, Mode, Restart,
    Rule, User, World, DAEMON_POINTS, POOL, RESTARTS, USERS, VOLUME,
};
use super::*;
use crate::manifest::Manifest;
use crate::record::Why;

/// How long recovery may take, in the world's time.
const LIMIT: Duration = Duration::from_secs(3 * 3600);

/// How long a stopped agent is down before the next one starts: longer
/// than an upload's lag, so that a cut can be counted and committed
/// while no agent looks (the race rule (R) closes, the plan's §0).
const DOWN: Duration = Duration::from_secs(30);

/// The point just after the daemon answered a `publish`.
const ASKED: Point = Point {
    effect: Effect::Asked,
    after: true,
};

type FakeAgent = Agent<FakeZfs, FakeDaemon, MemKeep, FakeClock>;

fn agent(world: &Rc<RefCell<World>>, keep: &MemKeep) -> FakeAgent {
    Agent::new(
        FakeZfs(world.clone()),
        FakeDaemon(world.clone()),
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
}

/// A loss, or a person's action, in one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// None.
    None,
    /// The agent stops at its point number `k`; the world then restarts.
    Agent(usize, Restart),
    /// The daemon dies at a point of its publish, and restarts.
    Daemon(DaemonPoint, Restart),
    /// A person acts at the agent's point number `k`.
    User(usize, User),
    /// The world restarts at the agent's point number `k`, and the agent
    /// goes on: a take-over, or the pool exported and imported again by
    /// a person while the agent runs.
    World(usize, Restart),
    /// The world restarts as the agent asks the daemon for its cuts for
    /// the `k`-th time, and the agent goes on.
    Cuts(usize, Restart),
    /// That, and the agent stops just after the next request it asks:
    /// it is down while the daemon counts and uploads cuts, so a
    /// request asked with what the restart changed is not withdrawn by
    /// its next round.
    CutsThenStop(usize, Restart),
    /// The world restarts at the agent's point number `k` and the agent
    /// goes on, then it stops at its point number `stop` (after `k`); the
    /// world does not restart again.
    WorldThenStop(usize, Restart, usize),
}

/// What one run left.
struct Ran {
    world: Rc<RefCell<World>>,
    files: Rc<RefCell<MemFiles>>,
    settled: Result<(), String>,
    outcomes: Vec<(String, Outcome)>,
    reports: Vec<(String, Report)>,
    points: usize,
    cuts_read: usize,
    stopped_at: Option<Point>,
    /// The publishes whose snapshot was not whole as the agent recorded
    /// them done.
    gone_at_done: BTreeSet<String>,
}

/// The point just before the record says a publish is done.
const DONE: Point = Point {
    effect: Effect::Done,
    after: false,
};

/// At [`DONE`], adds the publish to `gone` where the agent's last read
/// of its snapshot found it gone or replaced (a run here has one
/// publish, the program's last). A loss after that read is not the
/// agent's to report: it decided on what it saw, and a rewind after its
/// look leaves the cut that holds `pool@t`.
fn note_gone(world: &Rc<RefCell<World>>, gone: &Rc<RefCell<BTreeSet<String>>>, point: Point) {
    if point != DONE {
        return;
    }
    let world = world.borrow();
    let (Some(t), Some(read)) = (world.programs.keys().last(), &world.members_read) else {
        return;
    };
    let whole = read.as_ref().is_some_and(|members| {
        world.programs.get(t).is_some_and(|manifest| {
            let mut read: Vec<u64> = members.iter().map(|m| m.snapshot_guid).collect();
            let mut taken: Vec<u64> = manifest.members.iter().map(|m| m.snapshot_guid).collect();
            read.sort_unstable();
            taken.sort_unstable();
            read == taken
        })
    });
    if !whole {
        gone.borrow_mut().insert(t.clone());
    }
}

/// One publish in `mode` with `fault`, then recovery to quiescence, on a
/// daemon with the rule (R) it has ([`fake::DAEMON_RULE`]).
fn run(mode: Mode, fault: Fault) -> Ran {
    run_under(fake::DAEMON_RULE, mode, fault)
}

/// [`run`] on a daemon with the rule `rule`.
fn run_under(rule: Rule, mode: Mode, fault: Fault) -> Ran {
    let world = World::new(mode);
    world.borrow_mut().rule = rule;
    let keep = MemKeep::default();
    let count = Rc::new(Cell::new(0usize));
    let stopped_at = Rc::new(Cell::new(None));
    let gone = Rc::new(RefCell::new(BTreeSet::new()));
    match fault {
        Fault::Daemon(point, restart) => world.borrow_mut().die = Some((point, restart)),
        Fault::Cuts(k, restart) | Fault::CutsThenStop(k, restart) => {
            world.borrow_mut().restart_at_cuts = Some((k, restart));
        }
        _ => {}
    }
    let hook = {
        let (count, stopped_at, world) = (count.clone(), stopped_at.clone(), world.clone());
        let gone = gone.clone();
        move |point: Point, _: &str| {
            let k = count.get();
            count.set(k + 1);
            note_gone(&world, &gone, point);
            match fault {
                Fault::Agent(at, _) if at == k => {
                    stopped_at.set(Some(point));
                    true
                }
                Fault::User(at, user) if at == k => {
                    let mut world = world.borrow_mut();
                    let t = world.programs.keys().last().cloned().unwrap_or_default();
                    world.user(user, &t);
                    false
                }
                Fault::World(at, restart) | Fault::WorldThenStop(at, restart, _) if at == k => {
                    world.borrow_mut().restart(restart, None);
                    false
                }
                Fault::WorldThenStop(_, _, stop) if stop == k => {
                    stopped_at.set(Some(point));
                    true
                }
                Fault::CutsThenStop(at, _)
                    if point == ASKED
                        && world.borrow().cuts_read > at
                        && stopped_at.get().is_none() =>
                {
                    stopped_at.set(Some(point));
                    true
                }
                _ => false,
            }
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
            let restart = match fault {
                Fault::Agent(_, restart) => restart,
                Fault::CutsThenStop(..) | Fault::WorldThenStop(..) => Restart::None,
                _ => unreachable!("only these faults stop the agent"),
            };
            world.borrow_mut().restart(restart, None);
            world.borrow_mut().advance(DOWN);
            let note = {
                let (world, gone) = (world.clone(), gone.clone());
                move |point: Point, _: &str| {
                    note_gone(&world, &gone, point);
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
    Ran {
        world,
        files: keep.0.clone(),
        settled: settled.map_err(|e| e.to_string()),
        outcomes,
        reports,
        points: count.get(),
        cuts_read,
        stopped_at: stopped_at.get(),
        gone_at_done: gone.take(),
    }
}

/// Whether the agent stopped where `pool@t` may exist and its manifest is
/// not recorded: between the program and the record at `Taken`.
fn in_manifest_window(point: Option<Point>) -> bool {
    matches!(
        point,
        Some(Point {
            effect: Effect::Program,
            after: true
        }) | Some(Point {
            effect: Effect::Manifest,
            ..
        }) | Some(Point {
            effect: Effect::Taken,
            after: false
        })
    )
}

/// The checks after quiescence (the plan, §7):
/// 0. every `snap/t` names a cut that holds `pool@t` with its manifest's
///    GUIDs;
/// 1. each snapshot the agent took is published, or given up for a
///    reason the world confirms; where it survived whole, it is
///    published;
/// 2. no snapshot carries the agent's hold;
/// 3. the record has nothing under way, and no manifest's file is left;
/// 4. nothing stopped, and recovery came to an end;
/// 5. a publish reported done with its snapshot gone is published, and
///    its snapshot did not survive whole; no publish is reported for
///    making no progress (the world always cuts again).
fn check(ran: &Ran) -> Result<(), String> {
    let world = ran.world.borrow();
    ran.settled.clone()?;
    if let Some((name, outcome)) = ran
        .outcomes
        .iter()
        .find(|(_, o)| matches!(o, Outcome::Stopped(_)))
    {
        return Err(format!("{name} stopped: {outcome:?}"));
    }
    for (t, r) in &world.refs {
        let manifest = Manifest::decode(&r.manifest).map_err(|e| format!("snap/{t}: {e}"))?;
        let commit = world
            .commits
            .get(&r.commit)
            .ok_or(format!("snap/{t} names no commit"))?;
        if commit.cut.number != r.number || commit.cut.group != r.group {
            return Err(format!("snap/{t}'s row is not its commit's cut"));
        }
        for m in &manifest.members {
            let at = format!("{}@{t}", m.name);
            if commit.cut.snaps.get(&at) != Some(&m.snapshot_guid) {
                return Err(format!(
                    "snap/{t} names cut {} (group {}), which does not hold {at} as its manifest has it",
                    commit.cut.number, commit.cut.group
                ));
            }
        }
        if world.programs.get(t) != Some(&manifest) {
            return Err(format!("snap/{t}'s manifest is not the program's"));
        }
    }
    let files = ran.files.borrow();
    let record = files
        .record
        .as_deref()
        .map(Record::decode)
        .transpose()
        .map_err(|e| format!("the record: {e}"))?;
    let pool = world.pool.as_ref().ok_or("the pool is not imported")?;
    let premise = pool.layout.data.len() == 1 && pool.layout.data[0].children.is_empty();
    for (t, manifest) in &world.programs {
        if world.refs.contains_key(t) {
            continue;
        }
        let why = record
            .as_ref()
            .and_then(|r| r.abandoned.iter().find(|a| a.name == *t))
            .map(|a| a.why)
            .ok_or(format!("{t} neither published nor given up"))?;
        let survived = manifest.members.iter().all(|m| {
            pool.snaps
                .get(&format!("{}@{t}", m.name))
                .is_some_and(|s| s.guid == m.snapshot_guid)
        });
        let confirmed = match why {
            Why::ManifestLost => in_manifest_window(ran.stopped_at),
            Why::Destroyed | Why::Rewound | Why::Replaced => !survived,
            Why::Premise => !premise,
            // The world always cuts again in time: a publish that makes
            // no progress here is a defect of the engine.
            // Nor does its uploader stop for good in the matrices (it does
            // in `a_stopped_uploader_gives_each_publish_up_at_once`).
            Why::ManifestTooLarge
            | Why::NameTaken
            | Why::NoProgress
            | Why::TooOld
            | Why::Stopped
            // No person acts in the matrices.
            | Why::Person => false,
        };
        if !confirmed {
            return Err(format!(
                "{t} given up as {why:?}, which the world does not confirm (survived {survived}, premise {premise}, stopped at {:?})",
                ran.stopped_at
            ));
        }
    }
    for (t, report) in &ran.reports {
        match report {
            Report::SnapshotGone => {
                let survived = world.programs.get(t).is_some_and(|manifest| {
                    manifest.members.iter().all(|m| {
                        pool.snaps
                            .get(&format!("{}@{t}", m.name))
                            .is_some_and(|s| s.guid == m.snapshot_guid)
                    })
                });
                if !world.refs.contains_key(t) || survived {
                    return Err(format!(
                        "{t} reported done with its snapshot gone (published {}, survived {survived})",
                        world.refs.contains_key(t)
                    ));
                }
            }
            Report::NoProgress(_) | Report::Refused(_) | Report::HeldAway => {
                return Err(format!("{t} reported: {report:?}"))
            }
        }
    }
    // The other way: a publish recorded done with its snapshot gone is
    // reported (F1 and T1 of #144's review).
    for t in &ran.gone_at_done {
        let reported = ran
            .reports
            .iter()
            .any(|(name, r)| name == t && *r == Report::SnapshotGone);
        if !reported {
            return Err(format!(
                "{t} recorded done with its snapshot gone, and not reported"
            ));
        }
    }
    let held = holds(&world);
    if !held.is_empty() {
        return Err(format!("still held: {held:?}"));
    }
    if let Some(record) = &record {
        if !record.under_way.is_empty() {
            return Err(format!("still under way: {:?}", record.under_way));
        }
    }
    if !files.manifests.is_empty() {
        return Err(format!(
            "manifests left: {:?}",
            files.manifests.keys().collect::<Vec<_>>()
        ));
    }
    Ok(())
}

/// A failure's message, with the world's log.
fn failed(what: &str, ran: &Ran, why: &str) -> String {
    let world = ran.world.borrow();
    format!(
        "{what}: {why}\noutcomes: {:?}\n{}",
        ran.outcomes,
        world.log.join("\n")
    )
}

const MODES: [Mode; 4] = [Mode::Busy, Mode::Idle, Mode::Abandoned, Mode::ErrLow];

/// A publish with no loss, in each mode, ends published on a cut that
/// holds it, with nothing left held or under way, and in the time the
/// mode allows.
///
/// An abandoned cut on an idle pool costs the publish twice
/// [`plan::ASSIST_AFTER`]: the abandoned cut is still the daemon's newest
/// and is counted (`cuts` does not tell an abandoned cut), so the planner
/// sees a qualifying cut and helps only from twice `ASSIST_AFTER` after
/// the request was accepted (the bounded assist of #142), not at its
/// expiry. On a busy pool the next group comes on its own.
#[test]
fn a_publish_with_no_loss_is_published_in_each_mode() {
    let after = crate::plan::ASSIST_AFTER.as_secs();
    let times = [
        (Mode::Busy, 0, 30),
        (Mode::Idle, 0, 10),
        (Mode::Abandoned, 2 * after, 2 * after + 5),
        (Mode::ErrLow, 0, 30),
    ];
    assert_eq!(times.map(|(mode, ..)| mode), MODES);
    for (mode, least, most) in times {
        let ran = run(mode, Fault::None);
        if let Err(why) = check(&ran) {
            panic!("{}", failed(&format!("{mode:?}"), &ran, &why));
        }
        let world = ran.world.borrow();
        assert_eq!(world.refs.len(), 1, "{mode:?}: {:?}", ran.outcomes);
        assert!(
            matches!(ran.outcomes[..], [(_, Outcome::Published(_))]),
            "{mode:?}: {:?}",
            ran.outcomes
        );
        let took = (world.now_ms - fake::EPOCH_MS) / 1000;
        assert!(
            (least..=most).contains(&took),
            "{mode:?}: {took} s, not {least} to {most} s"
        );
    }
}

/// The agent's run with the daemon rebooted just after each of its first
/// `reboots` requests was accepted, on an idle pool: the held cuts are
/// lost, and a request asked anew (with a count read after the hold) is
/// carried only by a group the agent's own sync task writes.
fn rebooted_on_an_idle_pool(reboots: usize, plain_sync: bool) -> (Rc<RefCell<World>>, FakeAgent) {
    let world = World::new(Mode::Idle);
    world.borrow_mut().plain_sync = plain_sync;
    let keep = MemKeep::default();
    let rebooted = Rc::new(Cell::new(0));
    let hook = {
        let (world, rebooted) = (world.clone(), rebooted.clone());
        move |point: Point, _: &str| {
            if point == ASKED && rebooted.get() < reboots {
                rebooted.set(rebooted.get() + 1);
                world.borrow_mut().restart(Restart::Reboot, None);
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    agent.begin().expect("begun");
    if let Err(e) = agent.settle(LIMIT) {
        let world = world.borrow();
        let tail: Vec<_> = world
            .log
            .iter()
            .filter(|l| !l.contains("sync alone"))
            .collect();
        panic!(
            "{e}: {:?} {:?} {:?}\n{tail:#?}",
            agent.outcomes(),
            agent.reports(),
            agent.record().under_way
        );
    }
    (world, agent)
}

/// Mutant: the sync task without its property (a plain `zpool sync`).
/// After the reboot the publish is carried by the group of
/// `skyzfs:assist`.
#[test]
fn on_an_idle_pool_the_sync_task_writes_the_group_that_carries_a_request() {
    let (world, agent) = rebooted_on_an_idle_pool(1, false);
    assert!(
        matches!(agent.outcomes(), [(_, Outcome::Published(_))]),
        "{:?}",
        agent.outcomes()
    );
    assert!(agent.reports().is_empty(), "{:?}", agent.reports());
    let world = world.borrow();
    assert!(!world.assists.is_empty(), "no sync task ran");
}

/// Mutant: the sync task's value set again, fixed or not above the last
/// one (TG5 of #144's next look). Two publishes in a row on an idle pool,
/// the daemon rebooted as each is first accepted, each need a sync task
/// (a request asked anew is carried by no other group), and the agent's
/// clock steps back between them: each value is new, and no set repeats
/// the last. The fake writes no group for a value set again (ZFS 2.4.1
/// writes two, as for a new value), and logs it: the check reads the log,
/// since a repeat that writes nothing leaves no value in
/// [`World::assists`].
#[test]
fn each_sync_task_sets_a_value_of_its_own() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let armed = Rc::new(Cell::new(true));
    let hook = {
        let (world, armed) = (world.clone(), armed.clone());
        move |point: Point, _: &str| {
            if point == ASKED && armed.replace(false) {
                world.borrow_mut().restart(Restart::Reboot, None);
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    world.borrow_mut().agent_skew_ms = -3_600_000;
    world.borrow_mut().advance(Duration::from_secs(1));
    armed.set(true);
    agent.begin().expect("begun again");
    agent.settle(LIMIT).expect("settled again");
    assert!(
        matches!(
            agent.outcomes(),
            [(_, Outcome::Published(_)), (_, Outcome::Published(_))]
        ),
        "{:?}",
        agent.outcomes()
    );
    let world = world.borrow();
    let mut values = world.assists.clone();
    values.sort_unstable();
    values.dedup();
    assert!(
        world.assists.len() >= 2 && values.len() == world.assists.len(),
        "{:?}",
        world.assists
    );
    let again = world.log.iter().filter(|l| l.contains(" again)")).count();
    assert_eq!(again, 0, "{again} sets of a value the property held");
}

/// A take-over counts as a stall (F2 of #144's review, accepted by the
/// lead; TG4 of the next look): a daemon taken over as each request is
/// accepted loses every one before a cut carries it, and the publish is
/// reported at its third stall and given up at its eighth.
///
/// Mutant: a take-over's count read as a new daemon's (the stalls reset,
/// and the publish is asked for ever).
#[test]
fn a_publish_whose_daemon_is_taken_over_at_each_request_is_given_up() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let hook = {
        let world = world.clone();
        move |point: Point, _: &str| {
            if point == ASKED {
                world.borrow_mut().restart(Restart::TakeOver, None);
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    let t = match agent.outcomes() {
        [(t, Outcome::Abandoned(Why::NoProgress))] => t.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        agent.reports(),
        [(t, Report::NoProgress(plan::STALLS_REPORTED))]
    );
    let world = world.borrow();
    assert!(holds(&world).is_empty(), "{:?}", holds(&world));
    assert!(world.refs.is_empty());
}

/// A report is made once in a process for each publish and kind (TG2 of
/// #144's next look). A publish stalls until it is reported; then a sync
/// task carries it, it is published, and a rewind takes `pool@t` before
/// the agent reads `done`, so that it is recorded done with a report of
/// its snapshot gone; the save of that record fails once, in the same
/// process, and `Done` is planned again.
///
/// Mutant: no de-duplication (the snapshot reported twice), or one by
/// name alone (the snapshot's report dropped after the stalls').
#[test]
fn a_report_is_made_once_in_a_process_for_each_kind() {
    let world = World::new(Mode::Idle);
    world.borrow_mut().plain_sync = true;
    let keep = MemKeep::default();
    let (rebooted, rewound, failed) = (
        Rc::new(Cell::new(false)),
        Rc::new(Cell::new(false)),
        Rc::new(Cell::new(false)),
    );
    let hook = {
        let (world, files) = (world.clone(), keep.0.clone());
        let (rebooted, rewound, failed) = (rebooted.clone(), rewound.clone(), failed.clone());
        move |point: Point, _: &str| {
            if point == ASKED && !rebooted.replace(true) {
                world.borrow_mut().restart(Restart::Reboot, None);
            }
            if point == ASKED {
                let stalls = files
                    .borrow()
                    .record
                    .as_deref()
                    .and_then(|bytes| Record::decode(bytes).ok())
                    .and_then(|r| r.under_way.first().map(|p| p.stalls));
                if stalls >= Some(plan::STALLS_REPORTED) {
                    world.borrow_mut().plain_sync = false;
                }
            }
            if !world.borrow().refs.is_empty() && !rewound.replace(true) {
                world
                    .borrow_mut()
                    .restart(Restart::RewindF(Back::BelowCreated), None);
            }
            point == DONE && !failed.replace(true)
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    let mut stops = 0;
    while let Err(e) = agent.settle(LIMIT) {
        assert!(matches!(e, Error::Stopped), "{e}");
        stops += 1;
        assert!(stops < 3, "stopped {stops} times");
    }
    assert_eq!(stops, 1, "the save of done did not fail");
    assert!(rewound.get(), "not published");
    assert!(
        matches!(agent.outcomes(), [(n, Outcome::Published(_))] if *n == t),
        "{:?}",
        agent.outcomes()
    );
    assert_eq!(
        agent.reports(),
        [
            (t.clone(), Report::NoProgress(plan::STALLS_REPORTED)),
            (t, Report::SnapshotGone)
        ]
    );
}

/// A plain `zpool sync` writes no group on an idle pool (N1 of #142's
/// review), so the request asked anew after the reboot is never carried:
/// the publish is reported at its third stall, and given up at its
/// eighth, its hold released (the lead's decision for M2.6b2). This is
/// what the sync task's property is for.
///
/// Mutant: the bound not applied, applied a stall early or late, the
/// report made more than once, or the hold kept; the syncs of a request
/// not spaced to [`plan::ASSIST_AFTER`] after [`plan::ASSIST_QUICK`] of
/// them, or their count not started again at each request.
#[test]
fn a_publish_no_cut_carries_is_reported_then_given_up() {
    let (world, agent) = rebooted_on_an_idle_pool(1, true);
    let t = match agent.outcomes() {
        [(t, Outcome::Abandoned(Why::NoProgress))] => t.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        agent.reports(),
        [(t.clone(), Report::NoProgress(plan::STALLS_REPORTED))]
    );
    assert_eq!(
        agent.record().abandoned.last().map(|a| a.why),
        Some(Why::NoProgress)
    );
    assert!(agent.record().under_way.is_empty());
    let world = world.borrow();
    assert!(holds(&world).is_empty(), "{:?}", holds(&world));
    assert!(world.refs.is_empty());
    assert!(world.log.iter().any(|l| l.contains("zpool sync alone")));
    // After the reboot, each of the eight requests that stalls runs to
    // its expiry (the first's count, of the new daemon, resets the
    // stalls; the eighth end gives the publish up).
    let took = (world.now_ms - fake::EPOCH_MS) / 1000;
    let least = u64::from(plan::STALLS_ABANDONED) * EXPIRY_DEFAULT.as_secs();
    assert!(took >= least, "{took} s, less than {least} s");
    // The pace of the syncs (the lead's decision on #144): in each
    // request, ASSIST_QUICK a pause apart, then one each ASSIST_AFTER.
    let gaps: Vec<u64> = world.synced_at.windows(2).map(|w| w[1] - w[0]).collect();
    let after = u64::try_from(plan::ASSIST_AFTER.as_millis()).expect("ms");
    let short = gaps.iter().filter(|&&gap| gap < after).count();
    let stalled = plan::STALLS_ABANDONED as usize;
    let quick = plan::ASSIST_QUICK as usize;
    assert_eq!(short, (quick - 1) * stalled, "{gaps:?}");
    let most = stalled * (quick + (EXPIRY_DEFAULT.as_millis() / u128::from(after)) as usize);
    let syncs = world.synced_at.len();
    assert!(syncs <= most, "{syncs} sync tasks, more than {most}");
}

/// What [`stalling_run_stopped_at`] left.
struct Stalled {
    /// The record at the end.
    record: Record,
    /// The reports of the agent stopped, and of the one in its place.
    reports: [Vec<(String, Report)>; 2],
    /// The stalls the record held as each request was asked.
    asked: Vec<u32>,
    /// The points reached, in order.
    points: Vec<Point>,
}

/// The stalling run of [`a_publish_no_cut_carries_is_reported_then_given_up`]
/// with the agent stopped at its point number `stop` if that comes after
/// its first request (before it, a stop is a publish of the crash
/// matrices, which may lose its manifest), and a new agent started in
/// its place.
fn stalling_run_stopped_at(stop: usize) -> Stalled {
    let world = World::new(Mode::Idle);
    world.borrow_mut().plain_sync = true;
    let keep = MemKeep::default();
    let points = Rc::new(RefCell::new(Vec::new()));
    let rebooted = Rc::new(Cell::new(false));
    let asked = Rc::new(RefCell::new(Vec::new()));
    let hook = |stops: bool| {
        let (world, keep) = (world.clone(), keep.clone());
        let (points, rebooted, asked) = (points.clone(), rebooted.clone(), asked.clone());
        move |point: Point, _: &str| {
            if point == ASKED {
                if let Ok(Loaded::Present(record)) = keep.load() {
                    let stalls = record.under_way.iter().map(|p| p.stalls).max();
                    asked.borrow_mut().extend(stalls);
                }
                if !rebooted.get() {
                    rebooted.set(true);
                    world.borrow_mut().restart(Restart::Reboot, None);
                }
            }
            let k = points.borrow().len();
            points.borrow_mut().push(point);
            stops && rebooted.get() && k == stop
        }
    };
    let mut first = agent(&world, &keep).with_points(Box::new(hook(true)));
    let settled = first.begin().and_then(|_| first.settle(LIMIT));
    let mut reports = [first.reports().to_vec(), Vec::new()];
    drop(first);
    let record = match settled {
        Err(Error::Stopped) => {
            world.borrow_mut().advance(DOWN);
            let mut again = agent(&world, &keep).with_points(Box::new(hook(false)));
            again.settle(LIMIT).expect("settled after the stop");
            reports[1] = again.reports().to_vec();
            again.record().clone()
        }
        settled => {
            settled.expect("settled");
            match keep.load() {
                Ok(Loaded::Present(record)) => record,
                other => panic!("{other:?}"),
            }
        }
    };
    let asked = asked.borrow().clone();
    let points = points.borrow().clone();
    Stalled {
        record,
        reports,
        asked,
        points,
    }
}

/// The agent stopped at any point of a stalling run, and started again
/// (at each but the sync tasks after the first of a run of them, which
/// make most of the points):
/// each stall is counted once, the publish reported by each agent at most
/// once and by one of them at least (a report is not recorded), and
/// given up at its eighth stall.
///
/// One stop costs a stall: one from the record of the ask (`asking`,
/// before it is sent) to the record of its answer (`asked`). The record
/// then says asked, and the ask anew at the same place counts a stall,
/// as for a request accepted and lost: the cost of recording the ask
/// before it is sent (the lead's decision on L1 of #149's review;
/// `Agent::ask`). Such a stop counts one stall more, once; any other
/// counts none.
///
/// Mutant: the stalls committed with the publish left at `Asked` (a
/// request asked anew after a stop between the count and the ask is
/// counted twice), or the report made at each count (made more than
/// once by one agent).
#[test]
fn a_stop_at_any_point_of_a_stalling_run_counts_each_stall_once() {
    let whole = stalling_run_stopped_at(usize::MAX).points;
    // From after the record of the ask to before the record of its answer.
    let between_ask_and_answer = |k: usize| {
        let at =
            |j: usize, effect: Effect, after: bool| whole.get(j) == Some(&Point { effect, after });
        let mut j = k;
        loop {
            if at(j, Effect::Asking, true) {
                return true;
            }
            let inside = at(j, Effect::Ask, false)
                || at(j, Effect::Ask, true)
                || at(j, Effect::Asked, false);
            if !inside || j == 0 {
                return false;
            }
            j -= 1;
        }
    };
    let assist = |k: usize| whole.get(k).is_some_and(|p| p.effect == Effect::Assist);
    let stops = (0..whole.len()).filter(|&k| !assist(k) || k < 2 || !assist(k - 2));
    for k in stops {
        let run = stalling_run_stopped_at(k);
        let at = format!("stopped at point {k} of {}", whole.len());
        assert!(run.record.under_way.is_empty(), "{at}: {:?}", run.record);
        assert_eq!(
            run.record.abandoned.last().map(|a| a.why),
            Some(Why::NoProgress),
            "{at}"
        );
        let each: Vec<_> = run.reports.iter().map(Vec::len).collect();
        assert!(
            each.iter().all(|&n| n <= 1) && each.contains(&1),
            "{at}: {:?}",
            run.reports
        );
        let ok = |r: &(String, Report)| {
            matches!(r.1, Report::NoProgress(n)
                if (plan::STALLS_REPORTED..plan::STALLS_ABANDONED).contains(&n))
        };
        assert!(
            run.reports.iter().flatten().all(ok),
            "{at}: {:?}",
            run.reports
        );
        let more = run
            .asked
            .windows(2)
            .map(|w| w[1].saturating_sub(w[0] + 1))
            .sum::<u32>();
        let allowed = u32::from(between_ask_and_answer(k));
        assert!(
            more <= allowed,
            "{at}: a stall counted twice: {:?}",
            run.asked
        );
    }
}

/// A stalling publish whose stalls start again (the daemon rebooted once
/// they reach three: another attachment) is reported again when they
/// reach three anew.
///
/// Mutant: a publish reported once a process whatever its stalls did.
#[test]
fn a_publish_that_stalls_again_after_a_reset_is_reported_again() {
    let world = World::new(Mode::Idle);
    world.borrow_mut().plain_sync = true;
    let keep = MemKeep::default();
    let rebooted = Rc::new(Cell::new(0));
    let hook = {
        let (world, keep, rebooted) = (world.clone(), keep.clone(), rebooted.clone());
        move |point: Point, _: &str| {
            if point != ASKED {
                return false;
            }
            let stalls = match keep.load() {
                Ok(Loaded::Present(record)) => record.under_way.iter().map(|p| p.stalls).max(),
                _ => None,
            };
            let due = match rebooted.get() {
                0 => true,
                1 => stalls == Some(plan::STALLS_REPORTED),
                _ => false,
            };
            if due {
                rebooted.set(rebooted.get() + 1);
                world.borrow_mut().restart(Restart::Reboot, None);
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    assert_eq!(rebooted.get(), 2);
    let third = Report::NoProgress(plan::STALLS_REPORTED);
    assert_eq!(agent.reports(), [(t.clone(), third), (t, third)]);
    assert!(
        matches!(agent.outcomes(), [(_, Outcome::Abandoned(Why::NoProgress))]),
        "{:?}",
        agent.outcomes()
    );
}

/// Mutant: the planner's time for a publish accepted in this process
/// taken from the wall clock (S4 of #142's review). The agent's wall
/// clock steps forward two hours while the daemon is away: the hold is
/// kept until `held_until` on the clock that does not step.
#[test]
fn a_step_forward_of_the_wall_clock_does_not_release_a_hold_early() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let stepped = Rc::new(Cell::new(false));
    let hook = {
        let (world, stepped) = (world.clone(), stepped.clone());
        move |point: Point, _: &str| {
            if point == ASKED && !stepped.get() {
                stepped.set(true);
                let mut world = world.borrow_mut();
                world.agent_skew_ms = 2 * 3_600_000;
                world.away_until = world.now_ms + 120_000;
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    assert!(agent.settle(Duration::from_secs(60)).is_err());
    assert!(stepped.get());
    let held = holds(&world.borrow());
    assert!(
        !held.is_empty() && held.iter().all(|h| h.ends_with(&format!("@{t}"))),
        "the hold was released: {held:?}"
    );
    agent.settle(LIMIT).expect("settled");
    assert!(
        matches!(agent.outcomes(), [(_, Outcome::Published(_))]),
        "{:?}",
        agent.outcomes()
    );
}

/// Mutant: the same, or a sync recorded later than now not counted as
/// none (S11 of #142's review). The agent's wall clock steps back an
/// hour just after the request is accepted, behind an abandoned cut on
/// an idle pool: the syncs go on, and the publish takes the time it
/// takes with no step.
#[test]
fn a_step_back_of_the_wall_clock_does_not_hold_back_the_syncs() {
    let world = World::new(Mode::Abandoned);
    let keep = MemKeep::default();
    let hook = {
        let world = world.clone();
        move |point: Point, _: &str| {
            if point == ASKED {
                world.borrow_mut().agent_skew_ms = -3_600_000;
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    assert!(
        matches!(agent.outcomes(), [(_, Outcome::Published(_))]),
        "{:?}",
        agent.outcomes()
    );
    let after = crate::plan::ASSIST_AFTER.as_secs();
    let took = (world.borrow().now_ms - fake::EPOCH_MS) / 1000;
    assert!(
        (2 * after..=2 * after + 5).contains(&took),
        "{took} s, not {} to {} s",
        2 * after,
        2 * after + 5
    );
}

/// What a run whose publish is older than `age_max` left, with `hook`
/// at the agent's points: the world, the agent, and the publish's name.
fn aged_run(
    world: &Rc<RefCell<World>>,
    age_max: Duration,
    hook: impl FnMut(Point, &str) -> bool + 'static,
) -> (FakeAgent, String) {
    let keep = MemKeep::default();
    let mut agent = agent(world, &keep)
        .with_age_max(age_max)
        .with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    (agent, t)
}

/// Checks that `t` was given up as too old, with no hold left and
/// nothing published, no sooner than `least` after the world began.
fn given_up_too_old(world: &Rc<RefCell<World>>, agent: &FakeAgent, t: &str, least: Duration) {
    assert_eq!(
        agent.outcomes(),
        [(t.to_string(), Outcome::Abandoned(Why::TooOld))]
    );
    assert_eq!(
        agent.record().abandoned.last().map(|a| a.why),
        Some(Why::TooOld)
    );
    assert!(agent.record().under_way.is_empty());
    let world = world.borrow();
    assert!(holds(&world).is_empty(), "{:?}", holds(&world));
    assert!(world.refs.is_empty());
    let took = Duration::from_millis(world.now_ms - fake::EPOCH_MS);
    assert!(took >= least, "{took:?}, less than {least:?}");
}

/// N4 of #144's third look, the lead's L7: a publish too old whose
/// request the uploader itself ended (`refused rewound`, after `zpool
/// import -F`) is looked up and given up, its hold released and the code
/// reported, as one not too old ends in a round; `pool@t` lost by the
/// rewind is given up as `Rewound`.
///
/// Mutant: the ended arm waiting on the uploader's own ends (the hold
/// kept for good, and nobody told), or giving up with no report.
#[test]
fn a_publish_too_old_whose_request_the_uploader_ended_is_given_up_and_reported() {
    for (back, why) in [
        (Back::AfterHold, Why::TooOld),
        (Back::BelowCreated, Why::Rewound),
    ] {
        // A daemon that counts cuts and uploads none, and a rewind just
        // before the ended arm's first withdraw: the daemon's mark
        // passes the request's count, and it ends `refused rewound`.
        let world = World::new(Mode::Busy);
        world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
        let hook = {
            let world = world.clone();
            let rewound = Cell::new(false);
            move |point: Point, _: &str| {
                let withdraw = Point {
                    effect: Effect::Withdraw,
                    after: false,
                };
                if point == withdraw && !rewound.replace(true) {
                    world.borrow_mut().restart(Restart::RewindF(back), None);
                }
                false
            }
        };
        let (agent, t) = aged_run(&world, Duration::from_secs(60), hook);
        assert_eq!(
            agent.outcomes(),
            [(t.clone(), Outcome::Abandoned(why))],
            "{back:?}"
        );
        assert_eq!(
            agent.reports(),
            [(t, Report::Refused(skyzfs_control::Code::Rewound))],
            "{back:?}"
        );
        assert!(agent.record().under_way.is_empty(), "{back:?}");
        let world = world.borrow();
        assert!(holds(&world).is_empty(), "{back:?}: {:?}", holds(&world));
        assert!(world.refs.is_empty(), "{back:?}");
    }
}

/// The world and hook of a publish too old whose request the uploader
/// ends `refused rewound`: a daemon that counts cuts and uploads none,
/// and `zpool import -F` (`back`) just before the ended arm's first
/// withdraw. The hook returns whether the agent stops at a point: here
/// at the `stop`-th point after the rewind, if any.
fn rewound_when_too_old(
    back: Back,
    stop: Option<usize>,
) -> (
    Rc<RefCell<World>>,
    impl FnMut(Point, &str) -> bool + 'static,
) {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let hook = {
        let world = world.clone();
        let rewound = Cell::new(false);
        let after = Cell::new(0usize);
        move |point: Point, _: &str| {
            let withdraw = Point {
                effect: Effect::Withdraw,
                after: false,
            };
            if point == withdraw && !rewound.replace(true) {
                world.borrow_mut().restart(Restart::RewindF(back), None);
            }
            if !rewound.get() {
                return false;
            }
            after.set(after.get() + 1);
            stop == Some(after.get())
        }
    };
    (world, hook)
}

/// TG6 of #144's fourth look: the report of a publish too old given up
/// on a refusal that ends its request is made however the agent stops
/// after the rewind: at each point from the ended arm's first withdraw
/// on, the agent stops and another starts on its record; between the
/// two processes the code is reported at least once, and the publish is
/// given up with no hold left.
///
/// Mutant: the report made after the action that gives the publish up
/// (a stop just after the save of `Abandon` loses it in both).
#[test]
fn a_refused_report_is_made_however_the_agent_stops_after_the_end() {
    let age_max = Duration::from_secs(60);
    for (back, why) in [
        (Back::AfterHold, Why::TooOld),
        (Back::BelowCreated, Why::Rewound),
    ] {
        // Each stop in turn, until a run ends before its stop.
        let mut runs = 0;
        for stop in 1..1000 {
            let (world, hook) = rewound_when_too_old(back, Some(stop));
            let keep = MemKeep::default();
            let mut first = agent(&world, &keep)
                .with_age_max(age_max)
                .with_points(Box::new(hook));
            let t = first.begin().expect("begun");
            match first.settle(LIMIT) {
                Err(Error::Stopped) => {}
                // The run ended before its `stop`-th point after the
                // rewind: no later stop can come.
                Ok(()) => break,
                Err(e) => panic!("{back:?} at {stop}: {e}"),
            }
            runs += 1;
            let mut second = agent(&world, &keep).with_age_max(age_max);
            second.settle(LIMIT).expect("settled");
            let mut reports = first.reports().to_vec();
            reports.extend(second.reports().iter().cloned());
            assert!(
                reports.contains(&(t.clone(), Report::Refused(skyzfs_control::Code::Rewound))),
                "{back:?}, stopped at {stop}: {reports:?}"
            );
            // Given up once, by one process or the other (a stop after
            // the save leaves the outcome to the record alone).
            let mut outcomes = first.outcomes().to_vec();
            outcomes.extend(second.outcomes().iter().cloned());
            assert!(
                outcomes.len() <= 1
                    && outcomes
                        .iter()
                        .all(|o| *o == (t.clone(), Outcome::Abandoned(why))),
                "{back:?}, stopped at {stop}: {outcomes:?}"
            );
            let record = second.record();
            assert!(record.under_way.is_empty(), "{back:?} at {stop}");
            assert_eq!(
                record.abandoned.last().map(|a| (a.name.as_str(), a.why)),
                Some((t.as_str(), why)),
                "{back:?}, stopped at {stop}"
            );
            let world = world.borrow();
            assert!(
                holds(&world).is_empty(),
                "{back:?} at {stop}: {:?}",
                holds(&world)
            );
        }
        assert!((4..999).contains(&runs), "{back:?}: {runs} runs");
    }
}

/// TG7 of #144's fourth look: a publish reported for its stalls and then
/// given up on a refusal that ends its request is reported for both, in
/// one process. The no-cut run of
/// `a_publish_no_cut_carries_is_reported_then_given_up`; as the daemon
/// accepts the request that follows the report, the pool is rewound and
/// the daemon is away until the publish is older than its bound. The
/// daemon ends the request `refused rewound` as it counts the import's
/// group, and the agent reads it in the ended arm.
///
/// Mutant: the reports de-duplicated by name alone (the code's report
/// dropped after the stalls').
#[test]
fn a_publish_reported_for_its_stalls_then_ended_is_reported_for_both() {
    let age_max = Duration::from_secs(2 * 3600);
    let world = World::new(Mode::Idle);
    world.borrow_mut().plain_sync = true;
    let keep = MemKeep::default();
    let hook = {
        let (world, files) = (world.clone(), keep.0.clone());
        let (rebooted, rewound) = (Cell::new(false), Cell::new(false));
        move |point: Point, _: &str| {
            if point == ASKED && !rebooted.replace(true) {
                world.borrow_mut().restart(Restart::Reboot, None);
            }
            if point == ASKED && !rewound.get() {
                let stalls = files
                    .borrow()
                    .record
                    .as_deref()
                    .and_then(|bytes| Record::decode(bytes).ok())
                    .and_then(|r| r.under_way.first().map(|p| p.stalls));
                if stalls >= Some(plan::STALLS_REPORTED) {
                    rewound.set(true);
                    let mut world = world.borrow_mut();
                    world.restart(Restart::RewindF(Back::AfterHold), None);
                    let bound = u64::try_from(age_max.as_millis()).unwrap_or(u64::MAX);
                    world.away_until = fake::EPOCH_MS + bound + 60_000;
                }
            }
            false
        }
    };
    let mut agent = agent(&world, &keep)
        .with_age_max(age_max)
        .with_points(Box::new(hook));
    let t = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    assert_eq!(
        agent.outcomes(),
        [(t.clone(), Outcome::Abandoned(Why::TooOld))]
    );
    assert_eq!(
        agent.reports(),
        [
            (t.clone(), Report::NoProgress(plan::STALLS_REPORTED)),
            (t, Report::Refused(skyzfs_control::Code::Rewound)),
        ]
    );
    let world = world.borrow();
    assert!(holds(&world).is_empty(), "{:?}", holds(&world));
    assert!(world.refs.is_empty());
}

/// L2 of #144's review: a publish older than its bound is given up, and
/// its hold released, once its request is withdrawn and not published.
///
/// Mutant: the bound not applied, or applied from the wrong time; the
/// hold kept; a publish given up while the daemon is away (it may have
/// landed meanwhile); or the answer to a withdraw never remembered (the
/// publish withdrawn for ever).
#[test]
fn a_publish_older_than_its_bound_is_given_up_and_its_hold_released() {
    // The stalling run of `a_publish_no_cut_carries_is_reported_then_given_up`
    // (an idle pool rebooted after the first request, and syncs that
    // write no group): given up at half an hour, before its eighth stall.
    let world = World::new(Mode::Idle);
    world.borrow_mut().plain_sync = true;
    let hook = {
        let world = world.clone();
        let rebooted = Cell::new(false);
        move |point: Point, _: &str| {
            if point == ASKED && !rebooted.replace(true) {
                world.borrow_mut().restart(Restart::Reboot, None);
            }
            false
        }
    };
    let half = Duration::from_secs(1800);
    let (agent, t) = aged_run(&world, half, hook);
    given_up_too_old(&world, &agent, &t, half);
    let took = Duration::from_millis(world.borrow().now_ms - fake::EPOCH_MS);
    assert!(took < half + EXPIRY_DEFAULT, "{took:?}");

    // L2 of #144's review: a daemon that counts cuts and uploads none
    // (an uplink too slow, an uploader stuck). Each request expires
    // with a cut counted, so none is a stall; at the default bound the
    // publish is given up two hours after it was begun.
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let mut stuck = self::agent(&world, &keep);
    let t = stuck.begin().expect("begun");
    stuck.settle(LIMIT).expect("settled");
    given_up_too_old(&world, &stuck, &t, plan::AGE_MAX);
    assert!(stuck.reports().is_empty(), "{:?}", stuck.reports());
    let took = Duration::from_millis(world.borrow().now_ms - fake::EPOCH_MS);
    assert!(took < plan::AGE_MAX + EXPIRY_DEFAULT, "{took:?}");

    // The daemon away for twenty minutes from just after it accepted
    // the request, which lands meanwhile, and a bound of a minute: the
    // publish is not given up while the daemon is away, and is found
    // published when it answers again.
    let world = World::new(Mode::Busy);
    let away = Duration::from_secs(1200);
    let hook = {
        let world = world.clone();
        move |point: Point, _: &str| {
            if point == ASKED {
                let mut world = world.borrow_mut();
                world.away_until = world.now_ms + 1_200_000;
            }
            false
        }
    };
    let (agent, _) = aged_run(&world, Duration::from_secs(60), hook);
    assert!(
        matches!(agent.outcomes(), [(_, Outcome::Published(_))]),
        "{:?}",
        agent.outcomes()
    );
    let world = world.borrow();
    assert!(holds(&world).is_empty(), "{:?}", holds(&world));
    let took = Duration::from_millis(world.now_ms - fake::EPOCH_MS);
    assert!(took >= away, "{took:?}");
    drop(world);

    // The daemon away from just after the intent: given up with nothing
    // taken.
    let world = World::new(Mode::Busy);
    let hook = {
        let world = world.clone();
        move |point: Point, _: &str| {
            if point.effect == Effect::Intent && point.after {
                world.borrow_mut().away_until = u64::MAX;
            }
            false
        }
    };
    let minute = Duration::from_secs(60);
    let (agent, t) = aged_run(&world, minute, hook);
    given_up_too_old(&world, &agent, &t, minute);
    assert!(world.borrow().programs.is_empty());
}

/// Mutant: the age read on the wall clock for a publish begun in this
/// process, or on the clock that does not step after a restart (which
/// starts it again), or on the wall clock at each round after a restart
/// rather than anchored once (S2 of #144's next look). The wall clock
/// steps three hours forward just after the intent: the publish goes on
/// and is published; an agent started after the step reads the wall
/// clock and gives it up; one started before the step, and running
/// across it, publishes.
#[test]
fn a_publish_s_age_is_read_on_the_clock_that_does_not_step_in_its_process() {
    let step = |world: &Rc<RefCell<World>>| {
        let world = world.clone();
        move |point: Point, _: &str| {
            if point.effect == Effect::Intent && point.after {
                world.borrow_mut().agent_skew_ms = 3 * 3_600_000;
            }
            false
        }
    };
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut stepped = agent(&world, &keep).with_points(Box::new(step(&world)));
    stepped.begin().expect("begun");
    stepped.settle(LIMIT).expect("settled");
    assert!(
        matches!(stepped.outcomes(), [(_, Outcome::Published(_))]),
        "{:?}",
        stepped.outcomes()
    );

    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let t = {
        let mut first = agent(&world, &keep).with_points(Box::new(step(&world)));
        first.begin().expect("begun")
    };
    let mut next = agent(&world, &keep);
    next.settle(LIMIT).expect("settled");
    given_up_too_old(&world, &next, &t, Duration::ZERO);

    // S2 of #144's next look: an agent started before the step anchors
    // the age on the wall clock once, and the step after does not move
    // it.
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    agent(&world, &keep).begin().expect("begun");
    let stepped = Rc::new(Cell::new(false));
    let hook = {
        let (world, stepped) = (world.clone(), stepped.clone());
        move |_: Point, _: &str| {
            if !stepped.replace(true) {
                world.borrow_mut().agent_skew_ms = 3 * 3_600_000;
            }
            false
        }
    };
    let mut next = agent(&world, &keep).with_points(Box::new(hook));
    next.settle(LIMIT).expect("settled");
    assert!(stepped.get());
    assert!(
        matches!(next.outcomes(), [(_, Outcome::Published(_))]),
        "{:?}",
        next.outcomes()
    );
}

/// A refused premise takes nothing and publishes nothing.
#[test]
fn a_pool_refused_its_premise_gets_no_snapshot() {
    for user in [User::Attach, User::Add] {
        let world = World::new(Mode::Busy);
        world.borrow_mut().user(user, "");
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        assert!(matches!(agent.begin(), Err(Error::Premise(_))), "{user:?}");
        let world = world.borrow();
        assert!(world.programs.is_empty(), "{user:?}");
        assert!(world.refs.is_empty(), "{user:?}");
        assert!(keep.0.borrow().record.is_none(), "{user:?}");
    }
}

/// The matrix of the agent's losses: it stops at each of its points in
/// turn, the world restarts in each way (or not at all), and a new agent
/// recovers.
#[test]
fn the_agent_stopped_at_any_point_recovers_in_any_restart() {
    let mut restarts = vec![Restart::None];
    restarts.extend(RESTARTS);
    let mut runs = 0;
    for mode in MODES {
        let points = run(mode, Fault::None).points;
        assert!(points > 20, "{mode:?}: {points} points");
        for k in 0..points {
            for restart in &restarts {
                let fault = Fault::Agent(k, *restart);
                let ran = run(mode, fault);
                runs += 1;
                assert!(
                    ran.stopped_at.is_some(),
                    "{mode:?} {fault:?}: never stopped"
                );
                if let Err(why) = check(&ran) {
                    panic!("{}", failed(&format!("{mode:?} {fault:?}"), &ran, &why));
                }
            }
        }
    }
    assert!(runs > 500, "{runs} runs");
}

/// A publish recorded done with its snapshot gone is reported however
/// the agent stops after the loss (F1 of #144's review): each run in which
/// a rewind below `pool@t` under a running agent ends in a report of
/// [`Report::SnapshotGone`], with the agent then stopped at each later
/// point and another started.
///
/// Mutant: the report made after the record says done (a stop between
/// the two loses it), or not made.
#[test]
fn a_snapshot_gone_report_is_made_however_the_agent_stops_after_the_loss() {
    let mut reported = 0;
    let mut runs = 0;
    for mode in MODES {
        let points = run(mode, Fault::None).points;
        for k in 0..points {
            for restart in [
                Restart::RewindF(Back::BelowCreated),
                Restart::RewindT(Back::BelowCreated),
            ] {
                let ran = run(mode, Fault::World(k, restart));
                let gone = |ran: &Ran| {
                    ran.reports
                        .iter()
                        .filter(|(_, r)| *r == Report::SnapshotGone)
                        .count()
                };
                if gone(&ran) == 0 {
                    continue;
                }
                reported += 1;
                for stop in k + 1..ran.points {
                    let fault = Fault::WorldThenStop(k, restart, stop);
                    let stopped = run(mode, fault);
                    runs += 1;
                    if let Err(why) = check(&stopped) {
                        panic!("{}", failed(&format!("{mode:?} {fault:?}"), &stopped, &why));
                    }
                    // `check` requires the report of each publish
                    // recorded done with its snapshot gone: here, the one.
                    assert!(
                        !stopped.gone_at_done.is_empty(),
                        "{mode:?} {fault:?}: {:?}",
                        stopped.outcomes
                    );
                }
            }
        }
    }
    assert!(reported > 0, "no run reports its snapshot gone");
    assert!(runs > 10, "{runs} runs");
}

/// What a person changes between the agent's round and its request
/// asked anew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meddle {
    None,
    /// The agent's hold released (`zfs release`).
    Released,
    /// `pool@t` replaced by a snapshot of the same name (another GUID).
    Replaced,
}

/// D5 (the lead's order for a request asked anew; T2 of #144's review):
/// `load_guid`, then the members and the hold, then the count, then
/// `load_guid` again, and only then the request. Where the hold or the
/// snapshot changed after the round that planned it, nothing is counted
/// or asked: the next round sees the change.
///
/// Mutant: the count read before the members and the hold are checked,
/// or either check left out.
#[test]
fn asking_anew_checks_the_snapshot_and_its_hold_before_the_count() {
    for meddle in [Meddle::None, Meddle::Released, Meddle::Replaced] {
        let world = World::new(Mode::Busy);
        let keep = MemKeep::default();
        let armed = Rc::new(Cell::new(true));
        let hook = {
            let armed = armed.clone();
            move |point: Point, _: &str| point == ASKED && armed.replace(false)
        };
        let mut agent = agent(&world, &keep).with_points(Box::new(hook));
        agent.begin().expect("begun");
        assert!(matches!(agent.settle(LIMIT), Err(Error::Stopped)));
        let publish = agent
            .record()
            .under_way
            .first()
            .cloned()
            .expect("a publish under way");
        let t = publish.name.clone();
        {
            let mut world = world.borrow_mut();
            let pool = world.pool.as_mut().expect("the pool");
            let suffix = format!("@{t}");
            for (name, snap) in pool.snaps.iter_mut() {
                match meddle {
                    Meddle::Released if name.ends_with(&suffix) => snap.holds.clear(),
                    Meddle::Replaced if *name == format!("{POOL}{suffix}") => snap.guid ^= 1,
                    _ => {}
                }
            }
            world.calls.clear();
        }
        let moved = agent.ask_anew(&publish).expect("asked anew");
        let calls = world.borrow().calls.clone();
        match meddle {
            Meddle::None => {
                assert_eq!(moved, Moved::Yes);
                assert_eq!(
                    calls.get(..6),
                    Some(&["props", "members", "held", "cuts", "props", "publish"][..]),
                    "{calls:?}"
                );
            }
            Meddle::Released | Meddle::Replaced => {
                assert_eq!(moved, Moved::No, "{meddle:?}");
                assert_eq!(calls, ["props", "members", "held"], "{meddle:?}");
            }
        }
    }
}

/// The matrix of the daemon's losses: it dies at each point of its
/// publish and comes back each way.
#[test]
fn the_daemon_lost_at_any_point_of_a_publish_is_recovered_from() {
    let mut unreached = BTreeSet::new();
    for mode in MODES {
        for point in DAEMON_POINTS {
            for restart in RESTARTS {
                let fault = Fault::Daemon(point, restart);
                let ran = run(mode, fault);
                if let Err(why) = check(&ran) {
                    panic!("{}", failed(&format!("{mode:?} {fault:?}"), &ran, &why));
                }
                if ran.world.borrow().die.is_some() {
                    unreached.insert(format!("{mode:?} {point:?} {restart:?}"));
                }
            }
        }
    }
    // On an idle pool the only groups are the agent's: the program's and
    // the hold's are cut before the publish is asked, so a qualifying cut
    // is always known at its lookup and none is taken while it pends.
    let expected: BTreeSet<String> = RESTARTS
        .iter()
        .map(|restart| format!("Idle CutTaken {restart:?}"))
        .collect();
    assert_eq!(unreached, expected, "points the daemon never reached");
}

/// The matrix of a person's actions: each, at each of the agent's
/// points.
#[test]
fn a_persons_action_at_any_point_is_met() {
    for mode in MODES {
        let points = run(mode, Fault::None).points;
        for k in 0..points {
            for user in USERS {
                let fault = Fault::User(k, user);
                let ran = run(mode, fault);
                if let Err(why) = check(&ran) {
                    panic!("{}", failed(&format!("{mode:?} {fault:?}"), &ran, &why));
                }
            }
        }
    }
}

/// The matrix of restarts under a running agent: at each of its points,
/// the daemon is taken over, or the pool exported and imported with a
/// rewind, and the agent goes on.
#[test]
fn a_restart_under_a_running_agent_at_any_point_is_met() {
    for mode in MODES {
        let points = run(mode, Fault::None).points;
        for k in 0..points {
            for restart in RESTARTS {
                if restart == Restart::Reboot {
                    continue;
                }
                let fault = Fault::World(k, restart);
                let ran = run(mode, fault);
                if let Err(why) = check(&ran) {
                    panic!("{}", failed(&format!("{mode:?} {fault:?}"), &ran, &why));
                }
            }
        }
    }
}

/// The matrix of restarts between two of the agent's reads: as it asks
/// the daemon for its cuts, each time, the daemon is taken over or the
/// pool imported again with a rewind. Such a restart falls inside the
/// reads that AskAnew makes before it asks ("The rule for AskAnew"), and
/// between the planner's look and its action.
#[test]
fn a_restart_as_the_cuts_are_read_is_met() {
    for mode in MODES {
        let reads = run(mode, Fault::None).cuts_read;
        assert!(reads > 3, "{mode:?}: {reads} reads");
        for k in 0..reads {
            for restart in RESTARTS {
                if restart == Restart::Reboot {
                    continue;
                }
                let fault = Fault::Cuts(k, restart);
                let ran = run(mode, fault);
                if let Err(why) = check(&ran) {
                    panic!("{}", failed(&format!("{mode:?} {fault:?}"), &ran, &why));
                }
            }
        }
    }
}

/// The matrix of [`a_restart_as_the_cuts_are_read_is_met`], with the
/// agent stopped just after the next request it asks and down for
/// [`DOWN`]: its next round does not come before the daemon counts and
/// uploads the cuts after the restart. A restart inside the reads of
/// AskAnew ("The rule for AskAnew") is then met by those reads alone:
/// asked with the snapshot lost, with its hold lost, or with a count of
/// another import, the request would be carried by a cut that does not
/// hold `pool@t`, and no withdrawal at the next round would come first.
///
/// Each of those reads is not caught alone, since a rewind changes all
/// that it changes at once: the pool's `load_guid` with the snapshot or
/// its hold, and the daemon's rule (R) refuses a request counted before
/// the rewind. Mutant caught: AskAnew with none of its three checks.
#[test]
fn a_restart_as_the_cuts_are_read_is_met_with_the_agent_down_after_it_asks() {
    for mode in MODES {
        let reads = run(mode, Fault::None).cuts_read;
        for k in 0..reads {
            for restart in RESTARTS {
                if restart == Restart::Reboot {
                    continue;
                }
                let fault = Fault::CutsThenStop(k, restart);
                let ran = run(mode, fault);
                if let Err(why) = check(&ran) {
                    panic!("{}", failed(&format!("{mode:?} {fault:?}"), &ran, &why));
                }
            }
        }
    }
}

/// The hold outlasts the deadline of the last `accepted`, not the first:
/// a publish asked again of a new daemon pends until its new deadline,
/// and a hold let go before that lets a person destroy `pool@t` while a
/// cut may still be published for it (q11 of the review of #122). Here
/// the daemon is taken over 50 s after the first `accepted`, the agent
/// asks anew, the channel is then away past both deadlines, and a person
/// destroys `pool@t` as soon as it is not held. Mutant: `held_until`
/// counted from the first `accepted`.
#[test]
fn a_hold_outlasts_the_deadline_of_the_last_acceptance() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let asked = Rc::new(Cell::new(0));
    let hook = {
        let (world, asked) = (world.clone(), asked.clone());
        move |point: Point, _: &str| {
            if point
                == (Point {
                    effect: Effect::Asked,
                    after: true,
                })
            {
                asked.set(asked.get() + 1);
                if asked.get() == 2 {
                    let mut world = world.borrow_mut();
                    world.away_until = world.now_ms + 800_000;
                    world.destroyer = world.programs.keys().last().cloned();
                }
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    // No cut is uploaded before the take-over.
    world.borrow_mut().lag_ms = 100_000;
    agent.begin().expect("begun");
    assert!(matches!(
        agent.settle(Duration::from_secs(50)),
        Err(Error::Unsettled(_))
    ));
    assert_eq!(asked.get(), 1);
    {
        let mut world = world.borrow_mut();
        world.restart(Restart::TakeOver, None);
        world.lag_ms = 2_000;
    }
    let settled = agent.settle(LIMIT).map_err(|e| e.to_string());
    let ran = Ran {
        world: world.clone(),
        files: keep.0.clone(),
        settled,
        outcomes: agent.outcomes().to_vec(),
        reports: agent.reports().to_vec(),
        points: 0,
        cuts_read: 0,
        stopped_at: None,
        gone_at_done: BTreeSet::new(),
    };
    assert_eq!(asked.get(), 2, "asked anew of the next daemon");
    if let Err(why) = check(&ran) {
        panic!("{}", failed("the hold's deadline", &ran, &why));
    }
    assert!(
        matches!(ran.outcomes[..], [(_, Outcome::Abandoned(Why::Destroyed))]),
        "{:?}",
        ran.outcomes
    );
}

/// The manifest's file changed under the agent, after it was recorded:
/// the publish stops for a person, its hold kept and nothing asked.
/// Mutant: the file's hash not compared with the record's.
#[test]
fn a_manifest_changed_under_the_agent_stops_its_publish() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let hook = {
        let keep = keep.clone();
        move |point: Point, _: &str| {
            if point
                == (Point {
                    effect: Effect::Taken,
                    after: true,
                })
            {
                let mut files = keep.0.borrow_mut();
                for bytes in files.manifests.values_mut() {
                    // The same length, another content.
                    if let Some(last) = bytes.last_mut() {
                        *last ^= 1;
                    }
                }
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    let name = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    assert!(
        matches!(agent.outcomes(), [(n, Outcome::Stopped(_))] if *n == name),
        "{:?}",
        agent.outcomes()
    );
    let world = world.borrow();
    assert!(world.refs.is_empty());
    assert!(!world.daemon.asked());
    assert_eq!(holds(&world).len(), 3, "the hold is kept for a person");
    assert!(agent.record().publish(&name).is_some());
}

/// A name a person took first, in one member, is passed over: the
/// program takes nothing, and the publish takes the next name. Mutant:
/// the publish stopped or abandoned on a name that exists, or what the
/// process knows of it (its age's anchor) left under the old name.
#[test]
fn a_name_a_person_took_first_is_passed_over() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut agent = agent(&world, &keep);
    let name = agent.begin().expect("begun");
    let theirs = format!("{POOL}/db@{name}");
    {
        let mut world = world.borrow_mut();
        let pool = world.pool.as_mut().expect("the pool");
        let snap = fake::Snap {
            guid: 0xdead,
            createtxg: pool.txg,
            holds: BTreeSet::new(),
            held_in: None,
        };
        pool.snaps.insert(theirs.clone(), snap);
    }
    // Once the publish has its next name, the wall clock steps past the
    // age bound: begun in this process, it is aged on the clock that does
    // not step under its next name too (N2 of #144's next look).
    let refused = Rc::new(Cell::new(false));
    let hook = {
        let (world, refused) = (world.clone(), refused.clone());
        move |point: Point, _: &str| {
            match point.effect {
                Effect::Program if point.after => refused.set(true),
                Effect::Intent if point.after && refused.get() => {
                    world.borrow_mut().agent_skew_ms = 3 * 3_600_000;
                }
                _ => {}
            }
            false
        }
    };
    let mut agent = agent.with_points(Box::new(hook));
    agent.settle(LIMIT).expect("settled");
    assert_eq!(world.borrow().agent_skew_ms, 3 * 3_600_000);
    let next = format!("{name}-1");
    assert!(
        matches!(agent.outcomes(), [(n, Outcome::Published(_))] if *n == next),
        "{:?}",
        agent.outcomes()
    );
    let world = world.borrow();
    assert!(world.refs.contains_key(&next), "{:?}", world.refs.keys());
    let pool = world.pool.as_ref().expect("the pool");
    assert_eq!(pool.snaps.get(&theirs).map(|s| s.guid), Some(0xdead));
    assert!(!pool.snaps.contains_key(&format!("{POOL}@{name}")));
}

/// At most [`UNDER_WAY_MAX`] publishes are under way; the next is
/// refused before anything is taken. Mutant: no limit.
#[test]
fn at_most_eight_publishes_are_under_way() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut agent = agent(&world, &keep);
    for _ in 0..UNDER_WAY_MAX {
        agent.begin().expect("begun");
        world.borrow_mut().advance(Duration::from_secs(1));
    }
    assert!(matches!(agent.begin(), Err(Error::Full)));
    assert_eq!(agent.record().under_way.len(), UNDER_WAY_MAX);
    agent.settle(LIMIT).expect("settled");
    let published = agent
        .outcomes()
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Published(_)))
        .count();
    assert_eq!(published, UNDER_WAY_MAX, "{:?}", agent.outcomes());
    assert!(holds(&world.borrow()).is_empty());
}

/// Every fault of the matrices, in `mode` on a daemon with `rule`: the
/// agent stopped at each of its points with each restart, the world
/// restarted under it at each point and at each read of the cuts, a
/// person's action at each point, and the daemon lost at each of its
/// points. The points are counted under `rule`:
/// without (R) a publish takes fewer.
fn every_fault(rule: Rule, mode: Mode) -> Vec<Fault> {
    let ran = run_under(rule, mode, Fault::None);
    let mut faults = Vec::new();
    for k in 0..ran.cuts_read {
        for restart in RESTARTS {
            if restart != Restart::Reboot {
                faults.push(Fault::Cuts(k, restart));
            }
        }
    }
    let points = ran.points;
    for k in 0..points {
        faults.push(Fault::Agent(k, Restart::None));
        for restart in RESTARTS {
            faults.push(Fault::Agent(k, restart));
            if restart != Restart::Reboot {
                faults.push(Fault::World(k, restart));
            }
        }
        faults.extend(USERS.map(|user| Fault::User(k, user)));
    }
    for point in DAEMON_POINTS {
        faults.extend(RESTARTS.map(|restart| Fault::Daemon(point, restart)));
    }
    faults
}

/// The runs of every matrix that fail on a daemon with `rule`.
fn failing_under(rule: Rule) -> BTreeSet<String> {
    let mut failing = BTreeSet::new();
    for mode in MODES {
        for fault in every_fault(rule, mode) {
            if check(&run_under(rule, mode, fault)).is_err() {
                failing.insert(format!("{mode:?} {fault:?}"));
            }
        }
    }
    failing
}

/// The matrices need the daemon's rule (R), and in the form the fake
/// gives it ([`fake::DAEMON_RULE`]).
///
/// With no rule, publishes are made on cuts that do not hold their
/// snapshot (the race of the plan's §0). With the form M2.6a built
/// (a cut whose group is not above the previous cut's), three runs still
/// publish such a cut. Each is a cut whose group errs low, counted after
/// the publish was accepted: the rule marks it, and the agent asks anew
/// with the mark. The agent then stops, and the pool is imported again
/// with `-F` to before the snapshot. The new history's first group bears
/// the number of the old history's top and is above the low cut, so it
/// is not marked, and the publish asked anew is made on it. A daemon
/// that compares with the highest group it has seen marks it.
#[test]
fn the_matrices_need_the_rule_r_of_the_daemon() {
    let off = failing_under(Rule::Off);
    assert!(off.len() >= 10, "with no rule: {off:?}");
    let previous = failing_under(Rule::Previous);
    // The second ask's `ask` (after), `asked` (before) and `asked`
    // (after); two `asking` records before them since #149's L1.
    let expected: BTreeSet<String> = [29, 30, 31]
        .map(|k| format!("ErrLow Agent({k}, RewindF(BelowCreated))"))
        .into();
    assert_eq!(previous, expected, "under the rule as M2.6a built it");
    assert_eq!(failing_under(fake::DAEMON_RULE), BTreeSet::new());
}

/// The sweep releases only holds that no publish under way names: an
/// agent that starts again while a publish is asked keeps its hold, and
/// a person who tries at every point to destroy `pool@t` succeeds only
/// after the publish is done. Mutant: the sweep releases every hold of
/// the tag.
#[test]
fn the_sweep_leaves_the_holds_of_publishes_under_way() {
    for mode in MODES {
        let world = World::new(mode);
        let keep = MemKeep::default();
        let asked = Point {
            effect: Effect::Asked,
            after: true,
        };
        let mut first = agent(&world, &keep).with_points(Box::new(move |p, _: &str| p == asked));
        let name = first.begin().expect("begun");
        assert!(
            matches!(first.settle(LIMIT), Err(Error::Stopped)),
            "{mode:?}"
        );
        drop(first);
        let destroyed_while = Rc::new(RefCell::new(None));
        let hook = {
            let (world, keep, name) = (world.clone(), keep.clone(), name.clone());
            let destroyed_while = destroyed_while.clone();
            move |_: Point, _: &str| {
                let mut world = world.borrow_mut();
                if destroyed_while.borrow().is_some() {
                    return false;
                }
                world.user(User::Destroy, &name);
                let snapshot = format!("{POOL}@{name}");
                if world
                    .pool
                    .as_ref()
                    .is_some_and(|p| !p.snaps.contains_key(&snapshot))
                {
                    // What the record said of the publish then.
                    let record = keep.0.borrow().record.clone();
                    let phase = record
                        .and_then(|bytes| Record::decode(&bytes).ok())
                        .and_then(|r| r.publish(&name).map(|p| format!("{:?}", p.phase)));
                    *destroyed_while.borrow_mut() = Some(phase);
                }
                false
            }
        };
        let mut again = agent(&world, &keep).with_points(Box::new(hook));
        let settled = again.settle(LIMIT).map_err(|e| e.to_string());
        let ran = Ran {
            world: world.clone(),
            files: keep.0.clone(),
            settled,
            outcomes: again.outcomes().to_vec(),
            reports: again.reports().to_vec(),
            points: 0,
            cuts_read: 0,
            stopped_at: None,
            gone_at_done: BTreeSet::new(),
        };
        if let Err(why) = check(&ran) {
            panic!("{}", failed(&format!("{mode:?}"), &ran, &why));
        }
        assert!(
            matches!(&ran.outcomes[..], [(n, Outcome::Published(_))] if *n == name),
            "{mode:?}: {:?}",
            ran.outcomes
        );
        let phase = destroyed_while
            .borrow()
            .clone()
            .expect("destroyed once released");
        assert!(
            phase.as_deref().is_none_or(|p| p.starts_with("Done")),
            "{mode:?}: destroyed while {phase:?}"
        );
    }
}

/// The fake daemon, with `look` blind: it never sees `snap/<t>`, so a
/// `done` is not confirmed.
struct Blind(FakeDaemon);

impl Daemon for Blind {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let reply = self.0.ask(request, manifest)?;
        Ok(match request {
            Request::Look(_) => Reply::Absent,
            _ => reply,
        })
    }

    fn device(&mut self) -> io::Result<u64> {
        self.0.device()
    }
}

/// A publish stopped for a person by a `done` that `look` did not see,
/// in a world that the stopped agent left: its name.
fn stopped_for_a_person(world: &Rc<RefCell<World>>, keep: &MemKeep) -> String {
    let mut agent = Agent::new(
        FakeZfs(world.clone()),
        Blind(FakeDaemon(world.clone())),
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent");
    let t = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    assert_eq!(
        agent.outcomes(),
        [(t.clone(), Outcome::Stopped(DONE_NOT_SEEN.to_string()))]
    );
    assert_eq!(
        agent.record().publish(&t).and_then(|p| p.person),
        Some(Person::Stopped)
    );
    assert!(world.borrow().refs.contains_key(&t));
    assert_eq!(holds(&world.borrow()).len(), 3, "the hold is kept");
    t
}

/// S5 of #144's third look: a stop for a person outlasts a restart of
/// the agent, which asks nothing of the daemon for it, until `recover`;
/// retried, it is planned as the agent's own and published.
///
/// Mutant: the stop not recorded, the recorded stop planned again, or
/// `recover --retry` not clearing it.
#[test]
fn a_stop_for_a_person_outlasts_a_restart_until_recovered() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let t = stopped_for_a_person(&world, &keep);
    let mut again = agent(&world, &keep);
    let before = world.borrow().calls.len();
    again.settle(LIMIT).expect("settled");
    assert_eq!(
        again.outcomes(),
        [(t.clone(), Outcome::Stopped(STOPPED_BEFORE.to_string()))]
    );
    let asked: Vec<&str> = world.borrow().calls[before..]
        .iter()
        .copied()
        .filter(|c| !matches!(*c, "props"))
        .collect();
    assert!(asked.is_empty(), "{asked:?}");
    // Settled again in the same process: stopped once.
    again.settle(LIMIT).expect("settled");
    assert_eq!(again.outcomes().len(), 1);
    assert!(matches!(
        again.recover("skz-none", Choice::Retry),
        Err(Error::NotUnderWay(_))
    ));
    again.recover(&t, Choice::Retry).expect("recovered");
    assert_eq!(again.record().publish(&t).and_then(|p| p.person), None);
    again.settle(LIMIT).expect("settled");
    assert!(
        matches!(again.outcomes().last(), Some((n, Outcome::Published(_))) if *n == t),
        "{:?}",
        again.outcomes()
    );
    assert!(again.record().under_way.is_empty());
    assert!(holds(&world.borrow()).is_empty());
}

/// A person's give-up of a publish already made is a publish: a person
/// cannot unmake it. Mutant: given up on the person's word alone.
#[test]
fn a_person_s_give_up_of_a_publish_made_publishes_it() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let t = stopped_for_a_person(&world, &keep);
    let mut again = agent(&world, &keep);
    again.recover(&t, Choice::GiveUp).expect("recovered");
    assert_eq!(
        again.record().publish(&t).and_then(|p| p.person),
        Some(Person::GiveUp)
    );
    again.settle(LIMIT).expect("settled");
    assert!(
        matches!(again.outcomes(), [(n, Outcome::Published(_))] if *n == t),
        "{:?}",
        again.outcomes()
    );
    assert!(again.record().under_way.is_empty());
    assert!(holds(&world.borrow()).is_empty());
}

/// A person's give-up of a publish whose request pends: withdrawn,
/// looked up, released, and given up as [`Why::Person`], with nothing
/// published. Mutant: the hold released before the withdraw, or the
/// give-up recorded under another reason.
#[test]
fn a_person_gives_up_a_publish_whose_request_pends() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let mut agent0 = agent(&world, &keep).with_points(Box::new(|p: Point, _: &str| p == ASKED));
    let t = agent0.begin().expect("begun");
    assert!(matches!(agent0.settle(LIMIT), Err(Error::Stopped)));
    drop(agent0);
    let order = Rc::new(RefCell::new(Vec::new()));
    let hook = {
        let order = order.clone();
        move |p: Point, _: &str| {
            if !p.after {
                order.borrow_mut().push(p.effect);
            }
            false
        }
    };
    let mut again = agent(&world, &keep).with_points(Box::new(hook));
    again.recover(&t, Choice::GiveUp).expect("recovered");
    again.settle(LIMIT).expect("settled");
    assert_eq!(
        again.outcomes(),
        [(t.clone(), Outcome::Abandoned(Why::Person))]
    );
    assert_eq!(
        again.record().abandoned.last().map(|a| a.why),
        Some(Why::Person)
    );
    let order = order.borrow();
    let at = |e: Effect| order.iter().position(|&x| x == e);
    assert!(at(Effect::Recover) < at(Effect::Withdraw), "{order:?}");
    assert!(at(Effect::Withdraw) < at(Effect::Release), "{order:?}");
    assert!(at(Effect::Release) < at(Effect::Abandon), "{order:?}");
    let world = world.borrow();
    assert!(world.refs.is_empty());
    assert!(holds(&world).is_empty(), "{:?}", holds(&world));
}

/// A person's give-up while the uploader is in a long step (`progress`
/// and `withdraw` answered `refused no-answer`, as skyzfsd answers them)
/// keeps the hold until `held_until`, though a person stands ready to
/// destroy `pool@t` once it is let go. Where the request lands, it lands
/// on a cut that holds every member's `@t` as the manifest has it, and
/// the publish is published, not given up; where it does not, it has
/// expired before the hold goes, and no `snap/t` is ever made (B1 of
/// #149's review, its R1 and R2). Mutant: the silent answers read as a
/// daemon that answered.
#[test]
fn a_person_s_give_up_keeps_the_hold_while_the_uploader_is_silent() {
    for mode in [Mode::Busy, Mode::Idle, Mode::Abandoned] {
        let world = World::new(mode);
        world.borrow_mut().lag_ms = 60_000;
        let keep = MemKeep::default();
        let mut agent0 = agent(&world, &keep).with_points(Box::new(|p: Point, _: &str| p == ASKED));
        let t = agent0.begin().expect("begun");
        assert!(matches!(agent0.settle(LIMIT), Err(Error::Stopped)));
        drop(agent0);
        {
            let mut world = world.borrow_mut();
            world.silent_until = u64::MAX;
            world.destroyer = Some(t.clone());
        }
        let mut again = agent(&world, &keep);
        again.recover(&t, Choice::GiveUp).expect("recovered");
        again.settle(LIMIT).expect("settled");
        let published = match again.outcomes() {
            [(n, Outcome::Published(_))] if *n == t => true,
            [(n, Outcome::Abandoned(Why::Person))] if *n == t => false,
            other => panic!("{mode:?}: {other:?}"),
        };
        // Whatever still pends at the daemon has its time to land.
        world.borrow_mut().advance(Duration::from_secs(2 * 3600));
        let world = world.borrow();
        assert!(holds(&world).is_empty(), "{mode:?}: {:?}", holds(&world));
        let Some(r) = world.refs.get(&t) else {
            assert!(!published, "{mode:?}: published, and no snap/{t}");
            continue;
        };
        assert!(published, "{mode:?}: given up, and snap/{t} made");
        let manifest = Manifest::decode(&r.manifest).expect("the manifest");
        let cut = &world.commits[&r.commit].cut;
        for m in &manifest.members {
            let at = format!("{}@{t}", m.name);
            assert_eq!(
                cut.snaps.get(&at),
                Some(&m.snapshot_guid),
                "{mode:?}: snap/{t} names cut {} (group {}) without {at}",
                cut.number,
                cut.group
            );
        }
    }
}

/// Each report goes to the sink before the engine acts on it, and each
/// point is named for its publish (testing's `--trace-points`).
/// Mutant: the sink called after the act, or not at all; a point given
/// another publish's name, or none.
#[test]
fn the_sink_hears_a_report_before_the_act_and_points_name_their_publish() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let hook = {
        let (world, log) = (world.clone(), log.clone());
        let rewound = Cell::new(false);
        move |point: Point, name: &str| {
            log.borrow_mut()
                .push(format!("{} {} {name}", point.effect.word(), point.after));
            let withdraw = Point {
                effect: Effect::Withdraw,
                after: false,
            };
            if point == withdraw && !rewound.replace(true) {
                world
                    .borrow_mut()
                    .restart(Restart::RewindF(Back::AfterHold), None);
            }
            false
        }
    };
    let sink = {
        let log = log.clone();
        move |name: &str, report: &Report| {
            log.borrow_mut().push(format!("report {report:?} {name}"))
        }
    };
    let keep = MemKeep::default();
    let mut agent = agent(&world, &keep)
        .with_age_max(Duration::from_secs(60))
        .with_points(Box::new(hook))
        .with_sink(Box::new(sink));
    let t = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    assert_eq!(
        agent.outcomes(),
        [(t.clone(), Outcome::Abandoned(Why::TooOld))]
    );
    let log = log.borrow();
    let at = |line: &str| log.iter().position(|l| l == line);
    let report = format!("report Refused(Rewound) {t}");
    assert!(at(&report).is_some(), "{log:?}");
    assert!(at(&report) < at(&format!("release false {t}")), "{log:?}");
    assert_eq!(log.iter().filter(|l| l.starts_with("report")).count(), 1);
    for line in log.iter().filter(|l| !l.starts_with("report")) {
        let want = if line.starts_with("enrol ") {
            ""
        } else {
            t.as_str()
        };
        assert!(
            line.ends_with(&format!(" {want}")),
            "{line:?} is not named {want:?}"
        );
    }
}

/// With two publishes under way, each point of a round is named for the
/// publish the round acts on, not for the one begun last: at the point
/// after the snapshot program or a hold, the named publish's snapshot exists or is
/// held. Mutant: the round leaves the name of the last publish begun.
#[test]
fn each_round_names_its_own_publish_at_its_points() {
    let world = World::new(Mode::Busy);
    let checked = Rc::new(Cell::new(0_u32));
    let hook = {
        let (world, checked) = (world.clone(), checked.clone());
        move |point: Point, name: &str| {
            let world = world.borrow();
            let root = world.pool.as_ref().map(|p| p.datasets[0].0.clone());
            let snap = format!("{}@{name}", root.unwrap_or_default());
            match (point.effect, point.after) {
                (Effect::Program, true) => {
                    let taken = world
                        .pool
                        .as_ref()
                        .is_some_and(|p| p.snaps.contains_key(&snap));
                    assert!(taken, "take named {name:?}, but {snap} was not taken");
                    checked.set(checked.get() + 1);
                }
                (Effect::Hold, true) => {
                    assert!(
                        holds(&world).contains(&snap),
                        "hold named {name:?}, but {snap} is not held"
                    );
                    checked.set(checked.get() + 1);
                }
                _ => {}
            }
            false
        }
    };
    let keep = MemKeep::default();
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    let first = agent.begin().expect("begun");
    let second = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    let outcomes = agent.outcomes().to_vec();
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert!(
        outcomes
            .iter()
            .all(|(_, o)| matches!(o, Outcome::Published(..))),
        "{outcomes:?}"
    );
    assert_ne!(first, second);
    // Both takes and both holds were checked.
    assert_eq!(checked.get(), 4);
}

/// The rigs name an effect by its word, in any case.
#[test]
fn each_effect_is_named_by_a_word_of_its_own() {
    let words: BTreeSet<&str> = Effect::ALL.iter().map(|e| e.word()).collect();
    assert_eq!(words.len(), Effect::ALL.len());
    for effect in Effect::ALL {
        assert_eq!(Effect::from_word(effect.word()), Some(effect));
        assert_eq!(
            Effect::from_word(&effect.word().to_uppercase()),
            Some(effect)
        );
    }
    assert_eq!(Effect::from_word("stop"), Some(Effect::Stop));
    assert_eq!(Effect::from_word("recover"), Some(Effect::Recover));
    assert_eq!(Effect::from_word("unwind"), None);
    assert_eq!(Effect::from_word(""), None);
}

/// What [`with_the_uploader_stopped`] saw.
#[derive(Debug)]
struct StopRun {
    names: Vec<String>,
    outcomes: Vec<(String, Outcome)>,
    reports: Vec<(String, Report)>,
    holds: u32,
    asks: u32,
    releases: u32,
    looks: u32,
    /// The world's time from the stop to the end of the settle.
    after_stop_ms: u64,
    held_at_end: Vec<String>,
}

/// Begin `publishes` publishes and settle them, the daemon's uploader
/// stopping for good (skyzfsd's `Publishing::stop`) at the agent's
/// `nth` point equal to `at` (from 1), or before the first begins where
/// `at` is `None`. Nothing is ever uploaded (the lag is a year), so no
/// publish is made before the stop.
fn with_the_uploader_stopped(at: Option<(Point, u32)>, publishes: usize) -> StopRun {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let stopped_at = Rc::new(Cell::new(None::<u64>));
    let counts = Rc::new(Cell::new((0_u32, 0_u32, 0_u32, 0_u32)));
    if at.is_none() {
        let mut w = world.borrow_mut();
        w.stop_uploader();
        stopped_at.set(Some(w.now_ms));
    }
    let hook = {
        let (world, stopped_at, counts) = (world.clone(), stopped_at.clone(), counts.clone());
        let reached = Cell::new(0_u32);
        move |point: Point, _: &str| {
            let (mut holds, mut asks, mut releases, mut looks) = counts.get();
            match (point.effect, point.after) {
                (Effect::Hold, true) => holds += 1,
                (Effect::Ask, true) => asks += 1,
                (Effect::Release, true) => releases += 1,
                (Effect::Look, true) => looks += 1,
                _ => {}
            }
            counts.set((holds, asks, releases, looks));
            if let Some((p, nth)) = at {
                if point == p {
                    reached.set(reached.get() + 1);
                    if reached.get() == nth {
                        let mut w = world.borrow_mut();
                        w.stop_uploader();
                        stopped_at.set(Some(w.now_ms));
                    }
                }
            }
            false
        }
    };
    let keep = MemKeep::default();
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    let names = (0..publishes)
        .map(|_| agent.begin().expect("begun"))
        .collect::<Vec<_>>();
    agent.settle(LIMIT).expect("settled");
    let (hold_count, asks, releases, looks) = counts.get();
    let world = world.borrow();
    let stopped = stopped_at.get().expect("the uploader stopped");
    StopRun {
        names,
        outcomes: agent.outcomes().to_vec(),
        reports: agent.reports().to_vec(),
        holds: hold_count,
        asks,
        releases,
        looks,
        after_stop_ms: world.now_ms - stopped,
        held_at_end: holds(&world),
    }
}

/// Each publish is given up as `stopped` soon after the stop, reported
/// once, its hold released, and no more holds, asks, releases or looks
/// than `per` (holds, asks, releases, looks) for each: a look's sight
/// kept across the release, not looked for again (TG14 of #146's
/// review).
fn given_up_at_the_stop(run: &StopRun, per: (u32, u32, u32, u32)) {
    let n = u32::try_from(run.names.len()).expect("a few");
    for t in &run.names {
        assert!(
            run.outcomes
                .iter()
                .any(|(name, o)| name == t && *o == Outcome::Abandoned(Why::Stopped)),
            "{t}: {run:?}"
        );
        let reported = run
            .reports
            .iter()
            .filter(|(name, r)| name == t && *r == Report::Refused(skyzfs_control::Code::Stopped))
            .count();
        assert_eq!(reported, 1, "{t} reported once: {run:?}");
    }
    assert_eq!(run.outcomes.len(), run.names.len(), "{run:?}");
    assert_eq!(run.reports.len(), run.names.len(), "{run:?}");
    assert_eq!(
        (run.holds, run.asks, run.releases, run.looks),
        (per.0 * n, per.1 * n, per.2 * n, per.3 * n),
        "holds, asks, releases and looks: {run:?}"
    );
    assert!(run.held_at_end.is_empty(), "{run:?}");
    // A round or two of ticks, not until the publish is too old.
    assert!(run.after_stop_ms <= 10_000, "{run:?}");
}

/// TG12 of #146's review, N8: a publish whose `publish` is refused
/// `stopped` (the uploader stopped for good) is given up as `Stopped`
/// whatever its age, and so is one whose request the stop ended; each
/// once, with one hold and one release, reported once. The stopped
/// uploader refuses every `publish` after and does not keep it, so
/// `progress` of that name answers `unknown`: the answer refused must
/// outlast the release that follows the look, or the publish is held and
/// asked again and again, with no tick between, until it is too old.
/// Mutant: the answer forgotten at the release (N8); one report for two
/// publishes ended by one stop (X12b of the review).
#[test]
fn a_stopped_uploader_gives_each_publish_up_at_once() {
    let asked = (
        Point {
            effect: Effect::Asked,
            after: true,
        },
        1,
    );
    let held = (
        Point {
            effect: Effect::Held,
            after: true,
        },
        1,
    );
    // Asked: the stop ends the request that pends.
    given_up_at_the_stop(&with_the_uploader_stopped(Some(asked), 1), (1, 1, 1, 1));
    // Held and not asked: the ask is refused.
    given_up_at_the_stop(&with_the_uploader_stopped(Some(held), 1), (1, 1, 1, 1));
    // Begun after the stop.
    given_up_at_the_stop(&with_the_uploader_stopped(None, 1), (1, 1, 1, 1));
    // Two publishes asked, ended by one stop: each is reported.
    let second = (
        Point {
            effect: Effect::Asked,
            after: true,
        },
        2,
    );
    given_up_at_the_stop(&with_the_uploader_stopped(Some(second), 2), (1, 1, 1, 1));
    // Two begun after it.
    given_up_at_the_stop(&with_the_uploader_stopped(None, 2), (1, 1, 1, 1));
}

/// N8's other case: a publish whose request expired before the stop is
/// asked anew after it, the ask refused `stopped`; it is given up at
/// once, not held and asked anew until it is too old.
#[test]
fn a_publish_asked_anew_after_the_uploader_stopped_is_given_up() {
    let anew = (
        Point {
            effect: Effect::Count,
            after: false,
        },
        1,
    );
    let run = with_the_uploader_stopped(Some(anew), 1);
    given_up_at_the_stop(&run, (1, 2, 1, 2));
}

/// S13 of #146's next look: a daemon that refuses every `publish` with
/// an answer skyzfsd gives a request only once (`rewound`, `state`,
/// `attachment`, `asked`) cannot drive the rounds with no tick between:
/// a publish is asked at most once at an instant, so at most once a
/// tick, and the settle ends. Mutant: the guard removed (the world's
/// clock stands still while the agent asks, and the fake stops it).
#[test]
fn a_daemon_that_refuses_every_ask_is_asked_at_most_once_a_tick() {
    use skyzfs_control::Code;
    for code in [Code::Rewound, Code::State, Code::Attachment, Code::Asked] {
        let world = World::new(Mode::Busy);
        world.borrow_mut().refuse_every_publish = Some(code);
        let asks = Rc::new(Cell::new(0_u32));
        let hook = {
            let asks = asks.clone();
            move |point: Point, _: &str| {
                if point.effect == Effect::Ask && point.after {
                    asks.set(asks.get() + 1);
                }
                false
            }
        };
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep).with_points(Box::new(hook));
        agent.begin().expect("begun");
        let _ = agent.settle(LIMIT);
        let took_ms = world.borrow().now_ms - fake::EPOCH_MS;
        let asked = u64::from(asks.get());
        assert!(asked >= 2, "{code:?}: asked anew: {asked}");
        assert!(
            asked <= took_ms / 1000 + 1,
            "{code:?}: {asked} asks in {took_ms} ms"
        );
    }
}

/// An agent given a mistyped `--dir` finds no marker and no record
/// there, and releases none of the holds of the publishes under way in
/// the right one: neither its sweep nor a publish asked there, which
/// writes nothing in it (L4 of #149's review). Where the marker is
/// there with no record (a crash between the two, or the record
/// removed), the sweep releases as before. Mutants: the sweep without
/// the marker's check; the check after the first release; `begin`
/// writing before it sweeps.
#[test]
fn an_agent_given_a_mistyped_directory_releases_no_hold() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let right = MemKeep::default();
    let mut first = agent(&world, &right);
    let t = first.begin().expect("begun");
    assert!(first.settle(Duration::from_secs(60)).is_err());
    drop(first);
    let held = holds(&world.borrow());
    assert!(!held.is_empty(), "no hold to keep");
    assert!(right.0.borrow().marked);

    let mistyped = MemKeep::default();
    let mut wrong = agent(&world, &mistyped);
    match wrong.settle(LIMIT) {
        Err(Error::Unmarked(names)) => assert_eq!(names, vec![t.clone()]),
        other => panic!("settled in a mistyped directory: {other:?}"),
    }
    assert_eq!(holds(&world.borrow()), held);
    let mut wrong = agent(&world, &mistyped);
    match wrong.begin() {
        Err(Error::Unmarked(names)) => assert_eq!(names, vec![t.clone()]),
        other => panic!("begun in a mistyped directory: {other:?}"),
    }
    assert_eq!(holds(&world.borrow()), held);
    {
        let files = mistyped.0.borrow();
        assert!(files.record.is_none() && !files.marked && files.manifests.is_empty());
    }
    let shown = Error::Unmarked(vec![t.clone()]).to_string();
    assert!(
        shown.contains("no agent's marker (`agent`)") && shown.contains(&t),
        "{shown}"
    );

    // The marker with no record: the directory is the agent's.
    mistyped.0.borrow_mut().marked = true;
    let mut marked = agent(&world, &mistyped);
    marked.settle(LIMIT).expect("settled");
    assert!(
        holds(&world.borrow()).is_empty(),
        "{:?}",
        holds(&world.borrow())
    );
}

/// The record says a publish is asked, and is saved, before `publish` is
/// sent (the lead's decision on L1 of #149's review), so that a publish
/// recorded held has no request out. A crash between the save and the
/// send leaves it asked with nothing asked of the daemon; a person's
/// give-up then withdraws, looks and releases, and nothing is
/// published. A refusal that says nothing pends records it held again;
/// one that does not (`asked`, `ended`, `no-answer`) leaves it asked.
/// Mutants: the save after the send; held again on every refusal; never
/// held again.
#[test]
fn the_record_says_asked_before_the_request_is_sent() {
    // The crash between the save and the send.
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let stop = |point: Point, _: &str| point.effect == Effect::Ask && !point.after;
    let mut first = agent(&world, &keep).with_points(Box::new(stop));
    let t = first.begin().expect("begun");
    assert!(matches!(first.settle(LIMIT), Err(Error::Stopped)));
    drop(first);
    let saved = match keep.load().expect("loaded") {
        Loaded::Present(record) => record,
        other => panic!("{other:?}"),
    };
    let publish = saved.publish(&t).expect("under way");
    assert!(
        matches!(publish.phase, Phase::Asked { .. }),
        "{:?}",
        publish.phase
    );
    assert!(!world.borrow().daemon.asked(), "a request was sent");
    let mut after = agent(&world, &keep);
    after.recover(&t, Choice::GiveUp).expect("recovered");
    after.settle(LIMIT).expect("settled");
    assert!(
        matches!(after.outcomes(), [(n, Outcome::Abandoned(Why::Person))] if *n == t),
        "{:?}",
        after.outcomes()
    );
    assert!(world.borrow().refs.is_empty());
    assert!(
        holds(&world.borrow()).is_empty(),
        "{:?}",
        holds(&world.borrow())
    );

    // The crash after the send, the request accepted and its answer not
    // recorded: still asked, with `held_until` past the request's expiry
    // (the send plus the channel's wait); a person's give-up withdraws it.
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let sent = Rc::new(Cell::new(0_u64));
    let stop = {
        let (world, sent) = (world.clone(), sent.clone());
        move |point: Point, _: &str| {
            if point.effect == Effect::Ask && !point.after {
                sent.set(world.borrow().now_ms);
            }
            point.effect == Effect::Ask && point.after
        }
    };
    let mut first = agent(&world, &keep).with_points(Box::new(stop));
    let t = first.begin().expect("begun");
    assert!(matches!(first.settle(LIMIT), Err(Error::Stopped)));
    drop(first);
    assert!(world.borrow().daemon.asked(), "no request was sent");
    let saved = match keep.load().expect("loaded") {
        Loaded::Present(record) => record,
        other => panic!("{other:?}"),
    };
    match saved.publish(&t).map(|p| &p.phase) {
        Some(Phase::Asked { held_until_ms, .. }) => {
            let least = plan::held_until(sent.get(), EXPIRY_DEFAULT);
            assert!(*held_until_ms >= least, "{held_until_ms} < {least}");
        }
        other => panic!("{other:?}"),
    }
    let mut after = agent(&world, &keep);
    after.recover(&t, Choice::GiveUp).expect("recovered");
    after.settle(LIMIT).expect("settled");
    assert!(
        matches!(after.outcomes(), [(n, Outcome::Abandoned(Why::Person))] if *n == t),
        "{:?}",
        after.outcomes()
    );
    assert!(world.borrow().refs.is_empty());
    assert!(
        holds(&world.borrow()).is_empty(),
        "{:?}",
        holds(&world.borrow())
    );

    // What each refusal leaves recorded, seen right after the answer.
    use skyzfs_control::Code;
    for (code, pends) in [
        (Code::Attachment, false),
        (Code::State, false),
        (Code::Rewound, false),
        (Code::Busy, false),
        (Code::Taken, false),
        (Code::Stopped, false),
        (Code::NoUpload, false),
        (Code::Asked, true),
        (Code::Ended, true),
        (Code::NoAnswer, true),
    ] {
        let world = World::new(Mode::Busy);
        world.borrow_mut().refuse_every_publish = Some(code);
        let keep = MemKeep::default();
        // Stopped at the first point after the answer is handled.
        let hook = {
            let mut answered = false;
            move |point: Point, _: &str| {
                if answered && point.effect != Effect::Unasked {
                    return true;
                }
                answered |= point.effect == Effect::Ask && point.after;
                false
            }
        };
        let mut agent = agent(&world, &keep).with_points(Box::new(hook));
        let t = agent.begin().expect("begun");
        let _ = agent.settle(LIMIT);
        let phase = match keep.load().expect("loaded") {
            Loaded::Present(record) => record.publish(&t).map(|p| p.phase.clone()),
            other => panic!("{code:?}: {other:?}"),
        };
        let asked = matches!(phase, Some(Phase::Asked { .. }));
        assert_eq!(asked, pends, "{code:?}: {phase:?}");
    }
}
