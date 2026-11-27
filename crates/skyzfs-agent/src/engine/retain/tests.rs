//! Retention on the fake world ([module docs](super)).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use super::super::fake::{
    FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, Snap, World, POOL, VOLUME,
};
use super::super::{Agent, Effect, Error, Outcome, Point, Points};
use super::{ABANDONED_KEEP_DEFAULT, STEPS_MAX, STUCK_FORGOTTEN, STUCK_STEPS};
use crate::plan::Report;
use crate::record::{
    Left, Member, Record, Retire, Stuck, Why, ABANDONED_MAX, RETIRING_MAX, STUCK_MAX,
};

type FakeAgent = Agent<FakeZfs, FakeDaemon, MemKeep, FakeClock>;

/// How long a publish may take to settle, in the world's time.
const LIMIT: Duration = Duration::from_secs(3 * 3600);

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

/// `n` publishes made and settled, a few seconds apart: the names
/// published, oldest first.
fn published(n: usize) -> (Rc<RefCell<World>>, MemKeep, Vec<String>) {
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let mut a = agent(&world, &keep);
    for _ in 0..n {
        a.begin().expect("begun");
        a.settle(LIMIT).expect("settled");
        world.borrow_mut().advance(Duration::from_secs(3));
    }
    let names: Vec<String> = a
        .record()
        .published
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names.len(), n, "{:?}", a.outcomes());
    (world, keep, names)
}

/// The `@t` of the pool and its datasets that the world has.
fn snaps_of(world: &Rc<RefCell<World>>, t: &str) -> Vec<String> {
    let suffix = format!("@{t}");
    world
        .borrow()
        .pool
        .as_ref()
        .map(|p| {
            p.snaps
                .keys()
                .filter(|k| k.ends_with(&suffix))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn retired(outcomes: &[(String, Outcome)]) -> Vec<String> {
    outcomes
        .iter()
        .filter(|(_, o)| *o == Outcome::Retired)
        .map(|(n, _)| n.clone())
        .collect()
}

/// Change the record in `keep` with `change`, as a person's or an older
/// agent's record would be.
fn change_record(keep: &MemKeep, change: impl FnOnce(&mut Record)) {
    let bytes = keep.0.borrow().record.clone().expect("a record");
    let mut record = Record::decode(&bytes).expect("decoded");
    change(&mut record);
    keep.0.borrow_mut().record = Some(record.encode().expect("encoded"));
}

/// The newest `--keep` are kept; the older are retired oldest first:
/// `snap/<t>` retired, every member's `@t` destroyed, `retired` told,
/// and the name gone from the record.
///
/// Mutants: the newest retired, one more or fewer kept, a member left,
/// the ref left, the name left in the record.
#[test]
fn the_names_published_past_keep_are_retired() {
    let (world, keep, names) = published(5);
    let mut a = agent(&world, &keep);
    a.set_keep(2);
    a.retain().expect("retained");
    assert_eq!(retired(a.outcomes()), names[..3].to_vec());
    let left: Vec<String> = a
        .record()
        .published
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(left, names[3..].to_vec());
    for t in &names[..3] {
        assert!(
            snaps_of(&world, t).is_empty(),
            "{t}: {:?}",
            snaps_of(&world, t)
        );
        assert!(!world.borrow().refs.contains_key(t), "snap/{t}");
    }
    for t in &names[3..] {
        assert!(!snaps_of(&world, t).is_empty(), "{t} kept");
        assert!(world.borrow().refs.contains_key(t), "snap/{t} kept");
    }
    // Nothing more to do: a second round does nothing.
    let calls = world.borrow().calls.len();
    a.retain().expect("retained again");
    assert_eq!(world.borrow().calls.len(), calls);
    assert_eq!(retired(a.outcomes()).len(), 3);
}

/// Each member's `@t` is destroyed alone, children before their
/// parents and `pool@t` last, so a stop half way leaves `pool@t` to read
/// the rest through.
///
/// Mutant: `pool@t` first (then the rest is never read again).
#[test]
fn the_members_go_before_the_pool_s_own() {
    let (world, keep, names) = published(2);
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    let t = &names[0];
    let destroyed: Vec<String> = world
        .borrow()
        .log
        .iter()
        .filter_map(|l| l.strip_prefix("agent destroy "))
        .filter(|l| l.ends_with(&format!("@{t}")))
        .map(String::from)
        .collect();
    assert!(destroyed.len() >= 2, "{destroyed:?}");
    assert_eq!(destroyed.last(), Some(&format!("{POOL}@{t}")));
    let depth = |s: &String| s.matches('/').count();
    for pair in destroyed.windows(2) {
        assert!(depth(&pair[0]) >= depth(&pair[1]), "{destroyed:?}");
    }
    assert!(!world
        .borrow()
        .log
        .iter()
        .any(|l| l.starts_with(&format!("agent destroy -r {t}"))));
}

/// An `@t` the pool has with another GUID than the one recorded is not
/// this publish's, and is left; the rest is retired.
///
/// Mutant: members destroyed by name alone.
#[test]
fn an_at_t_of_another_guid_is_left() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let other = {
        let mut w = world.borrow_mut();
        let pool = w.pool.as_mut().expect("the pool");
        let key = pool
            .snaps
            .keys()
            .find(|k| k.contains('/') && k.ends_with(&format!("@{t}")))
            .cloned()
            .expect("a child's @t");
        if let Some(snap) = pool.snaps.get_mut(&key) {
            snap.guid ^= 0xdead_beef;
        }
        key
    };
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    assert_eq!(retired(a.outcomes()), vec![t.clone()]);
    assert_eq!(snaps_of(&world, &t), vec![other]);
}

/// `refused other`: `snap/<t>` names another commit. The name stops for
/// a person, its local snapshot kept and no ref touched; told once in a
/// process, and again by the next.
///
/// Mutants: the members destroyed, the ref retired, the stop told at
/// each round, or not after a restart.
#[test]
fn a_ref_of_another_commit_stops_the_retirement_for_a_person() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    if let Some(r) = world.borrow_mut().refs.get_mut(&t) {
        r.commit = [0x77; 32];
    }
    let snaps = snaps_of(&world, &t);
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    a.retain().expect("retained again");
    let stops: Vec<&(String, Outcome)> = a
        .outcomes()
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Stopped(_)))
        .collect();
    assert_eq!(stops.len(), 1, "{:?}", a.outcomes());
    let Outcome::Stopped(why) = &stops[0].1 else {
        unreachable!()
    };
    assert!(why.starts_with("retire refused other: "), "{why}");
    assert!(retired(a.outcomes()).is_empty());
    assert_eq!(snaps_of(&world, &t), snaps);
    assert_eq!(
        world.borrow().refs.get(&t).map(|r| r.commit),
        Some([0x77; 32])
    );
    let p = a
        .record()
        .published
        .iter()
        .find(|p| p.name == t)
        .expect("kept");
    assert_eq!(p.retire, Some(Retire::Other));
    drop(a);
    let mut again = agent(&world, &keep);
    again.set_keep(1);
    again.retain().expect("retained by the next process");
    assert!(
        matches!(again.outcomes(), [(n, Outcome::Stopped(_))] if *n == t),
        "{:?}",
        again.outcomes()
    );
}

