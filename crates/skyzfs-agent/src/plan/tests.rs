//! The planner's table: each row a phase, what is seen, and the one
//! action. A row names the defect it is there to notice where that is
//! not plain from the row.

use std::time::Duration;

use skyzfs_control::{Code, CutsReply, Reply, SnapReply, UnreadableReply, EXPIRY_DEFAULT};

use super::*;
use crate::record::{Count, Member, Phase, Publish, Taken, Why};

const LOAD: u64 = 0x10ad;
const ATTACHMENT: [u8; 16] = [0xa1; 16];
const STATE: [u8; 16] = [0x5e; 16];
/// The count the request was asked with, C.
const C: u64 = 3;
/// The snapshot's group, G.
const G: u64 = 40;
const ASKED: u64 = 1_764_032_066_000;
const UNTIL: u64 = ASKED + 640_000;
const COMMIT: [u8; 32] = [0xc0; 32];
const HASH: [u8; 32] = [0x4d; 32];
const SECOND: u64 = 1_000;

fn members() -> Vec<Member> {
    vec![
        Member {
            name: "tank".into(),
            snapshot_guid: 0x1111,
        },
        Member {
            name: "tank/db".into(),
            snapshot_guid: 0x2222,
        },
        Member {
            name: "tank/vol".into(),
            snapshot_guid: 0x3333,
        },
    ]
}

fn taken() -> Taken {
    Taken {
        group: G,
        members: members(),
        manifest_len: 300,
        manifest_hash: HASH,
    }
}

fn publish(phase: Phase) -> Publish {
    Publish {
        name: "skz-20261125T010000Z".into(),
        count: Count {
            attachment: ATTACHMENT,
            state: STATE,
            after: C,
            load_guid: LOAD,
        },
        held: !matches!(phase, Phase::Intent),
        phase,
        attempts: 0,
    }
}

fn intent() -> Phase {
    Phase::Intent
}

fn taken_phase() -> Phase {
    Phase::Taken(taken())
}

fn held() -> Phase {
    Phase::Held(taken())
}

fn asked() -> Phase {
    Phase::Asked {
        taken: taken(),
        asked_ms: ASKED,
        held_until_ms: UNTIL,
    }
}

fn done() -> Phase {
    Phase::Done {
        taken: taken(),
        commit: COMMIT,
    }
}

/// No cut has carried the request: the newest is the one counted at C.
fn cuts() -> CutsReply {
    CutsReply {
        attachment: ATTACHMENT,
        state: Some(STATE),
        count: C,
        newest: Some((G - 1, true)),
    }
}

/// The first cut that carries the request.
fn carrying() -> CutsReply {
    CutsReply {
        count: C + 1,
        newest: Some((G, true)),
        ..cuts()
    }
}

/// All well: the snapshot whole and held, `load_guid` as read, the
/// daemon here with the publish pending, a second after `accepted`.
fn seen() -> Seen {
    Seen {
        pool: PoolSeen {
            load_guid: LOAD,
            premise: true,
            snapshot: Some(members()),
            held: true,
        },
        daemon: DaemonSeen::Here {
            cuts: cuts(),
            answer: Reply::Pending,
        },
        snap: None,
        now_ms: ASKED + SECOND,
        assisted_ms: None,
    }
}

/// Changes to [`seen`], one a call.
trait With: Sized {
    fn with(self, change: impl FnOnce(&mut Seen)) -> Seen;

