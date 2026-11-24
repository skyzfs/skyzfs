
/// The review's cross-version check (next look at #134), the writer's
/// half: a status file of this version whose last check found `row_v1`,
/// written where `REVIEW_STATUS_OUT` says.
#[test]
fn review_write_a_status_that_says_row_v1() {
    let Ok(path) = std::env::var("REVIEW_STATUS_OUT") else {
        return;
    };
    let mut status = sample();
    status.shown.upload = Some(UploadShown {
        checked: CheckedShown::RowV1,
        ..sample_upload()
    });
    let bytes = encode(&status);
    assert_eq!(&bytes[568..576], &6u64.to_le_bytes());
    std::fs::write(path, &bytes).expect("written");
}
