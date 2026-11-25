//! Review scratch tests for #149 (M2.6c-3), head af50814. Copied into
//! the worktree as `crates/skyzfs-agent/src/program/review_scratch.rs`
//! with `#[cfg(test)] mod review_scratch;` appended to `program.rs`;
//! never committed to the product. Each test asserts what the reviewer
//! expects; a FAILED line is a finding (see review.md).

use std::cell::{Cell, RefCell};
use std::io;
use std::rc::Rc;
use std::time::Duration;

use skyzfs_control::{Code, CutsReply, Reply, Request};

use super::*;
use crate::engine::fake::{
    self, holds, FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, User, World, POOL, VOLUME,
};
use crate::plan::{self, Action, DaemonSeen, PoolSeen, Seen, SnapSeen};
use crate::record::{Count, Member, Publish, Taken};

const LIMIT: Duration = Duration::from_secs(3 * 3600);

/// The point just after the daemon answered a `publish`.
const ASKED: Point = Point {
    effect: Effect::Asked,
    after: true,
};

/// skyzfsd with its uploader in a long step: `progress` and `withdraw`
/// go to the uploader's inbox and are answered `refused no-answer` by
/// the control handler (`bins/skyzfsd/src/control/handler.rs`, `wait`);
/// `cuts` (the desk) and `look` (object storage) are answered. The
/// request stays pending in the fake.
struct Busy {
    inner: FakeDaemon,
    silent: Rc<Cell<bool>>,
}

impl Daemon for Busy {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        if self.silent.get() && matches!(request, Request::Progress(_) | Request::Withdraw(_)) {
            return Ok(Reply::Refused(
                Code::NoAnswer,
                "the uploader did not answer within what was left of the request's 30 s; \
                 a publish may have been taken: ask again, or withdraw it"
                    .into(),
            ));
        }
        self.inner.ask(request, manifest)
    }

    fn device(&mut self) -> io::Result<u64> {
        self.inner.device()
    }
}

fn agent_with<D: Daemon>(
    world: &Rc<RefCell<World>>,
    daemon: D,
    keep: &MemKeep,
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
}

/// An agent stopped just after the daemon accepted its request: the
/// request pends, the hold is placed.
fn asked_and_stopped(world: &Rc<RefCell<World>>, keep: &MemKeep) -> String {
    let mut agent0 = agent_with(world, FakeDaemon(world.clone()), keep)
        .with_points(Box::new(|p: Point, _: &str| p == ASKED));
    let t = agent0.begin().expect("begun");
    assert!(matches!(agent0.settle(LIMIT), Err(Error::Stopped)));
    t
}

/// R1. A person's give-up while the uploader is busy (`progress`
/// answered `refused no-answer`) must not release the hold while the
/// request may still pend (the planner's rule for every give-up: a
/// no-answer is a daemon away, and the hold is kept until `held_until`).
#[test]
fn r1_person_give_up_keeps_the_hold_while_the_uploader_does_not_answer() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let t = asked_and_stopped(&world, &keep);
    let silent = Rc::new(Cell::new(true));
    let daemon = Busy {
        inner: FakeDaemon(world.clone()),
        silent: silent.clone(),
    };
    let mut again = agent_with(&world, daemon, &keep);
    again.recover(&t, Choice::GiveUp).expect("recovered");
    let settled = again.settle(Duration::from_secs(120));
    let held = holds(&world.borrow());
    eprintln!(
        "R1: settled={settled:?} outcomes={:?} holds={held:?} asked={}",
        again.outcomes(),
        world.borrow().daemon.asked()
    );
    assert!(
        !held.is_empty(),
        "the hold was released while the request still pends: {:?}",
        again.outcomes()
    );
}

