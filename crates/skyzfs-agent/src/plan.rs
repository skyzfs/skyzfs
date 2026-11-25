//! The planner (the plan of M2.6, §3): given a publish under way and
//! what the agent sees now of the pool, the daemon and `snap/<t>`, the
//! one thing to do next. It is a pure function: the engine (M2.6b2) does
//! the action, records what it changed, looks again and asks again, so
//! that one table (this module's tests) holds every rule, and a crash
//! anywhere is met by planning from what is seen.
//!
//! # The rules
//!
//! - **Intent** (the name chosen and the count read): with no `pool@t`
//!   and the daemon here, take it ([`Action::Take`], with the count read
//!   anew), unless the pool's premise is refused, which gives the name up
//!   with nothing taken ([`Why::Premise`]; S1 of #142's review); with the
//!   daemon away, wait. With a `pool@t`, the
//!   agent stopped between the program and the manifest's file: give it
//!   up, kept and unpublished ([`Why::ManifestLost`], the lead's Q5).
//! - **The snapshot broken** (from Taken on): `pool@t` gone
//!   ([`Why::Destroyed`]), a member's `@t` not the manifest's
//!   ([`Why::Replaced`]), or the pool's premise refused
//!   ([`Why::Premise`]): release the hold if it is there, then give up.
//!   A request that pends is withdrawn first. Members are compared by
//!   their snapshots' GUIDs alone ([`same_snapshot`]): a dataset renamed
//!   since keeps its `@t` (S3 of #142's review).
//! - **The hold**: placed after the manifest is on disk, and placed again
//!   where it is not there. Only with the hold there, the members
//!   verified and `load_guid` as read with the count is the publish
//!   recorded held ([`Action::Held`]).
//! - **Asking**: the first request with the count read before the
//!   snapshot ([`Action::Ask`]); after that, and wherever `load_guid` has
//!   changed (rule (L)), with a count read anew after the hold is
//!   verified ([`Action::AskAnew`], "The rule for AskAnew").
//! - **Pending**: withdrawn where the hold is not there, the snapshot
//!   is broken, or `load_guid` changed. Otherwise, with no qualifying
//!   cut [`ASSIST_AFTER`] after `accepted`, `zpool sync`
//!   ([`Action::Assist`]), one at a time: [`ASSIST_QUICK`] of them
//!   [`ASSIST_PAUSE`] apart, then [`ASSIST_AFTER`] apart. With a
//!   qualifying cut, a sync once [`ASSIST_AFTER`] has passed since the
//!   last one, and no sooner than twice [`ASSIST_AFTER`] after
//!   `accepted` (`assist_due`): the daemon's `cuts` counts the newest
//!   cut and does not say whether it was abandoned, and one abandoned on
//!   an idle pool would otherwise stop every sync until the request
//!   expires (Codex on #142; the lead's D3). Where the cut lands, the
//!   request ends and so do the syncs. The engine's sync writes a group
//!   of its own first, since a plain `zpool sync` writes none on an idle
//!   pool (N1 of #142's review; [`crate::zfs::Zfs::assist`]).
//! - **No progress** (the lead's decision for M2.6b2): a request that
//!   ended with the daemon's count where it was asked (same attachment
//!   and state, no cut counted since) is a stall ([`stalls`]).
//!   [`STALLS_REPORTED`] in a row or more are reported (at least once:
//!   the engine reports a publish once a process) and the publish goes on;
//!   where an end would make [`STALLS_ABANDONED`], that stall is recorded
//!   and nothing asked ([`Action::Stalled`]), and a publish with as many
//!   is given up ([`Why::NoProgress`], the hold released first, a request
//!   that pends withdrawn, and `snap/t` looked up). The stall is recorded
//!   first because the agent's own release writes a group, which the
//!   daemon counts as a cut: decided from the count, the release would
//!   undo the decision.
//! - **Too old** (L2 of #144's review): a publish older than its bound
//!   ([`AGE_MAX`], [`Seen::too_old`]) is given up ([`Why::TooOld`])
//!   once no request of it can pend, unless its snapshot is broken (its
//!   own reason first) or it has stalled [`STALLS_ABANDONED`] times
//!   ([`Why::NoProgress`]). At Intent nothing was taken: given up at
//!   once, the daemon here or not. At Taken nothing was asked: the hold
//!   released, then given up. Held or asked, as at the last stall, the
//!   bound never lets the hold go while a request may still pend (the
//!   lead's L5 on #144, after N1 of its next look). By the daemon's
//!   answer:
//!   - `done`: `snap/t` looked up; this publish's, at the commit `done`
//!     names, is [`Action::Done`]. `done` that `look` does not confirm
//!     stops the publish for a person ([`Stop::DoneNotSeen`]), with its
//!     hold and its record kept, as when the publish is not too old (L6).
//!     The stop is the engine's, in its process: only a person, or
//!     M2.6c's `recover`, ends that entry while the daemon says `done`.
//!     After a restart the publish is planned again and stops again;
//!     once the daemon has forgotten it (`unknown`), it is withdrawn,
//!     looked up and given up as below (S5 of #144's third look).
//!   - A refusal that ends the request (the lead's L7 on #144,
//!     after N4 of its third look): the uploader ended it (`rewound`,
//!     `state`, `attachment`, `taken`, and `stopped`, N6 of its fourth
//!     look), or the daemon did not run it
//!     (`name`, `manifest`, `expiry`, `version`, `malformed`, `peer`,
//!     `store`). Nothing pends: `snap/t` looked up, a publish made is
//!     done, another's `snap/t` gives the name up ([`Why::NameTaken`]),
//!     and with none the hold is released and the publish given up
//!     ([`Why::Rewound`] where a rewind lost `pool@t`), the code
//!     reported ([`Report::Refused`]). Not too old, these answers end
//!     the publish in a round too (below), or stop it for a person;
//!     `stopped` gives it up as here ([`Why::Stopped`]).
//!   - `pending` or `refused asked`: the request withdrawn first. So is
//!     `absent`, which skyzfsd does not give to `progress` or `withdraw`
//!     (it answers `look`): a withdraw is safe whatever pends (S4 of the
//!     third look).
//!   - `withdrawn`, `unknown` or `expired`: withdrawn first, unless the
//!     daemon has already answered a withdraw so ([`Seen::withdrawn`]);
//!     then `snap/t` looked up, a publish made is done, and one not made
//!     has its hold released and is given up.
//!   - Any other answer, or none: as a daemon away (below), the hold
//!     kept until `held_until` for a publish asked, then released; kept
//!     for one held and not asked; nothing given up.
//!
//!   skyzfsd answers `no-answer` when its uploader has not answered
//!   within its limit, which a long step of the uploader gives while the
//!   request pends; a request may also have landed while the daemon was
//!   away, or been accepted and not recorded (step 8 of the engine
//!   records it after the answer), and only `look` tells. Where `cuts`
//!   is not answered no publish can be begun meanwhile; where only the
//!   uploader is silent publishes can be, up to
//!   [`crate::record::UNDER_WAY_MAX`], and the next is refused. The same
//!   holds for a publish that has stalled [`STALLS_ABANDONED`] times.
//!
//!   A give-up after a withdraw rests on the daemon's order: skyzfsd's
//!   control channel answers one connection at a time, in the order it
//!   accepts them, and hands an ask to the uploader's inbox (or drops it,
//!   past its limit) before it takes the next; `progress` and `withdraw`
//!   go to the same inbox. An ask of an agent that stopped while the
//!   store's desk was slow is therefore in the inbox, or gone, before its
//!   successor's withdraw (N5 of #144's third look). A daemon that
//!   answered connections side by side would break this.
//! - **What the daemon answers.** `done`, `expired`, `withdrawn`,
//!   `refused attachment`, `refused state` and `refused taken` are read
//!   with `look` first: `snap/t` that is this publish's is
//!   [`Action::Done`] (`done` must name the commit `look` sees); one
//!   that is not, or that cannot be read, gives the name up
//!   ([`Why::NameTaken`]); none is asked anew. `unknown` and `refused
//!   rewound` are asked anew without a look (nothing was published, or
//!   the uploader that knew has gone; a publish asked again is looked up
//!   by the uploader before it is made), but where the snapshot is
//!   broken, the publish will not be asked again, so it is looked up
//!   before it is given up (F1 and the lead's D1 on #142). `refused
//!   rewound` with `pool@t` gone and no `snap/t` is [`Why::Rewound`].
//!   `refused busy` waits; `refused asked` (another request of the name
//!   pends) withdraws it. A `look` refused (`store`) or not answered is
//!   no sight of `snap/t`: the engine leaves [`Seen::snap`] empty, and
//!   the planner asks for the look again.
//! - **Done with the snapshot broken.** `snap/t` that is this publish's,
//!   seen while `pool@t` or a member's `@t` is gone, is recorded done
//!   and reported ([`Report::SnapshotGone`]): it was made before a rewind
//!   (its cut holds `pool@t`) or after one through a gap in rule (R) (it
//!   does not), and only opening the cut tells (the lead's D1; S2). The
//!   engine makes the report before it records the publish done, so at
//!   least once: a stop between the two plans the same again (F1 of
//!   #144's review).
//! - **A daemon away** (no answer, or `ended`, `no-answer`, `no-upload`):
//!   wait; a publish asked keeps its hold until
//!   `held_until`, then releases it. When the daemon answers again, the
//!   answers above apply, and a hold released is placed again before the
//!   publish is asked anew.
//! - **Done**: release, then forget.
//!
//! # What is this module's reading
//!
//! - A `snap/t` is this publish's ([`SnapSeen::Ours`]) where its row
//!   records this manifest (its length and hash) and a cut whose group
//!   is the snapshot's or later. The plan also compared the cut's state
//!   and number with the request; they are not compared here, since a
//!   request asked before an [`Action::AskAnew`] (with another count)
//!   may be the one that landed, a withdrawal losing to it. Each request
//!   this agent made was taken by the daemon under its own rules, rule
//!   (R) among them, when it landed; the manifest's hash, over the
//!   members' random 64-bit snapshot GUIDs, names this snapshot. The
//!   lead agreed (#142); `an_earlier_request_that_landed_is_this_publishs`
//!   is its row.
//! - A publish held and not yet asked waits for a daemon away without
//!   a limit; the plan's `held_until` begins at `accepted`.
//! - A refusal that says the agent asked wrongly (`name`, `manifest`,
//!   `expiry`, `version`, `malformed`, `peer`) stops the publish and is
//!   reported ([`Action::Stop`]); so does `store`, which only `look`
//!   answers, where it comes as an answer about the publish, and `done`
//!   that `look` does not confirm. That holds for a publish not too old;
//!   one too old, or stalled [`STALLS_ABANDONED`] times, is looked up and
//!   given up on these refusals instead (L7, above; S7 of #144's fourth
//!   look).

