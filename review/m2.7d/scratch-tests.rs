//// ENGINE TESTS

/// The review's tests of #170 (M2.7d) at bb03454: gaps found by plants
/// that survive the head's suite, not for merging as they are.
mod review170 {
    use super::*;
    use crate::record::Member;
    use crate::zfs::{Dataset, Layout, Props};

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
        world
    }

    fn syncs(world: &Rc<RefCell<World>>) -> usize {
        world.borrow().synced_at.len()
    }

    /// RV1: a daemon that counts no cut after any sync gets one sync a
    /// call, and the bound still ends them: NoCut does not start the
    /// count again (kills R11).
    #[test]
    fn rv1_no_cut_at_each_call_still_meets_the_bound() {
        let world = rewound(5);
        world.borrow_mut().plain_sync = true;
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        for i in 0..SYNC_PAST_MAX {
            let past = agent.sync_past(&mut || true).expect("synced");
            assert!(
                matches!(past, Past::Behind { synced: 1, why: Stuck::NoCut, .. }),
                "{i}: {past:?}"
            );
        }
        let past = agent.sync_past(&mut || true).expect("seen");
        assert!(
            matches!(past, Past::Behind { synced: 0, why: Stuck::Bound, .. }),
            "{past:?}"
        );
        assert_eq!(syncs(&world), SYNC_PAST_MAX as usize);
    }

    /// A Zfs whose sync task always fails, after its call is counted.
    struct FailAssist {
        zfs: FakeZfs,
        tried: Rc<Cell<usize>>,
    }

    impl crate::zfs::Zfs for FailAssist {
        fn layout(&mut self, pool: &str) -> Result<Layout, ZfsError> {
            self.zfs.layout(pool)
        }
        fn props(&mut self, pool: &str) -> Result<Props, ZfsError> {
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
        fn datasets(&mut self, pool: &str, t: &str) -> Result<Vec<Dataset>, ZfsError> {
            self.zfs.datasets(pool, t)
        }
        fn destroy(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
            self.zfs.destroy(pool, t)
        }
        fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError> {
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
            members: Option<&[Member]>,
        ) -> Result<Held, ZfsError> {
            self.zfs.held(pool, t, tag, members)
        }
        fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
            self.zfs.tagged(pool, tag)
        }
        fn assist(&mut self, _pool: &str, _value: u64) -> Result<(), ZfsError> {
            self.tried.set(self.tried.get() + 1);
            Err(ZfsError::Other("zpool sync: timed out".into()))
        }
    }

    /// RV2: a sync that fails counts toward the bound, so a sync task
    /// that fails after it wrote (a `zfs set` that landed, a `zpool
    /// sync` that timed out) is not tried for ever (kills R6).
    #[test]
    fn rv2_a_failed_sync_counts_toward_the_bound() {
        let world = rewound(5);
        let keep = MemKeep::default();
        let tried = Rc::new(Cell::new(0));
        let mut agent = Agent::new(
            FailAssist {
                zfs: FakeZfs(world.clone()),
                tried: tried.clone(),
            },
            FakeDaemon(world.clone()),
            keep.clone(),
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        for i in 0..SYNC_PAST_MAX {
            let past = agent.sync_past(&mut || true);
            assert!(matches!(past, Err(Error::Zfs(_))), "{i}: {past:?}");
            agent.restart().expect("restarted");
        }
        let past = agent.sync_past(&mut || true).expect("seen");
        assert!(
            matches!(past, Past::Behind { synced: 0, why: Stuck::Bound, .. }),
            "{past:?}"
        );
        assert_eq!(tried.get(), SYNC_PAST_MAX as usize);
    }

    /// RV3: the premise is checked before the first sync of each call,
    /// not only of the first call: a pool attached to between two
    /// rounds is not synced (kills R7).
    #[test]
    fn rv3_the_premise_is_checked_at_each_call() {
        let world = rewound(5);
        world.borrow_mut().plain_sync = true;
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        assert!(matches!(past, Past::Behind { synced: 1, why: Stuck::NoCut, .. }), "{past:?}");
        world.borrow_mut().user(User::Attach, "");
        assert!(world.borrow().daemon.cuts.last().is_some_and(|c| !c.itself));
        let past = agent.sync_past(&mut || true);
        assert!(matches!(past, Err(Error::Premise(_))), "{past:?}");
        assert_eq!(syncs(&world), 1);
    }

    /// RV4: a daemon that holds no cut (a state begun anew) starts the
    /// count again, as `Past::Clear` and `past_synced` say (kills R1).
    #[test]
    fn rv4_no_cut_held_starts_the_count_again() {
        let world = rewound(10_000);
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let past = agent.sync_past(&mut || true).expect("synced");
        assert!(matches!(past, Past::Behind { why: Stuck::Bound, .. }), "{past:?}");
        world.borrow_mut().daemon.cuts.clear();
        assert_eq!(
            agent.sync_past(&mut || true).expect("seen"),
            Past::Clear { synced: 0 }
        );
        world.borrow_mut().sync("a person's change, below the abandoned top");
        let past = agent.sync_past(&mut || true).expect("synced");
        assert!(
            matches!(past, Past::Behind { synced: SYNC_PAST_MAX, why: Stuck::Bound, .. }),
            "{past:?}"
        );
    }

    /// RV5: NoCut is judged against the count before the last sync, not
    /// the first: syncs that make cuts and then one that makes none end
    /// there (kills R5).
    #[test]
    fn rv5_no_cut_is_judged_from_the_last_sync() {
        let world = rewound(10);
        let keep = MemKeep::default();
        let mut agent = agent(&world, &keep);
        let w = world.clone();
        let mut n = 0;
        let mut go = move || {
            n += 1;
            if n == 2 {
                w.borrow_mut().plain_sync = true;
            }
            true
        };
        let past = agent.sync_past(&mut go).expect("synced");
        assert!(matches!(past, Past::Behind { synced: 2, why: Stuck::NoCut, .. }), "{past:?}");
        assert_eq!(syncs(&world), 2);
    }

    /// RV6: an end asked before a pass of `settle_while` names what is
    /// still under way, and the pass's count and sweep do not run
    /// (kills R19).
    #[test]
    fn rv6_an_end_before_a_pass_names_what_is_under_way() {
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
}

//// PROGRAM TESTS

/// The review's tests of #170 (M2.7d) at bb03454, not for merging as they
/// are. RP1 and RP2 show findings and fail at the head.
mod review170 {
    use std::cell::Cell;
    use std::collections::BTreeSet;

    use super::*;
    use crate::engine::fake::User;
    use crate::record::{Loaded, SaveError};
    use crate::zfs::{Dataset, Held, Layout, Props, ZfsError};

    /// A Zfs whose sync task fails at its `fail`-th call (from 1).
    struct FailNth {
        zfs: FakeZfs,
        fail: usize,
        seen: usize,
    }

    impl Zfs for FailNth {
        fn layout(&mut self, pool: &str) -> Result<Layout, ZfsError> {
            self.zfs.layout(pool)
        }
        fn props(&mut self, pool: &str) -> Result<Props, ZfsError> {
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
        fn datasets(&mut self, pool: &str, t: &str) -> Result<Vec<Dataset>, ZfsError> {
            self.zfs.datasets(pool, t)
        }
        fn destroy(&mut self, pool: &str, t: &str) -> Result<(), ZfsError> {
            self.zfs.destroy(pool, t)
        }
        fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError> {
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
            members: Option<&[Member]>,
        ) -> Result<Held, ZfsError> {
            self.zfs.held(pool, t, tag, members)
        }
        fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
            self.zfs.tagged(pool, tag)
        }
        fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError> {
            self.seen += 1;
            if self.seen == self.fail {
                return Err(ZfsError::Other("zpool sync: timed out".into()));
            }
            self.zfs.assist(pool, value)
        }
    }

    /// A directory in memory that counts its loads and fails the
    /// `fail_load`-th (from 1), if any.
    struct Loads {
        keep: MemKeep,
        loads: Rc<Cell<usize>>,
        fail_load: usize,
    }

    impl Keep for Loads {
        fn load(&self) -> io::Result<Loaded> {
            self.loads.set(self.loads.get() + 1);
            if self.loads.get() == self.fail_load {
                return Err(io::Error::other("the record could not be read"));
            }
            self.keep.load()
        }
        fn save(&self, record: &Record) -> Result<(), SaveError> {
            self.keep.save(record)
        }
        fn write_manifest(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
            self.keep.write_manifest(name, bytes)
        }
        fn read_manifest(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
            self.keep.read_manifest(name)
        }
        fn remove_manifest(&self, name: &str) -> io::Result<()> {
            self.keep.remove_manifest(name)
        }
        fn manifest_names(&self) -> io::Result<Vec<String>> {
            self.keep.manifest_names()
        }
        fn marked(&self) -> io::Result<bool> {
            self.keep.marked()
        }
    }

    fn sched<Z: Zfs, D: Daemon, K: Keep>(
        agent: &mut Agent<Z, D, K, FakeClock>,
        every: Duration,
        go: &mut dyn FnMut() -> bool,
    ) -> (i32, String, String) {
        let (mut out, mut err) = (Shared::default(), Shared::default());
        let mut told = out.clone();
        agent.set_outcome_sink(Box::new(move |name, outcome| {
            let _ = told.write_all(outcome_line(name, outcome).as_bytes());
        }));
        let code = schedule(agent, every, go, &mut out, &mut err);
        (code, out.text(), err.text())
    }

    /// RP1 (fails at the head: a finding): an end asked for during the
    /// round's last sync, the one after which the newest cut opens, still
    /// lets the round begin a publish, with its count, sweep and reads,
    /// before the settle asks `go`. The module docs say that a signal
    /// ends `run` once its step is done, before the count and the sweep
    /// that would open the next.
    #[test]
    fn rp1_an_end_during_the_last_sync_begins_no_publish() {
        let world = rewound_t(5);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        // 1: the round's start; 2, 3, 4: before each of the three syncs.
        // The end comes while the third runs.
        let mut asked = 0u32;
        let mut go = || {
            asked += 1;
            asked <= 4
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
        let rounds = rounds(&out);
        assert_eq!(rounds.len(), 1, "{out}");
        assert_eq!(field(rounds[0], "synced"), "3", "{out}");
        // The syncs read `cuts` four times: before each sync, and after
        // the third, when the newest cut opens. What follows came after
        // the end was asked for.
        let calls = world.borrow().calls.clone();
        let fourth = calls
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == "cuts")
            .nth(3)
            .map_or(0, |(i, _)| i);
        assert_eq!(
            field(rounds[0], "begun"),
            "-",
            "calls after the end was asked: {:?}",
            &calls[fourth + 1..]
        );
    }

    /// RP2 (fails at the head: a finding): a round whose second sync
    /// failed ran two syncs, and its line says `synced=0`.
    #[test]
    fn rp2_a_round_whose_sync_failed_says_the_syncs_it_ran() {
        let world = rewound_t(10);
        let keep = MemKeep::default();
        let mut agent = Agent::new(
            FailNth {
                zfs: FakeZfs(world.clone()),
                fail: 2,
                seen: 0,
            },
            FakeDaemon(world.clone()),
            keep.clone(),
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        let start = mono(&world);
        let (_code, out, err) = sched(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 30_000),
        );
        let rounds = rounds(&out);
        assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
        assert!(err.contains("round 1: zfs: zpool sync: timed out"), "{err}");
        assert_eq!(world.borrow().synced_at.len(), 1, "{out}");
        assert_ne!(field(rounds[0], "synced"), "0", "{out}{err}");
    }

    /// RP3: an error of the syncs reads the record again at once, as a
    /// begin's does (kills R8).
    #[test]
    fn rp3_an_error_of_the_syncs_reads_the_record_again() {
        let world = rewound_t(5);
        let loads = Rc::new(Cell::new(0));
        let mut agent = Agent::new(
            FailNth {
                zfs: FakeZfs(world.clone()),
                fail: 1,
                seen: 0,
            },
            FakeDaemon(world.clone()),
            Loads {
                keep: MemKeep::default(),
                loads: loads.clone(),
                fail_load: 0,
            },
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        assert_eq!(loads.get(), 1);
        let start = mono(&world);
        let (_code, out, err) = sched(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 30_000),
        );
        assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
        assert_eq!(loads.get(), 2, "{out}{err}");
    }

    /// RP8: a refusal of the premise at the syncs reads the record no
    /// more than a begin's refusal does (kills R12).
    #[test]
    fn rp8_a_refusal_at_the_syncs_reads_the_record_once() {
        let world = rewound_t(5);
        world.borrow_mut().user(User::Attach, "");
        let loads = Rc::new(Cell::new(0));
        let mut agent = Agent::new(
            FakeZfs(world.clone()),
            FakeDaemon(world.clone()),
            Loads {
                keep: MemKeep::default(),
                loads: loads.clone(),
                fail_load: 0,
            },
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        let start = mono(&world);
        let (_code, out, err) = sched(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 30_000),
        );
        assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
        assert_eq!(loads.get(), 1, "{out}{err}");
    }

    /// RP4: a round whose syncs met a pool its premise refuses begins
    /// nothing: one error, the premise's (kills R9).
    #[test]
    fn rp4_a_round_whose_syncs_were_refused_begins_nothing() {
        let world = rewound_t(5);
        world.borrow_mut().user(User::Attach, "");
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let (_code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 30_000),
        );
        let rounds = rounds(&out);
        assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
        assert_eq!(field(rounds[0], "begun"), "-", "{out}");
        assert_eq!(err.lines().filter(|l| l.contains("round 1: ")).count(), 1, "{err}");
        assert_eq!(world.borrow().synced_at.len(), 0, "{out}");
    }

    /// RP5: an error of the syncs whose reading of the record fails
    /// settles nothing in that round (kills R10).
    #[test]
    fn rp5_a_failed_reading_after_the_syncs_error_settles_nothing() {
        let world = rewound_t(5);
        let loads = Rc::new(Cell::new(0));
        let mut agent = Agent::new(
            FailNth {
                zfs: FakeZfs(world.clone()),
                fail: 1,
                seen: 0,
            },
            FakeDaemon(world.clone()),
            Loads {
                keep: MemKeep::default(),
                loads: loads.clone(),
                fail_load: 2,
            },
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        let mut asked = 0u32;
        let mut go = || {
            asked += 1;
            asked <= 3
        };
        let (_code, out, err) = sched(&mut agent, Duration::from_secs(60), &mut go);
        assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
        assert!(err.contains("round 1: the record's directory: the record could not be read"), "{err}");
        let calls = world.borrow().calls.clone();
        assert!(!calls.contains(&"count"), "the settle ran: {calls:?}");
    }

    /// RP6: with the daemon unread at the syncs, the round still begins,
    /// and its begin says what is wrong (kills R3).
    #[test]
    fn rp6_the_daemon_unread_still_begins() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().away_until = u64::MAX;
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let (_code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 30_000),
        );
        assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
        assert!(err.contains("round 1: "), "{err}");
    }

    /// RP7: a round still behind settles (kills R20).
    #[test]
    fn rp7_a_round_still_behind_still_settles() {
        let world = rewound_t(10_000);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let (_code, out, _err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 30_000),
        );
        assert_eq!(field(rounds(&out)[0], "begun"), "-", "{out}");
        let calls = world.borrow().calls.clone();
        assert!(calls.contains(&"count"), "the settle did not run: {calls:?}");
    }
}