/// R2. The same, played on to its end: after the person's give-up the
/// person destroys `pool@t` (no hold refuses it), the cut that would
/// carry the request is abandoned and its map goes to the cut after the
/// destroy, which publishes `snap/t` without `pool@t` (design §5.2: "That
/// rests on the snapshot still being in the pool at the later cut, so
/// the agent places a hold").
#[test]
fn r2_person_give_up_under_a_busy_uploader_can_publish_a_cut_without_the_snapshot() {
    let wrong: Vec<Mode> = [Mode::Idle, Mode::Abandoned]
        .into_iter()
        .filter(|&mode| r2_in(mode))
        .collect();
    assert!(
        wrong.is_empty(),
        "snap/t was published by a cut without pool@t after the person's give-up, in {wrong:?}"
    );
}

/// Whether `snap/t` was published by a cut without `pool@t`.
fn r2_in(mode: Mode) -> bool {
    let world = World::new(mode);
    world.borrow_mut().lag_ms = 60_000;
    let keep = MemKeep::default();
    let t = asked_and_stopped(&world, &keep);
    let silent = Rc::new(Cell::new(true));
    let daemon = Busy {
        inner: FakeDaemon(world.clone()),
        silent: silent.clone(),
    };
    let mut again = agent_with(&world, daemon, &keep);
    again.recover(&t, Choice::GiveUp).expect("recovered");
    let settled = again.settle(Duration::from_secs(60));
    let outcomes = again.outcomes().to_vec();
    drop(again);
    {
        let mut w = world.borrow_mut();
        w.user(User::Destroy, &t);
        w.advance(Duration::from_secs(150));
        // One more group on the pool (any writer): a cut after the
        // destroy that can take an abandoned cut's map.
        w.sync("later");
        w.advance(Duration::from_secs(150));
    }
    let w = world.borrow();
    let row = w.refs.get(&t).cloned();
    let cut_holds = row.as_ref().and_then(|r| {
        w.commits
            .get(&r.commit)
            .map(|c| c.cut.snaps.contains_key(&format!("{POOL}@{t}")))
    });
    let destroyed = !w
        .pool
        .as_ref()
        .is_some_and(|p| p.snaps.contains_key(&format!("{POOL}@{t}")));
    eprintln!(
        "R2 {mode:?}: settled={settled:?} outcomes={outcomes:?} destroyed={destroyed} \
         snap/t={} cut holds pool@t={cut_holds:?}\nlog={:?}",
        row.is_some(),
        w.log
    );
    row.is_some() && cut_holds == Some(false)
}

/// R3. A publish whose name is taken by a child's `@t` between `begin`
/// and the program takes the next name (the engine's
/// `a_name_a_person_took_first_is_passed_over`). `publish` must exit by
/// that publish's outcome, not 7 (unsettled) for a publish made.
#[test]
fn r3_publish_exits_by_its_outcome_after_its_name_moved() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let hook = {
        let world = world.clone();
        let done = Cell::new(false);
        move |p: Point, name: &str| {
            if p.effect == Effect::Intent && p.after && !done.get() && !name.is_empty() {
                done.set(true);
                let mut w = world.borrow_mut();
                let pool = w.pool.as_mut().expect("pool");
                let snap = fake::Snap {
                    guid: 0xdead,
                    createtxg: pool.txg,
                    holds: std::collections::BTreeSet::new(),
                    held_in: None,
                };
                pool.snaps.insert(format!("{POOL}/db@{name}"), snap);
            }
            false
        }
    };
    let mut agent =
        agent_with(&world, FakeDaemon(world.clone()), &keep).with_points(Box::new(hook));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = execute(&mut agent, &Task::Publish, LIMIT, &mut out, &mut err);
    let out = String::from_utf8_lossy(&out).into_owned();
    eprintln!(
        "R3: code={code} out={out:?} err={:?}",
        String::from_utf8_lossy(&err)
    );
    assert!(out.starts_with("snap/"), "{out}");
    assert_eq!(code, exit::PUBLISHED, "a publish made exits {code}: {out}");
}

/// R4. `settle` with one publish stopped for a person and another
/// still under way at the limit: which code wins (a reading, printed).
#[test]
fn r4_settle_stopped_and_unsettled() {
    let outcomes = vec![("a".to_string(), Outcome::Stopped("x".into()))];
    eprintln!(
        "R4: worst(stopped, unsettled) = {}",
        worst(&outcomes, true)
    );
}

