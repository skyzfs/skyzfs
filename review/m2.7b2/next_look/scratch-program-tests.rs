    // Scratch test of the review's next look at #179 (78c3997), planted
    // in a copy only: inserted at the end of the module
    // `retention_in_rounds` in crates/skyzfs-agent/src/program/tests.rs.
    // Not a proposal of wording; it shows a residual of M1.

    /// The daemon of a stopped uploader as skyzfsd answers it: a retire
    /// is refused `stopped` (bins/skyzfsd/src/control.rs, RetireRefused::
    /// Stopped) while `stopped` holds.
    struct StoppedRetires {
        daemon: FakeDaemon,
        stopped: Rc<Cell<bool>>,
    }

    impl Daemon for StoppedRetires {
        fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
            if let Request::Retire(_) = request {
                if self.stopped.get() {
                    return Ok(Reply::Refused(
                        Code::Stopped,
                        "the uploader has ended".into(),
                    ));
                }
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

    /// As `hours_of_give_ups_leave_room_and_the_publishes_resume_at_once`,
    /// but with eleven names marked to be retired when the uploader stops
    /// (`--keep 1` over twelve published), and the retires refused
    /// `stopped` as the publishes are.
    #[test]
    fn review179n_a_backlog_of_marked_names_starves_the_early_leave() {
        let (world, keep) = twelve();
        world.borrow_mut().mode = Mode::Busy;
        world.borrow_mut().refuse_every_publish = Some(Code::Stopped);
        let stopped = Rc::new(Cell::new(true));
        let daemon = StoppedRetires {
            daemon: FakeDaemon(world.clone()),
            stopped: stopped.clone(),
        };
        let mut agent = agent_on(&world, daemon, &keep);
        let every = Duration::from_secs(60);
        let start = mono(&world);
        let (_, out, _) = scheduled_kept(
            &mut agent,
            every,
            keep_one(),
            &mut until(&world, start + 5 * 3600 * 1000),
        );
        let lines = rounds(&out);
        let idle = lines.iter().filter(|l| field(l, "begun") == "-").count();
        let first_idle = lines.iter().position(|l| field(l, "begun") == "-");
        let given_up = agent.record().abandoned.len();
        // The uploader is back.
        world.borrow_mut().refuse_every_publish = None;
        stopped.set(false);
        let back = mono(&world);
        let (_, out2, _) = scheduled_kept(
            &mut agent,
            every,
            keep_one(),
            &mut until(&world, back + 10 * 60_000),
        );
        let after = rounds(&out2);
        let first_begun = after.iter().position(|l| field(l, "begun") != "-");
        assert!(
            idle == 0 && first_begun == Some(0),
            "while stopped: {idle} of {} rounds began nothing, the first at round {first_idle:?}, \
             {given_up} names given up; after the uploader is back, the first round that began \
             is {first_begun:?} of {}",
            lines.len(),
            after.len()
        );
    }
