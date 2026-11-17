
// ---- Review scenarios for #112 at 88e4edc (M2.4b-1), on real files. Not part
// of the product: `scenarios.py` appends this to `tests/inplace_real.rs` in a
// checkout of its own, runs it and takes it out again. ----

/// R1. A cut state's file with an old, short or odd header: what the
/// start says, that it serves, and that the start after it takes up the
/// state it began. Printed; the two cases the head's own test and its
/// description rest on are judged.
#[cfg(feature = "test-hooks")]
#[test]
fn review_r1_old_short_and_odd_headers_of_the_cut_state() {
    use skyzfs_local::inplace::CutsTakenUp;
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        println!("R1: SKIPPED, this file system offers no direct I/O");
        return;
    };
    create(&dir, &config()).unwrap();
    let run_dir = tempfile::tempdir().unwrap();
    let region = run_dir.path().join("cuts.state");
    drop(start_holding(&dir, &region, BOOT, false));
    let good = std::fs::read(&region).unwrap();
    println!("R1: a state of this version is {} bytes", good.len());
    let with = |version: u64, len: usize| {
        let mut bytes = good.clone();
        bytes[8..16].copy_from_slice(&version.to_le_bytes());
        bytes.resize(len, 0);
        bytes
    };
    let full = good.len();
    let mut other_magic = with(1, full - 16);
    other_magic[0] ^= 1;
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("version 1, two words shorter (as version 1 left it)", with(1, full - 16)),
        ("version 1, the length of version 2", with(1, full)),
        ("version 1, one byte longer", with(1, full + 1)),
        ("version 1, cut to its 16 bytes of header", with(1, 16)),
        ("version 1, cut to 15 bytes", with(1, 15)),
        ("the magic alone, 8 bytes", with(1, 8)),
        ("empty", Vec::new()),
        ("version 2, two words shorter", with(2, full - 16)),
        ("version 2, one byte shorter", with(2, full - 1)),
        ("version 0, two words shorter", with(0, full - 16)),
        ("version 3, two words shorter", with(3, full - 16)),
        ("version u64::MAX, two words shorter", with(u64::MAX, full - 16)),
        ("another magic, version 1, two words shorter", other_magic),
    ];
    for (name, bytes) in cases {
        std::fs::write(&region, &bytes).unwrap();
        let store = start_holding(&dir, &region, BOOT, true);
        let said = format!("{:?}", store.cuts_at_start());
        let len = std::fs::metadata(&region).unwrap().len();
        drop(store);
        let after = start_holding(&dir, &region, BOOT, true);
        let then = format!("{:?}", after.cuts_at_start());
        drop(after);
        println!("R1: {name} ({} bytes): {said}; the file is {len} bytes; the start after: {then}", bytes.len());
        assert_eq!(len as usize, full, "{name}");
        assert_eq!(then, format!("{:?}", Some(&CutsTakenUp::TakenUp)), "{name}");
        if name.starts_with("version 1, two words") || name.starts_with("version 1, the length") {
            assert!(said.contains("version 1,"), "{name}: {said}");
        }
        if name.starts_with("version 2") {
            assert!(!said.contains("version"), "{name}: {said}");
        }
    }
}
