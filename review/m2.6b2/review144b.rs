// Scratch: the program's file system (`Disk`) traced. Appended to the
// end of `crates/skyzfs-agent/src/record/tests.rs` in a scratch worktree
// only, run alone under strace with REVIEW_DIR set to a fresh directory.
#[test]
#[ignore]
fn review_trace_the_disk_path() {
    let Ok(dir) = std::env::var("REVIEW_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir).join("vol");
    let store = Store::open(&dir).expect("opened");
    let name = "skz-20261125T010000Z";
    store.save(&Record::new([1; 16])).expect("first save");
    store.write_manifest(name, b"{}").expect("manifest");
    store.save(&Record::new([1; 16])).expect("second save");
    store.remove_manifest(name).expect("removed");
    // A second store in this process is refused while the first lives.
    assert!(matches!(Store::open(&dir), Err(OpenError::Held(_))));
}
