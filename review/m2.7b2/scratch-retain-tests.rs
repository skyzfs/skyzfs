// Scratch tests of the review of #179 (M2.7b-2): appended to
// crates/skyzfs-agent/src/engine/retain/tests.rs by apply.py, at adc391a
// and at d68471f. T3 and T4 assert what a reviewer would want (a failure
// is the finding); the gap tests (G-*) pass at the head and fail on the
// plant they name (plants.json, plants2.json).

/// T3. A name stopped `other` is never asked again, whatever a person
/// does to `snap/<t>`: put back to the recorded commit, or deleted. Its
/// members' `@t` stay, and it counts among the 64 being retired for as
/// long as the record lasts. The module docs say "until a person settles
/// it"; nothing in the agent settles it.
#[test]
fn review179_t3_a_name_stopped_other_is_never_asked_again() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let recorded = world.borrow().refs.get(&t).map(|r| r.commit).expect("ref");
    if let Some(r) = world.borrow_mut().refs.get_mut(&t) {
        r.commit = [0x77; 32];
    }
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    // A person puts snap/t back as the agent published it.
    if let Some(r) = world.borrow_mut().refs.get_mut(&t) {
        r.commit = recorded;
    }
    drop(a);
    for _ in 0..5 {
        let mut again = agent(&world, &keep);
        again.set_keep(1);
        again.retain().expect("retained again");
    }
    // And deletes it.
    world.borrow_mut().refs.remove(&t);
    for _ in 0..5 {
        let mut again = agent(&world, &keep);
        again.set_keep(1);
        again.retain().expect("retained again");
    }
    let a = agent(&world, &keep);
    let p = a.record().published.iter().find(|p| p.name == t).cloned();
    let snaps = snaps_of(&world, &t);
    eprintln!("T3: {t} in the record as {:?}; its @t: {snaps:?}", p.map(|p| p.retire));
    assert!(
        a.record().published.iter().all(|p| p.name != t),
        "a name stopped other stays in the record with its members' @t: {snaps:?}"
    );
}

/// T4. A step is spent each round on each of the oldest names marked,
/// in the record's order, whatever became of them: eight names whose
/// destroy is refused for good (a person's clone of each, or a hold of
/// a backup tool) take every step of every round, and the names after
/// them are never retired. Marks go on to 64, then stop; the names
/// published grow until `room()` refuses a begin (`Full`).
#[test]
fn review179_t4_eight_names_that_cannot_be_destroyed_starve_the_rest() {
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
    let left: Vec<&str> = a
        .record()
        .published
        .iter()
        .map(|p| p.name.as_str())
        .filter(|n| names[STEPS_MAX..names.len() - 1].iter().any(|t| t == n))
        .collect();
    eprintln!("T4: after six rounds, of the names that could go, still in the record: {left:?}");
    assert!(left.is_empty(), "never retired: {left:?}");
}


/// G-R12. A name whose ref is retired, with the pool not imported when
/// its members are read, stays in the record and is
/// retired once the pool is back. Kills R12 (a members read that fails
/// lets the name leave with its `@t`s).
#[test]
fn review179_g_r12_a_members_read_that_fails_keeps_the_name() {
    let (world, keep, names) = published(2);
    let t = names[0].clone();
    let snaps = snaps_of(&world, &t);
    let pool = world.borrow_mut().pool.take();
    let mut a = agent(&world, &keep);
    a.set_keep(1);
    a.retain().expect("retained");
    assert!(!world.borrow().refs.contains_key(&t), "the ref is retired");
    // Still in the record (published at `retire=members` at adc391a,
    // stuck at d68471f), and not told retired.
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

/// G-R15. A name a person gave up whose `snap/T` answers `unreadable`
/// is there: nothing is destroyed and it is forgotten for a person.
/// Kills R15 (unreadable taken as absent).
#[test]
fn review179_g_r15_an_unreadable_snap_t_is_there() {
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

/// G-R16. A name a person gave up, whose look is not answered (the
/// daemon away): nothing destroyed, the name kept, and once the daemon
/// is back and `snap/T` is there, forgotten for a person. Kills R16.
#[test]
fn review179_g_r16_a_look_not_answered_destroys_nothing() {
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

/// G-R16b. The same, with the look refused (`refused store`). Kills
/// R16b.
#[test]
fn review179_g_r16b_a_look_refused_destroys_nothing() {
    let (world, keep, names) = given_up_by_a_person(1);
    let t = names[0].clone();
    let kept = snaps_of(&world, &t);
    let mut a = Agent::new(
        FakeZfs(world.clone()),
        LookRefused {
            daemon: FakeDaemon(world.clone()),
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

/// The fake daemon, whose `look` is refused `store`.
struct LookRefused {
    daemon: FakeDaemon,
}

impl super::super::Daemon for LookRefused {
    fn ask(
        &mut self,
        request: &skyzfs_control::Request,
        manifest: &[u8],
    ) -> std::io::Result<skyzfs_control::Reply> {
        if let skyzfs_control::Request::Look(_) = request {
            return Ok(skyzfs_control::Reply::Refused(
                skyzfs_control::Code::Store,
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

/// G-R21. A name given up past its time, with members: a stop just
/// after the save that drops it has told `retired` already. Kills R21
/// (told after the name leaves).
#[test]
fn review179_g_r21_a_given_up_name_is_told_before_it_leaves() {
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

/// G-R22. A name given up with no GUIDs, past its time: a stop just
/// after the save that drops it has told `forgotten` already. Kills R22
/// (told after the name leaves).
#[test]
fn review179_g_r22_a_forgotten_name_is_told_before_it_leaves() {
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

/// G-E4. A publish given up after its take (the uploader stopped)
/// records its members, so retention destroys its `@t`s after
/// `--abandoned-keep-s`. Kills E4 (a take recorded as left=unknown).
#[test]
fn review179_g_e4_a_give_up_after_the_take_records_its_members() {
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
    assert!(snaps_of(&world, &t).is_empty(), "{:?}", snaps_of(&world, &t));
}