use std::time::Duration;

use skyzfs_control::{Code, CutsReply, Reply, ASK_LIMIT};

use crate::record::{Count, Member, Phase, Publish, Taken, Why};

/// How long after `accepted` a publish with no qualifying cut is helped
/// by `zpool sync`: two of ZFS's default transaction group timeouts (the
/// lead's Q8). With a qualifying cut, also the least time between two
/// syncs (`assist_due`).
pub const ASSIST_AFTER: Duration = Duration::from_secs(10);

/// The least time between two syncs of one publish. "One at a time" (the
/// plan, §6) is read as one done before the next, at most one a second.
/// It holds for the first [`ASSIST_QUICK`] syncs of a request; after
/// them, [`ASSIST_AFTER`] (`assist_due`).
pub const ASSIST_PAUSE: Duration = Duration::from_secs(1);

/// The syncs of a request with no qualifying cut that are run
/// [`ASSIST_PAUSE`] apart; each after them waits [`ASSIST_AFTER`] (the
/// lead's decision on #144). Through the `zfs` command each sync writes
/// two groups, the set's and the one in which the sync writes the set's
/// history record (the kernel probe of #145, ZFS 2.4.1), and the daemon
/// counts a cut for each. Unspaced, a pool whose cut never comes would
/// get two groups a second for as long as the publish stalls: up to
/// [`STALLS_ABANDONED`] requests that run to their expiry, about 80
/// minutes at the default expiry. Spaced, it costs about twelve groups
/// (and cuts) a minute: at the default expiry, three syncs from
/// [`ASSIST_AFTER`] after `accepted`, then one each [`ASSIST_AFTER`],
/// about 61 syncs and 122 groups a request; on a nearly empty pool, about
/// 440 uploaded blocks a minute (#145's review). The lead accepted that
/// cost for the command's route.
pub const ASSIST_QUICK: u32 = 3;

