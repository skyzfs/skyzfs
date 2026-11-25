// Scratch check of the next look at #144 (M2.6b2) at 45648ab. Appended
// to the end of `crates/skyzfs-agent/src/record/tests.rs` in a scratch
// worktree only.

/// The golden record of version 2 as it was at b5063b2, before
/// `begun_ms` was added under the same version number: what the agent
/// at 45648ab makes of it. (No agent has run either, so no such record
/// exists; this shows what the version number does not guard.)
#[test]
fn review_v2_before_begun_ms_is_read_as() {
    const OLD: [&str; 17] = [
        "535a414702000000dc0100000f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f01017081",
        "018201050169a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e",
        "5e5e5e5e5e5e03ad21000000140174a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e",
        "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad2101280103702f6401ac0241414141",
        "4141414141414141414141414141414141414141414141414141414100011501",
        "68a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e5e5e5e5e",
        "5e03ad2102290103702f6402ac02424242424242424242424242424242424242",
        "42424242424242424242424242420102160161a1a1a1a1a1a1a1a1a1a1a1a1a1",
        "a1a1a15e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad21032a0103702f6403ac02",
        "4343434343434343434343434343434343434343434343434343434343434343",
        "06070103170164a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e",
        "5e5e5e5e5e5e5e03ad21042b0103702f6404ac02444444444444444444444444",
        "4444444444444444444444444444444444444444c0c0c0c0c0c0c0c0c0c0c0c0",
        "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c00104180802773000e8070277",
        "3101e90702773202ea0702773303eb0702773404ec0702773505ed0702773606",
        "ee0702773707ef079f072722cd00b3fb3c406cb5fae66c76344ccd515ab42a01",
        "ae56f4a130f56cd5",
    ];
    let hex: String = OLD.concat();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
        .collect();
    let read = Record::decode(&bytes);
    eprintln!("b5063b2's golden record read at 45648ab: {read:?}");
    assert!(read.is_err(), "{read:?}");
}
