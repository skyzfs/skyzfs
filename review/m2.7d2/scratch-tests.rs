// The review's scratch tests for #172 (M2.7d-2) at edf9a17. Not for
// merging as they are. plants3.py appends the part after "ENGINE TESTS"
// to crates/skyzfs-agent/src/engine/tests.rs and the part after
// "PROGRAM TESTS" to crates/skyzfs-agent/src/program/tests.rs.
//// ENGINE TESTS
//// PROGRAM TESTS

mod review172 {
    use super::*;

    /// S1 (finding L3): the daemon unread after two syncs that returned.
    /// The round's line should count them, and an end asked during the
    /// last of them should begin no publish. Fails at edf9a17: the
    /// `Unseen` arm of `schedule` takes neither `last_synced` nor `go`.
    #[test]
    fn s1_a_round_whose_daemon_went_away_after_syncs_counts_them() {
        let world = rewound_t(10);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let watched = world.clone();
        let mut asked = 0u32;
        let mut go = move || {
            asked += 1;
            if asked == 3 {
                // Asked before the second sync: the daemon goes away
                // while it runs, and an end is asked during it.
                watched.borrow_mut().away_until = u64::MAX;
                return true;
            }
            asked < 3
        };
        let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        let rounds = rounds(&out);
        assert_eq!(world.borrow().synced_at.len(), 2, "{out}{err}");
        let calls = world.borrow().calls.clone();
        println!("{out}{err}calls: {calls:?}");
        assert_eq!(field(rounds[0], "synced"), "2", "{out}{err}");
    }

    /// S1b (finding L3, the end): as S1, and no call of a begin runs
    /// after the end was asked. Fails at edf9a17.
    #[test]
    fn s1b_an_end_during_the_syncs_then_the_daemon_unread_begins_nothing() {
        let world = rewound_t(10);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let watched = world.clone();
        let mut asked = 0u32;
        let ended_at = Rc::new(RefCell::new(None));
        let at = ended_at.clone();
        let mut go = move || {
            asked += 1;
            if asked == 3 {
                watched.borrow_mut().away_until = u64::MAX;
                *at.borrow_mut() = Some(watched.borrow().calls.len());
                return true;
            }
            asked < 3
        };
        let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        let from = ended_at.borrow().expect("asked three times");
        let calls = world.borrow().calls.clone();
        let after: Vec<&str> = calls[from..].to_vec();
        println!("{out}{err}");
        // The second sync's own calls (props of the assist, the set, the
        // sync) are allowed; a begin's count or sweep is not.
        assert!(
            !after.contains(&"count") && !after.contains(&"tagged"),
            "calls after the end was asked: {after:?}"
        );
    }

    /// S2 (plant F1): a round stopped by an end says `bound=0`.
    #[test]
    fn s2_a_round_stopped_by_an_end_is_not_at_the_bound() {
        let world = rewound_t(5);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let mut asked = 0u32;
        let mut go = move || {
            asked += 1;
            asked < 2
        };
        let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        let rounds = rounds(&out);
        assert_eq!(rounds.len(), 1, "{out}{err}");
        assert!(err.contains("an end was asked for"), "{err}");
        assert_eq!(field(rounds[0], "synced"), "0", "{out}");
        assert_eq!(field(rounds[0], "bound"), "0", "{out}");
    }

    /// S3 (plant F2): a round whose sync failed says `bound=0`.
    #[test]
    fn s3_a_round_whose_sync_failed_is_not_at_the_bound() {
        let world = rewound_t(5);
        world.borrow_mut().assist_fails = 1;
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let mut asked = 0u32;
        let mut go = move || {
            asked += 1;
            asked <= 2
        };
        let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        let rounds = rounds(&out);
        assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
        assert_eq!(field(rounds[0], "bound"), "0", "{out}");
    }

    /// S4 (plant F5): a round after one at the bound, where the daemon
    /// then holds no cut, says `bound=0`.
    #[test]
    fn s4_a_round_after_the_bound_that_finds_nothing_behind_says_bound_0() {
        let world = rewound_t(10_000);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let watched = world.clone();
        let mut cleared = false;
        let mut go = move || {
            let now = mono(&watched);
            if !cleared && now >= start + 60_000 {
                cleared = true;
                watched.borrow_mut().daemon.cuts.clear();
            }
            now < start + 90_000
        };
        let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        let rounds = rounds(&out);
        assert_eq!(rounds.len(), 2, "{out}{err}");
        assert_eq!(field(rounds[0], "bound"), "1", "{out}");
        assert_eq!(field(rounds[1], "bound"), "0", "{out}{err}");
    }

    /// S5 (plant F4): a round whose premise is refused before any sync,
    /// after a round whose sync failed after two returned, says
    /// `synced=0`: the count is the round's, not the round before's.
    #[test]
    fn s5_a_refusal_after_a_failed_sync_says_synced_0() {
        let world = rewound_t(10);
        {
            let mut world = world.borrow_mut();
            world.assist_fails = 1;
            world.assist_fails_after = 2;
        }
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let watched = world.clone();
        let mut attached = false;
        let mut go = move || {
            let now = mono(&watched);
            if !attached && now >= start + 60_000 {
                attached = true;
                watched.borrow_mut().user(User::Attach, "");
            }
            now < start + 90_000
        };
        let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        let rounds = rounds(&out);
        assert_eq!(rounds.len(), 2, "{out}{err}");
        assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
        assert_eq!(field(rounds[0], "synced"), "2", "{out}");
        assert!(rounds[1].starts_with("round 2 failed "), "{out}{err}");
        assert!(err.contains("round 2: "), "{err}");
        assert_eq!(field(rounds[1], "synced"), "0", "{out}{err}");
        assert_eq!(world.borrow().synced_at.len(), 2, "{out}");
    }
}