/// The stalls in a row ([`stalls`]) at which a publish is reported
/// ([`Report::NoProgress`]) and goes on (the lead's decision for M2.6b2).
pub const STALLS_REPORTED: u32 = 3;

/// The stalls in a row at which a publish is given up
/// ([`Why::NoProgress`]) instead of being asked anew.
pub const STALLS_ABANDONED: u32 = 8;

/// The age past which a publish is given up ([`Why::TooOld`]), whatever
/// the daemon says (L2 of #144's review): measured from when it was
/// begun ([`Publish::begun_ms`]). Two hours, the lead's figure, which
/// is longer than [`STALLS_ABANDONED`] requests that run to the default
/// expiry (about 81 minutes), so that a publish that stalls is given up
/// as [`Why::NoProgress`] first. This bound is for a publish that does
/// not stall and does not end: a daemon that answers `refused busy`
/// without end, or whose cuts are counted and none carries the request
/// (each abandoned, or never uploaded), so that each request expires
/// with the count moved. It keeps [`crate::record::UNDER_WAY_MAX`] from
/// filling with publishes nobody can finish. The engine takes another
/// for its tests and the rigs' (`Agent::with_age_max`).
pub const AGE_MAX: Duration = Duration::from_secs(2 * 60 * 60);

/// The margin past a publish's expiry for which the agent keeps its
/// hold while the daemon does not answer: the channel's own wait
/// ([`ASK_LIMIT`], 40 s; the lead's Q8).
pub const HOLD_MARGIN: Duration = ASK_LIMIT;

