// Scratch scenarios of the next look at #144 (M2.6b2) at 45648ab.
// Appended to the end of `crates/skyzfs-agent/src/engine/tests.rs` in a
// scratch worktree only (after `review144.rs`, the first look's, whose
// tests are re-run here). Each passes at 45648ab and shows the
// behaviour as built.

/// A daemon whose uploader does not answer `progress` or `withdraw`
/// within its limit, as skyzfsd answers `refused no-answer` while its
/// uploader is busy or its inbox is full (`control/handler.rs`, `wait`
/// and `send`: "a publish may have been taken: ask again, or withdraw
/// it"). `cuts` (the store's desk) and `look` (object storage) answer
/// as the fake's daemon does.
/// It answers as the fake's until it has accepted a publish.
struct NoAnswer(FakeDaemon, bool);

impl Daemon for NoAnswer {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        match request {
            Request::Progress(_) | Request::Withdraw(_) if self.1 => Ok(Reply::Refused(
                skyzfs_control::Code::NoAnswer,
                "the uploader did not answer within the limit".into(),
            )),
            _ => {
                let answer = self.0.ask(request, manifest);
                if matches!(request, Request::Publish(_)) && matches!(answer, Ok(Reply::Accepted)) {
                    self.1 = true;
                }
                answer
            }
        }
    }

    fn device(&mut self) -> io::Result<u64> {
        self.0.device()
    }
}

/// N1. A publish asked, whose uploader answers `no-answer` (it is in a
/// long step: skyzfsd's uploader answers requests only between two
/// turns), and which passes its age bound: the planner's too-old arm
/// comes before the answers the module docs call "a daemon away", so
/// `look` is asked, answers `absent`, and the hold is released and the
/// publish given up at once, long before `held_until`, while the request
/// still pends at the daemon. A person then destroys `pool@t`. The
/// upload in flight (a cut counted before the request, slow) ends, and
/// the cut that waits is the newest, counted after the destroy (the
/// fake's daemon, like skyzfsd, uploads the newest cut that waits): it
/// carries the request, and `snap/t` names a cut that does not hold
/// `pool@t` (check 0 of the matrices). The same run with the default
/// bound keeps the hold to `held_until`, and the request is carried by
/// a cut that holds `pool@t` and is found published.
#[test]
fn review_n1_a_too_old_publish_lets_its_hold_go_while_its_request_pends() {
    for age_max in [Duration::from_secs(60), plan::AGE_MAX] {
        let world = World::new(Mode::Idle);
        {
            // An upload in flight for 100 s, of a cut counted before any
            // request.
            let mut w = world.borrow_mut();
            w.advance(Duration::from_secs(3));
            w.lag_ms = 100_000;
            w.sync("before the publish");
        }
        let keep = MemKeep::default();
        let mut agent = Agent::new(
            FakeZfs(world.clone()),
            NoAnswer(FakeDaemon(world.clone()), false),
            keep.clone(),
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent")
        .with_age_max(age_max);
        let t = agent.begin().expect("begun");
        if age_max != plan::AGE_MAX {
            agent.settle(LIMIT).expect("settled");
            assert_eq!(
                agent.outcomes(),
                [(t.clone(), Outcome::Abandoned(Why::TooOld))]
            );
            let given_up_s = (world.borrow().now_ms - fake::EPOCH_MS) / 1000;
            // The daemon's own word on the request, past the agent's channel.
            let then = FakeDaemon(world.clone())
                .ask(&Request::Progress(t.clone()), &[])
                .expect("an answer");
            assert!(given_up_s < EXPIRY_DEFAULT.as_secs(), "{given_up_s} s");
            assert_eq!(then, Reply::Pending);
            // A person destroys pool@t: no hold refuses it now.
            world.borrow_mut().user(User::Destroy, &t);
            world.borrow_mut().advance(Duration::from_secs(300));
            let w = world.borrow();
            let on = w.refs.get(&t).map(|r| {
                let cut = &w.commits[&r.commit].cut;
                (cut.number, cut.snaps.contains_key(&format!("{POOL}@{t}")))
            });
            eprintln!(
                "age_max {age_max:?}: given up at {given_up_s} s with the daemon saying {then:?}; \
                 snap/{t} on (cut, holds pool@t): {on:?}\n{}",
                w.log.join("\n")
            );
            assert!(
                matches!(on, Some((_, false))),
                "snap/{t} on a cut without pool@{t}: {on:?}"
            );
        } else {
            // The same with the default bound: the hold kept to
            // `held_until`, and the request carried with `pool@t`.
            agent.settle(LIMIT).expect("settled");
            let w = world.borrow();
            let on = w.refs.get(&t).map(|r| {
                let cut = &w.commits[&r.commit].cut;
                (cut.number, cut.snaps.contains_key(&format!("{POOL}@{t}")))
            });
            eprintln!(
                "age_max {age_max:?}: {:?}; snap/{t} on (cut, holds pool@t): {on:?}",
                agent.outcomes()
            );
            assert!(matches!(agent.outcomes(), [(_, Outcome::Published(_))]));
            assert!(matches!(on, Some((_, true))), "{on:?}");
        }
    }
}

/// N3. A publish begun in this process and renamed at its program (a
/// person took its name first) loses its place on the clock that does
/// not step: the engine's memory of `begun` stays under the old name, so
/// its age is read on the wall clock. A step of the wall clock three
/// hours forward just after the intent gives it up as too old; the same
/// run without the rename publishes it.
#[test]
fn review_n3_a_publish_renamed_at_its_program_is_aged_on_the_wall_clock() {
    for rename in [false, true] {
        let world = World::new(Mode::Busy);
        let keep = MemKeep::default();
        let hook = {
            let world = world.clone();
            move |point: Point| {
                if point.effect == Effect::Intent && point.after {
                    world.borrow_mut().agent_skew_ms = 3 * 3_600_000;
                }
                false
            }
        };
        let mut agent = agent(&world, &keep).with_points(Box::new(hook));
        let name = agent.begin().expect("begun");
        if rename {
            let mut world = world.borrow_mut();
            let pool = world.pool.as_mut().expect("the pool");
            let snap = fake::Snap {
                guid: 0xdead,
                createtxg: pool.txg,
                holds: BTreeSet::new(),
                held_in: None,
            };
            pool.snaps.insert(format!("{POOL}/db@{name}"), snap);
        }
        agent.settle(LIMIT).expect("settled");
        eprintln!("rename {rename}: {:?}", agent.outcomes());
        if rename {
            // The next name is the stepped clock's second.
            assert!(
                matches!(agent.outcomes(), [(n, Outcome::Abandoned(Why::TooOld))] if *n != name),
                "{:?}",
                agent.outcomes()
            );
        } else {
            assert!(
                matches!(agent.outcomes(), [(_, Outcome::Published(_))]),
                "{:?}",
                agent.outcomes()
            );
        }
    }
}
