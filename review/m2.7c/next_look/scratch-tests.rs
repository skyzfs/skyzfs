
// Scratch tests of the review's next look at #173 (3b93a83), appended to
// crates/skyzfs-agent/src/program/tests.rs in a scratch worktree; not
// committed anywhere. N1 and N2 state what the docs or the criterion
// say and fail at the head; N3 records what the head does; N4 passes
// at the head and fails under the plant W12 (a test the head lacks).
mod review_next_look {
    use skyzfs_control::failure::{Failure, Held, Reason};

    use super::*;

    fn noted(held: Held) -> Noted {
        Noted::Failed(Failure {
            held,
            reason: Reason::ImageFailed,
            identity: [7; 16],
        })
    }

    /// N1. A volume lost while `run` waits (+5 s, `--every-s 60`); the
    /// early step's reopen leaves the pool ONLINE (the device still
    /// there, say), and a reopen made later suspends it. The retry comes
    /// at the tick after the one the early round took: about 115 s after
    /// the failure, where an on-time step is retried 60 s later.
    #[test]
    fn n1_a_retry_after_an_early_step_comes_within_a_tick() {
        let world = World::new(Mode::Idle);
        let start = mono(&world);
        let failed_at = world.borrow().now_ms + 5_000;
        {
            let mut w = world.borrow_mut();
            w.noted = Some(noted(Held::Yes));
            w.noted_from_ms = Some(failed_at);
            w.gone_from = Some(failed_at);
            w.suspends_after_ms = None;
        }
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let watched = world.clone();
        let mut go = until(&world, start + 600_000);
        let mut asked = move || {
            let first_done = {
                let w = watched.borrow();
                w.reopens >= 1 && w.suspends_after_ms.is_none()
            };
            if first_done {
                watched.borrow_mut().suspends_after_ms = Some(1_050);
            }
            go()
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut asked);
        eprintln!("N1 out:\n{out}N1 err:\n{err}");
        assert_eq!(code, exit::LOST, "{out}{err}");
        let second = world.borrow().suspended_at.expect("a second reopen") - 1_050;
        let after = second - failed_at;
        eprintln!(
            "N1: the second reopen {after} ms after the failure; reopens {}",
            world.borrow().reopens
        );
        assert!(after <= 65_000, "the retry {after} ms after the failure");
    }

    /// N2. The docs (program.rs, "The note is watched between rounds")
    /// say the watch stops the engine "before a begin after syncs". After
    /// a rewind import's three syncs, the note is there 1.5 s before the
    /// begin's go: the round begins a publish all the same.
    #[test]
    fn n2_news_before_a_begin_after_syncs_begins_nothing() {
        let world = rewound_t(5);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let base = world.borrow().synced_at.len();
        let watched = world.clone();
        let mut go = until(&world, start + 300_000);
        let mut done = false;
        let mut asked = move || {
            if !done && watched.borrow().synced_at.len() >= base + 3 {
                done = true;
                let mut w = watched.borrow_mut();
                w.noted = Some(noted(Held::Yes));
                w.suspends_after_ms = Some(1_050);
                w.advance(Duration::from_millis(1_500));
            }
            go()
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut asked);
        eprintln!("N2 out:\n{out}N2 err:\n{err}");
        assert_eq!(code, exit::LOST, "{out}{err}");
        let rounds = rounds(&out);
        assert_eq!(field(rounds[0], "synced"), "3", "{out}");
        assert_eq!(
            field(rounds[0], "begun"),
            "-",
            "a begin after the news: {out}"
        );
    }

    /// N3. A pool that never reads SUSPENDED after the reopen (a wrong
    /// `--pool`, or a device that waits for a server): what `run` does
    /// in ten minutes, at `--every-s 60` and at 1. Records only.
    #[test]
    fn n3_a_pool_that_never_suspends_is_reopened_at_each_round() {
        for every in [60u64, 1] {
            let world = World::new(Mode::Idle);
            {
                let mut w = world.borrow_mut();
                w.noted = Some(noted(Held::Yes));
                w.suspends_after_ms = None;
            }
            let keep = MemKeep::default();
            let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
            let start = mono(&world);
            let (code, out, _) = scheduled(
                &mut agent,
                Duration::from_secs(every),
                &mut until(&world, start + 600_000),
            );
            let r = rounds(&out);
            let failed = r.iter().filter(|l| l.contains(" failed ")).count();
            eprintln!(
                "N3 every={every}: code {code}, rounds {}, failed {failed}, reopens {}",
                r.len(),
                world.borrow().reopens
            );
        }
    }

    /// N4. The note comes after round 1 (+5 s, `--every-s 60`), and the
    /// early step leaves the pool ONLINE for good. The rounds after the
    /// retry keep to the ticks: they do not begin one after another.
    /// Fails under W12 (the watch set only at round 1).
    #[test]
    fn n4_the_rounds_after_an_early_retry_keep_to_the_ticks() {
        let world = World::new(Mode::Idle);
        let start = mono(&world);
        let failed_at = world.borrow().now_ms + 5_000;
        {
            let mut w = world.borrow_mut();
            w.noted = Some(noted(Held::Yes));
            w.noted_from_ms = Some(failed_at);
            w.gone_from = Some(failed_at);
            w.suspends_after_ms = None;
        }
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let (code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 300_000),
        );
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        let reopens = world.borrow().reopens;
        eprintln!("N4: {reopens} reopens in 300 s");
        assert!(reopens <= 6, "{reopens} reopens in 300 s: {out}");
    }
}