/// What the agent sees of the pool, for one publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSeen {
    /// The pool's `load_guid` now.
    pub load_guid: u64,
    /// Whether the pool's layout and state allow a publish (the plan,
    /// §6, "The premise").
    pub premise: bool,
    /// Each member's `@t`, by name, or `None` where `pool@t` does not
    /// exist.
    pub snapshot: Option<Vec<Member>>,
    /// Whether the agent's hold is on `pool@t`.
    pub held: bool,
}

/// What the agent sees of the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonSeen {
    /// Its channel did not answer.
    Away,
    /// It answered `cuts`, and of the publish: the refusal of the last
    /// request where it was refused, else `progress`.
    Here {
        /// Its count of cuts.
        cuts: CutsReply,
        /// What it said of the publish.
        answer: Reply,
    },
}

/// What `look` saw of `snap/<t>`, against the publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapSeen {
    /// No `snap/<t>`.
    Absent,
    /// `snap/<t>` names this commit and records this publish.
    Ours([u8; 32]),
    /// `snap/<t>` records another publish.
    NotOurs,
    /// `snap/<t>` has no row that can be read.
    Unreadable,
}

/// Everything the planner reads besides the publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The pool.
    pub pool: PoolSeen,
    /// The daemon.
    pub daemon: DaemonSeen,
    /// `snap/<t>`, where it was looked at since the last action.
    pub snap: Option<SnapSeen>,
    /// The wall clock, in ms since 1970.
    pub now_ms: u64,
    /// When this publish was last helped by a sync.
    pub assisted_ms: Option<u64>,
    /// The syncs run for the request under way since it was accepted, in
    /// this process (none after a restart: the conservative reading, as a
    /// restart is rare and gives at most [`ASSIST_QUICK`] syncs more).
    pub assists: u32,
    /// Whether the publish is older than its bound ([`too_old`]).
    pub too_old: bool,
    /// Whether the daemon has answered a withdraw of this publish's
    /// request with an answer that says nothing pends (`withdrawn`,
    /// `unknown`, `expired`), in this process and since a request of it
    /// was last accepted.
    pub withdrawn: bool,
}

/// The one thing to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run the channel program for `pool@t`, with the count read anew.
    Take,
    /// Place the hold on `pool@t`.
    Hold,
    /// Record the publish held: the hold, the members and `load_guid` are
    /// verified.
    Held,
    /// Ask the daemon with the recorded count.
    Ask,
    /// Read a count anew after the hold is verified, and ask with it.
    AskAnew,
    /// Record the request's end as the publish's last stall
    /// ([`STALLS_ABANDONED`]), at [`Phase::Held`], and ask nothing.
    Stalled,
    /// Withdraw the request that pends.
    Withdraw,
    /// Look `snap/<t>` up.
    Look,
    /// A sync task of the agent's own, then `zpool sync`, so that a
    /// group is written and a cut counted, even on an idle pool.
    Assist,
    /// Nothing now: see again later.
    Wait,
    /// Release the hold.
    Release,
    /// Record the publish done: `snap/<t>` names this commit; with a
    /// report where the snapshot was not whole when it was seen.
    Done {
        /// The commit `snap/<t>` names.
        commit: [u8; 32],
        /// What a person is told.
        report: Option<Report>,
    },
    /// Remove the publish from the record, and its manifest.
    Forget,
    /// Give the publish up, keeping `pool@t` where it is.
    Abandon(Why),
    /// Stop the publish and report: something the agent cannot settle.
    Stop(Stop),
}