/// A destroy that fails is told with its reason once, tried at each
/// round, told again when the reason changes, and the name retired once
/// it succeeds. From its first failure the name is stuck and no longer
/// published (the lead's ruling on #179).
///
/// Mutants: told at each round, not told again on a new reason, given up
/// after a few tries, the name kept among those published.
#[test]
fn a_destroy_that_fails_is_told_once_a_reason_and_tried_each_round() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let key = format!("{POOL}@{t}");
    world
        .borrow_mut()
        .destroy_refused
        .insert(key.clone(), "first reason".into());
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    let not_retired = |a: &FakeAgent| -> Vec<String> {
        a.reports()
            .iter()
            .filter_map(|(n, r)| match r {
                Report::NotRetired(why) if *n == t => Some(why.clone()),
                _ => None,
            })
            .collect()
    };
    for _ in 0..3 {
        a.retain().expect("retained");
    }
    let told = not_retired(&a);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("first reason"), "{told:?}");
    let tries = world
        .borrow()
        .log
        .iter()
        .filter(|l| **l == format!("agent destroy {key}: refused"))
        .count();
    assert_eq!(tries, 3);
    world
        .borrow_mut()
        .destroy_refused
        .insert(key.clone(), "second reason".into());
    a.retain().expect("retained");
    assert_eq!(not_retired(&a).len(), 2);
    assert!(!a.record().published.iter().any(|p| p.name == t));
    let s = a
        .record()
        .stuck
        .iter()
        .find(|s| s.name == t)
        .expect("stuck");
    assert!(!s.members.is_empty());
    world.borrow_mut().destroy_refused.clear();
    a.retain().expect("retained");
    assert_eq!(retired(a.outcomes()), vec![t.clone()]);
    assert!(snaps_of(&world, &t).is_empty());
    assert!(a.record().stuck.is_empty());
}

/// A daemon that does not answer `retire` leaves the name to retire at a
/// later round: told once, nothing destroyed.
///
/// Mutant: an unanswered retire taken as `gone`.
#[test]
fn a_retire_not_answered_destroys_nothing() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let snaps = snaps_of(&world, &t);
    let away = world.borrow().now_ms + 3_600_000;
    world.borrow_mut().away_until = away;
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    a.retain().expect("retained again");
    assert_eq!(snaps_of(&world, &t), snaps);
    let told: Vec<&(String, Report)> = a
        .reports()
        .iter()
        .filter(|(_, r)| matches!(r, Report::NotRetired(_)))
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    let p = a
        .record()
        .published
        .iter()
        .find(|p| p.name == t)
        .expect("kept");
    assert_eq!(p.retire, Some(Retire::Ref));
    world.borrow_mut().away_until = 0;
    a.retain().expect("retained");
    assert_eq!(retired(a.outcomes()), vec![t]);
}

/// At most [`RETIRING_MAX`] are marked at once, and a round takes at
/// most [`STEPS_MAX`] steps: a backlog is worked off over rounds.
///
/// Mutants: no bound on the marks, or on a round's steps.
#[test]
fn a_backlog_is_worked_off_a_few_names_a_round() {
    let (world, keep, names) = published(STEPS_MAX + 4);
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    assert_eq!(retired(a.outcomes()).len(), STEPS_MAX);
    a.retain().expect("retained");
    assert_eq!(retired(a.outcomes()), names[..STEPS_MAX + 3].to_vec());
    const { assert!(RETIRING_MAX >= STEPS_MAX) };
}

/// A name stopped `other` takes no step of the round: it is told once
/// and not asked again, so the others past `--keep` are worked off at
/// [`STEPS_MAX`] a round all the same.
///
/// Mutant: a name stopped `other` counted as a step.
#[test]
fn a_name_stopped_other_takes_no_step() {
    let (world, keep, names) = published(STEPS_MAX + 4);
    change_record(&keep, |r| {
        for p in r.published.iter_mut().take(3) {
            p.retire = Some(Retire::Other);
        }
    });
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    assert_eq!(
        retired(a.outcomes()),
        names[3..3 + STEPS_MAX].to_vec(),
        "{:?}",
        a.outcomes()
    );
}

/// The names given up past their time are worked off at [`STEPS_MAX`]
/// a round too, oldest first.
///
/// Mutant: no bound on the names given up in a round.
#[test]
fn a_backlog_given_up_is_worked_off_a_few_names_a_round() {
    let (world, keep, names) = published(STEPS_MAX + 3);
    let now = world.borrow().now_ms;
    change_record(&keep, |r| {
        let published = std::mem::take(&mut r.published);
        for p in published {
            r.abandoned.push(crate::record::Abandoned {
                name: p.name,
                why: Why::NotWhole,
                at_ms: now,
                left: Left::Members(p.members),
            });
        }
    });
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    let mut a = agent(&world, &keep);
    a.retain().expect("retained");
    assert_eq!(retired(a.outcomes()), names[..STEPS_MAX].to_vec());
    assert_eq!(a.record().abandoned.len(), 3);
    a.retain().expect("retained again");
    assert_eq!(retired(a.outcomes()), names);
    assert!(a.record().abandoned.is_empty());
}

/// The marks are bounded too: with more past `--keep` than
/// [`RETIRING_MAX`], no more are marked than that.
#[test]
fn no_more_are_marked_than_retiring_max() {
    let (world, keep, _) = published(3);
    change_record(&keep, |r| {
        let template = r.published[0].clone();
        r.published.clear();
        for i in 0..RETIRING_MAX + 5 {
            let mut p = template.clone();
            p.name = format!("{}x{i}", template.name);
            r.published.push(p);
        }
    });
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    // The daemon is away: the marks are made and nothing moves on.
    let away = world.borrow().now_ms + 3_600_000;
    world.borrow_mut().away_until = away;
    a.retain().expect("retained");
    let marked = a
        .record()
        .published
        .iter()
        .filter(|p| p.retire.is_some())
        .count();
    assert_eq!(marked, RETIRING_MAX);
}