    fn away(self) -> Seen {
        self.with(|s| s.daemon = DaemonSeen::Away)
    }
    fn answer(self, answer: Reply) -> Seen {
        self.with(|s| {
            if let DaemonSeen::Here { answer: a, .. } = &mut s.daemon {
                *a = answer;
            }
        })
    }
    fn refused(self, code: Code) -> Seen {
        self.answer(Reply::Refused(code, String::new()))
    }
    fn cuts(self, cuts: CutsReply) -> Seen {
        self.with(|s| {
            if let DaemonSeen::Here { cuts: c, .. } = &mut s.daemon {
                *c = cuts;
            }
        })
    }
    fn unheld(self) -> Seen {
        self.with(|s| s.pool.held = false)
    }
    fn gone(self) -> Seen {
        self.with(|s| {
            s.pool.snapshot = None;
            s.pool.held = false;
        })
    }
    fn members(self, members: Vec<Member>) -> Seen {
        self.with(|s| s.pool.snapshot = Some(members))
    }
    fn no_premise(self) -> Seen {
        self.with(|s| s.pool.premise = false)
    }
    fn moved(self) -> Seen {
        self.with(|s| s.pool.load_guid = LOAD + 1)
    }
    fn looked(self, snap: SnapSeen) -> Seen {
        self.with(|s| s.snap = Some(snap))
    }
    fn at(self, now_ms: u64) -> Seen {
        self.with(|s| s.now_ms = now_ms)
    }
    fn assisted(self, at_ms: u64) -> Seen {
        self.with(|s| s.assisted_ms = Some(at_ms))
    }
}

impl With for Seen {
    fn with(mut self, change: impl FnOnce(&mut Seen)) -> Seen {
        change(&mut self);
        self
    }
}

/// The members with `tank/db`'s `@t` another snapshot.
fn replaced() -> Vec<Member> {
    let mut members = members();
    members[1].snapshot_guid = 0x9999;
    members
}

