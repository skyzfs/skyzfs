
// ---- Review's scratch tests, next look at #179 (78c3997). Appended to
// crates/skyzfs-agent/src/zfs/cli/tests.rs in a scratch tree only.

/// `ZfsCli::destroy_one`: one member's `@t`, no `-r`, within the change
/// limit; killed at its limit, `NotRun`; the dataset gone, destroyed;
/// a dataset not under the pool, a dataset with an `@`, or a name that
/// is not a snapshot's, not destroyed and nothing run. No test of the
/// head calls `destroy_one` on the adapter. Mutants: B-cli-one-other,
/// B-cli-one-guard.
#[test]
fn review179n_the_destroy_of_one_member() {
    const ONE: &str = "zfs destroy tank/db@skz-1";
    let mut fake = Fake::default();
    fake.ok(ONE, "");
    let mut zfs = cli(fake);
    assert_eq!(zfs.destroy_one("tank", "tank/db", "skz-1"), Ok(()));
    assert_eq!(zfs.host().words(), [ONE]);
    assert_eq!(zfs.host().ran[0].limit, Limits::default().change);
    let mut fake = Fake::default();
    fake.give(ONE, Err(std::io::ErrorKind::TimedOut));
    assert!(matches!(
        cli(fake).destroy_one("tank", "tank/db", "skz-1"),
        Err(ZfsError::NotRun(why)) if why.starts_with("tank/db@skz-1 may not have been destroyed: ")
    ));
    let mut fake = Fake::default();
    fake.fails(ONE, 1, "cannot open 'tank/db': dataset does not exist\n");
    assert_eq!(cli(fake).destroy_one("tank", "tank/db", "skz-1"), Ok(()));
    for (dataset, t) in [
        ("other/db", "skz-1"),
        ("tankx", "skz-1"),
        ("tank@x", "skz-1"),
        ("tank/db", "skz-1%skz-9"),
    ] {
        let mut zfs = cli(Fake::default());
        assert!(zfs.destroy_one("tank", dataset, t).is_err(), "{dataset}@{t}");
        assert!(zfs.host().ran.is_empty(), "{dataset}@{t}");
    }
}

/// A member's `@t` already gone when it is destroyed: `zfs destroy
/// DATASET@T` without `-r` takes the same path in `zfs_do_destroy` as
/// with it (OpenZFS 2.4.1 zfs_main.c:1745-1767: the snapshot is not
/// found, ENOENT is dropped by `gather_snapshots`, and the list is
/// empty), so ZFS says what `destroy_r` already reads as destroyed.
/// Expected to FAIL at the head: `destroy_one` returns `Other`, and
/// retention moves the name to the names stuck.
#[test]
fn review179n_a_member_already_gone_is_destroyed() {
    const ONE: &str = "zfs destroy tank/db@skz-1";
    let mut fake = Fake::default();
    fake.fails(
        ONE,
        1,
        "could not find any snapshots to destroy; check snapshot names.\n",
    );
    assert_eq!(cli(fake).destroy_one("tank", "tank/db", "skz-1"), Ok(()));
}