/// Every stop of retention, at each of its points, then a new process:
/// each name past `--keep` is retired, its `@t` destroyed and its ref
/// gone, and `retired` told at least once for each and for no other.
///
/// Mutants: a step saved after the next is made, `retired` told after
/// the name leaves.
#[test]
fn a_stop_at_any_point_of_retention_is_taken_up_by_the_next_process() {
    let effects = [
        Effect::Retiring,
        Effect::Retire,
        Effect::Retired,
        Effect::DestroyMember,
        Effect::Retain,
    ];
    for effect in effects {
        for after in [false, true] {
            for nth in 1..=3 {
                let (world, keep, names) = published(3);
                let seen = Rc::new(RefCell::new(0usize));
                let points: Points = {
                    let seen = seen.clone();
                    Box::new(move |p: Point, _: &str| {
                        if p.effect == effect && p.after == after {
                            *seen.borrow_mut() += 1;
                            return *seen.borrow() == nth;
                        }
                        false
                    })
                };
                let mut first = agent(&world, &keep).with_points(points);
                first.set_keep(1);
                let stopped = first.retain();
                let mut told = retired(first.outcomes());
                drop(first);
                let mut next = agent(&world, &keep);
                next.set_keep(1);
                next.retain().expect("retained by the next process");
                told.extend(retired(next.outcomes()));
                let at = format!("{effect:?} after={after} nth={nth} ({stopped:?})");
                let told: BTreeSet<String> = told.into_iter().collect();
                let want: BTreeSet<String> = names[..2].iter().cloned().collect();
                assert_eq!(told, want, "{at}");
                for t in &names[..2] {
                    assert!(snaps_of(&world, t).is_empty(), "{at}: {t}");
                    assert!(!world.borrow().refs.contains_key(t), "{at}: snap/{t}");
                }
                assert!(!snaps_of(&world, &names[2]).is_empty(), "{at}");
                let left: Vec<&str> = next
                    .record()
                    .published
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                assert_eq!(left, vec![names[2].as_str()], "{at}");
                if matches!(stopped, Err(ref e) if !matches!(e, Error::Stopped)) {
                    panic!("{at}: {stopped:?}");
                }
            }
        }
    }
}

/// A name given up is kept for `--abandoned-keep-s`, then by what it
/// left: nothing leaves with no line; members are destroyed and
/// `retired` told; no GUIDs is told once while it is kept, then
/// `forgotten`, nothing destroyed.
///
/// Mutants: a name let go before its time, members not destroyed, a
/// snapshot destroyed with no GUIDs, `no-guids` told at each round.
#[test]
fn the_names_given_up_go_after_their_time_by_what_they_left() {
    let (world, keep, names) = published(3);
    let now = world.borrow().now_ms;
    let members = |r: &Record, t: &str| -> Vec<Member> {
        r.published
            .iter()
            .find(|p| p.name == *t)
            .map(|p| p.members.clone())
            .expect("its members")
    };
    let (gone, kept, unknown) = (names[0].clone(), names[1].clone(), names[2].clone());
    change_record(&keep, |r| {
        let left = members(r, &gone);
        r.published.clear();
        for (name, left) in [
            (&gone, Left::Members(left)),
            (&kept, Left::Nothing),
            (&unknown, Left::Unknown),
        ] {
            r.abandoned.push(crate::record::Abandoned {
                name: name.clone(),
                why: Why::NotWhole,
                at_ms: now,
                left,
            });
        }
    });
    let unknown_snaps = snaps_of(&world, &unknown);
    let mut a = agent(&world, &keep);
    a.retain().expect("retained");
    a.retain().expect("retained again");
    assert_eq!(a.record().abandoned.len(), 3, "kept before their time");
    assert!(!snaps_of(&world, &gone).is_empty());
    let no_guids = |a: &FakeAgent| {
        a.reports()
            .iter()
            .filter(|(n, r)| *n == unknown && *r == Report::NoGuids)
            .count()
    };
    assert_eq!(no_guids(&a), 1);
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    a.retain().expect("retained at the bound");
    assert!(
        a.record().abandoned.is_empty(),
        "{:?}",
        a.record().abandoned
    );
    assert_eq!(retired(a.outcomes()), vec![gone.clone()]);
    assert!(snaps_of(&world, &gone).is_empty());
    assert!(
        a.outcomes().contains(&(
            unknown.clone(),
            Outcome::Forgotten("no member guids; destroy by hand".into())
        )),
        "{:?}",
        a.outcomes()
    );
    assert_eq!(snaps_of(&world, &unknown), unknown_snaps);
    assert!(!a.outcomes().iter().any(|(n, _)| *n == kept));
}

/// A name a person gave up is looked up before its members are
/// destroyed: where `snap/<t>` is there, nothing is destroyed or
/// retired, and the name is forgotten for a person; where it is not,
/// its members are destroyed.
///
/// Mutants: no look, the ref retired with the look's commit, the
/// snapshot destroyed where the ref is there.
#[test]
fn a_name_a_person_gave_up_is_looked_up_first() {
    let (world, keep, names) = published(2);
    let now = world.borrow().now_ms;
    let (there, absent) = (names[0].clone(), names[1].clone());
    world.borrow_mut().refs.remove(&absent);
    change_record(&keep, |r| {
        let published = std::mem::take(&mut r.published);
        for p in published {
            r.abandoned.push(crate::record::Abandoned {
                name: p.name,
                why: Why::Person,
                at_ms: now,
                left: Left::Members(p.members),
            });
        }
    });
    let kept = snaps_of(&world, &there);
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    let mut a = agent(&world, &keep);
    // Only the retain's own calls: the publishes above made theirs.
    world.borrow_mut().calls.clear();
    a.retain().expect("retained");
    assert!(a.record().abandoned.is_empty());
    assert_eq!(snaps_of(&world, &there), kept);
    assert!(
        world.borrow().refs.contains_key(&there),
        "the ref is touched"
    );
    assert!(
        matches!(
            a.outcomes().iter().find(|(n, _)| *n == there),
            Some((_, Outcome::Forgotten(why))) if why.contains("snap/t exists")
        ),
        "{:?}",
        a.outcomes()
    );
    assert_eq!(retired(a.outcomes()), vec![absent.clone()]);
    assert!(snaps_of(&world, &absent).is_empty());
    let looks = world
        .borrow()
        .calls
        .iter()
        .filter(|c| **c == "look")
        .count();
    assert_eq!(looks, 2);
    assert!(!world.borrow().calls.contains(&"retire"));
}

/// `n` names stuck, oldest first, seeded in the record: each with its
/// `pool@<name>` in the world, whose destroy ZFS refuses.
fn seed_stuck(world: &Rc<RefCell<World>>, keep: &MemKeep, n: usize) -> Vec<String> {
    const GUID: u64 = 0x5700_0000;
    let names: Vec<String> = (0..n).map(|i| format!("skz-20261001T{i:06}Z")).collect();
    {
        let mut w = world.borrow_mut();
        for (i, name) in names.iter().enumerate() {
            let key = format!("{POOL}@{name}");
            w.pool.as_mut().expect("the pool").snaps.insert(
                key.clone(),
                Snap {
                    guid: GUID + i as u64,
                    createtxg: 1,
                    holds: BTreeSet::new(),
                    held_in: None,
                },
            );
            w.destroy_refused.insert(key, "a clone of it".into());
        }
    }
    change_record(keep, |r| {
        for (i, name) in names.iter().enumerate() {
            r.stuck.push(Stuck {
                name: name.clone(),
                members: vec![Member {
                    name: POOL.into(),
                    snapshot_guid: GUID + i as u64,
                }],
                at_ms: 1,
            });
        }
    });
    names
}

