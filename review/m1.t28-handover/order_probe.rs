
    /// Review probe (not part of the pull request): two writes of one
    /// block are in flight, the lost daemon answers some of them before
    /// the loss, and the harness hands those over. The target skips the
    /// reissue of a write its lost instance had logged
    /// (`Fault::SkipReissuedIfLogged`). Counts, over seeds, how often
    /// the suite catches it.
    #[test]
    fn review_probe_order_of_account_and_kill() {
        let mut caught = 0;
        let mut handed_one = 0;
        for seed in 0..200u64 {
            let config = MemConfig {
                schedule: Schedule::Seeded,
                apply: crate::mem::Timing::AtSubmit,
                fault: Some(Fault::SkipReissuedIfLogged),
                seed,
                ..MemConfig::default()
            };
            let mut harness = HandsOver {
                inner: MemHarness::new(config),
                before: Vec::new(),
                drops: false,
            };
            let suite = Config::default();
            let run = (|| -> Result<usize, Failure> {
                let mut d = Driver::new(&mut harness, &suite, &Spec::EMPTY)?;
                let a = d.write_op(3, 1);
                let b = d.write_op(3, 1);
                d.submit(a)?;
                d.submit(b)?;
                d.kill(seed)?;
                let left = d.inflight.len();
                d.drain()?;
                d.read(3, 1)?;
                Ok(left)
            })();
            match run {
                Ok(left) => handed_one += usize::from(left == 1),
                Err(_) => caught += 1,
            }
        }
        println!("PROBE caught {caught} of 200 seeds; passed with one write handed over and one in flight: {handed_one}");
    }