fn check(rows: Vec<(&str, Phase, Seen, Action)>) {
    let mut wrong = Vec::new();
    for (what, phase, seen, want) in rows {
        let got = plan(&publish(phase), &seen);
        if got != want {
            wrong.push(format!("{what}: {got:?}, not {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Mutant: a snapshot found at the intent taken for one to publish, or a
/// program run while the daemon is away.
#[test]
fn at_the_intent_the_snapshot_is_taken_or_given_up() {
    let none = || seen().with(|s| s.pool.snapshot = None).unheld();
    check(vec![
        ("away", intent(), none().away(), Action::Wait),
        ("here", intent(), none(), Action::Take),
        (
            "a snapshot with no manifest",
            intent(),
            seen().unheld(),
            Action::Abandon(Why::ManifestLost),
        ),
        ("the same, held", intent(), seen(), Action::Release),
        (
            "the same, the daemon away",
            intent(),
            seen().unheld().away(),
            Action::Abandon(Why::ManifestLost),
        ),
    ]);
}

/// Mutant: the members compared in their order, a publish held without
/// its members verified or with `load_guid` moved (rule (L)), or a hold
/// left on a snapshot given up.
#[test]
fn taken_it_is_held_once_its_members_and_load_guid_are_verified() {
    let mut reordered = members();
    reordered.reverse();
    let mut missing = members();
    missing.pop();
    let mut more = members();
    more.push(Member {
        name: "tank/new".into(),
        snapshot_guid: 0x4444,
    });
    check(vec![
        ("not held", taken_phase(), seen().unheld(), Action::Hold),
        (
            "not held, the daemon away",
            taken_phase(),
            seen().unheld().away(),
            Action::Hold,
        ),
        ("held", taken_phase(), seen(), Action::Held),
        (
            "held, the daemon away",
            taken_phase(),
            seen().away(),
            Action::Held,
        ),
        (
            "listed in another order",
            taken_phase(),
            seen().members(reordered),
            Action::Held,
        ),
        (
            "load_guid moved",
            taken_phase(),
            seen().moved(),
            Action::AskAnew,
        ),
        (
            "load_guid moved, the daemon away",
            taken_phase(),
            seen().moved().away(),
            Action::Wait,
        ),
        (
            "gone",
            taken_phase(),
            seen().gone(),
            Action::Abandon(Why::Destroyed),
        ),
        (
            "a member's @t replaced",
            taken_phase(),
            seen().members(replaced()).unheld(),
            Action::Abandon(Why::Replaced),
        ),
        (
            "a member's @t replaced, held",
            taken_phase(),
            seen().members(replaced()),
            Action::Release,
        ),
        (
            "a member missing",
            taken_phase(),
            seen().members(missing).unheld(),
            Action::Abandon(Why::Replaced),
        ),
        (
            "a member more",
            taken_phase(),
            seen().members(more).unheld(),
            Action::Abandon(Why::Replaced),
        ),
        (
            "the premise refused",
            taken_phase(),
            seen().no_premise().unheld(),
            Action::Abandon(Why::Premise),
        ),
        (
            "the premise refused, held",
            taken_phase(),
            seen().no_premise(),
            Action::Release,
        ),
    ]);
}

/// Mutant: a publish held asked with a count it cannot trust (rule (L)),
/// or a request pending left on a snapshot that is not held.
#[test]
fn held_it_is_asked_with_the_count_read_before_the_snapshot() {
    check(vec![
        ("pending", held(), seen(), Action::Ask),
        (
            "unknown",
            held(),
            seen().answer(Reply::Unknown),
            Action::Ask,
        ),
        (
            "unknown, load_guid moved",
            held(),
            seen().answer(Reply::Unknown).moved(),
            Action::AskAnew,
        ),
        (
            "unknown, not held",
            held(),
            seen().answer(Reply::Unknown).unheld(),
            Action::Hold,
        ),
        (
            "unknown, gone",
            held(),
            seen().answer(Reply::Unknown).gone(),
            Action::Abandon(Why::Destroyed),
        ),
        (
            "pending, load_guid moved",
            held(),
            seen().moved(),
            Action::Withdraw,
        ),
        (
            "pending, not held",
            held(),
            seen().unheld(),
            Action::Withdraw,
        ),
        (
            "pending, a member replaced",
            held(),
            seen().members(replaced()),
            Action::Withdraw,
        ),
        (
            "pending, the premise refused",
            held(),
            seen().no_premise(),
            Action::Withdraw,
        ),
        ("away", held(), seen().away(), Action::Wait),
        (
            "away, long past any expiry: the hold stays",
            held(),
            seen().away().at(UNTIL + 3_600 * SECOND),
            Action::Wait,
        ),
    ]);
}

/// Mutant: a hold let go before `held_until`, kept past it, or let go
/// while the daemon answers; a daemon that stopped not read as away.
#[test]
fn asked_with_the_daemon_away_the_hold_is_kept_until_held_until() {
    let mut rows = vec![
        ("away", asked(), seen().away(), Action::Wait),
        (
            "away, a ms before",
            asked(),
            seen().away().at(UNTIL - 1),
            Action::Wait,
        ),
        (
            "away, at held_until",
            asked(),
            seen().away().at(UNTIL),
            Action::Release,
        ),
        (
            "away, after it, released",
            asked(),
            seen().away().at(UNTIL + 1).unheld(),
            Action::Wait,
        ),
        (
            "away, gone",
            asked(),
            seen().away().at(UNTIL + 1).gone(),
            Action::Wait,
        ),
        (
            "here, past held_until: a publish pending keeps its hold",
            asked(),
            seen().at(UNTIL + 1).cuts(carrying()),
            Action::Wait,
        ),
    ];
    for code in [Code::Stopped, Code::Ended, Code::NoAnswer, Code::NoUpload] {
        rows.push((
            "refused as away",
            asked(),
            seen().refused(code),
            Action::Wait,
        ));
        rows.push((
            "refused as away, at held_until",
            asked(),
            seen().refused(code).at(UNTIL),
            Action::Release,
        ));
        rows.push((
            "refused as away, held",
            held(),
            seen().refused(code).at(UNTIL),
            Action::Wait,
        ));
    }
    check(rows);
}

/// Mutant: `zpool sync` run where a cut already carries the request,
/// before [`ASSIST_AFTER`], more often than [`ASSIST_PAUSE`], or where a
/// part of the cut's test is left out.
#[test]
fn pending_a_sync_helps_only_where_no_cut_has_carried_the_request() {
    let due = ASKED + 10 * SECOND;
    let pending = || seen().at(due);
    let mut rows = vec![
        ("a second after", asked(), seen(), Action::Wait),
        (
            "a ms before due",
            asked(),
            pending().at(due - 1),
            Action::Wait,
        ),
        ("due", asked(), pending(), Action::Assist),
        (
            "due, a cut carries it",
            asked(),
            pending().cuts(carrying()),
            Action::Wait,
        ),
        (
            "due, a cut of a later group carries it",
            asked(),
            pending().cuts(CutsReply {
                count: C + 5,
                newest: Some((G + 9, true)),
                ..cuts()
            }),
            Action::Wait,
        ),
        (
            "assisted a ms less than a pause ago",
            asked(),
            pending().assisted(due - ASSIST_PAUSE.as_millis() as u64 + 1),
            Action::Wait,
        ),
        (
            "assisted a pause ago",
            asked(),
            pending().assisted(due - ASSIST_PAUSE.as_millis() as u64),
            Action::Assist,
        ),
    ];
    let not_carrying = [
        (
            "another attachment",
            CutsReply {
                attachment: [0xa2; 16],
                ..carrying()
            },
        ),
        (
            "another cut state",
            CutsReply {
                state: Some([0x5f; 16]),
                ..carrying()
            },
        ),
        (
            "no cut state",
            CutsReply {
                state: None,
                ..carrying()
            },
        ),
        (
            "counted at C",
            CutsReply {
                count: C,
                ..carrying()
            },
        ),
        (
            "of a group before the snapshot's",
            CutsReply {
                newest: Some((G - 1, true)),
                ..carrying()
            },
        ),
        (
            "not opening as itself",
            CutsReply {
                newest: Some((G, false)),
                ..carrying()
            },
        ),
        (
            "no newest cut",
            CutsReply {
                newest: None,
                ..carrying()
            },
        ),
    ];
    for (what, cuts) in not_carrying {
        rows.push((what, asked(), pending().cuts(cuts), Action::Assist));
    }
    for (what, seen) in [
        ("not held", pending().unheld()),
        ("load_guid moved", pending().moved()),
        ("a member replaced", pending().members(replaced())),
        ("gone", pending().gone()),
        ("the premise refused", pending().no_premise()),
    ] {
        rows.push((what, asked(), seen, Action::Withdraw));
    }
    check(rows);
}

/// Mutant: `done` recorded without `look`, or with a `snap/t` that is
/// not this publish's, or not at the commit `done` named.
#[test]
fn done_is_recorded_only_as_look_sees_it() {
    let done_seen = || seen().answer(Reply::Done(COMMIT));
    check(vec![
        ("not looked", asked(), done_seen(), Action::Look),
        (
            "looked: ours",
            asked(),
            done_seen().looked(SnapSeen::Ours(COMMIT)),
            Action::Done(COMMIT),
        ),
        (
            "held, looked: ours",
            held(),
            done_seen().looked(SnapSeen::Ours(COMMIT)),
            Action::Done(COMMIT),
        ),
        (
            "looked: ours at another commit",
            asked(),
            done_seen().looked(SnapSeen::Ours([0xc1; 32])),
            Action::Stop(Stop::DoneNotSeen),
        ),
        (
            "looked: absent",
            asked(),
            done_seen().looked(SnapSeen::Absent),
            Action::Stop(Stop::DoneNotSeen),
        ),
        (
            "looked: another's",
            asked(),
            done_seen().looked(SnapSeen::NotOurs),
            Action::Stop(Stop::DoneNotSeen),
        ),
        (
            "looked: unreadable",
            asked(),
            done_seen().looked(SnapSeen::Unreadable),
            Action::Stop(Stop::DoneNotSeen),
        ),
        (
            "ours, released",
            asked(),
            done_seen().looked(SnapSeen::Ours(COMMIT)).unheld(),
            Action::Done(COMMIT),
        ),
    ]);
}

/// Mutant: an end that may hide a publish made asked anew without a
/// look, a name another holds published over, or a snapshot not
/// whole asked again.
#[test]
fn an_end_that_may_hide_a_publish_is_looked_up_first() {
    let mut rows = Vec::new();
    let ends: Vec<(&str, Reply)> = vec![
        ("expired", Reply::Expired),
        ("withdrawn", Reply::Withdrawn),
        (
            "refused attachment",
            Reply::Refused(Code::Attachment, String::new()),
        ),
        ("refused state", Reply::Refused(Code::State, String::new())),
        ("refused taken", Reply::Refused(Code::Taken, String::new())),
    ];
    for (what, end) in ends {
        let ended = || seen().answer(end.clone());
        for phase in [held(), asked()] {
            rows.push((what, phase.clone(), ended(), Action::Look));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Ours(COMMIT)),
                Action::Done(COMMIT),
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::NotOurs),
                Action::Release,
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::NotOurs).unheld(),
                Action::Abandon(Why::NameTaken),
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Unreadable).unheld(),
                Action::Abandon(Why::NameTaken),
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Absent),
                Action::AskAnew,
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Absent).moved(),
                Action::AskAnew,
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Absent).unheld(),
                Action::Hold,
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Absent).gone(),
                Action::Abandon(Why::Destroyed),
            ));
            rows.push((
                what,
                phase,
                ended().looked(SnapSeen::Absent).members(replaced()),
                Action::Release,
            ));
        }
    }
    check(rows);
}