/// How many destroys of `pool@<t>` ZFS has refused.
fn refused(world: &Rc<RefCell<World>>, t: &str) -> usize {
    let line = format!("agent destroy {POOL}@{t}: refused");
    world.borrow().log.iter().filter(|l| **l == line).count()
}

/// With [`STUCK_MAX`] stuck, one more stuck forgets the oldest: it is
/// told `forgotten <t> (destroy failed; destroy by hand)` once, its
/// snapshot is left, and the new name is the newest stuck.
///
/// Mutants: none forgotten past the cap, the newest forgotten, the
/// oldest forgotten with no line.
#[test]
fn past_stuck_max_the_oldest_stuck_is_forgotten() {
    let (world, keep, names) = published(2);
    let seeds = seed_stuck(&world, &keep, STUCK_MAX);
    let t = names[0].clone();
    world
        .borrow_mut()
        .destroy_refused
        .insert(format!("{POOL}@{t}"), "held by another".into());
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    let stuck: Vec<&str> = a.record().stuck.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(stuck.len(), STUCK_MAX);
    assert_eq!(stuck.first().copied(), Some(seeds[1].as_str()));
    assert_eq!(stuck.last().copied(), Some(t.as_str()));
    let forgotten: Vec<&(String, Outcome)> = a
        .outcomes()
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Forgotten(_)))
        .collect();
    assert_eq!(
        forgotten,
        vec![&(seeds[0].clone(), Outcome::Forgotten(STUCK_FORGOTTEN.into()))]
    );
    assert_eq!(STUCK_FORGOTTEN, "destroy failed; destroy by hand");
    assert_eq!(snaps_of(&world, &seeds[0]).len(), 1, "left for a person");
}

/// The names stuck are tried at most [`STUCK_STEPS`] a round, in turn:
/// at the cap, every name is tried once in each `STUCK_MAX /
/// STUCK_STEPS` rounds, none twice before all were tried, and each is
/// told once while its reason stays.
///
/// Mutants: all tried each round, the turn begun again at the first
/// name each round (the later names starved), told at each round.
#[test]
fn the_names_stuck_are_tried_a_few_a_round_in_turn() {
    let (world, keep, names) = published(2);
    let seeds = seed_stuck(&world, &keep, STUCK_MAX - 1);
    let t = names[0].clone();
    world
        .borrow_mut()
        .destroy_refused
        .insert(format!("{POOL}@{t}"), "held by another".into());
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    let mut all: Vec<String> = seeds.clone();
    all.push(t.clone());
    let tries = |world: &Rc<RefCell<World>>| -> Vec<usize> {
        all.iter().map(|n| refused(world, n)).collect()
    };
    // The first round tries the first STUCK_STEPS by name, and then
    // retires `t`, whose destroy fails: it is stuck, the cap reached.
    a.retain().expect("retained");
    assert_eq!(a.record().stuck.len(), STUCK_MAX);
    let first = tries(&world);
    assert_eq!(first.iter().sum::<usize>(), STUCK_STEPS + 1, "{first:?}");
    let rounds = STUCK_MAX / STUCK_STEPS;
    let mut before = first.clone();
    for round in 0..rounds {
        a.retain().expect("retained");
        let now = tries(&world);
        let tried: usize = now.iter().zip(&before).map(|(n, b)| n - b).sum();
        assert_eq!(tried, STUCK_STEPS, "round {round}");
        assert!(now.iter().zip(&before).all(|(n, b)| n - b <= 1));
        before = now;
    }
    // In the `rounds` after the first, each name was tried once: none
    // starved, none tried twice.
    let more: Vec<usize> = tries(&world)
        .iter()
        .zip(&first)
        .map(|(n, f)| n - f)
        .collect();
    assert!(more.iter().all(|m| *m == 1), "{more:?}");
    let told = a
        .reports()
        .iter()
        .filter(|(n, r)| *n == seeds[1] && matches!(r, Report::NotRetired(_)))
        .count();
    assert_eq!(told, 1);
    assert_eq!(a.record().stuck.len(), STUCK_MAX);
}

/// The oldest name stuck is told `forgotten` before the save that drops
/// it: a stop at that save has told it, and the record still keeps it.
///
/// Mutant: told after the save.
#[test]
fn the_oldest_stuck_is_told_before_it_is_dropped() {
    let (world, keep, names) = published(2);
    let seeds = seed_stuck(&world, &keep, STUCK_MAX);
    world
        .borrow_mut()
        .destroy_refused
        .insert(format!("{POOL}@{}", names[0]), "held by another".into());
    let points: Points = Box::new(|p: Point, _: &str| p.effect == Effect::Retain && !p.after);
    let mut a = agent(&world, &keep).with_points(points);
    a.set_keep(1);
    assert!(matches!(a.retain(), Err(Error::Stopped)));
    assert!(
        a.outcomes()
            .contains(&(seeds[0].clone(), Outcome::Forgotten(STUCK_FORGOTTEN.into()))),
        "{:?}",
        a.outcomes()
    );
    drop(a);
    let bytes = keep.0.borrow().record.clone().expect("a record");
    let record = Record::decode(&bytes).expect("decoded");
    assert!(record.stuck.iter().any(|s| s.name == seeds[0]));
}

/// A name given up whose members' destroy fails past its time is stuck
/// too (this module's reading of the lead's ruling), and leaves the
/// names given up.
///
/// Mutant: a name given up left among the given up.
#[test]
fn a_name_given_up_whose_destroy_fails_is_stuck() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let now = world.borrow().now_ms;
    change_record(&keep, |r| {
        let i = r.published.iter().position(|p| p.name == t).expect("it");
        let p = r.published.remove(i);
        r.abandoned.push(crate::record::Abandoned {
            name: p.name,
            why: Why::NotWhole,
            at_ms: now,
            left: Left::Members(p.members),
        });
    });
    world
        .borrow_mut()
        .destroy_refused
        .insert(format!("{POOL}@{t}"), "a clone of it".into());
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    let mut a = agent(&world, &keep);
    a.retain().expect("retained");
    assert!(
        a.record().abandoned.is_empty(),
        "{:?}",
        a.record().abandoned
    );
    let stuck: Vec<&str> = a.record().stuck.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(stuck, vec![t.as_str()]);
    world.borrow_mut().destroy_refused.clear();
    a.retain().expect("retained again");
    assert_eq!(retired(a.outcomes()), vec![t.clone()]);
    assert!(a.record().stuck.is_empty());
    assert!(snaps_of(&world, &t).is_empty());
}