/// Why a publish was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The daemon refused the request in a way that says the agent asked
    /// wrongly, or object storage failed a look.
    Refused(Code),
    /// The daemon said `done`, and `look` did not see this publish's
    /// `snap/<t>` with the commit `done` named. The hold and the record
    /// are kept, past the age bound too (the lead's L6 on #144): only a
    /// person, or M2.6c's `recover`, ends the entry while the daemon says
    /// `done`. The stop is the engine's, in its process ([module
    /// docs](self), "Too old").
    DoneNotSeen,
}

/// What the agent tells a person while it goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// `snap/<t>` is this publish's, and `pool@t` or a member's `@t` is
    /// gone: the publish was made before a rewind, or after one through a
    /// gap in rule (R), and only opening its cut tells which.
    SnapshotGone,
    /// The publish was asked anew this many times in a row with no cut
    /// counted ([`stalls`]).
    NoProgress(u32),
    /// A publish too old, or stalled [`STALLS_ABANDONED`] times, given up
    /// where the daemon refused it with this code ([`report`]).
    Refused(Code),
}

/// When a publish accepted at `asked_ms` with `expiry` stops being held
/// while the daemon does not answer.
pub fn held_until(asked_ms: u64, expiry: Duration) -> u64 {
    let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    asked_ms
        .saturating_add(ms(expiry))
        .saturating_add(ms(HOLD_MARGIN))
}

/// What a `look` answer says of `snap/<t>` against what was taken, or
/// `None` where it is not an answer of `look` (a refusal).
pub fn snap_seen(reply: &Reply, taken: &Taken) -> Option<SnapSeen> {
    match reply {
        Reply::Absent => Some(SnapSeen::Absent),
        Reply::Snap(row) => Some(
            if row.manifest_len == taken.manifest_len
                && row.manifest_hash == taken.manifest_hash
                && row.group >= taken.group
            {
                SnapSeen::Ours(row.commit)
            } else {
                SnapSeen::NotOurs
            },
        ),
        Reply::Unreadable(_) => Some(SnapSeen::Unreadable),
        _ => None,
    }
}

/// The next action for `publish` ([module docs](self)).
pub fn plan(publish: &Publish, seen: &Seen) -> Action {
    let pool = &seen.pool;
    match &publish.phase {
        Phase::Intent => match (&pool.snapshot, &seen.daemon) {
            (None, _) if seen.too_old => Action::Abandon(Why::TooOld),
            (None, DaemonSeen::Away) => Action::Wait,
            (None, DaemonSeen::Here { .. }) if !pool.premise => Action::Abandon(Why::Premise),
            (None, DaemonSeen::Here { .. }) => Action::Take,
            (Some(_), _) => give_up(pool, Why::ManifestLost),
        },
        Phase::Taken(taken) => {
            if let Some(why) = broken(pool, taken) {
                return give_up(pool, why);
            }
            if seen.too_old {
                return give_up(pool, Why::TooOld);
            }
            if !pool.held {
                return Action::Hold;
            }
            if pool.load_guid == publish.count.load_guid {
                return Action::Held;
            }
            match seen.daemon {
                DaemonSeen::Away => Action::Wait,
                DaemonSeen::Here { .. } => Action::AskAnew,
            }
        }
        Phase::Held(taken) => onward(publish, taken, None, seen),
        Phase::Asked {
            taken,
            asked_ms,
            held_until_ms,
        } => onward(publish, taken, Some((*asked_ms, *held_until_ms)), seen),
        Phase::Done { .. } if pool.held => Action::Release,
        Phase::Done { .. } => Action::Forget,
    }
}

/// Why the snapshot can no longer be published, if it cannot.
fn broken(pool: &PoolSeen, taken: &Taken) -> Option<Why> {
    let Some(members) = &pool.snapshot else {
        return Some(Why::Destroyed);
    };
    if !same_snapshot(members, taken) {
        return Some(Why::Replaced);
    }
    if !pool.premise {
        return Some(Why::Premise);
    }
    None
}

