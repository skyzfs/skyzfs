// Scratch tests of the review of #179 (M2.7b-2): appended to
// crates/skyzfs-agent/src/program/tests.rs by apply.py, at adc391a and at
// d68471f. Not for the repository as they are; each asserts what a
// reviewer would want, so a failure is the finding it shows (review.md).
// T5 was also run at the base 7cf6da6 in a module of its own
// (`mod review179 { use super::*; <T5> }`), where it passes.

mod review179 {
    use skyzfs_control::failure::{Failure, Held, Reason};

    use super::*;

    /// The fake daemon, whose `retire` is not answered: each costs
    /// `cost` of the fake clock (the daemon's ANSWER_LIMIT while the
    /// uploader is in a long step) and answers `no-answer`. At the first
    /// retire it may also leave a failure note, as a volume lost then.
    struct Unanswered {
        daemon: FakeDaemon,
        cost: Duration,
        asked: Rc<std::cell::Cell<usize>>,
        note_at_first: bool,
    }

    impl Daemon for Unanswered {
        fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
            if let Request::Retire(_) = request {
                let mut w = self.daemon.0.borrow_mut();
                if self.asked.get() == 0 && self.note_at_first {
                    w.noted = Some(Noted::Failed(Failure {
                        held: Held::Yes,
                        reason: Reason::ImageFailed,
                        identity: [7; 16],
                    }));
                    w.noted_from_ms = Some(w.now_ms);
                    w.suspends_after_ms = Some(1_050);
                }
                self.asked.set(self.asked.get() + 1);
                w.advance(self.cost);
                return Ok(Reply::Refused(
                    Code::NoAnswer,
                    "the uploader did not answer within 30 s".into(),
                ));
            }
            self.daemon.ask(request, manifest)
        }
        fn device(&mut self) -> io::Result<u64> {
            self.daemon.device()
        }
        fn failure(&mut self) -> io::Result<Option<Noted>> {
            self.daemon.failure()
        }
    }

    /// Twelve names published on an idle world, by an agent of its own.
    fn twelve() -> (Rc<RefCell<World>>, MemKeep) {
        let world = World::new(Mode::Idle);
        let keep = MemKeep::default();
        let mut a = agent_on(&world, FakeDaemon(world.clone()), &keep);
        for _ in 0..12 {
            a.begin().expect("begun");
            a.settle(LIMIT).expect("settled");
            world.borrow_mut().advance(Duration::from_secs(3));
        }
        assert_eq!(a.record().published.len(), 12);
        (world, keep)
    }

    /// T1. Retention's asks are not bounded by the round's tick: eight
    /// retires that the daemon does not answer hold the round for eight
    /// times its answer limit, and the ticks pass. The bound asked for
    /// here: no more than one unanswered retire a round (stop at the
    /// first `no-answer`), so a round with nothing else slow stays
    /// within its tick.
    #[test]
    fn review179_t1_unanswered_retires_hold_the_round_past_its_ticks() {
        let (world, keep) = twelve();
        let asked = Rc::new(std::cell::Cell::new(0));
        let daemon = Unanswered {
            daemon: FakeDaemon(world.clone()),
            cost: Duration::from_secs(30),
            asked: asked.clone(),
            note_at_first: false,
        };
        let mut agent = agent_on(&world, daemon, &keep);
        agent.set_keep(1);
        let start = mono(&world);
        let (_, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 30_000),
        );
        let rounds = rounds(&out);
        assert!(!rounds.is_empty(), "{out}{err}");
        let took = field(rounds[0], "took_ms").parse::<u64>().expect("ms");
        let asked = asked.get();
        eprintln!("T1: round 1 asked {asked} retires and took {took} ms: {}", rounds[0]);
        assert!(took < 60_000, "round 1 took {took} ms with {asked} unanswered retires");
    }

    /// T2. A volume lost while retention waits on unanswered retires is
    /// stepped on only after retention ends: neither the note nor an end
    /// is asked between its steps. What #173's F1 promised: within
    /// seconds.
    #[test]
    fn review179_t2_a_volume_lost_during_retention_waits_for_its_steps() {
        let (world, keep) = twelve();
        let asked = Rc::new(std::cell::Cell::new(0));
        let daemon = Unanswered {
            daemon: FakeDaemon(world.clone()),
            cost: Duration::from_secs(30),
            asked: asked.clone(),
            note_at_first: true,
        };
        let mut agent = agent_on(&world, daemon, &keep);
        agent.set_keep(1);
        let start = mono(&world);
        let (code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 900_000),
        );
        let failed_at = world.borrow().noted_from_ms.expect("noted");
        let reopened = world.borrow().suspended_at.expect("reopened") - 1_050;
        let after = reopened - failed_at;
        eprintln!(
            "T2: code {code}; the reopen {after} ms after the failure; asked {}\n{out}{err}",
            asked.get()
        );
        assert!(after <= 5_000, "the reopen {after} ms after the failure");
    }

    /// T5. The uploader stopped for a few hours (every publish refused
    /// `stopped`, given up at a round each): the names given up reach
    /// ABANDONED_MAX (256), and `room()` then refuses every begin until the
    /// oldest leave, `--abandoned-keep-s` (24 h) after them, though the
    /// uploader is back. Before version 3 the oldest were dropped past 64.
    #[test]
    fn review179_t5_hours_of_give_ups_stop_the_begins_after_the_uploader_is_back() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().refuse_every_publish = Some(Code::Stopped);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let every = Duration::from_secs(60);
        let (_, out, _) = scheduled(&mut agent, every, &mut until(&world, start + 5 * 3600 * 1000));
        let given_up = agent.record().abandoned.len();
        let full_at = rounds(&out)
            .iter()
            .position(|l| field(l, "begun") == "-")
            .map(|i| i + 1);
        // The uploader is back.
        world.borrow_mut().refuse_every_publish = None;
        let back = mono(&world);
        let (_, out2, err2) = scheduled(&mut agent, every, &mut until(&world, back + 3600 * 1000));
        // (T5b, at the head only: the rounds after that hour, to the first
        // that began.)
        // let (_, out3, _) = scheduled(&mut agent, every, &mut until(&world, back + 24 * 3600 * 1000));
        // let first = rounds(&out3).iter().position(|l| field(l, "begun") != "-");
        let begun = rounds(&out2)
            .iter()
            .filter(|l| field(l, "begun") != "-")
            .count();
        let published = out2.lines().filter(|l| l.starts_with("snap/")).count();
        eprintln!(
            "T5: {given_up} given up in 5 h, the first round that began nothing: {full_at:?}; \
             in the hour after the uploader is back: {} rounds, {begun} begun, {published} \
             published; its last error line: {:?}",
            rounds(&out2).len(),
            err2.lines().last()
        );
        assert!(published > 0, "nothing published in the hour after the uploader is back");
    }
}