/// Every stop of the retry of a name stuck, then a new process: the
/// name is retired, told at least once, and leaves.
///
/// Mutant: the name left stuck before `retired` is told.
#[test]
fn a_stop_in_the_retry_of_a_name_stuck_is_taken_up() {
    for effect in [Effect::DestroyMember, Effect::Retain] {
        for after in [false, true] {
            let (world, keep, names) = published(2);
            let t = names[0].clone();
            let key = format!("{POOL}@{t}");
            world
                .borrow_mut()
                .destroy_refused
                .insert(key.clone(), "a clone of it".into());
            let mut a = agent(&world, &keep);
            a.set_keep(1);
            a.retain().expect("stuck");
            assert_eq!(a.record().stuck.len(), 1);
            drop(a);
            world.borrow_mut().destroy_refused.clear();
            let points: Points =
                Box::new(move |p: Point, _: &str| p.effect == effect && p.after == after);
            let mut first = agent(&world, &keep).with_points(points);
            first.set_keep(1);
            let stopped = first.retain();
            let at = format!("{effect:?} after={after} ({stopped:?})");
            assert!(matches!(stopped, Err(Error::Stopped)), "{at}");
            let mut told = retired(first.outcomes());
            drop(first);
            let mut next = agent(&world, &keep);
            next.set_keep(1);
            next.retain().expect("retained by the next process");
            told.extend(retired(next.outcomes()));
            assert!(told.contains(&t), "{at}");
            assert!(told.iter().all(|n| *n == t), "{at}: {told:?}");
            assert!(next.record().stuck.is_empty(), "{at}");
            assert!(snaps_of(&world, &t).is_empty(), "{at}");
        }
    }
}

/// What the round asks of ZFS and the daemon: the calls counted by the
/// halt tests.
fn asks(world: &Rc<RefCell<World>>) -> usize {
    world
        .borrow()
        .calls
        .iter()
        .filter(|c| ["retire", "look", "members", "destroy", "destroy_one"].contains(c))
        .count()
}

/// Two names past `--keep`, two given up past their time (one by a
/// person, whose `snap/T` is absent, so that it is looked up and then
/// destroyed), and one stuck: a round with a step of each kind.
fn every_kind() -> (Rc<RefCell<World>>, MemKeep, Vec<String>) {
    let (world, keep, names) = published(6);
    world.borrow_mut().refs.remove(&names[2]);
    let now = world.borrow().now_ms;
    change_record(&keep, |r| {
        let given_up = r.published.remove(0);
        r.abandoned.push(crate::record::Abandoned {
            name: given_up.name,
            why: Why::ManifestLost,
            at_ms: now,
            left: Left::Members(given_up.members),
        });
        let stuck = r.published.remove(0);
        r.stuck.push(Stuck {
            name: stuck.name,
            members: stuck.members,
            at_ms: now,
        });
        let person = r.published.remove(0);
        r.abandoned.push(crate::record::Abandoned {
            name: person.name,
            why: Why::Person,
            at_ms: now,
            left: Left::Members(person.members),
        });
    });
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    world.borrow_mut().calls.clear();
    (world, keep, names)
}

/// An end asked (`go` says no), or news on the failure note, at any
/// point of a round: retention asks nothing more of ZFS or the daemon,
/// ends with no error, and the next round retires the rest, each name
/// told once (M2 of the review of #179, and Codex's P1 on retain.rs:176).
/// `go` and the note are asked before each step, each name stuck tried
/// again and each call to ZFS; a round with nothing to stop it makes 23
/// asks here (two retires, one look, five members reads, fifteen
/// destroys), and the stop is put before each of them: it must come
/// before that ask, not at the next ask after it.
///
/// Mutants: `go` not asked before a step, a look, a members read or a
/// member's destroy; the note not asked there; a halt that goes on to
/// the next step.
#[test]
fn an_end_or_a_note_stops_retention_before_its_next_ask() {
    use skyzfs_control::failure::{Failure, Held, Reason};

    use crate::engine::{Failed, Noted};

    let whole = {
        let (world, keep, _) = every_kind();
        let mut a = agent(&world, &keep);
        a.set_keep(1);
        a.retain().expect("retained");
        assert!(a.record().stuck.is_empty() && a.record().abandoned.is_empty());
        assert_eq!(a.record().published.len(), 1);
        asks(&world)
    };
    assert_eq!(whole, 23, "the round's asks");
    for note in [false, true] {
        for k in 0..whole {
            let (world, keep, names) = every_kind();
            let mut a = agent(&world, &keep);
            a.set_keep(1);
            a.watch_note(Some(Failed::No));
            let stopped_at = Rc::new(RefCell::new(None));
            let mut go = {
                let world = world.clone();
                let stopped_at = stopped_at.clone();
                move || {
                    if stopped_at.borrow().is_some() || asks(&world) < k {
                        return stopped_at.borrow().is_none();
                    }
                    *stopped_at.borrow_mut() = Some(asks(&world));
                    if !note {
                        return false;
                    }
                    let mut w = world.borrow_mut();
                    w.noted = Some(Noted::Failed(Failure {
                        held: Held::Yes,
                        reason: Reason::ImageFailed,
                        identity: [7; 16],
                    }));
                    w.advance(Duration::from_secs(1));
                    true
                }
            };
            let at = format!("note={note} k={k}");
            a.retain_while(&mut go).expect(&at);
            let stopped = stopped_at.borrow().expect(&at);
            assert_eq!(stopped, k, "{at}: no ask of `go` before the ask");
            assert_eq!(asks(&world), stopped, "{at}: asked after the stop");
            let mut told = retired(a.outcomes());
            drop(a);
            world.borrow_mut().noted = None;
            let mut next = agent(&world, &keep);
            next.set_keep(1);
            next.retain().expect(&at);
            told.extend(retired(next.outcomes()));
            told.sort();
            assert_eq!(told, names[..5].to_vec(), "{at}");
            assert!(next.record().stuck.is_empty(), "{at}");
            assert!(next.record().abandoned.is_empty(), "{at}");
            for t in &names[..5] {
                assert!(snaps_of(&world, t).is_empty(), "{at}: {t}");
            }
        }
    }
}

/// A name whose ref is retired, with the pool not imported when its
/// members are read, stays in the record, and is retired once the pool
/// is back (review's scratch test G-R12 on #179). Mutant: a members read
/// that fails lets the name leave with its `@t`s (R12, P2).
#[test]
fn a_members_read_that_fails_keeps_the_name() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let snaps = snaps_of(&world, &t);
    let pool = world.borrow_mut().pool.take();
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    assert!(!world.borrow().refs.contains_key(&t), "the ref is retired");
    // Still in the record (stuck), and not told retired.
    assert!(a.record().names(&t));
    assert!(retired(a.outcomes()).is_empty(), "{:?}", a.outcomes());
    world.borrow_mut().pool = pool;
    assert_eq!(snaps_of(&world, &t), snaps);
    a.retain().expect("retained again");
    assert_eq!(retired(a.outcomes()), vec![t.clone()]);
    assert!(snaps_of(&world, &t).is_empty());
}

