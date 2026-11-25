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
const BEGUN: u64 = ASKED - 30 * SECOND;

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
        stalls: 0,
        begun_ms: BEGUN,
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
        assists: 0,
        too_old: false,
        withdrawn: false,
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
    fn assists(self, n: u32) -> Seen {
        self.with(|s| s.assists = n)
    }
    fn too_old(self) -> Seen {
        self.with(|s| s.too_old = true)
    }
    fn withdrawn(self) -> Seen {
        self.with(|s| s.withdrawn = true)
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

/// Done at [`COMMIT`], with nothing to report.
fn published() -> Action {
    Action::Done {
        commit: COMMIT,
        report: None,
    }
}

/// Done at [`COMMIT`], reported: the snapshot was not whole when it was
/// seen.
fn published_gone() -> Action {
    Action::Done {
        commit: COMMIT,
        report: Some(Report::SnapshotGone),
    }
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

/// Mutant: a snapshot found at the intent taken for one to publish, a
/// program run while the daemon is away, or a snapshot taken of a pool
/// whose premise is refused (S1 of #142's review).
#[test]
fn at_the_intent_the_snapshot_is_taken_or_given_up() {
    let none = || seen().with(|s| s.pool.snapshot = None).unheld();
    check(vec![
        ("away", intent(), none().away(), Action::Wait),
        ("here", intent(), none(), Action::Take),
        // Nothing is taken: a refused pool gets no `pool@t` to keep.
        (
            "here, the premise refused",
            intent(),
            none().no_premise(),
            Action::Abandon(Why::Premise),
        ),
        (
            "away, the premise refused",
            intent(),
            none().no_premise().away(),
            Action::Wait,
        ),
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

/// Mutant: the members compared in their order or by their names, a
/// publish held without its members verified or with `load_guid` moved
/// (rule (L)), or a hold left on a snapshot given up.
#[test]
fn taken_it_is_held_once_its_members_and_load_guid_are_verified() {
    let mut reordered = members();
    reordered.reverse();
    // `zfs rename tank/db tank/pg` after the snapshot: its `@t` is the
    // same snapshot (S3 of #142's review).
    let mut renamed = members();
    renamed[1].name = "tank/pg".into();
    let mut renamed_and_replaced = replaced();
    renamed_and_replaced[1].name = "tank/pg".into();
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
            "a member renamed",
            taken_phase(),
            seen().members(renamed),
            Action::Held,
        ),
        (
            "a member renamed and its @t replaced",
            taken_phase(),
            seen().members(renamed_and_replaced).unheld(),
            Action::Abandon(Why::Replaced),
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
        // A request may be out at Held (asked anew, or asked before the
        // agent stopped): looked up first (the lead's D1 on #142).
        (
            "unknown, gone",
            held(),
            seen().answer(Reply::Unknown).gone(),
            Action::Look,
        ),
        (
            "unknown, gone, absent",
            held(),
            seen()
                .answer(Reply::Unknown)
                .gone()
                .looked(SnapSeen::Absent),
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
            // Helped a moment ago, so that no sync is due either.
            seen().at(UNTIL + 1).cuts(carrying()).assisted(UNTIL),
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

/// Mutant: `zpool sync` run where a cut already carries the request
/// more often than once per [`ASSIST_AFTER`] or before twice it, never
/// run while one does (an abandoned cut stops the help until the
/// expiry), run before [`ASSIST_AFTER`], more often than
/// [`ASSIST_PAUSE`], or where a part of the cut's test is left out; or
/// more often than once per [`ASSIST_AFTER`] after [`ASSIST_QUICK`]
/// syncs of the request, or less often before (the lead's decision on
/// #144: the fourth sync waits [`ASSIST_AFTER`]).
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
        (
            "the third, assisted a pause ago",
            asked(),
            pending()
                .at(due + 2 * SECOND)
                .assisted(due + SECOND)
                .assists(2),
            Action::Assist,
        ),
        (
            "the fourth, assisted a pause ago",
            asked(),
            pending()
                .at(due + 3 * SECOND)
                .assisted(due + 2 * SECOND)
                .assists(3),
            Action::Wait,
        ),
        (
            "the fourth, assisted a ms less than ASSIST_AFTER ago",
            asked(),
            pending()
                .at(due + 12 * SECOND - 1)
                .assisted(due + 2 * SECOND)
                .assists(3),
            Action::Wait,
        ),
        (
            "the fourth, assisted ASSIST_AFTER ago",
            asked(),
            pending()
                .at(due + 12 * SECOND)
                .assisted(due + 2 * SECOND)
                .assists(3),
            Action::Assist,
        ),
        (
            "the fiftieth, assisted ASSIST_AFTER ago",
            asked(),
            pending()
                .at(due + 500 * SECOND)
                .assisted(due + 490 * SECOND)
                .assists(49),
            Action::Assist,
        ),
        (
            "the fiftieth, assisted a pause ago",
            asked(),
            pending()
                .at(due + 500 * SECOND)
                .assisted(due + 499 * SECOND)
                .assists(49),
            Action::Wait,
        ),
    ];
    // A cut that carries it may have been abandoned, which `cuts` does
    // not say: a sync once ASSIST_AFTER has passed since the last, and
    // from twice ASSIST_AFTER after `accepted` (Codex on #142).
    let twice = ASKED + 20 * SECOND;
    let carried = |at: u64| seen().at(at).cuts(carrying());
    rows.extend([
        (
            "a cut carries it, a ms before twice due",
            asked(),
            carried(twice - 1),
            Action::Wait,
        ),
        (
            "a cut carries it, twice due",
            asked(),
            carried(twice),
            Action::Assist,
        ),
        (
            "a cut carries it, assisted before twice due, at twice due",
            asked(),
            carried(twice).assisted(due),
            Action::Assist,
        ),
        (
            "a cut carries it, assisted a ms less than ASSIST_AFTER ago",
            asked(),
            carried(twice + 5 * SECOND).assisted(twice - 5 * SECOND + 1),
            Action::Wait,
        ),
        (
            "a cut carries it, assisted ASSIST_AFTER ago",
            asked(),
            carried(twice + 5 * SECOND).assisted(twice - 5 * SECOND),
            Action::Assist,
        ),
        (
            // A sync of an earlier request of this publish, before this
            // one was accepted: the start at twice ASSIST_AFTER holds.
            "a cut carries it, assisted before it was accepted",
            asked(),
            carried(ASKED + 15 * SECOND).assisted(ASKED - 9 * SECOND),
            Action::Wait,
        ),
        (
            "a cut carries it, assisted a pause ago",
            asked(),
            carried(twice + 30 * SECOND).assisted(twice + 29 * SECOND),
            Action::Wait,
        ),
        // The clock stepped back an hour after a sync: the sync counts as
        // none, and syncs go on (S11 of #142's review).
        (
            "assisted an hour ahead of the clock",
            asked(),
            pending().assisted(due + 3_600 * SECOND),
            Action::Assist,
        ),
        (
            "a cut carries it, assisted an hour ahead of the clock",
            asked(),
            carried(twice).assisted(twice + 3_600 * SECOND),
            Action::Assist,
        ),
    ]);
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
            published(),
        ),
        (
            "held, looked: ours",
            held(),
            done_seen().looked(SnapSeen::Ours(COMMIT)),
            published(),
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
            published(),
        ),
        // What a rewind that rule (R) missed looks like from the agent:
        // done, reported (S2 of #142's review).
        (
            "ours, pool@t gone, load_guid moved",
            asked(),
            done_seen().looked(SnapSeen::Ours(COMMIT)).gone().moved(),
            published_gone(),
        ),
    ]);
}

/// Mutant: an end that may hide a publish made asked anew without a
/// look, a name another holds published over, a snapshot not whole
/// asked again, this publish's `snap/t` not taken as done where the
/// hold or `pool@t` has gone, or done with `pool@t` or a member's `@t`
/// gone and not reported (D1 and S2 of #142's review).
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
                published(),
            ));
            // This publish's `snap/t` is done whatever became of the hold
            // or of `pool@t` since. With `pool@t` or a member's `@t` gone,
            // the cut may hold the snapshot (made before a rewind) or not
            // (after one, through a gap in rule (R)): reported.
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Ours(COMMIT)).unheld(),
                published(),
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Ours(COMMIT)).gone(),
                published_gone(),
            ));
            rows.push((
                what,
                phase.clone(),
                ended()
                    .looked(SnapSeen::Ours(COMMIT))
                    .members(replaced())
                    .unheld(),
                published_gone(),
            ));
            rows.push((
                what,
                phase.clone(),
                ended().looked(SnapSeen::Ours(COMMIT)).no_premise(),
                published(),
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
/// rewind's loss reported as a destroy, `unknown` of a publish asked
/// taken to mean nothing was published and asked with the old count, or
/// a publish given up from `unknown` or `refused rewound` with no look
/// where the snapshot is broken and `snap/t` may be this publish's (F1
/// of #142's review; the lead's D1).
#[test]
fn rewound_and_unknown_are_asked_anew_and_looked_up_only_where_broken() {
    let rewound = || seen().refused(Code::Rewound);
    let unknown = || seen().answer(Reply::Unknown);
    let mut rows = vec![
        ("rewound", asked(), rewound(), Action::AskAnew),
        ("rewound, held phase", held(), rewound(), Action::AskAnew),
        (
            "rewound, not held",
            asked(),
            rewound().unheld(),
            Action::Hold,
        ),
        ("unknown, asked", asked(), unknown(), Action::AskAnew),
        (
            "unknown, asked, not held",
            asked(),
            unknown().unheld(),
            Action::Hold,
        ),
        (
            "rewound, gone, absent",
            asked(),
            rewound().gone().looked(SnapSeen::Absent),
            Action::Abandon(Why::Rewound),
        ),
        (
            "rewound, a member replaced, absent",
            asked(),
            rewound()
                .members(replaced())
                .unheld()
                .looked(SnapSeen::Absent),
            Action::Abandon(Why::Replaced),
        ),
        (
            "rewound, the premise refused, absent",
            asked(),
            rewound().no_premise().looked(SnapSeen::Absent),
            Action::Release,
        ),
        (
            "unknown, gone, absent",
            asked(),
            unknown().gone().looked(SnapSeen::Absent),
            Action::Abandon(Why::Destroyed),
        ),
        (
            "unknown, held phase, gone, absent",
            held(),
            unknown().gone().looked(SnapSeen::Absent),
            Action::Abandon(Why::Destroyed),
        ),
    ];
    type Breaks = fn(Seen) -> Seen;
    let broken: Vec<(&str, Breaks)> = vec![
        ("gone", |s| s.gone()),
        ("a member replaced", |s| s.members(replaced()).unheld()),
        ("the premise refused", |s| s.no_premise()),
    ];
    for (what, end) in [("rewound", rewound()), ("unknown", unknown())] {
        for (how, broke) in &broken {
            let report = *how != "the premise refused";
            let ended = || broke(end.clone());
            rows.push((what, asked(), ended(), Action::Look));
            rows.push((what, held(), ended(), Action::Look));
            rows.push((
                what,
                asked(),
                ended().looked(SnapSeen::Ours(COMMIT)),
                if report {
                    published_gone()
                } else {
                    published()
                },
            ));
            rows.push((
                what,
                asked(),
                ended().looked(SnapSeen::NotOurs).unheld(),
                Action::Abandon(Why::NameTaken),
            ));
        }
    }
    check(rows);
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
/// Mutant: a stall counted where a cut was counted, where the daemon is
/// another, or for a publish never accepted; the report made at another
/// stall than the third, or more than once; the last stall not recorded
/// before the publish is given up (the agent's own release writes a
/// group, a cut that would undo the decision); a publish given up with a
/// stall short, with a request pending, with no look, or with its hold
/// kept; or the bound applied where the snapshot is broken (its own
/// reason first).
#[test]
fn a_publish_that_makes_no_progress_is_reported_then_given_up() {
    let with = |stalls: u32, phase: Phase| Publish {
        stalls,
        ..publish(phase)
    };
    let count = with(0, asked()).count;
    let cut = Count {
        after: C + 1,
        ..count
    };
    let other = Count {
        attachment: [0xa2; 16],
        ..count
    };
    let reimported = Count {
        load_guid: LOAD + 1,
        ..count
    };
    assert_eq!(stalls(&with(0, asked()), &count), 1);
    assert_eq!(stalls(&with(6, asked()), &count), 7);
    assert_eq!(stalls(&with(6, asked()), &reimported), 7);
    assert_eq!(stalls(&with(6, asked()), &cut), 0);
    assert_eq!(stalls(&with(6, asked()), &other), 0);
    assert_eq!(stalls(&with(6, held()), &count), 6);
    assert_eq!(stalls(&with(6, taken_phase()), &cut), 6);
    let reports: Vec<_> = (0..=STALLS_ABANDONED).map(stalls_report).collect();
    let want: Vec<_> = (0..=STALLS_ABANDONED)
        .map(|n| (n >= 3).then_some(Report::NoProgress(n)))
        .collect();
    assert_eq!(reports, want);

    let mut wrong = Vec::new();
    let expired = || seen().answer(Reply::Expired).looked(SnapSeen::Absent);
    let rows: Vec<(&str, Publish, Seen, Action)> = vec![
        ("a stall", with(0, asked()), expired(), Action::AskAnew),
        ("the third", with(2, asked()), expired(), Action::AskAnew),
        ("the seventh", with(6, asked()), expired(), Action::AskAnew),
        ("the eighth", with(7, asked()), expired(), Action::Stalled),
        (
            "the eighth, released",
            with(7, asked()),
            expired().unheld(),
            Action::Stalled,
        ),
        (
            "the eighth, unknown",
            with(7, asked()),
            seen().answer(Reply::Unknown),
            Action::Stalled,
        ),
        (
            "the eighth, rewound",
            with(7, asked()),
            seen().refused(Code::Rewound),
            Action::Stalled,
        ),
        // The request is withdrawn first, and the publish given up only
        // once the daemon has answered the withdraw (the lead's L5).
        (
            "eight recorded, expired",
            with(8, held()),
            seen().answer(Reply::Expired),
            Action::Withdraw,
        ),
        (
            "eight recorded, withdrawn",
            with(8, held()),
            seen().answer(Reply::Expired).withdrawn(),
            Action::Look,
        ),
        (
            "eight recorded, withdrawn, absent",
            with(8, held()),
            expired().withdrawn(),
            Action::Release,
        ),
        // The release wrote a group, counted as a cut: given up all the
        // same.
        (
            "eight recorded, absent, released, a cut counted",
            with(8, held()),
            expired().withdrawn().unheld().cuts(carrying()),
            Action::Abandon(Why::NoProgress),
        ),
        (
            "eight recorded, unknown, absent, released",
            with(8, held()),
            seen()
                .answer(Reply::Unknown)
                .looked(SnapSeen::Absent)
                .withdrawn()
                .unheld(),
            Action::Abandon(Why::NoProgress),
        ),
        (
            "eight recorded, pending",
            with(8, held()),
            seen(),
            Action::Withdraw,
        ),
        (
            "eight recorded, ours",
            with(8, held()),
            seen()
                .answer(Reply::Withdrawn)
                .looked(SnapSeen::Ours(COMMIT))
                .withdrawn(),
            published(),
        ),
        (
            "eight recorded, absent, gone",
            with(8, held()),
            expired().gone().withdrawn(),
            Action::Abandon(Why::Destroyed),
        ),
        (
            "past the eighth (a record of an older bound)",
            with(30, asked()),
            expired().unheld().withdrawn(),
            Action::Abandon(Why::NoProgress),
        ),
        (
            "seven, and a cut counted",
            with(7, asked()),
            expired().cuts(carrying()),
            Action::AskAnew,
        ),
        (
            "seven, and another daemon",
            with(7, asked()),
            expired().cuts(CutsReply {
                attachment: [0xa2; 16],
                ..cuts()
            }),
            Action::AskAnew,
        ),
        (
            "seven, never accepted",
            with(7, held()),
            expired(),
            Action::AskAnew,
        ),
        (
            "seven, gone",
            with(7, asked()),
            expired().gone(),
            Action::Abandon(Why::Destroyed),
        ),
        ("seven, pending", with(7, asked()), seen(), Action::Wait),
    ];
    for (what, publish, seen, want) in rows {
        let got = plan(&publish, &seen);
        if got != want {
            wrong.push(format!("{what}: {got:?}, not {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// L2 of #144's review: a publish older than its bound is given up, as
/// at its last stall, but never while a request of it may still pend
/// (the lead's L5 and L6, after N1 of the next look).
///
/// Mutant: the bound not applied at some phase, or applied with the
/// hold kept, a request left pending, no look (a publish made given
/// up), its own reason over the snapshot's, the hold released before
/// `held_until` while the daemon is away, or a publish asked or held
/// given up while the daemon is away (a request may have landed, or
/// been accepted and not recorded, and only `look` tells); for any one
/// answer of the protocol: a request that may pend not withdrawn, a
/// publish given up before the daemon has answered a withdraw, an
/// answer that says nothing of the request read as one that does (the
/// hold released before `held_until`), `done` that `look` does not
/// confirm given up rather than stopped for a person, or `refused
/// taken` not looked up; or the age measured from the wrong end, or a
/// clock stepped back read as old (by a little or by the whole bound,
/// TG3).
#[test]
fn a_publish_older_than_its_bound_is_given_up_once_no_request_can_pend() {
    let ms = |d: Duration| u64::try_from(d.as_millis()).expect("ms");
    assert_eq!(AGE_MAX, Duration::from_secs(7200));
    assert!(!too_old(BEGUN, BEGUN + ms(AGE_MAX) - 1, AGE_MAX));
    assert!(too_old(BEGUN, BEGUN + ms(AGE_MAX), AGE_MAX));
    assert!(too_old(BEGUN, BEGUN + ms(AGE_MAX) + 1, AGE_MAX));
    assert!(!too_old(BEGUN, BEGUN - 1, AGE_MAX));
    assert!(!too_old(BEGUN, BEGUN - ms(AGE_MAX), AGE_MAX));
    assert!(!too_old(BEGUN, 0, AGE_MAX));
    assert!(!too_old(BEGUN, BEGUN + 4, Duration::from_millis(5)));
    assert!(too_old(BEGUN, BEGUN + 5, Duration::from_millis(5)));
    assert!(!too_old(0, u64::MAX - 1, Duration::MAX));

    let old = || seen().too_old();
    let none = || old().with(|s| s.pool.snapshot = None).unheld();
    // An answer to the withdraw the planner asked for first (L5).
    let expired = || {
        old()
            .answer(Reply::Expired)
            .looked(SnapSeen::Absent)
            .withdrawn()
    };
    let gave_up = || Action::Abandon(Why::TooOld);
    check(vec![
        ("intent", intent(), none(), gave_up()),
        ("intent, away", intent(), none().away(), gave_up()),
        (
            "intent, a snapshot with no manifest",
            intent(),
            old().unheld(),
            Action::Abandon(Why::ManifestLost),
        ),
        ("taken", taken_phase(), old(), Action::Release),
        (
            "taken, moved",
            taken_phase(),
            old().moved(),
            Action::Release,
        ),
        ("taken, released", taken_phase(), old().unheld(), gave_up()),
        (
            "taken, released, away",
            taken_phase(),
            old().unheld().away(),
            gave_up(),
        ),
        (
            "taken, gone",
            taken_phase(),
            old().gone(),
            Action::Abandon(Why::Destroyed),
        ),
        // A request accepted and not recorded pends: withdrawn, not asked.
        ("held, pending", held(), old(), Action::Withdraw),
        (
            "held, unknown",
            held(),
            old().answer(Reply::Unknown),
            Action::Withdraw,
        ),
        (
            "held, unknown, withdrawn",
            held(),
            old().answer(Reply::Unknown).withdrawn(),
            Action::Look,
        ),
        ("held, absent", held(), expired(), Action::Release),
        (
            "held, absent, released",
            held(),
            expired().unheld(),
            gave_up(),
        ),
        (
            "held, ours",
            held(),
            old()
                .answer(Reply::Unknown)
                .looked(SnapSeen::Ours(COMMIT))
                .withdrawn(),
            published(),
        ),
        ("held, away", held(), old().away(), Action::Wait),
        (
            "held, away, released",
            held(),
            old().unheld().away(),
            Action::Wait,
        ),
        ("asked, pending", asked(), old(), Action::Withdraw),
        (
            "asked, pending, a sync due",
            asked(),
            old().at(ASKED + 60 * SECOND),
            Action::Withdraw,
        ),
        (
            "asked, refused busy",
            asked(),
            old().refused(Code::Busy),
            Action::Wait,
        ),
        (
            "asked, expired, absent",
            asked(),
            expired(),
            Action::Release,
        ),
        (
            "asked, expired, absent, released",
            asked(),
            expired().unheld(),
            gave_up(),
        ),
        (
            "asked, expired, absent, released, a cut counted",
            asked(),
            expired().unheld().cuts(carrying()),
            gave_up(),
        ),
        (
            "asked, withdrawn, ours",
            asked(),
            old()
                .answer(Reply::Withdrawn)
                .looked(SnapSeen::Ours(COMMIT))
                .withdrawn(),
            published(),
        ),
        (
            "asked, expired, absent, gone",
            asked(),
            expired().gone(),
            Action::Abandon(Why::Destroyed),
        ),
        ("asked, away", asked(), old().away(), Action::Wait),
        (
            "asked, away, at held_until",
            asked(),
            old().away().at(UNTIL),
            Action::Release,
        ),
        (
            "asked, away, a ms before held_until",
            asked(),
            old().away().at(UNTIL - 1),
            Action::Wait,
        ),
        (
            "asked, away, released at held_until",
            asked(),
            old().away().unheld().at(UNTIL),
            Action::Wait,
        ),
        (
            "asked, here again, released, absent",
            asked(),
            expired().unheld().at(UNTIL),
            gave_up(),
        ),
        ("done", done(), old(), Action::Release),
        ("done, released", done(), old().unheld(), Action::Forget),
    ]);
    // Every answer of the protocol, too old (the lead's L5 on #144,
    // after N1 of its next look): the hold is never let go while a
    // request may pend. Each answer in four states: asked and not yet
    // looked up, a ms before `held_until`; the same at `held_until`;
    // asked, the daemon having answered a withdraw and `look` having
    // found no `snap/t`; and held, not asked, an hour after `held_until`.
    let mut answers = vec![
        Reply::Pending,
        Reply::Done(COMMIT),
        Reply::Expired,
        Reply::Withdrawn,
        Reply::Unknown,
        Reply::Cuts(cuts()),
        Reply::Accepted,
        Reply::Absent,
        Reply::Snap(row(G)),
        Reply::Unreadable(UnreadableReply {
            commit: COMMIT,
            why: skyzfs_control::Unread::NoRow,
        }),
    ];
    let codes = [
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
    ];
    answers.extend(codes.map(|code| Reply::Refused(code, String::new())));
    let mut wrong = Vec::new();
    for answer in answers {
        // L7: a refusal that says the request ended, or was never run.
        let ends = matches!(
            answer,
            Reply::Refused(
                Code::Rewound
                    | Code::Stopped
                    | Code::State
                    | Code::Attachment
                    | Code::Taken
                    | Code::Name
                    | Code::Manifest
                    | Code::Expiry
                    | Code::Version
                    | Code::Malformed
                    | Code::Peer
                    | Code::Store,
                _
            )
        );
        let kind = match answer {
            Reply::Done(_) => "look",
            _ if ends => "end",
            Reply::Pending | Reply::Absent | Reply::Refused(Code::Asked, _) => "withdraw",
            Reply::Withdrawn | Reply::Unknown | Reply::Expired => "withdraw first",
            _ => "keep",
        };
        let (fresh, at_until, after_withdraw, held_long) = match kind {
            "look" => (
                Action::Look,
                Action::Look,
                Action::Stop(Stop::DoneNotSeen),
                Action::Look,
            ),
            "end" => (Action::Look, Action::Look, Action::Release, Action::Look),
            "withdraw" => (
                Action::Withdraw,
                Action::Withdraw,
                Action::Withdraw,
                Action::Withdraw,
            ),
            "withdraw first" => (
                Action::Withdraw,
                Action::Withdraw,
                Action::Release,
                Action::Withdraw,
            ),
            _ => (Action::Wait, Action::Release, Action::Wait, Action::Wait),
        };
        let at = |s: Seen, ms: u64| s.with(|s| s.now_ms = ms);
        let cases = [
            (
                "asked",
                asked(),
                at(old().answer(answer.clone()), UNTIL - 1),
                fresh,
            ),
            (
                "asked, at held_until",
                asked(),
                at(old().answer(answer.clone()), UNTIL),
                at_until,
            ),
            (
                "asked, withdrawn, absent",
                asked(),
                at(
                    old()
                        .answer(answer.clone())
                        .withdrawn()
                        .looked(SnapSeen::Absent),
                    UNTIL - 1,
                ),
                after_withdraw,
            ),
            (
                "held, an hour after",
                held(),
                at(old().answer(answer.clone()), UNTIL + 3600 * SECOND),
                held_long,
            ),
        ];
        for (what, phase, seen, want) in cases {
            let publish = publish(phase);
            let got = plan(&publish, &seen);
            if got != want {
                wrong.push(format!("{answer:?}, {what}: {got:?}, not {want:?}"));
            }
            // The code is reported where the publish is given up on it,
            // and nowhere else.
            let told = report(&publish, &seen, &got);
            let want_told = match &answer {
                Reply::Refused(code, _) if ends && got == Action::Release => {
                    Some(Report::Refused(*code))
                }
                _ => None,
            };
            if told != want_told {
                wrong.push(format!(
                    "{answer:?}, {what}: reported {told:?}, not {want_told:?}"
                ));
            }
        }
    }
    // `done` and `refused taken` looked up: this publish's is done;
    // another's `snap/t` gives the name up after `refused taken`, and
    // stops the publish for a person after `done` (L6).
    let ours = || SnapSeen::Ours(COMMIT);
    for (what, seen, want) in [
        (
            "done, ours",
            old().answer(Reply::Done(COMMIT)).looked(ours()),
            published(),
        ),
        (
            "done, another commit",
            old()
                .answer(Reply::Done(COMMIT))
                .looked(SnapSeen::Ours([9; 32])),
            Action::Stop(Stop::DoneNotSeen),
        ),
        (
            "done, not ours",
            old().answer(Reply::Done(COMMIT)).looked(SnapSeen::NotOurs),
            Action::Stop(Stop::DoneNotSeen),
        ),
        (
            "taken, ours",
            old().refused(Code::Taken).looked(ours()),
            published(),
        ),
        (
            "taken, not ours",
            old().refused(Code::Taken).looked(SnapSeen::NotOurs),
            Action::Release,
        ),
        (
            "taken, not ours, released",
            old()
                .refused(Code::Taken)
                .looked(SnapSeen::NotOurs)
                .unheld(),
            Action::Abandon(Why::NameTaken),
        ),
        (
            "withdrawn, ours",
            old().answer(Reply::Withdrawn).withdrawn().looked(ours()),
            published(),
        ),
        (
            "withdrawn, not looked",
            old().answer(Reply::Unknown).withdrawn(),
            Action::Look,
        ),
        // L7: the request ended by the uploader, or never run.
        (
            "taken, absent, released",
            old().refused(Code::Taken).looked(SnapSeen::Absent).unheld(),
            Action::Abandon(Why::TooOld),
        ),
        (
            "rewound, ours",
            old().refused(Code::Rewound).looked(ours()),
            published(),
        ),
        (
            "rewound, absent, released",
            old()
                .refused(Code::Rewound)
                .looked(SnapSeen::Absent)
                .unheld(),
            Action::Abandon(Why::TooOld),
        ),
        (
            "rewound, absent, gone",
            old().refused(Code::Rewound).looked(SnapSeen::Absent).gone(),
            Action::Abandon(Why::Rewound),
        ),
        (
            "state, absent, gone",
            old().refused(Code::State).looked(SnapSeen::Absent).gone(),
            Action::Abandon(Why::Destroyed),
        ),
        (
            "version, not ours",
            old().refused(Code::Version).looked(SnapSeen::NotOurs),
            Action::Release,
        ),
        (
            "malformed, absent, released",
            old()
                .refused(Code::Malformed)
                .looked(SnapSeen::Absent)
                .unheld(),
            Action::Abandon(Why::TooOld),
        ),
        // N6 of #144's fourth look: the uploader stopped for good.
        (
            "stopped, ours",
            old().refused(Code::Stopped).looked(ours()),
            published(),
        ),
        (
            "stopped, absent",
            old().refused(Code::Stopped).looked(SnapSeen::Absent),
            Action::Release,
        ),
        (
            "stopped, absent, released",
            old()
                .refused(Code::Stopped)
                .looked(SnapSeen::Absent)
                .unheld(),
            Action::Abandon(Why::TooOld),
        ),
    ] {
        let publish = publish(asked());
        let got = plan(&publish, &seen);
        if got != want {
            wrong.push(format!("{what}: {got:?}, not {want:?}"));
        }
        // TG8 of #144's fourth look: reported only where the publish is
        // given up on a refusal that ends it; never beside one done.
        let told = report(&publish, &seen, &got);
        let want_told = match &seen.daemon {
            DaemonSeen::Here {
                answer: Reply::Refused(code, _),
                ..
            } if matches!(got, Action::Release | Action::Abandon(_))
                && !matches!(
                    code,
                    Code::Asked | Code::Busy | Code::Ended | Code::NoAnswer | Code::NoUpload
                ) =>
            {
                Some(Report::Refused(*code))
            }
            _ => None,
        };
        if told != want_told {
            wrong.push(format!("{what}: reported {told:?}, not {want_told:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // The report goes with the give-up itself too, not with a publish
    // given up on another answer or one not too old.
    let given_up = old().refused(Code::Peer).looked(SnapSeen::Absent).unheld();
    let gave = Action::Abandon(Why::TooOld);
    assert_eq!(plan(&publish(asked()), &given_up), gave);
    assert_eq!(
        report(&publish(asked()), &given_up, &gave),
        Some(Report::Refused(Code::Peer))
    );
    assert_eq!(report(&publish(held()), &expired().unheld(), &gave), None);
    let young = seen().refused(Code::Peer).looked(SnapSeen::Absent).unheld();
    assert_eq!(report(&publish(asked()), &young, &gave), None);
    assert_eq!(report(&publish(taken_phase()), &given_up, &gave), None);
    // Eight stalls are given up as such, too old or not.
    let stalled = Publish {
        stalls: STALLS_ABANDONED,
        ..publish(held())
    };
    assert_eq!(
        plan(&stalled, &expired().unheld()),
        Action::Abandon(Why::NoProgress)
    );
    // And a publish given up for its stalls keeps its hold as one too
    // old does: an answer that cannot tell waits, and one that says the
    // request ended is withdrawn first.
    let stalled_asked = Publish {
        stalls: STALLS_ABANDONED,
        ..publish(asked())
    };
    for code in [Code::Ended, Code::NoAnswer, Code::NoUpload] {
        let seen = seen().refused(code).looked(SnapSeen::Absent);
        assert_eq!(plan(&stalled_asked, &seen), Action::Wait, "{code:?}");
    }
    // `stopped` ends the request (N6 of #144's fourth look): the hold is
    // released at once, and the code reported.
    let stopped = seen().refused(Code::Stopped).looked(SnapSeen::Absent);
    assert_eq!(plan(&stalled_asked, &stopped), Action::Release);
    assert_eq!(
        report(&stalled_asked, &stopped, &Action::Release),
        Some(Report::Refused(Code::Stopped))
    );
    let expired_ = seen().answer(Reply::Expired).looked(SnapSeen::Absent);
    assert_eq!(plan(&stalled_asked, &expired_), Action::Withdraw);
    // An end the uploader gave is given up on at once, with the code.
    let ended = seen()
        .refused(Code::State)
        .looked(SnapSeen::Absent)
        .unheld();
    let gave = Action::Abandon(Why::NoProgress);
    assert_eq!(plan(&stalled_asked, &ended), gave);
    assert_eq!(
        report(&stalled_asked, &ended, &gave),
        Some(Report::Refused(Code::State))
    );
}

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

/// An earlier request landed: the publish was asked with C of a daemon
/// that carried it, by a cut it counted after C under its own rule (R).
/// That daemon stopped before the agent read `done`; the next one,
/// which knew nothing of the request, refused it `rewound` (its count
/// at start is above C), and the publish was asked anew with a count
/// read after (here 9, in another cut state). (In one daemon a request
/// refused `rewound` is dropped: it is the restart that lets both
/// happen.) The
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
    assert_eq!(plan(&anew, &landed()), published());
    assert_eq!(plan(&anew, &landed().unheld()), published());
    let mut first = publish(asked());
    first.count.after = C;
    assert_eq!(plan(&first, &landed()), published());
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
    assert_eq!(ASSIST_PAUSE, Duration::from_secs(1));
    assert_eq!(ASSIST_QUICK, 3);
}
