//! The engine on the fake world ([`fake`]), and the crash matrix (the
//! plan of M2.6, §7): every loss at every point, then recovery to
//! quiescence, then the checks of [`check`].

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use super::fake::{
    holds, DaemonPoint, FakeClock, FakeDaemon, FakeZfs, MemFiles, MemKeep, Mode, Restart, Rule,
    User, World, DAEMON_POINTS, POOL, RESTARTS, USERS, VOLUME,
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
    match fault {
        Fault::Daemon(point, restart) => world.borrow_mut().die = Some((point, restart)),
        Fault::Cuts(k, restart) | Fault::CutsThenStop(k, restart) => {
            world.borrow_mut().restart_at_cuts = Some((k, restart));
        }
        _ => {}
    }
    let hook = {
        let (count, stopped_at, world) = (count.clone(), stopped_at.clone(), world.clone());
        move |point: Point| {
            let k = count.get();
            count.set(k + 1);
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
                Fault::World(at, restart) if at == k => {
                    world.borrow_mut().restart(restart, None);
                    false
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
                Fault::CutsThenStop(..) => Restart::None,
                _ => unreachable!("only these faults stop the agent"),
            };
            world.borrow_mut().restart(restart, None);
            world.borrow_mut().advance(DOWN);
            let mut again = agent(&world, &keep);
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
            Why::ManifestTooLarge | Why::NameTaken | Why::NoProgress => false,
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
            Report::NoProgress(_) => return Err(format!("{t} reported: {report:?}")),
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
        move |point: Point| {
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

/// Mutant: the sync task's value set again (the fake writes no group for
/// a property set to the value it holds, the conservative reading). Two
/// publishes in a row on an idle pool, the daemon rebooted as each is
/// first accepted, each need a sync task (a request asked anew is carried
/// by no other group), and the agent's clock steps back between them:
/// each value is new.
#[test]
fn each_sync_task_sets_a_value_of_its_own() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let armed = Rc::new(Cell::new(true));
    let hook = {
        let (world, armed) = (world.clone(), armed.clone());
        move |point: Point| {
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
        move |point: Point| {
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
/// Mutant: the stalls committed with the publish left at `Asked` (a
/// request asked anew after a stop between the count and the ask is
/// counted twice), or the report made at each count (made more than
/// once by one agent).
#[test]
fn a_stop_at_any_point_of_a_stalling_run_counts_each_stall_once() {
    let whole = stalling_run_stopped_at(usize::MAX).points;
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
        assert!(
            run.asked.windows(2).all(|w| w[1] <= w[0] + 1),
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
        move |point: Point| {
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
        move |point: Point| {
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
        move |point: Point| {
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
        move |point: Point| {
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
        move |point: Point| {
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
/// the publish stopped or abandoned on a name that exists.
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
    agent.settle(LIMIT).expect("settled");
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
    let expected: BTreeSet<String> = [25, 26, 27]
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
        let mut first = agent(&world, &keep).with_points(Box::new(move |p| p == asked));
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
            move |_: Point| {
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
