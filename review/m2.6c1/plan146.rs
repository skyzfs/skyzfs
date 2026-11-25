// Scratch test of the look at #146 (cd3a67c), appended to
// `plan/tests.rs` in a scratch worktree: the "ended" arm of `onward`,
// every answer of the protocol (and none) against every state the arm
// reads, with what `report` says of the action, printed as tab-separated
// rows for `table146.py`. It asserts nothing. The fourth look's
// `fourth144plan.rs` (review/m2.6b2) unchanged but for its name, so that
// the rows at cf53e94 and at cd3a67c compare cell by cell; a publish not
// too old ("young") beside it is the control.

#[test]
fn review5_the_ended_arm_in_full() {
    let mut answers: Vec<(String, Option<Reply>)> = vec![
        ("(none: away)".into(), None),
        ("pending".into(), Some(Reply::Pending)),
        ("done".into(), Some(Reply::Done(COMMIT))),
        ("expired".into(), Some(Reply::Expired)),
        ("withdrawn".into(), Some(Reply::Withdrawn)),
        ("unknown".into(), Some(Reply::Unknown)),
        ("absent".into(), Some(Reply::Absent)),
        ("accepted".into(), Some(Reply::Accepted)),
        ("cuts".into(), Some(Reply::Cuts(cuts()))),
        ("snap".into(), Some(Reply::Snap(row(G)))),
        (
            "unreadable".into(),
            Some(Reply::Unreadable(UnreadableReply {
                commit: COMMIT,
                why: skyzfs_control::Unread::NoRow,
            })),
        ),
    ];
    for code in [
        Code::Name,
        Code::Attachment,
        Code::State,
        Code::Manifest,
        Code::Expiry,
        Code::Asked,
        Code::Busy,
        Code::Taken,
        Code::Stopped,
        Code::Rewound,
        Code::Version,
        Code::Malformed,
        Code::Peer,
        Code::NoUpload,
        Code::Ended,
        Code::NoAnswer,
        Code::Store,
    ] {
        answers.push((
            format!("refused {}", code.word()),
            Some(Reply::Refused(code, String::new())),
        ));
    }
    let snaps: [(&str, Option<SnapSeen>); 6] = [
        ("-", None),
        ("absent", Some(SnapSeen::Absent)),
        ("ours", Some(SnapSeen::Ours(COMMIT))),
        ("ours-other-commit", Some(SnapSeen::Ours([9; 32]))),
        ("not-ours", Some(SnapSeen::NotOurs)),
        ("unreadable", Some(SnapSeen::Unreadable)),
    ];
    println!("TABLE\tended\tphase\ttime\tpool\twithdrawn\tsnap\tanswer\taction\treport");
    for ended in ["too-old", "stalls-8", "young"] {
        for (phase_name, phase) in [("held", held()), ("asked", asked())] {
            for (time, now) in [("before-until", UNTIL - 1), ("at-until", UNTIL)] {
                for pool in ["whole-held", "whole-unheld", "gone"] {
                    for flag in [false, true] {
                        for (snap_name, snap) in snaps {
                            for (answer_name, answer) in &answers {
                                let mut publish = publish(phase.clone());
                                let mut s = seen().with(|s| s.now_ms = now);
                                match ended {
                                    "too-old" => s.too_old = true,
                                    "stalls-8" => publish.stalls = STALLS_ABANDONED,
                                    _ => {}
                                }
                                s = match pool {
                                    "whole-unheld" => s.unheld(),
                                    "gone" => s.gone(),
                                    _ => s,
                                };
                                s.withdrawn = flag;
                                s.snap = snap;
                                s = match answer {
                                    None => s.away(),
                                    Some(a) => s.answer(a.clone()),
                                };
                                let got = plan(&publish, &s);
                                let told = report(&publish, &s, &got);
                                println!(
                                    "TABLE\t{ended}\t{phase_name}\t{time}\t{pool}\t{flag}\t{snap_name}\t{answer_name}\t{got:?}\t{told:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
