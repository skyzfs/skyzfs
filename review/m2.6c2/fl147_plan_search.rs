
// ---- Fourth look at #147 (scratch, not part of the pull request) ----
// The planner, exhaustively: phase x stalls x snapshot x premise x
// load_guid x daemon (away, or cuts x every answer) x snap seen x time x
// assisted x too_old x withdrawn, each planned with the hold No, Part and
// All. The properties:
//   P1: plan(Part) == Release where plan(All) == Release, else plan(No);
//   P2: plan(Part) is never Held, Ask, AskAnew or Assist;
//   P3: plan(Part) == Release wherever plan(No) is Abandon or Forget
//       (but at the intent with no `pool@t`, where the hold is not read);
//   P4: report(Part) == report(All) where both release, and
//       report(Part) == report(No) elsewhere.
#[test]
fn fl147_the_planner_on_a_part_exhaustively() {
    let codes = [
        Code::Rewound, Code::Stopped, Code::State, Code::Attachment, Code::Taken,
        Code::Name, Code::Manifest, Code::Expiry, Code::Version, Code::Malformed,
        Code::Peer, Code::Store, Code::Asked, Code::Busy, Code::Ended,
        Code::NoAnswer, Code::NoUpload,
    ];
    let mut answers = vec![
        Reply::Pending,
        Reply::Done(COMMIT),
        Reply::Done([0xdd; 32]),
        Reply::Expired,
        Reply::Withdrawn,
        Reply::Unknown,
        Reply::Absent,
        Reply::Accepted,
        Reply::Cuts(cuts()),
        Reply::Snap(row(G)),
        Reply::Unreadable(UnreadableReply {
            commit: COMMIT,
            why: skyzfs_control::Unread::NoRow,
        }),
    ];
    for code in codes {
        answers.push(Reply::Refused(code, String::new()));
    }
    let other_cuts = CutsReply {
        attachment: [0xb2; 16],
        ..cuts()
    };
    let mut daemons = vec![DaemonSeen::Away];
    for c in [cuts(), carrying(), other_cuts] {
        for a in &answers {
            daemons.push(DaemonSeen::Here {
                cuts: c,
                answer: a.clone(),
            });
        }
    }
    let snaps = [
        None,
        Some(SnapSeen::Absent),
        Some(SnapSeen::Ours(COMMIT)),
        Some(SnapSeen::Ours([0xdd; 32])),
        Some(SnapSeen::NotOurs),
        Some(SnapSeen::Unreadable),
    ];
    let snapshots = [Some(members()), Some(replaced()), None];
    let times = [ASKED + SECOND, ASKED + 25 * SECOND, UNTIL - 1, UNTIL, UNTIL + 1];
    let phases = [intent(), taken_phase(), held(), asked(), done()];
    let mut cases = 0u64;
    let mut differ = 0u64;
    let mut releases_part = 0u64;
    let mut bad: Vec<String> = Vec::new();
    for phase in &phases {
        for stalls in [0, STALLS_ABANDONED - 1, STALLS_ABANDONED] {
            let mut p = publish(phase.clone());
            p.stalls = stalls;
            for snapshot in &snapshots {
                for premise in [true, false] {
                    for load in [LOAD, LOAD + 1] {
                        for daemon in &daemons {
                            for snap in &snaps {
                                for now in times {
                                    for assisted in [None, Some(ASKED + 15 * SECOND)] {
                                        for too_old in [false, true] {
                                            for withdrawn in [false, true] {
                                                let base = Seen {
                                                    pool: PoolSeen {
                                                        load_guid: load,
                                                        premise,
                                                        snapshot: snapshot.clone(),
                                                        hold: Held::No,
                                                    },
                                                    daemon: daemon.clone(),
                                                    snap: *snap,
                                                    now_ms: now,
                                                    assisted_ms: assisted,
                                                    assists: 0,
                                                    too_old,
                                                    withdrawn,
                                                };
                                                let with = |h: Held| {
                                                    let mut s = base.clone();
                                                    s.pool.hold = h;
                                                    s
                                                };
                                                let (sn, sp, sa) =
                                                    (with(Held::No), with(Held::Part), with(Held::All));
                                                let no = plan(&p, &sn);
                                                let part = plan(&p, &sp);
                                                let all = plan(&p, &sa);
                                                cases += 1;
                                                if part != all {
                                                    differ += 1;
                                                }
                                                if part == Action::Release {
                                                    releases_part += 1;
                                                }
                                                let want = if all == Action::Release {
                                                    Action::Release
                                                } else {
                                                    no.clone()
                                                };
                                                let mut why = Vec::new();
                                                if part != want {
                                                    why.push(format!("P1 want {want:?}"));
                                                }
                                                if matches!(
                                                    part,
                                                    Action::Held
                                                        | Action::Ask
                                                        | Action::AskAnew
                                                        | Action::Assist
                                                ) {
                                                    why.push("P2 goes on".to_string());
                                                }
                                                // At the intent with no `pool@t` the hold is not
                                                // read: none can be the agent's there.
                                                let intent_none = matches!(phase, Phase::Intent)
                                                    && snapshot.is_none();
                                                if matches!(no, Action::Abandon(_) | Action::Forget)
                                                    && part != Action::Release
                                                    && !intent_none
                                                {
                                                    why.push("P3 no release".to_string());
                                                }
                                                let rp = report(&p, &sp, &part);
                                                let rw = if all == Action::Release {
                                                    report(&p, &sa, &all)
                                                } else {
                                                    report(&p, &sn, &no)
                                                };
                                                if rp != rw {
                                                    why.push(format!("P4 report {rp:?} not {rw:?}"));
                                                }
                                                if !why.is_empty() && bad.len() < 40 {
                                                    bad.push(format!(
                                                        "{phase:?} stalls {stalls} snap {snapshot:?} premise {premise} load {load} {daemon:?} {snap:?} now {now} too_old {too_old} withdrawn {withdrawn}: no {no:?} part {part:?} all {all:?}: {}",
                                                        why.join("; ")
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "FL147 planner search: {cases} cases x 3 holds; Part differs from All in {differ}; Part releases in {releases_part}; violations {}",
        bad.len()
    );
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