fn taken() -> Taken {
    Taken {
        group: 40,
        members: vec![Member {
            name: POOL.into(),
            snapshot_guid: 7,
        }],
        manifest_len: 100,
        manifest_hash: [5; 32],
    }
}

fn publish_asked(person: Option<Person>) -> Publish {
    Publish {
        name: "skz-t".into(),
        count: Count {
            attachment: [1; 16],
            state: [2; 16],
            after: 10,
            load_guid: 99,
        },
        phase: Phase::Asked {
            taken: taken(),
            asked_ms: 1_000_000,
            held_until_ms: 1_000_000 + 640_000,
        },
        held: true,
        attempts: 1,
        stalls: 0,
        begun_ms: 1_000_000,
        person,
    }
}

fn seen_with(answer: Reply, snap: Option<SnapSeen>, too_old: bool, withdrawn: bool) -> Seen {
    Seen {
        pool: PoolSeen {
            load_guid: 99,
            premise: true,
            snapshot: Some(taken().members),
            held: true,
        },
        daemon: DaemonSeen::Here {
            cuts: CutsReply {
                attachment: [1; 16],
                state: Some([2; 16]),
                count: 11,
                newest: Some((41, true)),
            },
            answer,
        },
        snap,
        now_ms: 1_000_000 + 60_000,
        assisted_ms: None,
        assists: 0,
        too_old,
        withdrawn,
        assist_after: plan::ASSIST_AFTER,
    }
}

/// R5. The planner's table: a person's give-up against the age bound's
/// give-up (the order of every give-up), for each answer of the daemon,
/// before `held_until`, with the hold there. Printed; asserted only for
/// the answer that says the uploader is silent (a request may pend),
/// with no withdraw of its own answered.
#[test]
fn r5_person_give_up_against_the_age_bound_per_answer() {
    let answers: Vec<(&str, Reply)> = vec![
        ("pending", Reply::Pending),
        ("accepted", Reply::Accepted),
        ("absent", Reply::Absent),
        ("unknown", Reply::Unknown),
        ("expired", Reply::Expired),
        ("withdrawn", Reply::Withdrawn),
        ("done", Reply::Done([9; 32])),
        ("refused asked", Reply::Refused(Code::Asked, String::new())),
        ("refused busy", Reply::Refused(Code::Busy, String::new())),
        ("refused no-answer", Reply::Refused(Code::NoAnswer, String::new())),
        ("refused ended", Reply::Refused(Code::Ended, String::new())),
        ("refused no-upload", Reply::Refused(Code::NoUpload, String::new())),
        ("refused rewound", Reply::Refused(Code::Rewound, String::new())),
        ("refused stopped", Reply::Refused(Code::Stopped, String::new())),
        ("refused store", Reply::Refused(Code::Store, String::new())),
    ];
    let mut wrong = Vec::new();
    for (word, answer) in answers {
        for snap in [None, Some(SnapSeen::Absent)] {
            for withdrawn in [false, true] {
                let person = plan::plan(
                    &publish_asked(Some(Person::GiveUp)),
                    &seen_with(answer.clone(), snap, false, withdrawn),
                );
                let old = plan::plan(
                    &publish_asked(None),
                    &seen_with(answer.clone(), snap, true, withdrawn),
                );
                eprintln!(
                    "R5: {word:18} snap={snap:?} withdrawn={withdrawn}: person {person:?} | too-old {old:?}"
                );
                let silent = matches!(
                    answer,
                    Reply::Refused(Code::NoAnswer, _)
                );
                // With a withdraw of its own answered `withdrawn`,
                // `unknown` or `expired` (`Seen::withdrawn`), nothing
                // pends and a release is safe: asserted only without.
                if silent && !withdrawn && matches!(person, Action::Release | Action::Abandon(_))
                {
                    wrong.push(format!("{word} snap={snap:?} withdrawn={withdrawn}: {person:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "released with a silent uploader: {wrong:#?}");
}