/// Mutant: a request asked again with the count it was refused for, a
/// rewind's loss reported as a destroy, or `unknown` of a publish asked
/// taken to mean nothing was published and asked with the old count.
#[test]
fn rewound_and_unknown_are_asked_anew_without_a_look() {
    let rewound = || seen().refused(Code::Rewound);
    check(vec![
        ("rewound", asked(), rewound(), Action::AskAnew),
        ("rewound, held phase", held(), rewound(), Action::AskAnew),
        (
            "rewound, not held",
            asked(),
            rewound().unheld(),
            Action::Hold,
        ),
        (
            "rewound, gone",
            asked(),
            rewound().gone(),
            Action::Abandon(Why::Rewound),
        ),
        (
            "rewound, a member replaced",
            asked(),
            rewound().members(replaced()).unheld(),
            Action::Abandon(Why::Replaced),
        ),
        (
            "rewound, the premise refused",
            asked(),
            rewound().no_premise(),
            Action::Release,
        ),
        (
            "unknown, asked",
            asked(),
            seen().answer(Reply::Unknown),
            Action::AskAnew,
        ),
        (
            "unknown, asked, not held",
            asked(),
            seen().answer(Reply::Unknown).unheld(),
            Action::Hold,
        ),
        (
            "unknown, asked, gone",
            asked(),
            seen().answer(Reply::Unknown).gone(),
            Action::Abandon(Why::Destroyed),
        ),
    ]);
}