/// Whether `members` are the snapshot that was taken: the same snapshot
/// GUIDs, in any order and under any names (a dataset renamed since
/// keeps its `@t`; S3 of #142's review). A GUID is random and 64 bits,
/// and names one `@t`.
pub fn same_snapshot(members: &[Member], taken: &Taken) -> bool {
    let guids = |members: &[Member]| {
        let mut guids: Vec<u64> = members.iter().map(|m| m.snapshot_guid).collect();
        guids.sort_unstable();
        guids
    };
    guids(members) == guids(&taken.members)
}

/// The stalls of `publish` once it is asked anew with `count`: one more
/// where it was asked (accepted) and the daemon's count is where it was
/// (the same attachment and state, no cut counted since), none where a
/// cut was counted or the count is of another attachment or state, and
/// as they were where it was not asked. A reboot (a new attachment and
/// state) or a rewind resets them (the conservative reading; the lead's
/// words were "with no cut taken"). A take-over does not: a new daemon
/// on the same attachment and state that answers `unknown` with the
/// count where it was asked is a stall, so a daemon that dies before
/// each cut gives a publish up after eight take-overs, in as little time
/// as they take, and not only after eight expiries (F2 of #144's
/// review).
pub fn stalls(publish: &Publish, count: &Count) -> u32 {
    match publish.phase {
        Phase::Asked { .. } if same_place(count, &publish.count) => {
            publish.stalls.saturating_add(1)
        }
        Phase::Asked { .. } => 0,
        _ => publish.stalls,
    }
}

/// Whether two counts are at one place: the same attachment, state and
/// number of cuts (`load_guid` aside).
fn same_place(a: &Count, b: &Count) -> bool {
    a.attachment == b.attachment && a.state == b.state && a.after == b.after
}

/// The report due for a publish with `stalls`: from the third on. A
/// report is not recorded, so the engine makes it once in a process, and
/// a restart makes it again (at least once, not exactly once).
pub fn stalls_report(stalls: u32) -> Option<Report> {
    (stalls >= STALLS_REPORTED).then_some(Report::NoProgress(stalls))
}

/// Whether a publish begun at `begun_ms` is older than `age_max` at
/// `now_ms`. A time begun later than now (the clock stepped back) is an
/// age of none: the bound comes later, not sooner.
pub fn too_old(begun_ms: u64, now_ms: u64, age_max: Duration) -> bool {
    let age_max = u64::try_from(age_max.as_millis()).unwrap_or(u64::MAX);
    now_ms.saturating_sub(begun_ms) >= age_max
}

/// Give the publish up for `why`, the hold released first.
fn give_up(pool: &PoolSeen, why: Why) -> Action {
    if pool.held {
        Action::Release
    } else {
        Action::Abandon(why)
    }
}

/// Whether a publish pending since `asked_ms`, last helped at
/// `assisted_ms`, is due a `zpool sync` at `now_ms`.
///
/// With no qualifying cut: from [`ASSIST_AFTER`] after `asked_ms`, each
/// [`ASSIST_PAUSE`] after the last while fewer than [`ASSIST_QUICK`] were
/// run for the request (`assists`), each [`ASSIST_AFTER`] after the last
/// from then on. With one (`carried`): a cut that
/// carries the request is on its way, so a sync helps only where that cut
/// was abandoned (the daemon's `cuts` cannot say): once [`ASSIST_AFTER`]
/// has passed since the last sync, and since twice [`ASSIST_AFTER`]
/// after `asked_ms`. It cannot spin: a sync is due at most once per
/// [`ASSIST_AFTER`] here (and per [`ASSIST_PAUSE`] without such a cut),
/// since the engine records each in [`Seen::assisted_ms`] before it plans
/// again (a time recorded later than `now_ms` counts as none), and the
/// request ends, at the latest at its expiry, after which
/// the publish is looked up and asked anew (and a sync may be due again).
fn assist_due(
    carried: bool,
    asked_ms: u64,
    assisted_ms: Option<u64>,
    assists: u32,
    now_ms: u64,
) -> bool {
    let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    // A sync recorded later than now (the clock stepped back) counts as
    // none, so that it does not stop every sync until the clock passes
    // it (S11 of #142's review).
    let assisted_ms = assisted_ms.filter(|&last| last <= now_ms);
    let after = |at: u64, d: Duration| now_ms >= at.saturating_add(ms(d));
    if carried {
        let from = asked_ms.saturating_add(ms(ASSIST_AFTER));
        let from = assisted_ms.map_or(from, |last| last.max(from));
        after(from, ASSIST_AFTER)
    } else {
        let pause = if assists < ASSIST_QUICK {
            ASSIST_PAUSE
        } else {
            ASSIST_AFTER
        };
        after(asked_ms, ASSIST_AFTER) && assisted_ms.is_none_or(|last| after(last, pause))
    }
}

