// Scratch tests for the look at #168 (M2.6c-5f) at 2233537, appended to
// crates/skyzfs-agent/src/zfs/cli/tests.rs on a scratch worktree, run
// once, and removed. Not for the repository.

/// N4': in `held`, `hold` and `release` the holds come first and the
/// count after. A hold released between them (a `zfs send`'s, say),
/// at every read, gives more lines than holds; the error says holds
/// were added between the two reads.
#[test]
fn scratch_n4_direction_in_held() {
    const WHEN: &str = "\tWed Nov 25 10:00 2026\n";
    let line = |snapshot: &str, tag: &str| format!("{snapshot}\t{tag}{WHEN}");
    let holds = [line("tank@skz-1", HOLD_TAG), line("tank@skz-1", ".send-77-1")].concat();
    let counted = refs_words(&["tank@skz-1"]);
    let mut fake = Fake::default();
    // The holds -r saw the send's hold; the count, after it was released, does not.
    fake.ok(HOLDS_R, &holds).ok(&counted, "tank@skz-1\t1\n");
    let mut zfs = cli(fake);
    let got = zfs.held("tank", "skz-1", HOLD_TAG, Some(&[]));
    eprintln!("SCRATCH n4 held: {got:?}");
    match got {
        Err(ZfsError::Other(why)) => {
            assert!(why.contains("holds were added between the two reads"), "{why}");
            assert!(why.ends_with("(read 3 times)"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

/// A snapshot of the agent's renamed by a person while held
/// (`zfs rename tank@skz-1 tank@keep`): its hold is no longer swept.
#[test]
fn scratch_renamed_snapshot_not_swept() {
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@keep\t1\ntank/db@keep\t1\n");
    let mut zfs = cli(fake);
    let got = zfs.tagged("tank", HOLD_TAG);
    eprintln!("SCRATCH renamed: {got:?} words {:?}", zfs.host().words());
    assert_eq!(got, Ok(BTreeSet::new()));
    assert_eq!(zfs.host().words(), [TAGGED_LIST]);
}