/// Mutant: a refusal that says the agent asked wrongly retried, a busy
/// daemon given up on, or another request of the name left pending.
#[test]
fn the_other_refusals_wait_withdraw_or_stop() {
    let mut rows = Vec::new();
    for phase in [held(), asked()] {
        rows.push((
            "busy",
            phase.clone(),
            seen().refused(Code::Busy),
            Action::Wait,
        ));
        rows.push((
            "asked",
            phase.clone(),
            seen().refused(Code::Asked),
            Action::Withdraw,
        ));
        for code in [
            Code::Name,
            Code::Manifest,
            Code::Expiry,
            Code::Version,
            Code::Malformed,
            Code::Peer,
            Code::Store,
        ] {
            rows.push((
                "stops",
                phase.clone(),
                seen().refused(code),
                Action::Stop(Stop::Refused(code)),
            ));
        }
        // Answers that are not of a publish: a daemon this agent does not
        // understand, seen again later.
        for answer in [
            Reply::Cuts(cuts()),
            Reply::Accepted,
            Reply::Absent,
            Reply::Snap(row(G)),
            Reply::Unreadable(UnreadableReply {
                commit: COMMIT,
                why: skyzfs_control::Unread::NoRow,
            }),
        ] {
            rows.push((
                "not of a publish",
                phase.clone(),
                seen().answer(answer),
                Action::Wait,
            ));
        }
    }
    check(rows);
}

/// Mutant: a hold left on a publish done, or the record kept.
#[test]
fn done_it_is_released_then_forgotten() {
    check(vec![
        ("held", done(), seen(), Action::Release),
        (
            "held, the daemon away",
            done(),
            seen().away(),
            Action::Release,
        ),
        ("released", done(), seen().unheld(), Action::Forget),
        ("gone", done(), seen().gone(), Action::Forget),
    ]);
}

/// `look`'s answer for the publish taken, as a row of group `group`.
fn row(group: u64) -> SnapReply {
    SnapReply {
        commit: COMMIT,
        state: STATE,
        number: C + 1,
        group,
        manifest_len: 300,
        manifest_hash: HASH,
    }
}