/// Whether the daemon's newest cut carries the request: of its state,
/// counted after its count, of the snapshot's group or later, and opened
/// as itself.
fn qualifies(cuts: &CutsReply, count: &Count, taken: &Taken) -> bool {
    cuts.attachment == count.attachment
        && cuts.state == Some(count.state)
        && cuts.count > count.after
        && cuts
            .newest
            .is_some_and(|(group, itself)| itself && group >= taken.group)
}

/// Whether a refusal of `progress` or `withdraw` says that no request of
/// the publish pends: the uploader ended it (`rewound`, `state`,
/// `attachment`, `taken`, `stopped`), or the daemon did not run what it
/// was asked (`name`, `manifest`, `expiry`, `version`, `malformed`,
/// `peer`, `store`): a daemon that cannot read this agent's requests
/// accepted none of them.
///
/// `stopped` is the uploader's answer for good: it ends every publish
/// pending when it stops (the lease lost, a transaction fenced, a cut
/// kept committed), takes no step after, and refuses every request after
/// it until skyzfsd restarts (N6 of #144's fourth look; the lead's
/// decision for M2.6c). A publish is given up on it whatever its age
/// (the lead's ruling on #146): waiting would only hold `pool@t` for up
/// to [`AGE_MAX`], and the next publish covers a restart.
///
/// Not `asked` (one pends), `busy`, or the daemon's silence (`ended`,
/// `no-answer`, `no-upload`). skyzfsd gives `busy` only to a `publish`
/// it did not accept; here it can only be an ask's answer kept for a
/// round, and the next round reads `progress` (S7 of the fourth look).
fn ends(code: Code) -> bool {
    match code {
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
        | Code::Store => true,
        Code::Asked | Code::Busy | Code::Ended | Code::NoAnswer | Code::NoUpload => false,
    }
}

/// What a person is told where `action`, planned from `seen`, gives up a
/// publish too old, or stalled [`STALLS_ABANDONED`] times, on a refusal
/// that ends its request ([`Report::Refused`], the lead's L7 on #144),
/// or any publish on `refused stopped` (the lead's ruling on #146): the
/// release, and the give-up after it. The engine makes it before it
/// acts, once a process (as every report: at least once).
pub fn report(publish: &Publish, seen: &Seen, action: &Action) -> Option<Report> {
    let under_way = matches!(publish.phase, Phase::Held(_) | Phase::Asked { .. });
    let ended = publish.stalls >= STALLS_ABANDONED || seen.too_old;
    let gives_up = matches!(action, Action::Release | Action::Abandon(_));
    match &seen.daemon {
        DaemonSeen::Here {
            answer: Reply::Refused(code, _),
            ..
        } if under_way && (ended || *code == Code::Stopped) && gives_up && ends(*code) => {
            Some(Report::Refused(*code))
        }
        _ => None,
    }
}

