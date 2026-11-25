// Scratch rows of the next look at #144 (M2.6b2) at 45648ab. Appended to
// the end of `crates/skyzfs-agent/src/plan/tests.rs` in a scratch
// worktree only. Each row is what the module docs ("Too old" and "A
// daemon away") and the lead's reading say; the test prints what the
// planner plans and asserts that it differs, so that it passes at the
// head and shows the behaviour as built.

/// N1. Too old, with the daemon here and the request's answer one that
/// the module docs call "a daemon away" (`stopped`, `ended`,
/// `no-answer`, `no-upload`), or `refused asked` (another request of
/// the name pends): the docs keep the hold of a publish asked until
/// `held_until`, and of one held and not asked; `refused asked` is
/// withdrawn. The planner looks, and on `absent` releases the hold.
#[test]
fn review_n1_too_old_with_an_answer_of_a_daemon_away() {
    let old = || seen().too_old().looked(SnapSeen::Absent);
    let rows: Vec<(&str, Phase, Seen, Action)> = vec![
        (
            "asked, no-answer, before held_until",
            asked(),
            old().refused(Code::NoAnswer),
            Action::Wait,
        ),
        (
            "asked, stopped, before held_until",
            asked(),
            old().refused(Code::Stopped),
            Action::Wait,
        ),
        (
            "asked, ended, before held_until",
            asked(),
            old().refused(Code::Ended),
            Action::Wait,
        ),
        (
            "asked, no-upload, before held_until",
            asked(),
            old().refused(Code::NoUpload),
            Action::Wait,
        ),
        (
            "held, no-answer (to a publish: 'a publish may have been taken')",
            held(),
            old().refused(Code::NoAnswer),
            Action::Wait,
        ),
        (
            "asked, refused asked (another request of the name pends)",
            asked(),
            old().refused(Code::Asked),
            Action::Withdraw,
        ),
        (
            "asked, no-answer, not yet looked",
            asked(),
            seen().too_old().refused(Code::NoAnswer),
            Action::Wait,
        ),
    ];
    let mut differ = Vec::new();
    for (what, phase, seen, docs) in rows {
        let got = plan(&publish(phase), &seen);
        eprintln!("{what}: planned {got:?}; the docs: {docs:?}");
        if got != docs {
            differ.push(what);
        }
    }
    assert_eq!(differ.len(), 7, "{differ:?}");
    // Not too old, the same answers keep the hold (the rows as built).
    assert_eq!(
        plan(
            &publish(asked()),
            &seen().refused(Code::NoAnswer).looked(SnapSeen::Absent)
        ),
        Action::Wait
    );
    assert_eq!(
        plan(&publish(asked()), &seen().refused(Code::Asked)),
        Action::Withdraw
    );
}

/// N4. Too old, the daemon says `done` and `look` does not see it: the
/// planner gives the publish up as too old, where a publish not too old
/// stops for a person (`Stop::DoneNotSeen`).
#[test]
fn review_n4_too_old_done_not_seen_is_given_up() {
    let done = || seen().answer(Reply::Done(COMMIT)).looked(SnapSeen::Absent);
    assert_eq!(
        plan(&publish(asked()), &done()),
        Action::Stop(Stop::DoneNotSeen)
    );
    assert_eq!(plan(&publish(asked()), &done().too_old()), Action::Release);
}