/// The fake daemon, whose `look` of a name in `unreadable` answers
/// `unreadable` (snap/T there, its row not read).
struct LookUnreadable {
    daemon: FakeDaemon,
    unreadable: String,
}

impl super::super::Daemon for LookUnreadable {
    fn ask(
        &mut self,
        request: &skyzfs_control::Request,
        manifest: &[u8],
    ) -> std::io::Result<skyzfs_control::Reply> {
        if let skyzfs_control::Request::Look(name) = request {
            if *name == self.unreadable {
                return Ok(skyzfs_control::Reply::Unreadable(
                    skyzfs_control::UnreadableReply {
                        commit: [0x55; 32],
                        why: skyzfs_control::Unread::NoRow,
                    },
                ));
            }
        }
        self.daemon.ask(request, manifest)
    }
    fn device(&mut self) -> std::io::Result<u64> {
        self.daemon.device()
    }
    fn failure(&mut self) -> std::io::Result<Option<crate::engine::Noted>> {
        self.daemon.failure()
    }
}

/// A name a person gave up whose `snap/T` answers `unreadable` is
/// there: nothing is destroyed, and it is forgotten for a person
/// (review's G-R15 on #179). Mutant: `unreadable` taken as absent.
#[test]
fn an_unreadable_snap_t_is_there() {
    let (world, keep, names) = published(1);
    let t = names[0].clone();
    let now = world.borrow().now_ms;
    change_record(&keep, |r| {
        let published = std::mem::take(&mut r.published);
        for p in published {
            r.abandoned.push(crate::record::Abandoned {
                name: p.name,
                why: Why::Person,
                at_ms: now,
                left: Left::Members(p.members),
            });
        }
    });
    let kept = snaps_of(&world, &t);
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    let mut a = Agent::new(
        FakeZfs(world.clone()),
        LookUnreadable {
            daemon: FakeDaemon(world.clone()),
            unreadable: t.clone(),
        },
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent");
    a.retain().expect("retained");
    assert_eq!(snaps_of(&world, &t), kept);
    assert!(
        matches!(
            a.outcomes().iter().find(|(n, _)| *n == t),
            Some((_, Outcome::Forgotten(why))) if why.contains("snap/t exists")
        ),
        "{:?}",
        a.outcomes()
    );
}

/// Names given up by a person, past their time, with their members.
fn given_up_by_a_person(n: usize) -> (Rc<RefCell<World>>, MemKeep, Vec<String>) {
    let (world, keep, names) = published(n);
    let now = world.borrow().now_ms;
    change_record(&keep, |r| {
        let published = std::mem::take(&mut r.published);
        for p in published {
            r.abandoned.push(crate::record::Abandoned {
                name: p.name,
                why: Why::Person,
                at_ms: now,
                left: Left::Members(p.members),
            });
        }
    });
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    (world, keep, names)
}

/// A name a person gave up, whose look is not answered (the daemon
/// away): nothing destroyed, the name kept, nothing told (review's G-R16
/// on #179). Mutant: a look not answered taken as absent.
#[test]
fn a_look_not_answered_destroys_nothing() {
    let (world, keep, names) = given_up_by_a_person(1);
    let t = names[0].clone();
    let kept = snaps_of(&world, &t);
    let away = world.borrow().now_ms + 60_000;
    world.borrow_mut().away_until = away;
    let mut a = agent(&world, &keep);
    a.retain().expect("retained");
    assert_eq!(snaps_of(&world, &t), kept);
    assert_eq!(a.record().abandoned.len(), 1);
    assert!(a.outcomes().is_empty(), "{:?}", a.outcomes());
}

/// The same, with the look refused (`refused store`; review's G-R16b on
/// #179). Mutant: a look refused taken as absent.
#[test]
fn a_look_refused_destroys_nothing() {
    let (world, keep, names) = given_up_by_a_person(1);
    let t = names[0].clone();
    let kept = snaps_of(&world, &t);
    let mut a = Agent::new(
        FakeZfs(world.clone()),
        LookRefused {
            daemon: FakeDaemon(world.clone()),
            code: skyzfs_control::Code::Store,
            looks: Rc::default(),
        },
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent");
    a.retain().expect("retained");
    assert_eq!(snaps_of(&world, &t), kept);
    assert_eq!(a.record().abandoned.len(), 1);
    assert!(a.outcomes().is_empty(), "{:?}", a.outcomes());
}

/// The fake daemon, whose `look` is refused with `code`; `looks`
/// counts them.
struct LookRefused {
    daemon: FakeDaemon,
    code: skyzfs_control::Code,
    looks: Rc<std::cell::Cell<usize>>,
}

impl super::super::Daemon for LookRefused {
    fn ask(
        &mut self,
        request: &skyzfs_control::Request,
        manifest: &[u8],
    ) -> std::io::Result<skyzfs_control::Reply> {
        if let skyzfs_control::Request::Look(_) = request {
            self.looks.set(self.looks.get() + 1);
            return Ok(skyzfs_control::Reply::Refused(
                self.code,
                "object storage: away".into(),
            ));
        }
        self.daemon.ask(request, manifest)
    }
    fn device(&mut self) -> std::io::Result<u64> {
        self.daemon.device()
    }
    fn failure(&mut self) -> std::io::Result<Option<crate::engine::Noted>> {
        self.daemon.failure()
    }
}

/// A name given up past its time, with members: a stop just after the
/// save that drops it has told `retired` already (review's G-R21 on
/// #179). Mutant: told after the name leaves.
#[test]
fn a_given_up_name_is_told_before_it_leaves() {
    let (world, keep, names) = published(1);
    let t = names[0].clone();
    let now = world.borrow().now_ms;
    change_record(&keep, |r| {
        let published = std::mem::take(&mut r.published);
        for p in published {
            r.abandoned.push(crate::record::Abandoned {
                name: p.name,
                why: Why::ManifestLost,
                at_ms: now,
                left: Left::Members(p.members),
            });
        }
    });
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    let points: Points = Box::new(|p: Point, _: &str| p.effect == Effect::Retain && p.after);
    let mut a = agent(&world, &keep).with_points(points);
    let stopped = a.retain();
    assert!(stopped.is_err(), "the stop");
    assert!(agent(&world, &keep).record().abandoned.is_empty());
    assert_eq!(retired(a.outcomes()), vec![t]);
}

/// A name given up with no GUIDs, past its time: a stop just after the
/// save that drops it has told `forgotten` already (review's G-R22 on
/// #179). Mutant: told after the name leaves.
#[test]
fn a_forgotten_name_is_told_before_it_leaves() {
    let (world, keep, names) = published(1);
    let t = names[0].clone();
    let now = world.borrow().now_ms;
    change_record(&keep, |r| {
        let published = std::mem::take(&mut r.published);
        for p in published {
            r.abandoned.push(crate::record::Abandoned {
                name: p.name,
                why: Why::ManifestLost,
                at_ms: now,
                left: Left::Unknown,
            });
        }
    });
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    let points: Points = Box::new(|p: Point, _: &str| p.effect == Effect::Retain && p.after);
    let mut a = agent(&world, &keep).with_points(points);
    let stopped = a.retain();
    assert!(stopped.is_err(), "the stop");
    assert!(agent(&world, &keep).record().abandoned.is_empty());
    assert!(
        matches!(a.outcomes(), [(n, Outcome::Forgotten(_))] if *n == t),
        "{:?}",
        a.outcomes()
    );
}

/// A publish given up after its take (the uploader stopped) records its
/// members, so retention destroys its `@t`s after `--abandoned-keep-s`
/// (review's G-E4 on #179). Mutant: a take recorded as left=unknown.
#[test]
fn a_give_up_after_the_take_records_its_members() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().refuse_every_publish = Some(skyzfs_control::Code::Stopped);
    let keep = MemKeep::default();
    let mut a = agent(&world, &keep);
    let t = a.begin().expect("begun");
    let _ = a.settle(LIMIT);
    let given_up = a.record().abandoned.iter().find(|g| g.name == t).cloned();
    let Some(given_up) = given_up else {
        panic!("not given up: {:?}", a.outcomes());
    };
    assert!(
        matches!(&given_up.left, Left::Members(m) if !m.is_empty()),
        "{:?}",
        given_up.left
    );
    world.borrow_mut().refuse_every_publish = None;
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    a.retain().expect("retained");
    assert!(
        snaps_of(&world, &t).is_empty(),
        "{:?}",
        snaps_of(&world, &t)
    );
}

/// The turn of the names stuck goes by name, whatever their order in
/// the record, and holds across [`Agent::restart`] within the process;
/// a name whose retry succeeds leaves alone, the others stay stuck
/// (Q8, Q9 and M5 of the review of #179).
///
/// Mutants: the names tried in the record's order (Q8); the turn begun
/// again at the first name by [`Agent::restart`] (Q9); a retry that
/// succeeds clears every name stuck (M5).
#[test]
fn the_turn_goes_by_name_holds_across_a_restart_and_clears_one_name() {
    let (world, keep, _) = published(1);
    let seeds = seed_stuck(&world, &keep, 2 * STUCK_STEPS);
    // The record holds them newest first: not the names' order.
    change_record(&keep, |r| r.stuck.reverse());
    let mut a = agent(&world, &keep);
    a.retain().expect("retained");
    let tried = |world: &Rc<RefCell<World>>| -> Vec<usize> {
        seeds.iter().map(|n| refused(world, n)).collect()
    };
    let first = tried(&world);
    let mut expected = vec![1; STUCK_STEPS];
    expected.extend(vec![0; STUCK_STEPS]);
    assert_eq!(first, expected, "the first STUCK_STEPS by name");
    a.restart().expect("restarted");
    // One of the next names can go now.
    let freed = seeds[STUCK_STEPS + 1].clone();
    world
        .borrow_mut()
        .destroy_refused
        .remove(&format!("{POOL}@{freed}"));
    a.retain().expect("retained");
    let second = tried(&world);
    let mut expected = vec![1; 2 * STUCK_STEPS];
    expected[STUCK_STEPS + 1] = 0;
    assert_eq!(second, expected, "the next STUCK_STEPS by name");
    assert_eq!(retired(a.outcomes()), vec![freed.clone()]);
    let stuck: BTreeSet<&str> = a.record().stuck.iter().map(|s| s.name.as_str()).collect();
    let others: BTreeSet<&str> = seeds
        .iter()
        .filter(|n| **n != freed)
        .map(String::as_str)
        .collect();
    assert_eq!(stuck, others);
}

/// A name stuck keeps every member it had, so that each `@t` left is
/// destroyed at a retry (M7 of the review of #179).
///
/// Mutant: a name stuck keeps its first member only.
#[test]
fn a_name_stuck_keeps_all_its_members() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let members = agent(&world, &keep).record().published[0].members.clone();
    assert!(members.len() > 1, "{members:?}");
    // The first destroy, of a child, is refused: every `@t` is left.
    let child = members
        .iter()
        .find(|m| m.name != POOL)
        .expect("a child")
        .name
        .clone();
    world
        .borrow_mut()
        .destroy_refused
        .insert(format!("{child}@{t}"), "a clone of it".into());
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    let [stuck] = a.record().stuck.as_slice() else {
        panic!("{:?}", a.record().stuck);
    };
    assert_eq!(stuck.members, members);
    world.borrow_mut().destroy_refused.clear();
    a.retain().expect("retained again");
    assert!(
        snaps_of(&world, &t).is_empty(),
        "{:?}",
        snaps_of(&world, &t)
    );
    assert_eq!(retired(a.outcomes()), vec![t]);
}

