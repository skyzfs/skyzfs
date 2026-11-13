
    // ---- review probes (not part of the pull request) ----

    /// Probe A: a block under a discard that no flush covered, at a power
    /// cut. §4.1's discard rule lets its reads differ until the next
    /// write. Does the model take two different answers at the read-back,
    /// and after it?
    #[test]
    fn probe_a_discarded_block_reads_may_differ_after_a_cut() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.flush();
        let d = c.discard(0, 1);
        c.ack(d);
        // Before the cut both answers are fine, in alternation.
        c.sees(0, &[0]).unwrap();
        c.sees(0, &[1]).unwrap();
        c.model.power_cut();
        assert_eq!(c.model.readback(), vec![0]);
        let first = c.sees(0, &[1]);
        let second = c.sees(0, &[0]);
        eprintln!("probe A: read-back [1] then [0]: {first:?}, {second:?}");
        assert!(first.is_ok() && second.is_ok());
    }

    /// Probe A2: the same with the discard flushed and folded: the block
    /// is not in the read-back, and its reads may differ after the cut.
    #[test]
    fn probe_a2_flushed_discard_reads_may_differ_after_a_cut() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.flush();
        let d = c.discard(0, 1);
        c.ack(d);
        c.flush();
        c.model.power_cut();
        eprintln!("probe A2: read-back wants {:?}", c.model.readback());
        let first = c.sees(0, &[1]);
        let second = c.sees(0, &[0]);
        eprintln!("probe A2: [1] then [0]: {first:?}, {second:?}");
        assert!(first.is_ok() && second.is_ok());
    }

    /// Probe B: X, two blocks, is in flight and never acknowledged; a
    /// read shows it on block 0; Y overwrites block 0, is acknowledged
    /// and flushed; power cut. §4.1: the flush covers what was
    /// acknowledged before it, so block 1 (written by X alone) may hold
    /// its old content.
    #[test]
    fn probe_b_an_unacknowledged_write_before_a_flushed_one() {
        let mut c = Client::new();
        let _x = c.write(0, 2, 1);
        c.sees(0, &[1]).unwrap();
        c.wrote(0, 1, 2);
        c.flush();
        c.model.power_cut();
        let ok = c.would_see(0, &[2, 0]);
        eprintln!("probe B: [2, 0] accepted: {ok}; [2, 1]: {}", c.would_see(0, &[2, 1]));
        assert!(ok);
    }

    /// Probe C: a tear at half a sector.
    #[test]
    fn probe_c_a_tear_at_half_a_sector_is_refused() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.model.power_cut();
        let mut half = vec![1u8; B];
        half[256..512].fill(0);
        let r = c.read(0, 1);
        let got = c.returns(r, half);
        eprintln!("probe C: {got:?}");
        assert!(got.is_err());
    }

    /// Probe D: two cuts with a read-back begun and not finished between
    /// them: what the first read-back saw of block 0 may not change.
    #[test]
    fn probe_d_a_second_cut_during_the_read_back() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.wrote(1, 1, 1);
        c.model.power_cut();
        assert_eq!(c.model.readback(), vec![0, 1]);
        c.sees(0, &[1]).unwrap();
        c.model.power_cut();
        let changed = c.would_see(0, &[0, 0]);
        eprintln!("probe D: block 0 read as 1, cut again, read as 0: accepted {changed}");
        assert!(!changed);
    }

    /// Probe E: a write that failed on a failing volume, then reopened:
    /// the block may hold it in part.
    #[test]
    fn probe_e_a_failed_write_may_be_there_in_part() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.flush();
        c.model.log_failed();
        let w = c.write(0, 2, 2);
        c.done(w, Err(BlkError::Failed)).unwrap();
        c.model.reopened();
        eprintln!("probe E: read-back wants {:?}", c.model.readback());
        let mut part = vec![1u8; 2 * B];
        part[512..1024].fill(2);
        part[B..].fill(0);
        part[B + 1024..B + 1536].fill(2);
        let r = c.read(0, 2);
        let got = c.returns(r, part);
        eprintln!("probe E: {got:?}");
        assert!(got.is_ok());
        // And nothing older than the flush.
        let mut c2 = Client::new();
        c2.wrote(0, 1, 1);
        c2.flush();
        c2.model.log_failed();
        let w = c2.write(0, 1, 2);
        c2.done(w, Err(BlkError::Failed)).unwrap();
        c2.model.reopened();
        assert!(!c2.would_see(0, &[0]));
    }
}