/// An earlier request landed: the publish was asked with C, refused
/// `rewound`, and asked anew with a count read after the rewind (here 9,
/// in another cut state). The first request was the one the daemon
/// carried, by a cut it counted after C under its own rule (R). The
/// request asked anew ends `refused taken`, and `look` shows a row with
/// the first count's cut (number C + 1, the first state) and this
/// manifest. That row is this publish's: it is recorded done, at the
/// commit `look` sees, whatever count the record now holds (the lead's
/// decision on #142's reading 1).
///
/// Mutant: the row's number or state compared with the count recorded
/// last (the row is then another's, and the name given up).
#[test]
fn an_earlier_request_that_landed_is_this_publishs() {
    let landed = || {
        let snap = snap_seen(&Reply::Snap(row(G)), &taken()).expect("a look's answer");
        seen()
            .answer(Reply::Refused(Code::Taken, String::new()))
            .looked(snap)
    };
    let mut anew = publish(asked());
    anew.count.after = 9;
    anew.count.state = [0x5f; 16];
    assert_eq!(plan(&anew, &landed()), Action::Done(COMMIT));
    assert_eq!(plan(&anew, &landed().unheld()), Action::Done(COMMIT));
    let mut first = publish(asked());
    first.count.after = C;
    assert_eq!(plan(&first, &landed()), Action::Done(COMMIT));
}

/// Mutant: a `snap/t` of another manifest, or of a cut before the
/// snapshot's group, read as ours; or one asked with another count (an
/// earlier request, or of another cut state) not read as ours.
#[test]
fn snap_t_is_ours_where_it_records_this_manifest_and_a_group_not_before_g() {
    let t = taken();
    let seen = |reply: Reply| snap_seen(&reply, &t);
    assert_eq!(seen(Reply::Absent), Some(SnapSeen::Absent));
    assert_eq!(seen(Reply::Snap(row(G))), Some(SnapSeen::Ours(COMMIT)));
    assert_eq!(seen(Reply::Snap(row(G + 7))), Some(SnapSeen::Ours(COMMIT)));
    assert_eq!(seen(Reply::Snap(row(G - 1))), Some(SnapSeen::NotOurs));
    let other = |change: &dyn Fn(&mut SnapReply)| {
        let mut row = row(G);
        change(&mut row);
        seen(Reply::Snap(row))
    };
    assert_eq!(other(&|r| r.manifest_len += 1), Some(SnapSeen::NotOurs));
    assert_eq!(other(&|r| r.manifest_hash[0] ^= 1), Some(SnapSeen::NotOurs));
    // Not compared (this module's reading): the cut's state and number.
    assert_eq!(
        other(&|r| r.state = [0x5f; 16]),
        Some(SnapSeen::Ours(COMMIT))
    );
    assert_eq!(other(&|r| r.number = 1), Some(SnapSeen::Ours(COMMIT)));
    assert_eq!(
        seen(Reply::Unreadable(UnreadableReply {
            commit: COMMIT,
            why: skyzfs_control::Unread::OtherCommit,
        })),
        Some(SnapSeen::Unreadable)
    );
    for reply in [
        Reply::Refused(Code::Store, String::new()),
        Reply::Refused(Code::NoAnswer, String::new()),
        Reply::Pending,
    ] {
        assert_eq!(seen(reply), None);
    }
}

/// Mutant: the margin or the expiry left out, or an overflow.
#[test]
fn held_until_is_the_acceptance_plus_the_expiry_plus_the_margin() {
    assert_eq!(HOLD_MARGIN, Duration::from_secs(40));
    assert_eq!(held_until(ASKED, EXPIRY_DEFAULT), ASKED + 600_000 + 40_000);
    assert_eq!(held_until(5, Duration::from_millis(7)), 5 + 7 + 40_000);
    assert_eq!(held_until(u64::MAX - 1, EXPIRY_DEFAULT), u64::MAX);
    assert_eq!(held_until(0, Duration::MAX), u64::MAX);
    assert_eq!(ASSIST_AFTER, Duration::from_secs(10));
}
