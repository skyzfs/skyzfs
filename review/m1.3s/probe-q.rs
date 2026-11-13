
    // ---- review probe (not part of the pull request) ----

    /// Probe Q: two overlapping writes in flight together, both
    /// acknowledged; a discard of both blocks, acknowledged; power cut.
    /// While block 1 is still to be read back, is block 0 allowed to read
    /// as zeros, as the one write and as the other? No one order of the
    /// operations has both writes as "the content before the discard".
    #[test]
    fn probe_q_discard_freedom_is_judged_read_by_read() {
        let mut c = Client::new();
        let a = c.write(0, 2, 1);
        let b = c.write(0, 2, 2);
        c.ack(a);
        c.ack(b);
        let d = c.discard(0, 2);
        c.ack(d);
        c.model.power_cut();
        eprintln!("probe Q: read-back wants {:?}", c.model.readback());
        let r0 = c.sees(0, &[0]);
        let r1 = c.sees(0, &[1]);
        let r2 = c.sees(0, &[2]);
        eprintln!("probe Q: block 0 as 0, 1, 2 inside the window: {r0:?}, {r1:?}, {r2:?}");
        let end = c.sees(1, &[0]);
        eprintln!("probe Q: block 1 as 0, which ends the window: {end:?}");
        let a1 = c.sees(0, &[1]);
        let a2 = c.sees(0, &[2]);
        eprintln!("probe Q: after the window, block 0 as 1 then 2: {a1:?}, {a2:?}");
    }
}
