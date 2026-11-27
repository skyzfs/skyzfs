
/// Scratch S6 (the review of #175, its next look): a name retired, then
/// published again, is kept in view as the newest are, and not dropped
/// early for an entry the retire left in the order. Mutant: the outcome
/// removed and the name left in `order` (`retired-no-order`).
#[test]
fn review_s6_a_name_published_again_after_its_retire_stays_in_view() {
    let mut publishing = publishing();
    let found = Found {
        commit: [1; 32],
        row: None,
    };
    publishing.ask(request("t", 0, 0));
    let _ = publishing.found("t", &found);
    publishing.retired("t");
    assert_eq!(publishing.progress("t"), Progress::Unknown);
    publishing.ask(request("t", 0, 0));
    let _ = publishing.found("t", &found);
    for i in 0..OUTCOMES_KEPT - 1 {
        let name = format!("u{i}");
        publishing.ask(request(&name, 0, 0));
        let _ = publishing.found(&name, &found);
    }
    assert_eq!(publishing.progress("t"), Progress::Refused(Refusal::Taken));
}