/// Held or asked: `asked` is when it was accepted and how long it is
/// held, once it was.
fn onward(publish: &Publish, taken: &Taken, asked: Option<(u64, u64)>, seen: &Seen) -> Action {
    let pool = &seen.pool;
    let away = || match asked {
        Some((_, until)) if seen.now_ms >= until && pool.held => Action::Release,
        _ => Action::Wait,
    };
    let (cuts, answer) = match &seen.daemon {
        DaemonSeen::Away => return away(),
        DaemonSeen::Here { cuts, answer } => (cuts, answer),
    };
    let broken = broken(pool, taken);
    let moved = pool.load_guid != publish.count.load_guid;
    let report = match broken {
        Some(Why::Destroyed | Why::Replaced) => Some(Report::SnapshotGone),
        _ => None,
    };
    let done = |commit: [u8; 32]| Action::Done { commit, report };
    // `done` must name the commit `look` sees; otherwise a person looks.
    let said_done = |commit: &[u8; 32]| match seen.snap {
        None => Action::Look,
        Some(SnapSeen::Ours(at)) if at == *commit => done(at),
        Some(_) => Action::Stop(Stop::DoneNotSeen),
    };
    let look = |then: &dyn Fn() -> Action| match seen.snap {
        None => Action::Look,
        Some(SnapSeen::Ours(commit)) => done(commit),
        Some(SnapSeen::NotOurs | SnapSeen::Unreadable) => give_up(pool, Why::NameTaken),
        Some(SnapSeen::Absent) => then(),
    };
    // A request accepted ended with no cut counted since its count.
    let stalled = asked.is_some()
        && cuts.attachment == publish.count.attachment
        && cuts.state == Some(publish.count.state)
        && cuts.count == publish.count.after;
    // Ask again: the snapshot whole and held, with a count read anew.
    let anew = |rewound: bool| {
        if let Some(why) = broken {
            let why = match why {
                Why::Destroyed if rewound => Why::Rewound,
                why => why,
            };
            return give_up(pool, why);
        }
        if stalled && publish.stalls.saturating_add(1) >= STALLS_ABANDONED {
            return Action::Stalled;
        }
        if !pool.held {
            return Action::Hold;
        }
        Action::AskAnew
    };
    let ended = if publish.stalls >= STALLS_ABANDONED {
        Some(Why::NoProgress)
    } else if seen.too_old {
        Some(Why::TooOld)
    } else {
        None
    };
    if let Some(why) = ended {
        // The bound never lets the hold go while a request may still
        // pend (the lead's L5 on #144, after N1 of its next look): the
        // publish is given up only once the daemon has answered a
        // withdraw and `look` has not found it published.
        let ended = || give_up(pool, broken.unwrap_or(why));
        return match answer {
            Reply::Done(commit) => said_done(commit),
            // The daemon ended the request itself, or never ran it (the
            // lead's L7 on #144, after N4 of its third look): nothing
            // pends, and the code is reported ([`report`]).
            Reply::Refused(Code::Rewound, _) => look(&|| match broken {
                Some(Why::Destroyed) => give_up(pool, Why::Rewound),
                _ => ended(),
            }),
            Reply::Refused(code, _) if ends(*code) => look(&ended),
            Reply::Pending | Reply::Absent | Reply::Refused(Code::Asked, _) => Action::Withdraw,
            Reply::Withdrawn | Reply::Unknown | Reply::Expired if seen.withdrawn => look(&ended),
            Reply::Withdrawn | Reply::Unknown | Reply::Expired => Action::Withdraw,
            _ => away(),
        };
    }
    match answer {
        Reply::Pending => {
            if !pool.held || broken.is_some() || moved {
                return Action::Withdraw;
            }
            let Some((asked_ms, _)) = asked else {
                // Accepted before the agent stopped, and not recorded.
                return Action::Ask;
            };
            let carried = qualifies(cuts, &publish.count, taken);
            if assist_due(
                carried,
                asked_ms,
                seen.assisted_ms,
                seen.assists,
                seen.now_ms,
            ) {
                Action::Assist
            } else {
                Action::Wait
            }
        }
        Reply::Done(commit) => said_done(commit),
        Reply::Expired | Reply::Withdrawn => look(&|| anew(false)),
        Reply::Unknown => match asked {
            None if pool.held && broken.is_none() && !moved => Action::Ask,
            _ if broken.is_some() => look(&|| anew(false)),
            _ => anew(false),
        },
        Reply::Refused(code, _) => match code {
            Code::Attachment | Code::State | Code::Taken => look(&|| anew(false)),
            Code::Rewound if broken.is_some() => look(&|| anew(true)),
            Code::Rewound => anew(true),
            Code::Busy => Action::Wait,
            Code::Asked => Action::Withdraw,
            // The uploader has stopped for good and takes no step in this
            // run of skyzfsd: looked up, then given up and reported, the
            // hold released (the lead's ruling on #146). The next
            // publish covers a restart.
            Code::Stopped => look(&|| give_up(pool, broken.unwrap_or(Why::Stopped))),
            Code::Ended | Code::NoAnswer | Code::NoUpload => away(),
            Code::Name
            | Code::Manifest
            | Code::Expiry
            | Code::Version
            | Code::Malformed
            | Code::Peer
            | Code::Store => Action::Stop(Stop::Refused(*code)),
        },
        // Not an answer about a publish: the daemon is not one this agent
        // understands. Seen again later.
        Reply::Cuts(_)
        | Reply::Accepted
        | Reply::Absent
        | Reply::Snap(_)
        | Reply::Unreadable(_) => Action::Wait,
    }
}

#[cfg(test)]
mod tests;