/// Eight names whose destroy is refused for good (a person's clone of
/// each, or a hold of a backup tool) do not starve the names after
/// them: they move to the names stuck and take no step of a later
/// round (L1 of the review of #179; its scratch test T4, which failed
/// before the names stuck). Mutant: a name whose destroy fails stays
/// among the names published.
#[test]
fn names_that_cannot_be_destroyed_do_not_starve_the_rest() {
    let (world, keep, names) = published(12);
    for t in &names[..STEPS_MAX] {
        world.borrow_mut().destroy_refused.insert(
            format!("{POOL}/db@{t}"),
            "cannot destroy: filesystem has dependent clones".into(),
        );
    }
    for _ in 0..6 {
        let mut a = agent(&world, &keep);
        a.set_keep(1);
        a.retain().expect("retained");
    }
    let a = agent(&world, &keep);
    let left: Vec<&String> = names[STEPS_MAX..names.len() - 1]
        .iter()
        .filter(|n| a.record().names(n))
        .collect();
    assert!(left.is_empty(), "never retired: {left:?}");
    assert_eq!(a.record().stuck.len(), STEPS_MAX);
}

/// A destroy or a read of the members that the host does not run to its
/// end (killed at its limit, or held back by the commands left running)
/// is not a failed destroy: the name stays among the published, is
/// told, and is tried again; it is never stuck (the lead's ruling on
/// decision 4 of #179's review). Mutants: a destroy not run taken for
/// a failed one, and so for a read.
#[test]
fn a_destroy_the_host_did_not_run_is_tried_again_and_never_stuck() {
    for read in [false, true] {
        let (world, keep, names) = published(2);
        let t = names[0].clone();
        let member = format!("{POOL}@{t}");
        if read {
            world.borrow_mut().members_not_run = true;
        } else {
            world.borrow_mut().destroy_not_run.insert(member.clone());
        }
        let mut a = agent(&world, &keep);
        a.set_keep(1);
        for _ in 0..3 {
            a.retain().expect("retained");
            assert!(
                a.record().stuck.is_empty(),
                "read={read}: {:?}",
                a.record().stuck
            );
            assert!(
                a.record().published.iter().any(|p| p.name == t),
                "read={read}: the name left the published"
            );
            assert!(retired(a.outcomes()).is_empty(), "{:?}", a.outcomes());
        }
        assert!(!snaps_of(&world, &t).is_empty());
        world.borrow_mut().members_not_run = false;
        world.borrow_mut().destroy_not_run.clear();
        a.retain().expect("retained again");
        assert_eq!(retired(a.outcomes()), vec![t.clone()], "read={read}");
        assert!(snaps_of(&world, &t).is_empty());
        assert!(!a.record().names(&t));
    }
}

