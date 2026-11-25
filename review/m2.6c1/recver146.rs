// Scratch tests of the look at #146 (item 2: the record stays version 2
// with `Why::Stopped` appended), appended to `record/tests.rs` in a
// scratch worktree by `recver146.py`. They compile at every head of the
// pull request and print what they find.
//
// - `recver_new_bytes`: the golden bytes of 4ff4549 (one abandoned entry
//   `Stopped` more), read by the head they run at. Run at the base
//   (8663cd7, the agent as merged; its record is cd3a67c's).
// - `recver_old_bytes`: the golden bytes of the base and cd3a67c, read by
//   the head they run at. Run at 4ff4549, against its `golden()` less its
//   last abandoned entry.
// - `recver_one_stopped`: a record whose only abandoned entry is
//   `Stopped`, made at the head it runs at, its bytes printed (run at
//   4ff4549; the bytes are then read at the base by `recver_new_bytes`'s
//   second half, pasted).

const GOLDEN_4FF4549: [&str; 17] = [
    "535a414702000000ed0100000f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f01017081",
    "018201050169a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e",
    "5e5e5e5e5e5e03ad2100000014600174a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad2101280103702f6401ac02414141",
    "4141414141414141414141414141414141414141414141414141414141000115",
    "610168a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e5e5e",
    "5e5e5e03ad2102290103702f6402ac0242424242424242424242424242424242",
    "42424242424242424242424242424242010216620161a1a1a1a1a1a1a1a1a1a1",
    "a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad21032a0103702f64",
    "03ac024343434343434343434343434343434343434343434343434343434343",
    "4343430607010317630164a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e",
    "5e5e5e5e5e5e5e5e5e5e5e03ad21042b0103702f6404ac024444444444444444",
    "444444444444444444444444444444444444444444444444c0c0c0c0c0c0c0c0",
    "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0010418640a027730",
    "00e80702773101e90702773202ea0702773303eb0702773404ec0702773505ed",
    "0702773606ee0702773707ef0702773808f00702773909f1070545dc512e04dd",
    "a024450c85490e0ba1e949a3a9bac375000d8d273edb82d3c2",
];

const GOLDEN_BASE: [&str; 17] = [
    "535a414702000000e70100000f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f01017081",
    "018201050169a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e",
    "5e5e5e5e5e5e03ad2100000014600174a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad2101280103702f6401ac02414141",
    "4141414141414141414141414141414141414141414141414141414141000115",
    "610168a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e5e5e",
    "5e5e5e03ad2102290103702f6402ac0242424242424242424242424242424242",
    "42424242424242424242424242424242010216620161a1a1a1a1a1a1a1a1a1a1",
    "a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad21032a0103702f64",
    "03ac024343434343434343434343434343434343434343434343434343434343",
    "4343430607010317630164a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e",
    "5e5e5e5e5e5e5e5e5e5e5e03ad21042b0103702f6404ac024444444444444444",
    "444444444444444444444444444444444444444444444444c0c0c0c0c0c0c0c0",
    "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c00104186409027730",
    "00e80702773101e90702773202ea0702773303eb0702773404ec0702773505ed",
    "0702773606ee0702773707ef0702773808f0076316f2b2ac52930bdf0feaed14",
    "b32d2fb27637bd2ffa3878db109b5ca374cbdc",
];

/// A record of version 2 with one abandoned entry, `Stopped`, as 4ff4549
/// writes it (printed by `recver_one_stopped` at 4ff4549).
const ONE_STOPPED_4FF4549: &str = "ONE_STOPPED";

fn recver_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn recver_new_bytes() {
    let read = Record::decode(&recver_bytes(&GOLDEN_4FF4549.concat()));
    println!("RECVER new golden read here: {:?}", read.as_ref().map(|r| r.abandoned.len()));
    if ONE_STOPPED_4FF4549 != "ONE_STOPPED" {
        let read = Record::decode(&recver_bytes(ONE_STOPPED_4FF4549));
        println!("RECVER one Stopped read here: {read:?}");
    }
}

#[test]
fn recver_old_bytes() {
    let read = Record::decode(&recver_bytes(&GOLDEN_BASE.concat()));
    let mut want = golden();
    let dropped = want.abandoned.pop();
    println!(
        "RECVER base golden read here: ok {}, equal to golden() less {:?}: {}",
        read.is_ok(),
        dropped.as_ref().map(|a| a.why),
        read.as_ref().ok() == Some(&want)
    );
}

#[test]
fn recver_one_stopped() {
    let mut record = golden();
    record.under_way.clear();
    record.abandoned.retain(|a| format!("{:?}", a.why) == "Stopped");
    if record.abandoned.is_empty() {
        println!("RECVER one Stopped: no such variant here");
        return;
    }
    let bytes = record.encode().expect("encoded");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    println!("RECVER one Stopped bytes: {hex}");
}
