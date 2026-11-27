//! Retention on the fake world ([module docs](super)).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use super::super::fake::{
    FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, Snap, World, POOL, VOLUME,
};
use super::super::{Agent, Effect, Error, Outcome, Point, Points};
use super::{ABANDONED_KEEP_DEFAULT, STEPS_MAX, STUCK_FORGOTTEN};
use crate::plan::Report;
use crate::record::{Left, Member, Record, Retire, Stuck, Why, RETIRING_MAX, STUCK_MAX};

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
/// snapshot is left, and the new name is the newest stuck. Every name
/// stuck is tried at each round, outside the round's steps, and told
/// once while its reason stays.
///
/// Mutants: none forgotten past the cap, the newest forgotten, the
/// oldest forgotten with no line, the names stuck tried within the
/// round's steps or not at all, told at each round.
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
    const { assert!(STUCK_MAX > STEPS_MAX) };
    for seed in &seeds {
        assert_eq!(refused(&world, seed), 1, "{seed}");
    }
    a.retain().expect("retained again");
    assert_eq!(refused(&world, &seeds[0]), 1, "forgotten: not tried");
    for seed in &seeds[1..] {
        assert_eq!(refused(&world, seed), 2, "{seed}");
    }
    assert_eq!(refused(&world, &t), 2);
    let told = a
        .reports()
        .iter()
        .filter(|(n, r)| *n == seeds[1] && matches!(r, Report::NotRetired(_)))
        .count();
    assert_eq!(told, 1);
    assert_eq!(a.record().stuck.len(), STUCK_MAX);
    assert!(a.outcomes().iter().all(|(n, _)| *n != seeds[1]));
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
