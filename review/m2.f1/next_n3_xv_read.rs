
/// The review's cross-version check (next look at #134), the reader's
/// half: this version, from before F1's N3, reads a status file that the
/// version with `row_v1` wrote (`REVIEW_STATUS_IN`).
#[test]
fn review_read_a_status_that_says_row_v1() {
    let Ok(path) = std::env::var("REVIEW_STATUS_IN") else {
        return;
    };
    let bytes = std::fs::read(path).expect("the file");
    assert_eq!(&bytes[568..576], &6u64.to_le_bytes());
    let read = decode(&bytes).expect("a status this version reads");
    let upload = read.shown.upload.as_ref().expect("upload");
    assert_eq!(upload.checked, CheckedShown::Unsaid);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "upload_checked"), Some("unsaid"));
    println!("REVIEW: decoded; upload_checked {:?}; upload {:?}", value(&lines, "upload_checked"), value(&lines, "upload"));
}
