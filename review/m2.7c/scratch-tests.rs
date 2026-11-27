
/// The review's scratch tests of M2.7c (skyzfs/skyzfs#173), appended to
/// `crates/skyzfs-agent/src/program/tests.rs` in a scratch worktree. R1
/// is what the review asks for and fails at the head; the others record
/// what the head does, and pass there.
mod review_m27c {
    use skyzfs_control::failure::{Failure, Held, Reason};

    use super::*;
    use crate::engine::fake::EPOCH_MS;
    use crate::engine::Noted;

    fn held_yes() -> Noted {
        Noted::Failed(Failure {
            held: Held::Yes,
            reason: Reason::ImageFailed,
            identity: [7; 16],
        })
    }

    /// A daemon that fails at `at_ms` (the fake's clock that does not
    /// step): from then its socket does not answer and its note is there.
    struct FailsAt {
        daemon: FakeDaemon,
        at_ms: u64,
        note: Noted,
    }

    impl Daemon for FailsAt {
        fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
            if mono(&self.daemon.0) >= self.at_ms {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no socket"));
            }
            self.daemon.ask(request, manifest)
        }
        fn device(&mut self) -> io::Result<u64> {
            self.daemon.device()
        }
        fn failure(&mut self) -> io::Result<Option<Noted>> {
            Ok((mono(&self.daemon.0) >= self.at_ms).then_some(self.note))
        }
    }

    /// When the reopen was made, on the fake's clock that does not step
    /// (the fake sets `suspended_at` to the reopen's time plus 1050 ms).
    fn reopened_mono(world: &Rc<RefCell<World>>) -> u64 {
        world.borrow().suspended_at.expect("a reopen") - 1_050 - EPOCH_MS
    }

    fn run_on(world: &Rc<RefCell<World>>) -> (i32, String, String) {
        let keep = MemKeep::default();
        let mut agent = agent_on(world, FakeDaemon(world.clone()), &keep);
        let start = mono(world);
        scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(world, start + 600_000),
        )
    }

    /// R1 (fails at the head). The volume fails 5 s into `run`, between
    /// two rounds. The M2.7 criterion: "the pool is suspended within
    /// seconds of the failure". The note is read only at a round's
    /// start, so the reopen comes at the next tick, about 55 s later.
    #[test]
    fn r1_the_step_comes_within_seconds_of_the_failure() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().suspends_after_ms = Some(1_050);
        let keep = MemKeep::default();
        let start = mono(&world);
        let at = start + 5_000;
        let daemon = FailsAt {
            daemon: FakeDaemon(world.clone()),
            at_ms: at,
            note: held_yes(),
        };
        let mut agent = agent_on(&world, daemon, &keep);
        let (code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 600_000),
        );
        assert_eq!(code, exit::LOST, "{out}{err}");
        let reopened = reopened_mono(&world);
        eprintln!(
            "R1: failed at +{} ms, reopened at +{} ms: {} ms after the failure\n{out}{err}",
            at - start,
            reopened - start,
            reopened - at
        );
        assert!(
            reopened - at < 10_000,
            "the reopen came {} ms after the failure",
            reopened - at
        );
    }

    /// R1b (passes at the head): the same with the failure while a
    /// publish is under way in the round's settle; the round runs to its
    /// end, and the step waits for the next tick.
    #[test]
    fn r1b_the_step_waits_for_the_next_tick_at_the_head() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().suspends_after_ms = Some(1_050);
        let keep = MemKeep::default();
        let start = mono(&world);
        let at = start + 5_000;
        let daemon = FailsAt {
            daemon: FakeDaemon(world.clone()),
            at_ms: at,
            note: held_yes(),
        };
        let mut agent = agent_on(&world, daemon, &keep);
        let (code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 600_000),
        );
        assert_eq!(code, exit::LOST, "{out}{err}");
        let reopened = reopened_mono(&world);
        assert!(reopened - start >= 60_000, "{reopened} {start}\n{out}{err}");
        assert!(out.contains("\nlost tank "), "{out}");
        assert!(out.contains("\nround 2 "), "{out}");
    }

    /// R2 (passes at the head): a pool that does not read SUSPENDED
    /// within the look ends `run` with 9 all the same, in a round line
    /// that says `ok`; a `run` started again on the same note reopens it
    /// again. So "no second reopen" holds only where the first step
    /// showed the pool suspended.
    #[test]
    fn r2_a_pool_not_shown_suspended_ends_9_and_a_restart_reopens_again() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().noted = Some(held_yes());
        let (code, out, err) = run_on(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines[0].ends_with(" step=reopened reopen=ok state=ONLINE looked_ms=10000"),
            "{out}"
        );
        assert!(lines[1].starts_with("round 1 ok "), "{out}");
        let (code, out2, _) = run_on(&world);
        assert_eq!(code, exit::LOST);
        assert_eq!(world.borrow().reopens, 2, "{out2}");
        eprintln!("R2:\n{out}{out2}");
    }

    /// R3 (passes at the head): a reopen that is refused (as by the
    /// host's cap of four commands that did not end, or a reopen killed
    /// at its limit) on a pool that then does not suspend: `run` ends
    /// with 9, round `ok`, and the pool reads ONLINE.
    #[test]
    fn r3_a_refused_reopen_on_a_pool_that_stays_online_ends_9() {
        let world = World::new(Mode::Busy);
        {
            let mut w = world.borrow_mut();
            w.noted = Some(held_yes());
            w.reopen_refused = true;
        }
        let (code, out, err) = run_on(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        assert!(
            out.lines().next().expect("a line").ends_with(
                " step=reopened reopen=failed state=ONLINE looked_ms=10000"
            ),
            "{out}"
        );
        assert!(out.lines().nth(1).expect("a line").starts_with("round 1 ok "), "{out}");
        eprintln!("R3:\n{out}{err}");
    }

    /// A Zfs whose pool state fails at the `n`th read (from 1).
    struct StateFails {
        zfs: FakeZfs,
        reads: u32,
        n: u32,
    }

    impl crate::zfs::Zfs for StateFails {
        fn layout(&mut self, pool: &str) -> Result<crate::zfs::Layout, crate::zfs::ZfsError> {
            self.zfs.layout(pool)
        }
        fn props(&mut self, pool: &str) -> Result<crate::zfs::Props, crate::zfs::ZfsError> {
            self.zfs.props(pool)
        }
        fn nocacheflush(&mut self) -> Result<bool, crate::zfs::ZfsError> {
            self.zfs.nocacheflush()
        }
        fn count(&mut self, pool: &str) -> Result<u64, crate::zfs::ZfsError> {
            self.zfs.count(pool)
        }
        fn snapshot(&mut self, pool: &str, t: &str) -> Result<(), crate::zfs::ZfsError> {
            self.zfs.snapshot(pool, t)
        }
        fn datasets(
            &mut self,
            pool: &str,
            t: &str,
        ) -> Result<Vec<crate::zfs::Dataset>, crate::zfs::ZfsError> {
            self.zfs.datasets(pool, t)
        }
        fn destroy(&mut self, pool: &str, t: &str) -> Result<(), crate::zfs::ZfsError> {
            self.zfs.destroy(pool, t)
        }
        fn members(
            &mut self,
            pool: &str,
            t: &str,
        ) -> Result<Option<Vec<Member>>, crate::zfs::ZfsError> {
            self.zfs.members(pool, t)
        }
        fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), crate::zfs::ZfsError> {
            self.zfs.hold(pool, t, tag)
        }
        fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), crate::zfs::ZfsError> {
            self.zfs.release(pool, t, tag)
        }
        fn held(
            &mut self,
            pool: &str,
            t: &str,
            tag: &str,
            members: Option<&[Member]>,
        ) -> Result<crate::zfs::Held, crate::zfs::ZfsError> {
            self.zfs.held(pool, t, tag, members)
        }
        fn tagged(
            &mut self,
            pool: &str,
            tag: &str,
        ) -> Result<std::collections::BTreeSet<String>, crate::zfs::ZfsError> {
            self.zfs.tagged(pool, tag)
        }
        fn assist(&mut self, pool: &str, value: u64) -> Result<(), crate::zfs::ZfsError> {
            self.zfs.assist(pool, value)
        }
        fn reopen(&mut self, pool: &str) -> Result<(), crate::zfs::ZfsError> {
            self.zfs.reopen(pool)
        }
        fn pool_state(
            &mut self,
            pool: &str,
        ) -> Result<Option<crate::zfs::PoolState>, crate::zfs::ZfsError> {
            self.reads += 1;
            if self.reads == self.n {
                return Err(crate::zfs::ZfsError::Other("the state: EIO".into()));
            }
            self.zfs.pool_state(pool)
        }
    }

    fn run_state_fails(n: u32) -> (Rc<RefCell<World>>, i32, String, String) {
        let world = World::new(Mode::Busy);
        {
            let mut w = world.borrow_mut();
            w.noted = Some(held_yes());
            w.suspends_after_ms = Some(1_050);
        }
        let keep = MemKeep::default();
        let mut agent = Agent::new(
            StateFails {
                zfs: FakeZfs(world.clone()),
                reads: 0,
                n,
            },
            FakeDaemon(world.clone()),
            keep,
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent");
        let start = mono(&world);
        let (mut out, mut err) = (Shared::default(), Shared::default());
        let code = schedule(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 600_000),
            &mut out,
            &mut err,
        );
        (world, code, out.text(), err.text())
    }

    /// R4 (passes at the head): an error of the first look after the
    /// reopen. A reopen was made, but the line says `step=failed
    /// reopen=-`, and a reopen's own error would be lost with it.
    #[test]
    fn r4_an_error_of_a_look_after_the_reopen_hides_the_reopen() {
        let (world, code, out, err) = run_state_fails(2);
        assert_eq!(code, exit::LOST, "{out}{err}");
        assert_eq!(world.borrow().reopens, 1);
        assert!(
            out.lines()
                .next()
                .expect("a line")
                .ends_with(" step=failed reopen=- state=- looked_ms=-"),
            "{out}"
        );
        assert!(out.lines().nth(1).expect("a line").starts_with("round 1 failed "), "{out}");
        eprintln!("R4:\n{out}{err}");
    }

    /// R5 (passes at the head): the state cannot be read before the
    /// reopen: no reopen, and `run` ends with 9 all the same, a code on
    /// which it is not to be started again.
    #[test]
    fn r5_no_state_before_the_reopen_ends_9_with_no_reopen() {
        let (world, code, out, err) = run_state_fails(1);
        assert_eq!(code, exit::LOST, "{out}{err}");
        assert_eq!(world.borrow().reopens, 0);
        assert!(out.contains(" step=failed reopen=- "), "{out}");
        eprintln!("R5:\n{out}{err}");
    }

    /// A daemon whose note cannot be read.
    struct NoteUnreadable(FakeDaemon);

    impl Daemon for NoteUnreadable {
        fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
            self.0.ask(request, manifest)
        }
        fn device(&mut self) -> io::Result<u64> {
            self.0.device()
        }
        fn failure(&mut self) -> io::Result<Option<Noted>> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "EACCES"))
        }
    }

    /// R6 (passes at the head): a note that cannot be read fails the
    /// round, which goes on and begins; `publish` refuses with 1.
    #[test]
    fn r6_a_note_that_cannot_be_read() {
        let world = World::new(Mode::Busy);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, NoteUnreadable(FakeDaemon(world.clone())), &keep);
        let start = mono(&world);
        let (code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 120_000),
        );
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        let r = rounds(&out);
        assert!(r[0].starts_with("round 1 failed "), "{out}");
        assert_ne!(field(r[0], "begun"), "-", "{out}");
        assert!(err.contains("its failure note: EACCES"), "{err}");
        let world = World::new(Mode::Busy);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, NoteUnreadable(FakeDaemon(world.clone())), &keep);
        let (code, _, err) = executed(&mut agent, &Task::Publish, LIMIT);
        assert_eq!(code, exit::ERROR, "{err}");
        eprintln!("R6: {err}");
    }

    /// R7 (passes at the head): `recover` reads no note: on a lost
    /// volume it goes on to the pool (its settle's count and sweep).
    #[test]
    fn r7_recover_reads_no_note() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().noted = Some(held_yes());
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let (code, out, err) = executed(
            &mut agent,
            &Task::Recover("nope".into(), Choice::GiveUp),
            LIMIT,
        );
        eprintln!("R7: {code}\n{out}{err}");
        assert!(!err.contains("lost for this boot"), "{err}");
    }
}
