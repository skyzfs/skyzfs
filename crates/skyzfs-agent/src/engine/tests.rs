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
    /// The publishes that pended at the daemon as the agent released its
    /// hold ([`note_released`]).
    released_pending: Vec<String>,
}

/// The point just before the record says a publish is done.
const DONE: Point = Point {
    effect: Effect::Done,
    after: false,
};

/// The point just before the agent releases its hold.
const RELEASE: Point = Point {
    effect: Effect::Release,
    after: false,
};

/// At [`RELEASE`], notes each publish that pends at the daemon: the hold
/// is released only once no request of the publish can still be carried
/// (FL-T2 of #147's review; a run has one publish).
fn note_released(world: &Rc<RefCell<World>>, pending: &Rc<RefCell<Vec<String>>>, point: Point) {
    if point == RELEASE {
        pending.borrow_mut().extend(world.borrow().daemon.pending());
    }
}

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
    // The agent's take, or a person's under its name that it adopted.
    let whole = read.as_ref().is_some_and(|members| {
        let mut read: Vec<u64> = members.iter().map(|m| m.snapshot_guid).collect();
        read.sort_unstable();
        [world.programs.get(t), world.retakes.get(t)]
            .into_iter()
            .flatten()
            .any(|manifest| {
                let mut taken: Vec<u64> =
                    manifest.members.iter().map(|m| m.snapshot_guid).collect();
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
    let released = Rc::new(RefCell::new(Vec::new()));
    match fault {
        Fault::Daemon(point, restart) => world.borrow_mut().die = Some((point, restart)),
        Fault::Cuts(k, restart) | Fault::CutsThenStop(k, restart) => {
            world.borrow_mut().restart_at_cuts = Some((k, restart));
        }
        _ => {}
    }
    let hook = {
        let (count, stopped_at, world) = (count.clone(), stopped_at.clone(), world.clone());
        let (gone, released) = (gone.clone(), released.clone());
        move |point: Point, _: &str| {
            let k = count.get();
            count.set(k + 1);
            note_gone(&world, &gone, point);
            note_released(&world, &released, point);
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
                let (world, gone, released) = (world.clone(), gone.clone(), released.clone());
                move |point: Point, _: &str| {
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
        released_pending: released.take(),
    }
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
///    making no progress (the world always cuts again);
/// 6. no hold was released while a request of its publish pended at the
///    daemon (FL-T2 of #147's review).
fn check(ran: &Ran) -> Result<(), String> {
    let world = ran.world.borrow();
    ran.settled.clone()?;
    if !ran.released_pending.is_empty() {
        return Err(format!(
            "a hold released while a request pended: {:?}",
            ran.released_pending
        ));
    }
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
        // The agent's take, or a person's whole take under its name that
        // landed before the agent adopted it (`World::retakes`).
        if world.programs.get(t) != Some(&manifest) && world.retakes.get(t) != Some(&manifest) {
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
        // A take the agent destroyed itself, not whole or not as read
        // before it: the publish went on under another name (M2.6c-2b).
        if world.log.contains(&format!("agent destroy -r {t}")) {
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
            Why::Destroyed | Why::Rewound | Why::Replaced => !survived,
            Why::Premise => !premise,
            // A person released the hold on some members: `hold -r`
            // refuses until the publish is too old (TL-F1 of #147's
            // review, no repair by the lead's ruling).
            Why::TooOld => world.log.contains(&format!("partly released {t}")),
            // The world always cuts again in time: a publish that makes
            // no progress here is a defect of the engine.
            // Nor does its uploader stop for good in the matrices (it does
            // in `a_stopped_uploader_gives_each_publish_up_at_once`).
            // A take the agent could not adopt is destroyed, and given
            // up kept only where the destroy failed (M2.6c-2b).
            Why::ManifestLost => {
                survived
                    && world
                        .log
                        .iter()
                        .any(|l| l.starts_with(&format!("agent destroy -r {t} refused")))
            }
            Why::ManifestTooLarge
            | Why::NameTaken
            | Why::NoProgress
            | Why::Stopped
            // Three failed checks in a row take a person's action at each
            // take; the matrices make one.
            | Why::NotWhole
            // No person gives a publish up in the matrices.
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
            // A publish whose hold a person partly released is given up
            // too old, and its last answer is reported where the daemon
            // had refused it (in ErrLow, the person's release is the cut
            // with the stale group, and the daemon refuses `rewound`).
            // Only that refusal, and only for a publish given up so
            // (FL-S1 of #147's review).
            Report::Refused(skyzfs_control::Code::Rewound)
                if world.log.contains(&format!("partly released {t}"))
                    && record.as_ref().is_some_and(|r| {
                        r.abandoned
                            .iter()
                            .any(|a| a.name == *t && a.why == Why::TooOld)
                    }) => {}
            Report::NoProgress(_)
            | Report::Refused(_)
            | Report::HeldAway
            | Report::NotWhole(_)
            | Report::Unreadable(_)
            | Report::NotRetired(_)
            | Report::NoGuids => return Err(format!("{t} reported: {report:?}")),
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

/// The fake daemon retires as skyzfsd does: a retire that lands, or
/// finds no `snap/T`, ends what it kept of the publish, so that its
/// progress answers `unknown`; one refused `other` keeps it; a retire is
/// refused `stopped` once the uploader has stopped (D5 and L3 of the
/// next look at #175).
///
/// Mutants: the outcome kept after `retired`; `stopped` not refused.
#[test]
fn the_fake_daemon_retires_as_skyzfsd_does() {
    let ran = run(Mode::Busy, Fault::None);
    let world = ran.world.clone();
    let (t, commit) = {
        let w = world.borrow();
        let (t, r) = w.refs.iter().next().expect("snap/t");
        (t.clone(), r.commit)
    };
    let mut daemon = FakeDaemon(world.clone());
    let mut ask = |request: Request| daemon.ask(&request, &[]).expect("an answer");
    assert_eq!(ask(Request::Progress(t.clone())), Reply::Done(commit));
    let other = ask(Request::Retire(skyzfs_control::Retire {
        name: t.clone(),
        commit: [0xee; 32],
    }));
    assert!(matches!(other, Reply::Refused(Code::Other, _)), "{other:?}");
    assert_eq!(ask(Request::Progress(t.clone())), Reply::Done(commit));
    let retire = Request::Retire(skyzfs_control::Retire {
        name: t.clone(),
        commit,
    });
    assert_eq!(ask(retire.clone()), Reply::Retired);
    assert_eq!(ask(Request::Progress(t.clone())), Reply::Unknown);
    assert_eq!(ask(retire.clone()), Reply::Gone);
    world.borrow_mut().stop_uploader();
    let mut daemon = FakeDaemon(world.clone());
    let stopped = daemon.ask(&retire, &[]).expect("an answer");
    assert!(
        matches!(stopped, Reply::Refused(Code::Stopped, _)),
        "{stopped:?}"
    );
}

/// A retire of a name whose publish pends at the fake daemon is refused
/// `asked`, as skyzfsd refuses it, and leaves the publish pending.
#[test]
fn the_fake_daemon_refuses_a_retire_while_the_publish_pends() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let replies = Rc::new(RefCell::new(Vec::new()));
    let hook = {
        let (world, replies) = (world.clone(), replies.clone());
        move |point: Point, _: &str| {
            if point == ASKED && replies.borrow().is_empty() {
                let pending = world.borrow().daemon.pending();
                for name in pending {
                    let retire = Request::Retire(skyzfs_control::Retire {
                        name: name.clone(),
                        commit: [0xee; 32],
                    });
                    let reply = FakeDaemon(world.clone()).ask(&retire, &[]);
                    let still = world.borrow().daemon.pending().contains(&name);
                    replies.borrow_mut().push((reply, still));
                }
            }
            false
        }
    };
    let mut agent = agent(&world, &keep).with_points(Box::new(hook));
    agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    let replies = replies.borrow();
    assert_eq!(replies.len(), 1, "{replies:?}");
    let (reply, still) = &replies[0];
    assert!(
        matches!(reply, Ok(Reply::Refused(Code::Asked, _))),
        "{reply:?}"
    );
    assert!(still, "the publish no longer pends");
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
    assert_eq!(agent.reports(), [(t.clone(), third.clone()), (t, third)]);
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
            // Given up and told at least once, by one process or the
            // other, and twice only where the first stopped between the
            // save at `Abandoning` and the one at `Abandon` (M2.7b-2).
            let mut outcomes = first.outcomes().to_vec();
            outcomes.extend(second.outcomes().iter().cloned());
            assert!(
                (1..=2).contains(&outcomes.len())
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
    /// The tag cleared on `tank/db@t` only (FL-T1 of #147's review).
    ReleasedChild,
}

/// D5 (the lead's order for a request asked anew; T2 of #144's review):
/// `load_guid`, then the members and the hold, then the count, then
/// `load_guid` again, and only then the request. Where the hold or the
/// snapshot changed after the round that planned it, nothing is counted
/// or asked: the next round sees the change.
///
/// Mutant: the count read before the members and the hold are checked,
/// either check left out, or the hold taken as whole where one member
/// still has the tag.
#[test]
fn asking_anew_checks_the_snapshot_and_its_hold_before_the_count() {
    for meddle in [
        Meddle::None,
        Meddle::Released,
        Meddle::Replaced,
        Meddle::ReleasedChild,
    ] {
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
                    Meddle::ReleasedChild if *name == format!("{POOL}/db{suffix}") => {
                        snap.holds.clear();
                    }
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
            Meddle::Released | Meddle::Replaced | Meddle::ReleasedChild => {
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
        released_pending: Vec::new(),
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
                Effect::Take if point.after => refused.set(true),
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
    assert!(
        !world.log.iter().any(|l| l.starts_with("agent destroy")),
        "a name someone else holds is not destroyed: {:?}",
        world.log
    );
}

/// What one publish left, run by [`one`].
struct One {
    world: Rc<RefCell<World>>,
    name: String,
    outcomes: Vec<(String, Outcome)>,
}

impl One {
    /// The publish ended published under `t`, with no hold left and
    /// nothing under way, and its manifest is the take's truth.
    fn published_as(&self, t: &str) {
        assert!(
            matches!(self.outcomes.as_slice(), [.., (n, Outcome::Published(_))] if n == t),
            "{:?}\n{}",
            self.outcomes,
            self.world.borrow().log.join("\n")
        );
        let world = self.world.borrow();
        let published = world.refs.get(t).expect("snap/t");
        let manifest = Manifest::decode(&published.manifest).expect("its manifest");
        assert_eq!(world.programs.get(t), Some(&manifest), "the take's truth");
        assert!(holds(&world).is_empty());
    }

    /// The publish ended published under a name other than its first,
    /// as [`One::published_as`]; that name.
    fn published_anew(&self) -> String {
        let Some((t, _)) = self.outcomes.last() else {
            panic!("no outcome");
        };
        assert_ne!(*t, self.name, "published under its first name");
        self.published_as(t);
        t.clone()
    }

    /// Whether the agent destroyed its own take of `t`.
    fn destroyed(&self, t: &str) -> bool {
        self.world
            .borrow()
            .log
            .contains(&format!("agent destroy -r {t}"))
    }
}

/// One publish in a busy world set up by `set`, with `hook` at the
/// agent's points (true stops it), then a new agent, as after a crash,
/// where the first stopped.
fn one(set: impl FnOnce(&mut World), hook: impl FnMut(Point) -> bool + 'static) -> One {
    let mut hook = hook;
    one_with(set, |_| Box::new(move |p: Point, _: &str| hook(p)))
}

/// [`one`], the hook made with the world.
fn one_with(set: impl FnOnce(&mut World), hook: impl FnOnce(&Rc<RefCell<World>>) -> Points) -> One {
    let world = World::new(Mode::Busy);
    set(&mut world.borrow_mut());
    let keep = MemKeep::default();
    let mut first = agent(&world, &keep).with_points(hook(&world));
    let begun = first.begin();
    let name = match &begun {
        Ok(name) => name.clone(),
        Err(_) => first
            .record()
            .under_way
            .first()
            .map(|p| p.name.clone())
            .expect("a publish under way"),
    };
    let settled = begun.and_then(|_| first.settle(LIMIT));
    let mut outcomes = first.outcomes().to_vec();
    drop(first);
    match settled {
        Ok(()) => {}
        Err(Error::Stopped) => {
            world.borrow_mut().advance(DOWN);
            let mut again = agent(&world, &keep);
            again.settle(LIMIT).expect("settled after the restart");
            outcomes.extend(again.outcomes().iter().cloned());
        }
        Err(e) => panic!("{e:?}"),
    }
    One {
        world,
        name,
        outcomes,
    }
}

/// The point at `effect`, before or after it.
fn at(effect: Effect, after: bool) -> Point {
    Point { effect, after }
}

/// Each round begins with the count of the pool's datasets, which sets
/// the limits of its calls (the lead's rule on M2.6c-2b), then the sweep:
/// the first count comes before the sweep's first read of the holds,
/// each is followed by the sweep's read of the pool, and every round
/// counts. Mutants: no count, a count after the sweep, one count for all
/// the rounds.
#[test]
fn each_round_counts_the_datasets_before_its_calls() {
    let one = one(|_| {}, |_| false);
    one.published_as(&one.name);
    let world = one.world.borrow();
    let calls = &world.calls;
    let first = calls.iter().position(|c| *c == "count");
    let tagged = calls.iter().position(|c| *c == "tagged");
    assert!(first.is_some() && first < tagged, "{calls:?}");
    let counts: Vec<usize> = (0..calls.len()).filter(|&i| calls[i] == "count").collect();
    assert!(
        counts.iter().all(|&i| calls.get(i + 1) == Some(&"props")),
        "{calls:?}"
    );
    let takes = calls.iter().filter(|c| **c == "snapshot").count();
    let held = calls.iter().filter(|c| **c == "held").count();
    // A round to take, then one at least for each hold read after it.
    assert!(counts.len() > held && held >= 1 && takes == 1, "{calls:?}");
}

/// A take killed at its limit lands in its own time (the `zfs snapshot
/// -r` ioctl is not cancelled by killing its client): the next round
/// finds `pool@t` and adopts it, under its name, with nothing destroyed
/// (the lead's rule for an `EEXIST` at the take, M2.6c-2b). Landed before
/// the command's error, the next round plans [`Action::Adopt`]; landed
/// only as the next take is asked, that take says `EEXIST` and the
/// agent adopts the first. Mutant: the next name taken at a late take,
/// or the late take destroyed.
#[test]
fn a_take_killed_at_its_limit_is_adopted_when_it_lands() {
    for (late, how) in [(0, "before the error"), (usize::MAX, "at the next take")] {
        let one = one(|w| w.late_take = Some(late), |_| false);
        one.published_as(&one.name);
        assert!(!one.destroyed(&one.name), "{how}");
        let world = one.world.borrow();
        assert!(
            world
                .log
                .contains(&format!("late take of {} landed: Ok(())", one.name)),
            "{how}: {:?}",
            world.log
        );
        let snapshots = world.calls.iter().filter(|c| **c == "snapshot").count();
        assert_eq!(snapshots, if late == 0 { 1 } else { 2 }, "{how}");
    }
}

/// The agent stopped between the record at Intent and the take takes it
/// under the same name in the next process. Stopped after the take and
/// before the record says so, the next process has no reading of the pool
/// from before the take, so it cannot tell the properties read after it
/// for the take's: it destroys the take, and the publish takes the next
/// name. Mutant: the take adopted with no reading from before it, or
/// given up.
#[test]
fn a_crash_around_the_take_takes_again_or_destroys_and_retakes() {
    let stopped = |point: Point| {
        let mut done = false;
        move |p: Point| {
            let stop = p == point && !done;
            done |= stop;
            stop
        }
    };
    let one_ = one(|_| {}, stopped(at(Effect::Intent, true)));
    one_.published_as(&one_.name);
    assert!(!one_.destroyed(&one_.name));

    let one_ = one(|_| {}, stopped(at(Effect::Take, false)));
    one_.published_as(&one_.name);
    assert!(!one_.destroyed(&one_.name));

    let one_ = one(|_| {}, stopped(at(Effect::Take, true)));
    assert!(one_.destroyed(&one_.name));
    one_.published_anew();
    let world = one_.world.borrow();
    let pool = world.pool.as_ref().expect("the pool");
    assert!(!pool
        .snaps
        .keys()
        .any(|k| k.ends_with(&format!("@{}", one_.name))));
}

/// A property of a member set while the take is under way: the
/// properties read after the take are not the take's, so the take is
/// destroyed and the publish takes the next name, whose manifest has the
/// property as it was at that take. Set before the take is asked, or
/// after it and before it is read. Mutant: the members not compared with
/// the reading before the take.
#[test]
fn a_property_set_around_the_take_has_it_taken_again() {
    for after in [false, true] {
        let mut set = false;
        let one = one_with(
            |_| {},
            move |world| {
                let world = world.clone();
                Box::new(move |p: Point, _: &str| {
                    if p == at(Effect::Take, after) && !set {
                        set = true;
                        world.borrow_mut().user(User::SetProperty, "");
                    }
                    false
                })
            },
        );
        assert!(one.destroyed(&one.name), "after {after}");
        let next = one.published_anew();
        let world = one.world.borrow();
        let db = &world.programs[&next].members[1];
        assert_eq!(
            db.local.get(fake::NOTE).map(Vec::as_slice),
            Some(&b"1"[..]),
            "{}",
            db.name
        );
    }
}

/// A take the agent must destroy that cannot be destroyed (a person's
/// hold on `pool@t`) gives the publish up as [`Why::ManifestLost`], the
/// snapshot kept and unpublished (the lead's ruling, M2.6c-2b): it is
/// not taken again under another name while its `@t`s are left in the
/// pool, nor tried again. Mutants: the next name taken after a destroy
/// that failed; the destroy tried again.
#[test]
fn a_take_that_cannot_be_destroyed_is_given_up_and_kept() {
    let mut set = false;
    let one = one_with(
        |_| {},
        move |world| {
            let world = world.clone();
            Box::new(move |p: Point, _: &str| {
                let mut world = world.borrow_mut();
                if p == at(Effect::Take, true) && !set {
                    set = true;
                    // The properties change around the take, so the
                    // agent destroys it; a person holds `pool@t`.
                    world.user(User::SetProperty, "");
                    let t = world.programs.keys().last().cloned().unwrap_or_default();
                    if let Some(snap) = world
                        .pool
                        .as_mut()
                        .and_then(|p| p.snaps.get_mut(&format!("{POOL}@{t}")))
                    {
                        snap.holds.insert("theirs".into());
                    }
                }
                false
            })
        },
    );
    let world = one.world.borrow();
    let destroy = format!("agent destroy -r {}", one.name);
    let refused = world
        .log
        .iter()
        .filter(|l| l.starts_with(&format!("{destroy} refused")))
        .count();
    assert_eq!(refused, 1, "refused once, not tried again: {:?}", world.log);
    assert!(!world.log.contains(&destroy), "{:?}", world.log);
    assert_eq!(
        one.outcomes,
        [(one.name.clone(), Outcome::Abandoned(Why::ManifestLost))],
        "{:?}",
        world.log
    );
    assert!(world.refs.is_empty(), "nothing published");
    let pool = world.pool.as_ref().expect("the pool");
    assert!(
        pool.snaps.contains_key(&format!("{POOL}@{}", one.name)),
        "the snapshot kept"
    );
}

/// A crash at each side of the destroy of a take not as read before it:
/// stopped before the destroy, the next process has no reading from
/// before the take, destroys it and takes the next name; stopped after
/// it, the record still holds the name at Intent with no `pool@t`, and
/// the next process takes it again under that name. Mutants: the take
/// adopted with no reading from before it; the destroy taken for
/// nothing to do.
#[test]
fn a_crash_around_the_destroy_takes_again() {
    for after in [false, true] {
        let (mut set, mut stopped) = (false, false);
        let one = one_with(
            |_| {},
            move |world| {
                let world = world.clone();
                Box::new(move |p: Point, _: &str| {
                    if p == at(Effect::Take, true) && !set {
                        set = true;
                        world.borrow_mut().user(User::SetProperty, "");
                    }
                    let stop = p == at(Effect::Destroy, after) && !stopped;
                    stopped |= stop;
                    stop
                })
            },
        );
        assert!(one.destroyed(&one.name), "{after}");
        if after {
            one.published_as(&one.name);
        } else {
            one.published_anew();
        }
    }
}

/// A late take whose members a person changed before it was adopted (a
/// member's `@t` destroyed) is not one whole take of the pool: the agent
/// destroys what is left of it and takes the next name. Mutant: a take
/// with a member missing adopted.
#[test]
fn a_late_take_not_whole_is_destroyed_and_taken_again() {
    let mut done = false;
    let one = one_with(
        |w| w.late_take = Some(0),
        move |world| {
            let world = world.clone();
            Box::new(move |p: Point, _: &str| {
                if p == at(Effect::Take, true) && !done {
                    done = true;
                    let t = world.borrow().programs.keys().last().cloned();
                    world
                        .borrow_mut()
                        .user(User::DestroyChild, &t.unwrap_or_default());
                }
                false
            })
        },
    );
    assert!(one.destroyed(&one.name));
    one.published_anew();
}

/// A read of the datasets that fails, before the take or after it, is
/// reported with its error, which names the dataset and the property,
/// and the publish goes on at a later round (the lead's ruling on
/// M2.6c-5, option (a)): here the read before the take fails twice with
/// one error and the read after it once with another, and the publish
/// is made. The same error is reported once in a process. Mutants: no
/// report before the take or after it, the error not carried, the
/// publish given up or stopped on it, the take adopted without its
/// read.
#[test]
fn a_read_that_fails_is_reported_and_read_again() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().unreadable = [(0, "user:odd"), (1, "user:odd"), (3, "user:late")]
        .into_iter()
        .map(|(read, property)| (read, property.to_string()))
        .collect();
    let keep = MemKeep::default();
    let mut agent = agent(&world, &keep);
    agent.begin().expect("begun");
    if let Err(e) = agent.settle(LIMIT) {
        panic!(
            "{e}: {:?}\n{}",
            agent.outcomes(),
            world.borrow().log.join("\n")
        );
    }
    let [(t, Outcome::Published(_))] = agent.outcomes() else {
        panic!("{:?}\n{}", agent.outcomes(), world.borrow().log.join("\n"));
    };
    let [(first, Report::Unreadable(before)), (second, Report::Unreadable(after))] =
        agent.reports()
    else {
        panic!("{:?}", agent.reports());
    };
    assert_eq!((first, second), (t, t));
    for (why, property) in [(before, "`user:odd`"), (after, "`user:late`")] {
        assert!(
            why.contains(property) && why.contains(&format!("`{POOL}/vol`")),
            "{why}"
        );
    }
    let world = world.borrow();
    assert!(world.datasets_read >= 5, "{}", world.datasets_read);
    assert!(world.refs.contains_key(t));
}

/// What a run of one publish on a pool whose `pool/vol` every take
/// skips left: the agent's outcomes and reports, the reports given to
/// the sink, the world's time at each take asked, and the processes it
/// took.
struct NeverWhole {
    world: Rc<RefCell<World>>,
    outcomes: Vec<(String, Outcome)>,
    reports: Vec<(String, Report)>,
    told: Vec<(String, Report)>,
    takes_ms: Vec<u64>,
    processes: usize,
}

/// The processes [`never_whole`] runs at most.
const PROCESSES_MAX: usize = 20;

/// One publish on a pool whose `pool/vol` `zfs snapshot -r` skips (an
/// inconsistent dataset: a full `zfs receive` into it under way), with
/// `hook` at the agent's points, given the process's number from 0;
/// where it stops, a new agent goes on, up to [`PROCESSES_MAX`].
fn never_whole(
    hook: impl FnMut(&Rc<RefCell<World>>, usize, Point) -> bool + 'static,
) -> NeverWhole {
    never_whole_in(
        |world| world.skip = BTreeSet::from([format!("{POOL}/vol")]),
        hook,
    )
}

/// [`never_whole`] on a world that `setup` makes.
fn never_whole_in(
    setup: impl FnOnce(&mut World),
    hook: impl FnMut(&Rc<RefCell<World>>, usize, Point) -> bool + 'static,
) -> NeverWhole {
    let world = World::new(Mode::Busy);
    setup(&mut world.borrow_mut());
    let keep = MemKeep::default();
    let takes_ms = Rc::new(RefCell::new(Vec::new()));
    let told = Rc::new(RefCell::new(Vec::new()));
    let hook = Rc::new(RefCell::new(hook));
    let (mut outcomes, mut reports) = (Vec::new(), Vec::new());
    let mut processes = 0;
    loop {
        assert!(processes < PROCESSES_MAX, "{:?}", world.borrow().log);
        let process = processes;
        processes += 1;
        let points: Points = Box::new({
            let (world, takes_ms, hook) = (world.clone(), takes_ms.clone(), hook.clone());
            move |p: Point, _: &str| {
                if p == at(Effect::Take, false) {
                    takes_ms.borrow_mut().push(world.borrow().now_ms);
                }
                (hook.borrow_mut())(&world, process, p)
            }
        });
        let sink: Sink = Box::new({
            let told = told.clone();
            move |name: &str, report: &Report| {
                told.borrow_mut().push((name.to_string(), report.clone()));
            }
        });
        let mut agent = agent(&world, &keep).with_points(points).with_sink(sink);
        let settled = if process == 0 {
            agent.begin().and_then(|_| agent.settle(LIMIT))
        } else {
            agent.settle(LIMIT)
        };
        outcomes.extend(agent.outcomes().iter().cloned());
        reports.extend(agent.reports().iter().cloned());
        drop(agent);
        match settled {
            Ok(()) => break,
            Err(Error::Stopped) => world.borrow_mut().advance(DOWN),
            Err(e) => panic!("{e:?}"),
        }
    }
    let takes_ms = takes_ms.borrow().clone();
    let told = told.borrow().clone();
    NeverWhole {
        world,
        outcomes,
        reports,
        told,
        takes_ms,
        processes,
    }
}

/// A take that is never whole (a dataset `zfs snapshot -r` skips) is
/// destroyed and taken again at most twice, each after
/// [`plan::RETAKE_PAUSE`], and the third check that fails gives the
/// publish up as [`Why::NotWhole`], its take destroyed and the dataset
/// named in the report (the lead's ruling on D1 of #153's review).
/// Mutants: no bound (the loop the review found), the bound off by one
/// either way, no pause or a shorter one, the last take kept, no report
/// or another dataset named, the failed checks not counted in the
/// record.
#[test]
fn a_take_never_whole_is_given_up_at_the_third_failed_check() {
    let run = never_whole(|_, _, _| false);
    let world = run.world.borrow();
    let [(t, outcome)] = run.outcomes.as_slice() else {
        panic!("{:?}\n{}", run.outcomes, world.log.join("\n"));
    };
    assert_eq!(
        *outcome,
        Outcome::Abandoned(Why::NotWhole),
        "{}",
        world.log.join("\n")
    );
    assert_eq!(
        run.reports,
        [(t.clone(), Report::NotWhole(vec![format!("{POOL}/vol")]))]
    );
    assert_eq!(
        run.takes_ms.len(),
        plan::CHECKS_FAILED_MAX as usize,
        "{:?}",
        world.log
    );
    // The assist's 10 s (the lead's ruling).
    let pause = 10_000;
    assert!(
        run.takes_ms.windows(2).all(|w| w[1] - w[0] >= pause),
        "{:?}",
        run.takes_ms
    );
    let destroyed = world
        .log
        .iter()
        .filter(|l| l.starts_with("agent destroy -r ") && !l.contains("refused"))
        .count();
    assert_eq!(destroyed, 3, "each take destroyed: {:?}", world.log);
    let pool = world.pool.as_ref().expect("the pool");
    assert!(pool.snaps.is_empty(), "{:?}", pool.snaps.keys());
    assert!(world.refs.is_empty() && holds(&world).is_empty());
}

/// The give-up's report reaches the sink at least once, whatever the
/// stops about the give-up: after the third failed check is recorded,
/// about the destroy of its take, about the give-up's record and about
/// the manifest's file removed (T12 of #153's review), and its outcome
/// line once, or twice where the stop fell between the give-up's two
/// saves (M2.7b-2), never lost. Both kinds of
/// failed check: a take that is not whole is judged again by the next
/// process, and one that is not as read before is destroyed by it
/// unjudged, so that the give-up is made with no take
/// ([`Agent::take`]), its report naming nothing; that process is
/// stopped again about its own give-up. Mutants: no report at the
/// give-up with no take, the report kept from the sink, or made after
/// the give-up's record, with a take or with none.
#[test]
fn a_give_up_is_told_at_least_once_whatever_the_stop() {
    let about = [
        at(Effect::Destroy, false),
        at(Effect::Destroy, true),
        at(Effect::Abandoning, false),
        at(Effect::Abandoning, true),
        at(Effect::Abandon, false),
        at(Effect::Abandon, true),
        at(Effect::Unlink, false),
        at(Effect::Unlink, true),
    ];
    let mut cases: Vec<(bool, Vec<Point>)> = Vec::new();
    for not_as_before in [false, true] {
        cases.push((not_as_before, vec![at(Effect::Failed, true)]));
        for stop in about {
            cases.push((not_as_before, vec![stop]));
        }
    }
    for stop in about.into_iter().filter(|p| p.effect != Effect::Destroy) {
        cases.push((true, vec![at(Effect::Failed, true), stop]));
    }
    for (not_as_before, stops) in cases {
        let (mut failed, mut left) = (0, stops.clone());
        left.reverse();
        let run = never_whole_in(
            move |world| {
                if !not_as_before {
                    world.skip = BTreeSet::from([format!("{POOL}/vol")]);
                }
            },
            move |world, _, p| {
                if not_as_before && p == at(Effect::Take, false) {
                    world.borrow_mut().after_take = Some(User::SetProperty);
                }
                if p == at(Effect::Failed, true) {
                    failed += 1;
                }
                if failed >= plan::CHECKS_FAILED_MAX && left.last() == Some(&p) {
                    left.pop();
                    return true;
                }
                false
            },
        );
        let world = run.world.borrow();
        let case = format!(
            "not as before {not_as_before}, stops {stops:?}: {} processes, {:?} {:?}\n{}",
            run.processes,
            run.outcomes,
            run.told,
            world.log.join("\n")
        );
        assert_eq!(run.processes, stops.len() + 1, "{case}");
        // The give-up is told after it is recorded at `Abandoning` and
        // before it moves to the names given up (M2.7b-2): told once,
        // and twice only where a process stopped between the two, whose
        // successor tells it again from the record. Never lost.
        let twice = stops.last() == Some(&at(Effect::Abandon, false));
        let want = if twice { 2 } else { 1 };
        assert_eq!(run.outcomes.len(), want, "{case}");
        assert!(
            run.outcomes
                .iter()
                .all(|(_, o)| *o == Outcome::Abandoned(Why::NotWhole)),
            "{case}"
        );
        let [(_, Report::NotWhole(_)), ..] = run.told.as_slice() else {
            panic!("{case}");
        };
        assert!(world.refs.is_empty(), "{case}");
    }
}

/// The report of a give-up names every dataset that broke the last
/// take, in the order read (T8 of #153's review): here two that
/// `zfs snapshot -r` skips. Mutants: one name only, or another order.
#[test]
fn a_take_left_out_by_two_datasets_names_both() {
    let run = never_whole_in(
        |world| world.skip = BTreeSet::from([format!("{POOL}/db"), format!("{POOL}/vol")]),
        |_, _, _| false,
    );
    let world = run.world.borrow();
    let [(t, outcome)] = run.outcomes.as_slice() else {
        panic!("{:?}\n{}", run.outcomes, world.log.join("\n"));
    };
    assert_eq!(*outcome, Outcome::Abandoned(Why::NotWhole));
    assert_eq!(
        run.reports,
        [(
            t.clone(),
            Report::NotWhole(vec![format!("{POOL}/db"), format!("{POOL}/vol")])
        )]
    );
}

/// The report of a give-up whose last take was whole but not as read
/// before it names the datasets that differ (T8 of #153's review): a
/// member whose property a person set as each take landed, then a
/// dataset read before the third take and destroyed as it landed, which
/// no member is. Each take is a failed check, the third gives the
/// publish up. Mutants: the members that differ not named, the datasets
/// with no member not named, the two in the other order, or nothing
/// named for a take not as before.
#[test]
fn a_take_not_as_before_names_the_datasets_that_differ() {
    let mut takes = 0;
    let run = never_whole_in(
        |_| {},
        move |world, _, p| {
            if p == at(Effect::Take, false) {
                world.borrow_mut().after_take = Some(User::SetProperty);
            }
            if p == at(Effect::Take, true) {
                takes += 1;
                if takes == plan::CHECKS_FAILED_MAX {
                    let mut world = world.borrow_mut();
                    let t = world.programs.keys().last().cloned().unwrap_or_default();
                    world.user(User::DestroyDataset, &t);
                }
            }
            false
        },
    );
    let world = run.world.borrow();
    let [(t, outcome)] = run.outcomes.as_slice() else {
        panic!("{:?}\n{}", run.outcomes, world.log.join("\n"));
    };
    assert_eq!(
        *outcome,
        Outcome::Abandoned(Why::NotWhole),
        "{}",
        world.log.join("\n")
    );
    assert_eq!(run.takes_ms.len(), plan::CHECKS_FAILED_MAX as usize);
    assert_eq!(
        run.reports,
        [(
            t.clone(),
            Report::NotWhole(vec![format!("{POOL}/db"), format!("{POOL}/vol")])
        )]
    );
}

/// The give-up of [`a_take_never_whole_is_given_up_at_the_third_failed_check`]
/// where the third take cannot be destroyed (a person's hold on
/// `pool@t`): given up as [`Why::ManifestLost`], the snapshot kept,
/// with the report made. Mutant: the give-up taken for done where the
/// destroy failed.
#[test]
fn a_take_never_whole_that_cannot_be_destroyed_is_given_up_kept() {
    let mut takes = 0;
    let run = never_whole(move |world, _, p| {
        if p == at(Effect::Take, true) {
            takes += 1;
            if takes == plan::CHECKS_FAILED_MAX {
                let mut world = world.borrow_mut();
                let t = world.programs.keys().last().cloned().unwrap_or_default();
                if let Some(snap) = world
                    .pool
                    .as_mut()
                    .and_then(|p| p.snaps.get_mut(&format!("{POOL}@{t}")))
                {
                    snap.holds.insert("theirs".into());
                }
            }
        }
        false
    });
    let world = run.world.borrow();
    let [(t, outcome)] = run.outcomes.as_slice() else {
        panic!("{:?}\n{}", run.outcomes, world.log.join("\n"));
    };
    assert_eq!(*outcome, Outcome::Abandoned(Why::ManifestLost));
    assert_eq!(
        run.reports,
        [(t.clone(), Report::NotWhole(vec![format!("{POOL}/vol")]))]
    );
    let pool = world.pool.as_ref().expect("the pool");
    assert!(pool.snaps.contains_key(&format!("{POOL}@{t}")), "kept");
}

/// A whole take retaken where the process has no reading from before it
/// (a restart between the take and its record) was not judged, and is
/// not counted: where every later take is not whole, the publish is
/// given up at the third failed check, after four takes. The
/// conservative reading ([`Agent::failed`]). A take not whole is
/// counted with or without a reading (the first test). Mutant: every
/// retake counted.
#[test]
fn a_take_retaken_after_a_restart_is_not_counted_as_failed() {
    let mut stopped = false;
    let run = never_whole(move |world, _, p| {
        if stopped {
            return false;
        }
        if p == at(Effect::Take, false) {
            // The first take is whole; the dataset is skipped from the
            // next.
            world.borrow_mut().skip.clear();
        }
        let stop = p == at(Effect::Take, true);
        if stop {
            world.borrow_mut().skip = BTreeSet::from([format!("{POOL}/vol")]);
        }
        stopped |= stop;
        stop
    });
    let world = run.world.borrow();
    assert!(
        matches!(
            run.outcomes.as_slice(),
            [(_, Outcome::Abandoned(Why::NotWhole))]
        ),
        "{:?}\n{}",
        run.outcomes,
        world.log.join("\n")
    );
    assert_eq!(run.takes_ms.len(), 4, "{:?}", world.log);
}

/// A stop at the same point of every process, from the first take on,
/// for [`STOPPING`] processes, then a process with no stop: at each
/// side of the count of a failed check, of the destroy of the take and
/// of the record of the next name, the publish is still given up as
/// [`Why::NotWhole`] with at most [`plan::CHECKS_FAILED_MAX`] takes,
/// nothing left in the pool, and every report a [`Report::NotWhole`]
/// (T9 of #153's next look). Mutants: the count after the destroy (D2:
/// a take more each time a process stops after a destroy); no give-up
/// without a take after the last destroy (the same, at the last check).
#[test]
fn a_take_never_whole_is_given_up_whatever_point_each_process_stops_at() {
    /// The processes that stop.
    const STOPPING: usize = 8;
    let mut found = Vec::new();
    for effect in [Effect::Failed, Effect::Destroy, Effect::Intent] {
        for after in [false, true] {
            let (mut taken, mut stops) = (false, 0);
            let mut stopped_in = None;
            let stops_seen = Rc::new(Cell::new(0));
            let seen = stops_seen.clone();
            let run = never_whole(move |_, process, p| {
                taken |= p == at(Effect::Take, true);
                let stop = taken
                    && process < STOPPING
                    && stopped_in != Some(process)
                    && p == at(effect, after);
                if stop {
                    stopped_in = Some(process);
                    stops += 1;
                    seen.set(stops);
                }
                stop
            });
            let world = run.world.borrow();
            let pool = world.pool.as_ref().expect("the pool");
            let line = format!(
                "{effect:?} after={after}: {} takes, {} processes, {} stops, {:?}, {:?}",
                run.takes_ms.len(),
                run.processes,
                stops_seen.get(),
                run.outcomes,
                run.reports
            );
            let given_up = matches!(
                run.outcomes.as_slice(),
                [(_, Outcome::Abandoned(Why::NotWhole))]
            );
            let reported = !run.reports.is_empty()
                && run
                    .reports
                    .iter()
                    .all(|(_, r)| matches!(r, Report::NotWhole(_)));
            // The course: the point was reached and stopped at, twice at
            // least.
            if !given_up
                || !reported
                || run.takes_ms.len() > plan::CHECKS_FAILED_MAX as usize
                || !pool.snaps.is_empty()
                || !world.refs.is_empty()
                || !holds(&world).is_empty()
                || stops_seen.get() < 2
            {
                found.push(line);
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}

/// A dataset renamed after the take and before its adoption: the members
/// read after the take are not the datasets read before it (by name), so
/// the take is destroyed and the publish takes the next name, whose
/// manifest names the dataset as renamed (T1 of #153's review, P3).
/// Mutant: the names not compared.
#[test]
fn a_dataset_renamed_after_the_take_has_it_taken_again() {
    let one = one(|w| w.after_take = Some(User::Rename), |_| false);
    assert!(one.destroyed(&one.name));
    let next = one.published_anew();
    let world = one.world.borrow();
    let names: Vec<&str> = world.programs[&next]
        .members
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert!(names.contains(&format!("{POOL}/db2").as_str()), "{names:?}");
}

/// A dataset destroyed with its snapshots after the take and before its
/// adoption: what is left is one whole take of the pool as it is now,
/// but not of the datasets read before it (one fewer), so it is
/// destroyed and the publish takes the next name (T1 of #153's review,
/// P5). Mutant: the count of the members not compared.
#[test]
fn a_dataset_destroyed_after_the_take_has_it_taken_again() {
    let one = one(|w| w.after_take = Some(User::DestroyDataset), |_| false);
    assert!(one.destroyed(&one.name));
    let next = one.published_anew();
    let world = one.world.borrow();
    assert_eq!(world.programs[&next].members.len(), 2);
}

/// A take killed at its limit that lands at each of the agent's later
/// calls to ZFS, a property set as soon as it lands: whatever call it
/// lands at, what is published is the take's truth. Landed at the
/// reading before the next take, that reading is not of the pool before
/// a take, and the take is adopted against the reading kept from before
/// the first (T1 of #153's review, P7). Mutant: no early return in
/// `Agent::take` where the reading shows the take landed.
#[test]
fn a_late_take_landing_at_any_call_publishes_its_truth() {
    for late in 0..12 {
        let one = one(
            |w| {
                w.late_take = Some(late);
                w.after_take = Some(User::SetProperty);
            },
            |_| false,
        );
        let Some((t, Outcome::Published(_))) = one.outcomes.last() else {
            panic!(
                "{late}: {:?}\n{}",
                one.outcomes,
                one.world.borrow().log.join("\n")
            );
        };
        one.published_as(t);
    }
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
            released_pending: Vec::new(),
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

    fn failure(&mut self) -> io::Result<Option<Noted>> {
        self.0.failure()
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
        // The round's own reads of the pool, which are not for this
        // publish: its count (the limits) and the sweep's read of the
        // holds (M2.6c-2b's fake records both).
        .filter(|c| !matches!(*c, "props" | "count" | "tagged"))
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
            let root = world.pool.as_ref().map(|p| p.datasets[0].name.clone());
            let snap = format!("{}@{name}", root.unwrap_or_default());
            match (point.effect, point.after) {
                (Effect::Take, true) => {
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

/// [`Effect::ALL`] holds every effect once, in the order declared (T13
/// of #153's review: an effect left out of it would be refused by
/// `--kill-at`). The match below has no wildcard, so an effect added to
/// the enum does not compile here until it is given its place.
#[test]
fn every_effect_is_in_the_list_of_all() {
    fn place(effect: Effect) -> usize {
        match effect {
            Effect::Enrol => 0,
            Effect::Intent => 1,
            Effect::Take => 2,
            Effect::Failed => 3,
            Effect::Destroy => 4,
            Effect::Manifest => 5,
            Effect::Taken => 6,
            Effect::Holding => 7,
            Effect::Hold => 8,
            Effect::Held => 9,
            Effect::Count => 10,
            Effect::Stalled => 11,
            Effect::Asking => 12,
            Effect::Ask => 13,
            Effect::Asked => 14,
            Effect::Unasked => 15,
            Effect::Withdraw => 16,
            Effect::Look => 17,
            Effect::Assist => 18,
            Effect::Release => 19,
            Effect::Released => 20,
            Effect::Done => 21,
            Effect::Sweep => 22,
            Effect::Forget => 23,
            Effect::Abandoning => 24,
            Effect::Abandon => 25,
            Effect::Unlink => 26,
            Effect::Retiring => 27,
            Effect::Retire => 28,
            Effect::Retired => 29,
            Effect::LookGivenUp => 30,
            Effect::DestroyMember => 31,
            Effect::Retain => 32,
            Effect::Stop => 33,
            Effect::Recover => 34,
        }
    }
    for (at, effect) in Effect::ALL.into_iter().enumerate() {
        assert_eq!(place(effect), at, "{effect:?}");
    }
    assert_eq!(Effect::ALL.len(), place(Effect::Recover) + 1);
}

/// A person releases the agent's hold on `pool/db@t` once it is held:
/// the hold left on the root alone is no hold to go on with. The publish
/// is not asked, `hold -r` refuses until it is too old, and it is then
/// given up with the root's tag released (TL-F1 of #147's review).
/// Mutants: a part taken as held, in the adapter's fake or the planner
/// (the publish is published on a hold that is not whole; F1e of the
/// review), or as no hold where it ends (the root's tag left).
#[test]
fn a_hold_a_person_partly_released_is_given_up_and_released() {
    for mode in MODES {
        let world = World::new(mode);
        let keep = MemKeep::default();
        let name = Rc::new(RefCell::new(String::new()));
        let held = Point {
            effect: Effect::Held,
            after: true,
        };
        let hook = {
            let (world, name) = (world.clone(), name.clone());
            move |p: Point, _: &str| {
                if p == held {
                    world.borrow_mut().user(User::ReleaseChild, &name.borrow());
                }
                false
            }
        };
        let mut agent = agent(&world, &keep).with_points(Box::new(hook));
        *name.borrow_mut() = agent.begin().expect("begun");
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
            released_pending: Vec::new(),
        };
        if let Err(why) = check(&ran) {
            panic!("{}", failed(&format!("{mode:?}"), &ran, &why));
        }
        let name = name.borrow().clone();
        assert!(
            world
                .borrow()
                .log
                .contains(&format!("partly released {name}")),
            "{mode:?}: the hook did not release"
        );
        assert_eq!(
            ran.outcomes,
            vec![(name, Outcome::Abandoned(Why::TooOld))],
            "{mode:?}"
        );
    }
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

/// The sweep reads only the holds on snapshots named as the agent names
/// them ([`crate::names::PREFIX`]): a hold of the agent's tag on a
/// snapshot of another name (a person's hold, or a snapshot of the
/// agent's renamed) is left, and does not stop the sweep, which releases
/// the stray hold on a name of the agent's beside it (A4 of #165's
/// review, TG2 of #168's). Mutants of the fake's filter (the engine's
/// sweep has none: the CLI's own tests hold `Zfs::tagged`'s): dropped,
/// matched without its case or without its dash, applied to the pool's
/// own snapshots only, or matched anywhere in the name (A9e of the next
/// look at #168).
#[test]
fn the_sweep_leaves_a_hold_of_the_tag_on_a_name_not_the_agents() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    keep.0.borrow_mut().marked = true;
    let others = [
        format!("{POOL}@keep"),
        format!("{POOL}/db@SKZ-1"),
        format!("{POOL}@skz"),
        format!("{POOL}@keep-skz-1"),
    ];
    let stray = format!("{POOL}@skz-stray");
    {
        let mut world = world.borrow_mut();
        let pool = world.pool.as_mut().expect("the pool");
        for name in others.iter().chain([&stray]) {
            let snap = fake::Snap {
                guid: 0xbeef,
                createtxg: pool.txg,
                holds: [HOLD_TAG.to_string()].into(),
                held_in: None,
            };
            pool.snaps.insert(name.clone(), snap);
        }
    }
    let mut agent = agent(&world, &keep);
    agent.settle(LIMIT).expect("settled");
    let mut left = others.to_vec();
    left.sort();
    assert_eq!(holds(&world.borrow()), left);
}

/// The record says a publish is asked, and is saved, before `publish` is
/// sent (the lead's decision on L1 of #149's review), so that a publish
/// recorded held has no request out. A crash between the save and the
/// send leaves it asked with nothing asked of the daemon; a person's
/// give-up then withdraws, looks and releases, and nothing is
/// published. A refusal that says nothing pends records it held again;
/// one that does not (`asked`, `ended`, `no-answer`) leaves it asked.
/// Mutants: the save after the send; held again on every refusal; never
/// held again; the asking `held_until` without the channel's wait.
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
            // The channel's wait counts too: the request may be taken as
            // late as `ASK_LIMIT` after the send (T-b of the next look at
            // #149; mutant: the asking `held_until` without it).
            let wait = u64::try_from(skyzfs_control::ASK_LIMIT.as_millis()).expect("ms");
            let least = plan::held_until(sent.get() + wait, EXPIRY_DEFAULT);
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

/// A daemon that accepts the first `publish` and loses its answer (an
/// `Err`, or `refused no-answer`); as it loses it, it goes away for
/// `away_s`, and the agent's wall clock steps by `skew_ms`. Taken from
/// the review of #149 (N5, its scratch daemon `LostAnswer`).
struct LostAnswer {
    world: Rc<RefCell<World>>,
    lost: bool,
    no_answer: bool,
    away_s: u64,
    skew_ms: i64,
}

impl Daemon for LostAnswer {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let answer = FakeDaemon(self.world.clone()).ask(request, manifest);
        if matches!(request, Request::Publish(_)) && !self.lost {
            self.lost = true;
            let mut w = self.world.borrow_mut();
            w.away_until = w.now_ms + self.away_s * 1000;
            w.agent_skew_ms = self.skew_ms;
            w.log
                .push(format!("publish answered {answer:?}; the answer is lost"));
            return if self.no_answer {
                Ok(Reply::Refused(Code::NoAnswer, String::new()))
            } else {
                Err(io::Error::new(io::ErrorKind::ConnectionReset, "lost"))
            };
        }
        answer
    }

    fn device(&mut self) -> io::Result<u64> {
        FakeDaemon(self.world.clone()).device()
    }

    fn failure(&mut self) -> io::Result<Option<Noted>> {
        FakeDaemon(self.world.clone()).failure()
    }
}

/// An ask sent in this process whose answer is lost (`Err` or
/// `no-answer`) keeps its hold until `held_until` on the clock that does
/// not step, as an accepted one does, whether the agent goes on alone or
/// a person gives the publish up in the same process: with the daemon
/// away for 900 s, with no step of the wall clock and with a step
/// forward of two hours, `snap/t` is never made on a cut without
/// `pool@t` (a person destroys it as soon as it is not held), and no
/// hold is left at the end. N5 and T-a of the next look at #149.
/// Mutants: the hold timed on the wall clock until an `accepted` is
/// read (N5); an ask with no answer at all recorded held again (L1j).
#[test]
fn an_ask_whose_answer_is_lost_keeps_its_hold_on_the_clock_that_does_not_step() {
    let mut wrong = Vec::new();
    for mode in [Mode::Idle, Mode::Abandoned, Mode::Busy] {
        for no_answer in [false, true] {
            for give_up in [false, true] {
                for skew_ms in [0_i64, 2 * 3_600_000] {
                    let world = World::new(mode);
                    let keep = MemKeep::default();
                    let daemon = LostAnswer {
                        world: world.clone(),
                        lost: false,
                        no_answer,
                        away_s: 900,
                        skew_ms,
                    };
                    let mut a = Agent::new(
                        FakeZfs(world.clone()),
                        daemon,
                        keep.clone(),
                        FakeClock(world.clone()),
                        VOLUME,
                        POOL,
                    )
                    .expect("the agent");
                    let t = a.begin().expect("begun");
                    world.borrow_mut().destroyer = Some(t.clone());
                    if give_up {
                        // The answer is lost in the first rounds; then the
                        // person gives up.
                        let _ = a.settle(Duration::from_secs(5));
                        a.recover(&t, Choice::GiveUp).expect("recovered");
                    }
                    let settled = a.settle(LIMIT);
                    let outcomes = a.outcomes().to_vec();
                    drop(a);
                    // Whatever still pends at the daemon has its time to
                    // land.
                    world.borrow_mut().advance(Duration::from_secs(2 * 3600));
                    let w = world.borrow();
                    let case = format!(
                        "{mode:?} no_answer={no_answer} give_up={give_up} skew={skew_ms}: \
                         settled {settled:?}, outcomes {outcomes:?}, snap/t on {:?}",
                        w.refs.get(&t).map(|r| r.number)
                    );
                    if w.refs.get(&t).is_some_and(|r| {
                        !w.commits[&r.commit]
                            .cut
                            .snaps
                            .contains_key(&format!("{POOL}@{t}"))
                    }) {
                        wrong.push(format!("{case}: made on a cut without {POOL}@{t}"));
                    }
                    if !holds(&w).is_empty() {
                        wrong.push(format!("{case}: holds left {:?}", holds(&w)));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A daemon that loses the answer to its `n`-th `publish` (an `Err`, or
/// `refused no-answer`); the request itself is the fake's, accepted.
struct LoseNth {
    world: Rc<RefCell<World>>,
    n: usize,
    seen: usize,
    no_answer: bool,
    lost: Rc<Cell<bool>>,
}

impl Daemon for LoseNth {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let answer = FakeDaemon(self.world.clone()).ask(request, manifest);
        if matches!(request, Request::Publish(_)) {
            self.seen += 1;
            if self.seen == self.n {
                self.lost.set(true);
                self.world.borrow_mut().log.push(format!(
                    "publish {} answered {answer:?}; the answer is lost",
                    self.n
                ));
                return if self.no_answer {
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

    fn failure(&mut self) -> io::Result<Option<Noted>> {
        FakeDaemon(self.world.clone()).failure()
    }
}

/// Whether the daemon has a request of `t` pending now, asked behind
/// the agent's back (the daemon's absence and silence set aside, and the
/// call not counted): the time, where it has.
fn pending_now(world: &Rc<RefCell<World>>, t: &str) -> Option<u64> {
    let (now, away, silent) = {
        let mut w = world.borrow_mut();
        let saved = (w.now_ms, w.away_until, w.silent_until);
        w.away_until = 0;
        w.silent_until = 0;
        saved
    };
    let reply = FakeDaemon(world.clone()).ask(&Request::Progress(t.to_string()), &[]);
    let mut w = world.borrow_mut();
    w.away_until = away;
    w.silent_until = silent;
    w.calls.pop();
    matches!(reply, Ok(Reply::Pending)).then_some(now)
}

/// One run of N6: the first request accepted; where `moved`, the pool
/// exported and imported (`-T`, one group back, after the hold) while it
/// pends, so that the agent withdraws it (the answer `withdrawn` sets
/// `memory.withdrawn`) and asks anew; the answer to that ask (or, without
/// `moved`, to the first) lost; then a person's give-up in the same
/// process, and a person who destroys `pool@t` as soon as it is not
/// held. Whether a hold was released while a request pended at the
/// daemon, whether `snap/t` was made on a cut without `pool@t`, and
/// whether the course ran as above: the answer lost, where `moved` a
/// withdraw asked and two publishes sent, and no hold left at the end.
fn n6_run(mode: Mode, no_answer: bool, moved: bool) -> (bool, bool, bool, String) {
    let world = World::new(mode);
    world.borrow_mut().lag_ms = 60_000;
    let keep = MemKeep::default();
    let lost = Rc::new(Cell::new(false));
    let daemon = LoseNth {
        world: world.clone(),
        n: if moved { 2 } else { 1 },
        seen: 0,
        no_answer,
        lost: lost.clone(),
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
                if let Some(now) = pending_now(&world, &t) {
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
    let lost_at = rounds;
    a.recover(&t, Choice::GiveUp).expect("recovered");
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
    let early = !early.borrow().is_empty();
    let count = |call: &str| w.calls.iter().filter(|c| **c == call).count();
    let (publishes, withdraws) = (count("publish"), count("withdraw"));
    let course =
        lost.get() && holds(&w).is_empty() && (!moved || (withdraws >= 1 && publishes >= 2));
    let line = format!(
        "{mode:?} no_answer={no_answer} moved={moved}: publishes {publishes}, withdraws {withdraws}; lost {} after {lost_at} s; settled {settled:?}; outcomes {:?}; released while pending {early}; snap/t on {:?}; wrong {wrong}; holds {:?}\n  log tail {:?}",
        lost.get(),
        outcomes.iter().map(|(n, o)| format!("{n} {o:?}")).collect::<Vec<_>>(),
        w.refs.get(&t).map(|r| r.number),
        holds(&w),
        w.log.iter().rev().take(10).collect::<Vec<_>>()
    );
    (early, wrong, course, line)
}

/// A withdraw answered for one request does not count for the next
/// (N6 of #149's third look; the reviewer's scratch test, taken): the
/// first request accepted and pending; where `moved`, the pool imported
/// back one group while it pends, so that the agent withdraws it (the
/// answer `withdrawn`) and asks anew; the answer to that ask (or, without
/// `moved`, to the first) lost; then a person's give-up in the same
/// process, and a person who destroys `pool@t` as soon as it is not held.
/// No hold is released while a request pends at the daemon, and `snap/t`
/// is never made on a cut without `pool@t`; each row checks that its
/// course ran (T1 of #149's next look). Mutant: `withdrawn` not cleared
/// at the asking commit (4 of the 4 `moved` rows fail).
#[test]
fn a_withdraw_answered_does_not_count_for_the_request_asked_anew() {
    let mut found = Vec::new();
    for mode in [Mode::Idle, Mode::Busy] {
        for no_answer in [false, true] {
            for moved in [false, true] {
                let (early, wrong, course, line) = n6_run(mode, no_answer, moved);
                if early || wrong || !course {
                    found.push(line);
                }
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}

/// One run of C1: the agent's first `asking` record takes `slow_s` to
/// save (the clock moves on as it is saved); the first request sent is
/// accepted and its answer lost (an `Err`, or `refused no-answer`), and
/// the daemon goes away for 900 s; where `stuck`, no upload ever ends,
/// so that a request accepted pends until it expires; where `give_up`, a
/// person gives the
/// publish up in the same process; a person destroys `pool@t` as soon as
/// it is not held. Whether a hold was released while a request pended,
/// whether `snap/t` was made on a cut without `pool@t`, how many
/// `asking` records were saved and how many requests sent.
fn c1_run(mode: Mode, no_answer: bool, give_up: bool, stuck: bool, slow_s: u64) -> C1Run {
    let world = World::new(mode);
    if stuck {
        world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    }
    let keep = MemKeep::default();
    let daemon = LostAnswer {
        world: world.clone(),
        lost: false,
        no_answer,
        away_s: 900,
        skew_ms: 0,
    };
    let name: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let early: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let askings = Rc::new(Cell::new(0_u32));
    let span = Rc::new(Cell::new(0_u64));
    let hook = {
        let (world, name, early, askings, span, saved) = (
            world.clone(),
            name.clone(),
            early.clone(),
            askings.clone(),
            span.clone(),
            keep.clone(),
        );
        move |p: Point, _: &str| {
            if p == (Point {
                effect: Effect::Asking,
                after: true,
            }) {
                askings.set(askings.get() + 1);
                if askings.get() == 1 {
                    let t = name.borrow().clone();
                    if let Ok(Loaded::Present(record)) = saved.load() {
                        if let Some(Phase::Asked {
                            asked_ms,
                            held_until_ms,
                            ..
                        }) = record.publish(&t).map(|p| &p.phase)
                        {
                            span.set(held_until_ms.saturating_sub(*asked_ms));
                        }
                    }
                    world.borrow_mut().advance(Duration::from_secs(slow_s));
                }
            }
            if p.effect == Effect::Release && !p.after {
                let t = name.borrow().clone();
                if let Some(now) = pending_now(&world, &t) {
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
        a.recover(&t, Choice::GiveUp).expect("recovered");
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
    let sent = w.calls.iter().filter(|c| **c == "publish").count();
    let early = !early.borrow().is_empty();
    C1Run {
        early,
        wrong,
        span_ms: span.get(),
        askings: askings.get(),
        sent,
        holds: holds(&w).len(),
        line: format!(
            "{mode:?} no_answer={no_answer} give_up={give_up} stuck={stuck} slow={slow_s} s: settled {settled:?}; outcomes {:?}; held for {} ms from asked; askings {}; sent {sent}; released while pending {early}; snap/t on {:?}; wrong {wrong}; holds {:?}",
            outcomes.iter().map(|(n, o)| format!("{n} {o:?}")).collect::<Vec<_>>(),
            span.get(),
            askings.get(),
            w.refs.get(&t).map(|r| r.number),
            holds(&w),
        ),
    }
}

/// What [`c1_run`] saw.
struct C1Run {
    early: bool,
    wrong: bool,
    /// The first `asking` record's `held_until` less its `asked`.
    span_ms: u64,
    askings: u32,
    sent: usize,
    holds: usize,
    line: String,
}

/// A request is sent within [`plan::ASKING_SLACK`] of the time its
/// `asking` record counts from, or not at all (C1 of #149's next look;
/// Codex's P2 there; the reviewer's probe, taken): with the save of the
/// first `asking` record taking from 0 to 200 s, the answer to the first
/// request sent lost and the daemon away for 900 s, with uploads that
/// end or never do, alone or with a person's give-up, no hold is released while a request pends, `snap/t`
/// is never made on a cut without `pool@t`, and no hold is left. The
/// `asking` record holds for the slack, the channel's wait, the expiry
/// and the margin from its `asked`. A save within the slack sends its
/// request; a longer one sends none, and the publish is asked anew, or,
/// where a person gives it up first, never sent. Mutants: no check
/// before the send (the rows past 80 s with uploads that never end
/// release the hold early; 80 s is the fake's threshold, since it
/// accepts at the send and so cannot show the slack's own share); the
/// slack not counted in the record's `held_until` (the record's span).
#[test]
fn a_request_is_sent_within_the_slack_of_its_record_or_not_at_all() {
    let slack = plan::ASKING_SLACK.as_secs();
    let span = [
        plan::ASKING_SLACK,
        skyzfs_control::ASK_LIMIT,
        EXPIRY_DEFAULT,
        plan::HOLD_MARGIN,
    ]
    .iter()
    .map(|d| u64::try_from(d.as_millis()).expect("ms"))
    .sum::<u64>();
    let mut found = Vec::new();
    for slow_s in [0, slack, slack + 1, 41, 81, 200] {
        for mode in [Mode::Idle, Mode::Abandoned, Mode::Busy] {
            for no_answer in [false, true] {
                for (give_up, stuck) in [(false, false), (false, true), (true, false), (true, true)]
                {
                    let run = c1_run(mode, no_answer, give_up, stuck, slow_s);
                    let mut bad = run.early || run.wrong || run.holds > 0;
                    bad |= run.span_ms != span;
                    // The course: within the slack, a request sent (and its
                    // answer lost); past it, the first record not sent, and
                    // one sent from a record after it unless the person
                    // gave the publish up first.
                    if slow_s <= slack {
                        bad |= run.sent == 0;
                    } else if give_up {
                        bad |= run.sent != 0;
                    } else {
                        bad |= run.sent == 0 || run.sent + 1 > run.askings as usize;
                    }
                    if bad {
                        found.push(run.line);
                    }
                }
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}

/// The world's pool read through the CLI adapter (B1 of #157's review):
/// [`Zfs::datasets`] renders the pool as `zfs list` and the two `zfs
/// get -s local` print it, with a snapshot limit set on the root, so
/// that every dataset has `filesystem_count` and `snapshot_count` with
/// the source `local`, counted as ZFS counts them (the datasets and the
/// snapshots under it); and reads that output through
/// [`crate::zfs::cli::ZfsCli`]. Every other call is the world's.
struct ThroughCli(FakeZfs);

impl ThroughCli {
    /// The three reads' output of `pool`: the list, the pairs, the
    /// values.
    fn rendered(&self, pool: &str) -> (String, String, String) {
        let world = self.0 .0.borrow();
        let (mut list, mut pairs, mut values) = (String::new(), String::new(), String::new());
        let Some(p) = &world.pool else {
            return (list, pairs, values);
        };
        let mut datasets = p.datasets.clone();
        datasets.sort_by(|a, b| a.name.cmp(&b.name));
        for d in &datasets {
            let kind = match d.kind {
                crate::manifest::Kind::Filesystem => "filesystem",
                crate::manifest::Kind::Volume => "volume",
            };
            list.push_str(&format!(
                "{}\t{}\t{kind}\t{}\n",
                d.name, d.guid, d.createtxg
            ));
            let own = format!("{}@", d.name);
            for (name, snap) in p.snaps.iter().filter(|(name, _)| name.starts_with(&own)) {
                list.push_str(&format!(
                    "{name}\t{}\tsnapshot\t{}\n",
                    snap.guid, snap.createtxg
                ));
            }
            let under = |name: &str| name == d.name || name.starts_with(&format!("{}/", d.name));
            let filesystems = datasets
                .iter()
                .filter(|e| e.name != d.name && under(&e.name))
                .count();
            let snapshots = p
                .snaps
                .keys()
                .filter(|k| k.split_once('@').is_some_and(|(ds, _)| under(ds)))
                .count();
            let mut local: Vec<(String, String)> = d
                .local
                .iter()
                .map(|(k, v)| (k.clone(), String::from_utf8_lossy(v).into_owned()))
                .collect();
            if d.name == pool {
                local.push(("snapshot_limit".into(), "100".into()));
            }
            local.push(("filesystem_count".into(), filesystems.to_string()));
            local.push(("snapshot_count".into(), snapshots.to_string()));
            for (property, value) in local {
                pairs.push_str(&format!("{}\t{property}\n", d.name));
                values.push_str(&format!("{}\t{property}\t{value}\n", d.name));
            }
        }
        (list, pairs, values)
    }
}

impl Zfs for ThroughCli {
    fn layout(&mut self, pool: &str) -> Result<zfs::Layout, ZfsError> {
        self.0.layout(pool)
    }
    fn props(&mut self, pool: &str) -> Result<zfs::Props, ZfsError> {
        self.0.props(pool)
    }
    fn nocacheflush(&mut self) -> Result<bool, ZfsError> {
        self.0.nocacheflush()
    }
    fn count(&mut self, pool: &str) -> Result<u64, ZfsError> {
        self.0.count(pool)
    }
    fn snapshot(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        self.0.snapshot(pool, t)
    }
    fn datasets(&mut self, pool: &str, t: &str) -> Result<Vec<zfs::Dataset>, ZfsError> {
        // The world's own read first: its calls and its faults.
        self.0.datasets(pool, t)?;
        let (list, pairs, values) = self.rendered(pool);
        let mut host = crate::host::fake::Fake::default();
        host.ok(
            &format!("zfs list -r -H -p -t filesystem,volume,snapshot -o name,guid,type,createtxg {pool}"),
            &list,
        );
        host.ok(
            &format!("zfs get -r -H -p -s local -t filesystem,volume -o name,property all {pool}"),
            &pairs,
        );
        host.ok(
            &format!(
                "zfs get -r -H -p -s local -t filesystem,volume -o name,property,value all {pool}"
            ),
            &values,
        );
        zfs::cli::ZfsCli::new(host, zfs::cli::Limits::default()).datasets(pool, t)
    }
    fn destroy(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        self.0.destroy(pool, t)
    }
    fn destroy_one(&mut self, pool: &str, dataset: &str, t: &str) -> Result<(), ZfsError> {
        self.0.destroy_one(pool, dataset, t)
    }
    fn members(
        &mut self,
        pool: &str,
        t: &str,
    ) -> Result<Option<Vec<crate::record::Member>>, ZfsError> {
        self.0.members(pool, t)
    }
    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        self.0.hold(pool, t, tag)
    }
    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        self.0.release(pool, t, tag)
    }
    fn held(
        &mut self,
        pool: &str,
        t: &str,
        tag: &str,
        members: Option<&[crate::record::Member]>,
    ) -> Result<zfs::Held, ZfsError> {
        self.0.held(pool, t, tag, members)
    }
    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
        self.0.tagged(pool, tag)
    }
    fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError> {
        self.0.assist(pool, value)
    }

    fn reopen(&mut self, pool: &str) -> Result<(), ZfsError> {
        self.0.reopen(pool)
    }

    fn pool_state(&mut self, pool: &str) -> Result<Option<crate::zfs::PoolState>, ZfsError> {
        self.0.pool_state(pool)
    }
}

/// A publish on a pool with a snapshot limit, read through the CLI, with
/// a file system created at the point `create` (none where `None`): the
/// takes it made and its outcomes.
fn counted_publish(create: Option<Point>, after_take: bool) -> (usize, Vec<(String, Outcome)>) {
    let world = World::new(Mode::Busy);
    if after_take {
        world.borrow_mut().after_take = Some(User::CreateDataset);
    }
    let keep = MemKeep::default();
    let takes = Rc::new(Cell::new(0));
    let points: Points = Box::new({
        let (world, takes) = (world.clone(), takes.clone());
        move |p: Point, _: &str| {
            if p == at(Effect::Take, false) {
                takes.set(takes.get() + 1);
            }
            if Some(p) == create && takes.get() == 1 {
                world.borrow_mut().user(User::CreateDataset, "");
            }
            false
        }
    });
    let mut agent = Agent::new(
        ThroughCli(FakeZfs(world.clone())),
        FakeDaemon(world.clone()),
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
    .with_points(points);
    agent.begin().expect("begun");
    if let Err(e) = agent.settle(LIMIT) {
        panic!(
            "{e}: {:?}\n{}",
            agent.outcomes(),
            world.borrow().log.join("\n")
        );
    }
    (takes.get(), agent.outcomes().to_vec())
}

/// On a pool with a limit, `filesystem_count` and `snapshot_count` are
/// read as local and rise with each take; the read leaves them out, so a
/// take is as read before it (B1 of #157's review, the lead's test). A
/// file system created after the take, before the read after it, has no
/// `@t` and raises its parent's `filesystem_count`: the first take is
/// published. One created between the read before the take and the take
/// is a member not read before: that take is taken again, once, and the
/// next is published; neither ends `NotWhole`. Mutants: either counter
/// kept.
#[test]
fn counts_that_rise_with_a_take_do_not_fail_its_check() {
    for (create, after_take, takes_made) in [
        (None, false, 1),
        (None, true, 1),
        (Some(at(Effect::Take, false)), false, 2),
    ] {
        let (takes, outcomes) = counted_publish(create, after_take);
        assert!(
            matches!(outcomes[..], [(_, Outcome::Published(_))]),
            "{create:?} {after_take}: {outcomes:?}"
        );
        assert_eq!(takes, takes_made, "{create:?} {after_take}");
    }
}

/// The world's pool, with a person's `zfs release` and `zfs destroy -r
/// pool@t` landing between the agent's two reads of a round, after its
/// members' read found `pool@t` and before its holds' read
/// (`armed`): that read goes through the CLI adapter, whose `zfs holds
/// -r` says what libzfs says of a snapshot gone between its open and
/// its read (`no such pool or dataset`; testing's agent rig, case
/// `destroy`), then, read again, what it says of a `pool@t` that is
/// gone: exit 0, no line, `no datasets available` (as read in OpenZFS
/// 2.2's `zfs_do_holds`, not recorded: nit F of #161's review).
struct DestroyedBetween {
    zfs: FakeZfs,
    armed: Rc<Cell<bool>>,
}

impl Zfs for DestroyedBetween {
    fn layout(&mut self, pool: &str) -> Result<zfs::Layout, ZfsError> {
        self.zfs.layout(pool)
    }
    fn props(&mut self, pool: &str) -> Result<zfs::Props, ZfsError> {
        self.zfs.props(pool)
    }
    fn nocacheflush(&mut self) -> Result<bool, ZfsError> {
        self.zfs.nocacheflush()
    }
    fn count(&mut self, pool: &str) -> Result<u64, ZfsError> {
        self.zfs.count(pool)
    }
    fn snapshot(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        self.zfs.snapshot(pool, t)
    }
    fn datasets(&mut self, pool: &str, t: &str) -> Result<Vec<zfs::Dataset>, ZfsError> {
        self.zfs.datasets(pool, t)
    }
    fn destroy(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
        self.zfs.destroy(pool, t)
    }
    fn destroy_one(&mut self, pool: &str, dataset: &str, t: &str) -> Result<(), ZfsError> {
        self.zfs.destroy_one(pool, dataset, t)
    }
    fn members(
        &mut self,
        pool: &str,
        t: &str,
    ) -> Result<Option<Vec<crate::record::Member>>, ZfsError> {
        self.zfs.members(pool, t)
    }
    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        self.zfs.hold(pool, t, tag)
    }
    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        self.zfs.release(pool, t, tag)
    }
    fn held(
        &mut self,
        pool: &str,
        t: &str,
        tag: &str,
        members: Option<&[crate::record::Member]>,
    ) -> Result<zfs::Held, ZfsError> {
        if !self.armed.replace(false) {
            return self.zfs.held(pool, t, tag, members);
        }
        {
            let mut world = self.zfs.0.borrow_mut();
            let own = format!("@{t}");
            if let Some(p) = &mut world.pool {
                for (_, snap) in p.snaps.iter_mut().filter(|(name, _)| name.ends_with(&own)) {
                    snap.holds.clear();
                }
            }
            world.user(User::Destroy, t);
            let gone = world
                .pool
                .as_ref()
                .is_some_and(|p| !p.snaps.contains_key(&format!("{pool}@{t}")));
            assert!(gone, "the person's destroy took");
        }
        let words = format!("zfs holds -H -r {pool}@{t}");
        let mut host = crate::host::fake::Fake::default();
        host.fails(
            &words,
            1,
            &format!("cannot get holds for '{pool}/db@{t}': no such pool or dataset\n"),
        );
        host.give(
            &words,
            Ok(crate::host::fake::output(0, "", "no datasets available\n")),
        );
        host.ok("zpool list -H -o name", &format!("{pool}\n"));
        zfs::cli::ZfsCli::new(host, zfs::cli::Limits::default()).held(pool, t, tag, members)
    }
    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
        self.zfs.tagged(pool, tag)
    }
    fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError> {
        self.zfs.assist(pool, value)
    }

    fn reopen(&mut self, pool: &str) -> Result<(), ZfsError> {
        self.zfs.reopen(pool)
    }

    fn pool_state(&mut self, pool: &str) -> Result<Option<crate::zfs::PoolState>, ZfsError> {
        self.zfs.pool_state(pool)
    }
}

/// A person destroys `pool@t` while its publish is asked, between the
/// agent's read of its members and its read of its holds, which says
/// `no such pool or dataset` (testing's agent rig, case `destroy`; the
/// lead's ask): the pool is there, so that is no missing pool, and the
/// holds are read again, which finds `pool@t` gone. The round goes on,
/// and the publish is given up as destroyed in this process, not at its
/// next start. Not under [`Mode::ErrLow`], whose daemon refuses the
/// publish as rewound first. Mutants: any `no such pool` read as a
/// missing pool without `zpool list`, a vanished read not read again.
#[test]
fn a_destroy_between_two_reads_is_no_missing_pool() {
    for mode in [Mode::Busy, Mode::Idle, Mode::Abandoned] {
        let world = World::new(mode);
        let keep = MemKeep::default();
        let asked = at(Effect::Asked, true);
        let mut first = agent(&world, &keep).with_points(Box::new(move |p, _: &str| p == asked));
        let name = first.begin().expect("begun");
        assert!(
            matches!(first.settle(LIMIT), Err(Error::Stopped)),
            "{mode:?}"
        );
        drop(first);
        let armed = Rc::new(Cell::new(true));
        let mut again = Agent::new(
            DestroyedBetween {
                zfs: FakeZfs(world.clone()),
                armed: armed.clone(),
            },
            FakeDaemon(world.clone()),
            keep.clone(),
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        let settled = again.settle(LIMIT);
        assert!(!armed.get(), "{mode:?}: the destroy landed");
        assert!(
            settled.is_ok(),
            "{mode:?}: {settled:?}\n{}",
            world.borrow().log.join("\n")
        );
        assert!(
            matches!(
                again.outcomes(),
                [(n, Outcome::Abandoned(Why::Destroyed))] if *n == name
            ),
            "{mode:?}: {:?}",
            again.outcomes()
        );
    }
}

/// The syncs after a rewind import (M2.7d; design §5.2: after `zpool
/// import -T` no cut opens as itself until the pool's groups have passed
/// the abandoned ones, and the agent syncs the pool until they have, a
/// group at a time).
mod past {
    use super::*;

    /// An idle pool imported with `-T`, whose abandoned history's top is
    /// `gap` groups above the group the import wrote: the daemon's newest
    /// cut does not open as itself.
    fn rewound(gap: u64) -> Rc<RefCell<World>> {
        let world = World::new(Mode::Idle);
        {
            let mut world = world.borrow_mut();
            for _ in 0..4 {
                world.sync("before the rewind");
            }
            world.restart(Restart::RewindT(Back::AfterHold), None);
            let pool = world.pool.as_mut().expect("the pool");
            pool.abandoned_top = pool.txg + gap;
        }
        assert_eq!(newest(&world).map(|(_, itself)| itself), Some(false));
        world
    }

    /// News on the failure note stops the syncs before the next sync, as
    /// an end would (F1 of the review of #173), and the line names it.
    /// Mutant: the watch not asked before a sync.
    #[test]
    fn news_on_the_failure_note_stops_the_syncs() {
        use skyzfs_control::failure::{Failure, Held, Reason};
        let keep = MemKeep::default();
        let world = rewound(5);
        let mut agent = super::agent(&world, &keep);
        agent.watch_note(Some(Failed::No));
        let watched = world.clone();
        let mut asked = 0u32;
        let mut go = move || {
            asked += 1;
            if asked == 2 {
                // Before the second sync: the daemon failed a second
                // ago, while a client held its device.
                let mut w = watched.borrow_mut();
                w.noted = Some(Noted::Failed(Failure {
                    held: Held::Yes,
                    reason: Reason::ImageFailed,
                    identity: [7; 16],
                }));
                w.advance(Duration::from_secs(1));
            }
            true
        };
        let past = agent.sync_past(&mut go).expect("the syncs");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: 1,
                    why: Stuck::Noted,
                    ..
                }
            ),
            "{past:?}"
        );
        assert_eq!(syncs(&world), 1);
        assert_eq!(Stuck::Noted.to_string(), "the volume's failure note came");
    }

    /// The daemon's newest cut: its group, and whether it opens as itself.
    fn newest(world: &Rc<RefCell<World>>) -> Option<(u64, bool)> {
        world
            .borrow()
            .daemon
            .cuts
            .last()
            .map(|c| (c.group, c.itself))
    }

    fn syncs(world: &Rc<RefCell<World>>) -> usize {
        world.borrow().synced_at.len()
    }

    /// The agent syncs until the newest cut opens as itself, and no
    /// more; with that cut there, it syncs nothing. Each sync writes two
    /// groups in the fake (the kernel probe of #145), so passing five
    /// abandoned groups and one more takes three. Mutants: no sync; one
    /// sync only; one after the cut opens; the newest cut's group
    /// compared instead of whether it opens.
    #[test]
    fn after_a_rewind_import_the_agent_syncs_until_a_cut_opens_as_itself() {
        let world = rewound(5);
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        assert_eq!(past, Past::Clear { synced: 3 });
        assert_eq!(syncs(&world), 3);
        assert_eq!(newest(&world).map(|(_, itself)| itself), Some(true));
        let again = agent.sync_past(&mut || true).expect("seen");
        assert_eq!(again, Past::Clear { synced: 0 });
        assert_eq!(syncs(&world), 3);
        // Nothing was written in the record's directory.
        assert!(keep.0.borrow().record.is_none());
    }

    /// A cut that opens as itself while still under the abandoned top
    /// (4 KiB ring slots) is not clear: the agent syncs on until the
    /// newest cut is above the daemon's rewind mark, so that no later cut
    /// moves the mark (the lead's ruling in M2.7b-2). Mutant: "opens as
    /// itself" alone (the syncs stop at the first such cut, at the mark).
    #[test]
    fn a_cut_that_opens_as_itself_at_the_mark_is_not_clear() {
        let world = rewound(9);
        {
            let mut w = world.borrow_mut();
            let pool = w.pool.as_mut().expect("the pool");
            pool.itself_under_top = 4;
            let top = pool.abandoned_top;
            // The daemon has seen the abandoned groups (the volume's
            // rings show them).
            w.highest_written = w.highest_written.max(top);
        }
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        let reply = cuts(&world);
        let (_, itself) = reply.newest.expect("a cut");
        assert!(itself, "{past:?}");
        assert!(reply.count > reply.rewound, "{reply:?}");
        // Two groups a sync, from the import's group G: the first cut
        // that opens as itself (G + 6, over G + 9 - 4) comes with the
        // third sync and is at the mark, as is G + 8; G + 10, of the
        // fifth, is the first above the abandoned top G + 9.
        assert_eq!(past, Past::Clear { synced: 5 });
        // A publish asked now is not refused `rewound`: the next cut is
        // above every group seen, and the mark stays.
        world.borrow_mut().sync("after");
        assert_eq!(cuts(&world).rewound, reply.rewound);
    }

    /// The daemon's answer to `cuts`.
    fn cuts(world: &Rc<RefCell<World>>) -> skyzfs_control::CutsReply {
        let w = world.borrow();
        skyzfs_control::CutsReply {
            attachment: w.daemon.attachment,
            state: Some(w.daemon.state),
            count: w.daemon.cuts.last().map_or(0, |c| c.number),
            newest: w.daemon.cuts.last().map(|c| (c.group, c.itself)),
            rewound: w.daemon.rewound,
        }
    }

    /// With no cut, or a newest cut that opens as itself, or a daemon
    /// that does not answer, nothing is synced. Mutant: a sync where the
    /// daemon holds no cut, or says nothing.
    #[test]
    fn with_no_cut_behind_nothing_is_synced() {
        let world = World::new(Mode::Idle);
        world.borrow_mut().daemon.cuts.clear();
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        assert_eq!(
            agent.sync_past(&mut || true).expect("seen"),
            Past::Clear { synced: 0 }
        );
        world.borrow_mut().sync("a person's change");
        assert_eq!(
            agent.sync_past(&mut || true).expect("seen"),
            Past::Clear { synced: 0 }
        );
        let world = rewound(5);
        world.borrow_mut().away_until = u64::MAX;
        let mut agent = super::agent(&world, &keep);
        assert_eq!(
            agent.sync_past(&mut || true).expect("unseen"),
            Past::Unseen { synced: 0 }
        );
        assert_eq!(syncs(&world), 0);
    }

    /// A daemon that goes away between two syncs ends the syncs as
    /// `Unseen`, which counts the syncs that returned before it (L3 of
    /// the review of #172). Mutant: `Unseen` with no count.
    #[test]
    fn a_daemon_gone_between_syncs_is_unseen_after_the_syncs_it_saw() {
        let keep = MemKeep::default();
        let world = rewound(5);
        let mut agent = super::agent(&world, &keep);
        let watched = world.clone();
        let mut asked = 0u32;
        let mut go = move || {
            asked += 1;
            if asked == 2 {
                // Before the second sync: the daemon goes away while it
                // runs.
                watched.borrow_mut().away_until = u64::MAX;
            }
            true
        };
        let past = agent.sync_past(&mut go).expect("unseen");
        assert_eq!(past, Past::Unseen { synced: 2 });
        assert_eq!(syncs(&world), 2);
    }

    /// Where the abandoned groups are more than one ring holds, the
    /// agent stops at [`SYNC_PAST_MAX`] syncs, in this process and not
    /// per call, until it sees a cut that opens as itself; a restart of
    /// the engine does not start the count again. Mutants: no bound; a
    /// bound per call; the count not reset by a cut that opens; the count
    /// reset by [`Agent::restart`].
    #[test]
    fn the_syncs_stop_at_their_bound_until_a_cut_opens() {
        let world = rewound(10_000);
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        let Past::Behind { synced, group, why } = past else {
            panic!("{past:?}");
        };
        assert_eq!((synced, why), (SYNC_PAST_MAX, Stuck::Bound));
        assert_eq!(Some(group), newest(&world).map(|(group, _)| group));
        assert_eq!(syncs(&world), SYNC_PAST_MAX as usize);
        agent.restart().expect("restarted");
        let past = agent.sync_past(&mut || true).expect("seen");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: 0,
                    why: Stuck::Bound,
                    ..
                }
            ),
            "{past:?}"
        );
        assert_eq!(syncs(&world), SYNC_PAST_MAX as usize);
        // The pool passes the abandoned groups by itself: the count
        // starts again, and a later rewind is synced past anew.
        {
            let mut world = world.borrow_mut();
            world.pool.as_mut().expect("the pool").abandoned_top = 0;
            world.sync("a person's change");
        }
        assert_eq!(
            agent.sync_past(&mut || true).expect("seen"),
            Past::Clear { synced: 0 }
        );
        {
            let mut world = world.borrow_mut();
            let pool = world.pool.as_mut().expect("the pool");
            pool.abandoned_top = pool.txg + 3;
            world.sync("a person's change, below the abandoned top");
        }
        assert_eq!(
            agent.sync_past(&mut || true).expect("synced"),
            Past::Clear { synced: 2 }
        );
    }

    /// The ring of uberblocks holds at most [`SYNC_PAST_MAX`] of them, so
    /// a `-T` import is at most that many groups below the abandoned top,
    /// and each sync writes at least one group: the bound is never met
    /// after a rewind import alone.
    #[test]
    fn the_bound_is_the_most_slots_a_ring_holds() {
        assert_eq!(
            u64::from(SYNC_PAST_MAX),
            skyzfs_uberblock::RING_SIZE / skyzfs_uberblock::SLOT_MIN
        );
    }

    /// A sync after which the daemon counts no cut ends the syncs: they
    /// do not reach the volume. Mutant: syncing on regardless.
    #[test]
    fn a_sync_that_makes_no_cut_ends_the_syncs() {
        let world = rewound(5);
        world.borrow_mut().plain_sync = true;
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: 1,
                    why: Stuck::NoCut,
                    ..
                }
            ),
            "{past:?}"
        );
        assert_eq!(syncs(&world), 1);
    }

    /// `go` is asked before each sync: an end asked for stops them.
    /// Mutant: `go` not asked.
    #[test]
    fn an_end_stops_the_syncs() {
        let world = rewound(5);
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let mut left = 1;
        let mut go = || {
            left -= 1;
            left >= 0
        };
        let past = agent.sync_past(&mut go).expect("told");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: 1,
                    why: Stuck::Told,
                    ..
                }
            ),
            "{past:?}"
        );
        assert_eq!(syncs(&world), 1);
    }

    /// A pool that the agent's premise refuses is not synced: the agent
    /// changes nothing on a pool it would not publish. Mutant: the
    /// premise not checked before the first sync.
    #[test]
    fn a_pool_refused_its_premise_is_not_synced() {
        for user in [User::Attach, User::Add] {
            let world = rewound(5);
            world.borrow_mut().user(user, "");
            let keep = MemKeep::default();
            let mut agent = agent(&world, &keep);
            let past = agent.sync_past(&mut || true);
            assert!(matches!(past, Err(Error::Premise(_))), "{user:?}: {past:?}");
            assert_eq!(syncs(&world), 0, "{user:?}");
        }
    }

    /// A sync that fails is not counted toward the bound: after more
    /// failures than the bound, a ZFS that takes the syncs again carries
    /// the pool past the abandoned groups (Astra's P2 on #170). Mutant:
    /// the count raised before the sync returns.
    #[test]
    fn a_sync_that_failed_is_not_charged_to_the_bound() {
        let world = rewound(5);
        world.borrow_mut().assist_fails = SYNC_PAST_MAX + 1;
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        for n in 0..=SYNC_PAST_MAX {
            let past = agent.sync_past(&mut || true);
            assert!(matches!(past, Err(Error::Zfs(_))), "{n}: {past:?}");
        }
        assert_eq!(syncs(&world), 0);
        assert_eq!(
            agent.sync_past(&mut || true).expect("synced"),
            Past::Clear { synced: 3 }
        );
        assert_eq!(newest(&world).map(|(_, itself)| itself), Some(true));
    }

    /// A daemon that counts no cut after any sync gets one sync a call,
    /// and the bound still ends them: `NoCut` does not start the count
    /// again (R11 of the review of #170: one sync a round for ever).
    #[test]
    fn a_sync_that_makes_no_cut_still_counts_toward_the_bound() {
        let world = rewound(5);
        world.borrow_mut().plain_sync = true;
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        for n in 0..SYNC_PAST_MAX {
            let past = agent.sync_past(&mut || true).expect("synced");
            assert!(
                matches!(
                    past,
                    Past::Behind {
                        synced: 1,
                        why: Stuck::NoCut,
                        ..
                    }
                ),
                "{n}: {past:?}"
            );
        }
        let past = agent.sync_past(&mut || true).expect("seen");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: 0,
                    why: Stuck::Bound,
                    ..
                }
            ),
            "{past:?}"
        );
        assert_eq!(syncs(&world), SYNC_PAST_MAX as usize);
    }

    /// The premise is checked before the first sync of each call, not
    /// only of the first call: a pool attached to between two rounds is
    /// not synced (R7 of the review of #170).
    #[test]
    fn the_premise_is_checked_at_each_call() {
        let world = rewound(5);
        world.borrow_mut().plain_sync = true;
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: 1,
                    why: Stuck::NoCut,
                    ..
                }
            ),
            "{past:?}"
        );
        world.borrow_mut().user(User::Attach, "");
        assert_eq!(newest(&world).map(|(_, itself)| itself), Some(false));
        let past = agent.sync_past(&mut || true);
        assert!(matches!(past, Err(Error::Premise(_))), "{past:?}");
        assert_eq!(syncs(&world), 1);
    }

    /// A daemon that holds no cut (a state begun anew) starts the count
    /// again, as a cut that opens as itself does (R1 of the review of
    /// #170).
    #[test]
    fn no_cut_held_starts_the_count_again() {
        let world = rewound(10_000);
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        assert!(
            matches!(
                past,
                Past::Behind {
                    why: Stuck::Bound,
                    ..
                }
            ),
            "{past:?}"
        );
        world.borrow_mut().daemon.cuts.clear();
        assert_eq!(
            agent.sync_past(&mut || true).expect("seen"),
            Past::Clear { synced: 0 }
        );
        world
            .borrow_mut()
            .sync("a person's change, below the abandoned top");
        let past = agent.sync_past(&mut || true).expect("synced");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: SYNC_PAST_MAX,
                    why: Stuck::Bound,
                    ..
                }
            ),
            "{past:?}"
        );
    }

    /// `NoCut` is judged against the count before the last sync, not the
    /// first: a sync that makes cuts and then one that makes none end
    /// there (R5 of the review of #170).
    #[test]
    fn no_cut_is_judged_from_the_last_sync() {
        let world = rewound(10);
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let watched = world.clone();
        let mut asked = 0;
        let mut go = move || {
            asked += 1;
            if asked == 2 {
                watched.borrow_mut().plain_sync = true;
            }
            true
        };
        let past = agent.sync_past(&mut go).expect("synced");
        assert!(
            matches!(
                past,
                Past::Behind {
                    synced: 2,
                    why: Stuck::NoCut,
                    ..
                }
            ),
            "{past:?}"
        );
        assert_eq!(syncs(&world), 2);
    }

    /// The syncs of a call that failed after some returned are told
    /// ([`Agent::last_synced`], L2 of the review of #170), and a call
    /// that runs none says 0.
    #[test]
    fn the_syncs_before_a_failure_are_told() {
        let world = rewound(10);
        {
            let mut world = world.borrow_mut();
            world.assist_fails = 1;
            world.assist_fails_after = 2;
        }
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true);
        assert!(matches!(past, Err(Error::Zfs(_))), "{past:?}");
        assert_eq!(agent.last_synced(), 2);
        world.borrow_mut().daemon.cuts.clear();
        assert_eq!(
            agent.sync_past(&mut || true).expect("seen"),
            Past::Clear { synced: 0 }
        );
        assert_eq!(agent.last_synced(), 0);
    }
}

/// An end asked before a pass of `settle_while` names what is still
/// under way, and the pass's count and sweep do not run (R19 of the
/// review of #170; `run` treats both results alike).
#[test]
fn an_end_before_a_pass_names_what_is_under_way() {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let mut agent = agent(&world, &keep);
    let name = agent.begin().expect("begun");
    let before = world.borrow().calls.len();
    match agent.settle_while(Duration::from_secs(60), &mut || false) {
        Err(Error::Unsettled(names)) => assert_eq!(names, vec![name]),
        other => panic!("{other:?}"),
    }
    assert_eq!(world.borrow().calls.len(), before);
}