/// The same for a name given up past its time: it stays given up and is
/// tried again, not stuck.
#[test]
fn a_given_up_destroy_the_host_did_not_run_stays_given_up() {
    let (world, keep, names) = published(1);
    let t = names[0].clone();
    {
        let mut a = agent(&world, &keep);
        let p = a.record().published[0].clone();
        a.commit(Effect::Retain, |r| {
            r.published.clear();
            r.abandoned.push(crate::record::Abandoned {
                name: p.name.clone(),
                why: Why::ManifestLost,
                at_ms: 0,
                left: Left::Members(p.members.clone()),
            });
        })
        .expect("saved");
    }
    world.borrow_mut().advance(ABANDONED_KEEP_DEFAULT);
    world.borrow_mut().members_not_run = true;
    let mut a = agent(&world, &keep);
    a.retain().expect("retained");
    assert!(a.record().stuck.is_empty(), "{:?}", a.record().stuck);
    assert!(a.record().abandoned.iter().any(|g| g.name == t));
    world.borrow_mut().members_not_run = false;
    a.retain().expect("retained again");
    assert_eq!(retired(a.outcomes()), vec![t.clone()]);
    assert!(!a.record().names(&t));
}

/// How many times the round asked the daemon `what`.
fn asked(world: &Rc<RefCell<World>>, what: &str) -> usize {
    world.borrow().calls.iter().filter(|c| **c == what).count()
}

/// A look not answered, by an error or by `no-answer`, ends the round's
/// asks of the daemon: of three names a person gave up, one is looked up
/// a round; a look refused for another reason does not (L2 of the review
/// of #179, for the looks). Mutants: a look's error or `no-answer` that
/// leaves the round's asks on; the looks not stopped when they are off.
#[test]
fn a_look_not_answered_ends_the_round_s_looks() {
    let (world, keep, _) = given_up_by_a_person(3);
    let away = world.borrow().now_ms + 60_000;
    world.borrow_mut().away_until = away;
    let mut a = agent(&world, &keep);
    world.borrow_mut().calls.clear();
    a.retain().expect("retained");
    assert_eq!(asked(&world, "look"), 1, "an error");
    assert_eq!(a.record().abandoned.len(), 3);
    for (code, looks) in [
        (skyzfs_control::Code::NoAnswer, 1),
        (skyzfs_control::Code::Store, 3),
    ] {
        let (world, keep, _) = given_up_by_a_person(3);
        let counted = Rc::default();
        let mut a = Agent::new(
            FakeZfs(world.clone()),
            LookRefused {
                daemon: FakeDaemon(world.clone()),
                code,
                looks: Rc::clone(&counted),
            },
            keep.clone(),
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        a.retain().expect("retained");
        assert_eq!(counted.get(), looks, "{code:?}");
        assert_eq!(a.record().abandoned.len(), 3, "{code:?}");
    }
}

/// A retire not answered by an error ends the round's retires: of
/// three names past `--keep`, one is asked a round (L2 of the review of
/// #179, the error's arm; `no-answer` is in `program/tests.rs`).
/// Mutant: an error that leaves the round's asks on.
#[test]
fn a_retire_not_answered_by_an_error_ends_the_round_s_retires() {
    let (world, keep, names) = published(4);
    let away = world.borrow().now_ms + 60_000;
    world.borrow_mut().away_until = away;
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    world.borrow_mut().calls.clear();
    a.retain().expect("retained");
    assert_eq!(asked(&world, "retire"), 1);
    assert!(retired(a.outcomes()).is_empty());
    world.borrow_mut().away_until = 0;
    a.retain().expect("retained again");
    assert_eq!(retired(a.outcomes()), names[..3].to_vec());
}

/// Where the names given up alone keep the next begin from its room,
/// the oldest leave early, each looked up first: with `snap/T` there,
/// the local snapshot is kept and the name is forgotten for a person
/// (M1 of the review of #179, as the lead ruled). Only as many leave as
/// [`Record::abandoned_over`] counts, and the newer stay.
///
/// Mutants: an early name not looked up; the newest leaving first.
#[test]
fn the_oldest_given_up_leave_early_for_room_looked_up_first() {
    let (world, keep, names) = published(2);
    let now = world.borrow().now_ms;
    let (oldest, newer) = (names[0].clone(), names[1].clone());
    change_record(&keep, |r| {
        let published = std::mem::take(&mut r.published);
        for (k, p) in published.into_iter().enumerate() {
            r.abandoned.push(crate::record::Abandoned {
                name: p.name,
                why: Why::ManifestLost,
                at_ms: now - 2_000 + 1_000 * k as u64,
                left: Left::Members(p.members),
            });
        }
        for i in 0..ABANDONED_MAX - 2 {
            r.abandoned.push(crate::record::Abandoned {
                name: format!("n{i}"),
                why: Why::Premise,
                at_ms: now,
                left: Left::Nothing,
            });
        }
    });
    let kept = snaps_of(&world, &oldest);
    assert!(!kept.is_empty());
    let mut a = agent(&world, &keep);
    assert_eq!(a.record().abandoned_over(), 1);
    world.borrow_mut().calls.clear();
    a.retain().expect("retained");
    assert_eq!(
        snaps_of(&world, &oldest),
        kept,
        "the local snapshot is kept"
    );
    assert!(
        world.borrow().refs.contains_key(&oldest),
        "the ref is touched"
    );
    assert!(
        matches!(
            a.outcomes(),
            [(n, Outcome::Forgotten(why))]
                if *n == oldest && why.starts_with("snap/t exists though given up;")
        ),
        "{:?}",
        a.outcomes()
    );
    assert_eq!(asked(&world, "look"), 1);
    assert_eq!(a.record().abandoned.len(), ABANDONED_MAX - 1);
    assert!(a.record().abandoned.iter().any(|g| g.name == newer));
    assert_eq!(a.record().abandoned_over(), 0);
}
