
// ---- review of #165: scratch tests, not to be committed ----
mod review_165 {
    use super::*;

    const W: &[u8] = b"\tWed Nov 25 21:24 2026\n";

    fn line(snapshot: &str, tag: &[u8]) -> Vec<u8> {
        [snapshot.as_bytes(), b"\t", tag, W].concat()
    }

    /// R1, the sweep: a forging tag on tank/db@backup names tank@skz-1,
    /// which had one hold (the forger's `h`) at the list and none at the
    /// holds (released in the window). The forged line passes the count.
    #[test]
    fn review_race_sweep_forged_line_passes() {
        let held = ["tank/db@backup", "tank@skz-1"];
        let holds = line("tank/db@backup", b"x\tY\ntank@skz-1\tskyzfs-publish");
        let mut fake = Fake::default();
        fake.ok(TAGGED_LIST, "tank/db@backup\t1\ntank@skz-1\t1\n")
            .give(&holds_of(&held), Ok(bytes_out(holds)));
        let mut zfs = cli(fake);
        let got = zfs.tagged("tank", HOLD_TAG);
        eprintln!("RACE-SWEEP: {got:?} words={:?}", zfs.host().words());
        assert_eq!(got, Ok(["skz-1".to_string()].into()));
    }

    /// R1, held: the agent's hold on tank/db@skz-1 is gone (released by
    /// hand); a forging tag there forges it; a hold added on it between
    /// `holds -r` and the count makes the count 2. Held::All is read.
    #[test]
    fn review_race_held_forged_line_passes() {
        const BOTH: &str = r#"{"return":{"members":{"tank":"11","tank/db":"22"}}}"#;
        let holds = [
            line("tank@skz-1", b"skyzfs-publish"),
            line("tank/db@skz-1", b"x\tY\ntank/db@skz-1\tskyzfs-publish"),
        ]
        .concat();
        let both = refs_words(&["tank/db@skz-1", "tank@skz-1"]);
        let mut fake = Fake::default();
        fake.give(HOLDS_R, Ok(bytes_out(holds.clone())))
            .ok(MEMBERS_WORDS, BOTH)
            .ok(&both, "tank/db@skz-1\t2\ntank@skz-1\t1\n");
        let mut zfs = cli(fake);
        let members = zfs.members("tank", "skz-1").expect("members");
        let got = zfs.held("tank", "skz-1", HOLD_TAG, members.as_deref());
        eprintln!("RACE-HELD: {got:?}");
        assert_eq!(got, Ok(Held::All));
        // Without the race (count 1), an error.
        let mut fake = Fake::default();
        fake.give(HOLDS_R, Ok(bytes_out(holds)))
            .ok(MEMBERS_WORDS, BOTH)
            .ok(&both, "tank/db@skz-1\t1\ntank@skz-1\t1\n");
        let mut zfs = cli(fake);
        let members = zfs.members("tank", "skz-1").expect("members");
        let got = zfs.held("tank", "skz-1", HOLD_TAG, members.as_deref());
        eprintln!("NO-RACE-HELD: {got:?}");
        assert!(matches!(got, Err(ZfsError::Other(_))));
    }

    /// A4: a harmless tag with a tab, or with a newline, of another holder,
    /// on a held snapshot that the agent has nothing to do with, fails the
    /// sweep after three reads; and on a member's `@t`, fails `held`.
    #[test]
    fn review_harmless_odd_tags_fail_the_reads() {
        for tag in [&b"backup\t2026"[..], &b"note\nmore"[..], &b"trailing\n"[..]] {
            let held = ["tank/db@backup", "tank@skz-1"];
            let holds = [line("tank/db@backup", tag), line("tank@skz-1", b"skyzfs-publish")].concat();
            let mut fake = Fake::default();
            fake.ok(TAGGED_LIST, "tank/db@backup\t1\ntank@skz-1\t1\n")
                .give(&holds_of(&held), Ok(bytes_out(holds)));
            let mut zfs = cli(fake);
            let got = zfs.tagged("tank", HOLD_TAG);
            eprintln!("HARMLESS-SWEEP {:?}: {got:?} ({} commands)", String::from_utf8_lossy(tag), zfs.host().words().len());
            assert!(matches!(got, Err(ZfsError::Other(_))));

            let holds = [line("tank@skz-1", b"skyzfs-publish"), line("tank/db@skz-1", b"skyzfs-publish"), line("tank/db@skz-1", tag)].concat();
            let both = refs_words(&["tank/db@skz-1", "tank@skz-1"]);
            let mut fake = Fake::default();
            fake.give(HOLDS_R, Ok(bytes_out(holds)))
                .ok(&both, "tank/db@skz-1\t2\ntank@skz-1\t1\n");
            let mut zfs = cli(fake);
            let got = zfs.held("tank", "skz-1", HOLD_TAG, None);
            eprintln!("HARMLESS-HELD {:?}: {got:?}", String::from_utf8_lossy(tag));
            assert!(matches!(got, Err(ZfsError::Other(_))));
        }
    }

    /// N-v: a forged line naming an `@t` under the pool that does not
    /// exist: the count fails `dataset does not exist`, read as Vanished,
    /// three times; the error is Vanished, with no word of an odd tag.
    #[test]
    fn review_forged_missing_snapshot_reads_as_vanished() {
        let holds = [
            line("tank@skz-1", b"skyzfs-publish"),
            line("tank@skz-1", b"x\tY\ntank/nope@skz-1\tskyzfs-publish"),
        ]
        .concat();
        let both = refs_words(&["tank/nope@skz-1", "tank@skz-1"]);
        let mut fake = Fake::default();
        fake.give(HOLDS_R, Ok(bytes_out(holds)))
            .fails(&both, 1, "cannot open 'tank/nope@skz-1': dataset does not exist\n");
        let mut zfs = cli(fake);
        let got = zfs.held("tank", "skz-1", HOLD_TAG, None);
        eprintln!("MISSING: {got:?} words={}", zfs.host().words().len());
        assert!(matches!(got, Err(ZfsError::Vanished(_))));
    }
}
