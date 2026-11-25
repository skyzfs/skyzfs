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
//!   anew); with the daemon away, wait. With a `pool@t`, the
//!   agent stopped between the program and the manifest's file: give it
//!   up, kept and unpublished ([`Why::ManifestLost`], the lead's Q5).
//! - **The snapshot broken** (from Taken on): `pool@t` gone
//!   ([`Why::Destroyed`]), a member's `@t` not the manifest's
//!   ([`Why::Replaced`]), or the pool's premise refused
//!   ([`Why::Premise`]): release the hold if it is there, then give up.
//!   A request that pends is withdrawn first.
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
//!   ([`Action::Assist`]), one at a time, [`ASSIST_PAUSE`] apart. With a
//!   qualifying cut, a sync once [`ASSIST_AFTER`] has passed since the
//!   last one, and no sooner than twice [`ASSIST_AFTER`] after
//!   `accepted` ([`assist_due`]): the daemon's `cuts` counts the newest
//!   cut and does not say whether it was abandoned, and one abandoned on
//!   an idle pool would otherwise stop every sync until the request
//!   expires (Codex on #142; the lead's D3). Where the cut lands, the
//!   request ends and so do the syncs.
//! - **What the daemon answers.** `done`, `expired`, `withdrawn`,
//!   `refused attachment`, `refused state` and `refused taken` are read
//!   with `look` first: `snap/t` that is this publish's is
//!   [`Action::Done`] (`done` must name the commit `look` sees); one
//!   that is not, or that cannot be read, gives the name up
//!   ([`Why::NameTaken`]); none is asked anew. `unknown` and `refused
//!   rewound` are asked anew without a look (nothing was published, or
//!   the uploader that knew has gone; a publish asked again is looked up
//!   by the uploader before it is made). `refused rewound` with `pool@t`
//!   gone is [`Why::Rewound`]. `refused busy` waits; `refused asked`
//!   (another request of the name pends) withdraws it. A `look` refused
//!   (`store`) or not answered is no sight of `snap/t`: the engine leaves
//!   [`Seen::snap`] empty, and the planner asks for the look again.
//! - **A daemon away** (no answer, or `stopped`, `ended`, `no-answer`,
//!   `no-upload`): wait; a publish asked keeps its hold until
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
//!   that `look` does not confirm.

use std::time::Duration;

use skyzfs_control::{Code, CutsReply, Reply, ASK_LIMIT};

use crate::record::{Count, Member, Phase, Publish, Taken, Why};

/// How long after `accepted` a publish with no qualifying cut is helped
/// by `zpool sync`: two of ZFS's default transaction group timeouts (the
/// lead's Q8). With a qualifying cut, also the least time between two
/// syncs ([`assist_due`]).
pub const ASSIST_AFTER: Duration = Duration::from_secs(10);

/// The least time between two syncs of one publish. "One at a time" (the
/// plan, §6) is read as one done before the next, at most one a second.
pub const ASSIST_PAUSE: Duration = Duration::from_secs(1);

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
    /// Withdraw the request that pends.
    Withdraw,
    /// Look `snap/<t>` up.
    Look,
    /// `zpool sync`, so that a group is written and a cut counted.
    Assist,
    /// Nothing now: see again later.
    Wait,
    /// Release the hold.
    Release,
    /// Record the publish done: `snap/<t>` names this commit.
    Done([u8; 32]),
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
    /// `snap/<t>` with the commit `done` named.
    DoneNotSeen,
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
            (None, DaemonSeen::Away) => Action::Wait,
            (None, DaemonSeen::Here { .. }) => Action::Take,
            (Some(_), _) => give_up(pool, Why::ManifestLost),
        },
        Phase::Taken(taken) => {
            if let Some(why) = broken(pool, taken) {
                return give_up(pool, why);
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
    let mut seen = members.clone();
    seen.sort();
    let mut wanted = taken.members.clone();
    wanted.sort();
    if seen != wanted {
        return Some(Why::Replaced);
    }
    if !pool.premise {
        return Some(Why::Premise);
    }
    None
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
/// [`ASSIST_PAUSE`] after the last. With one (`carried`): a cut that
/// carries the request is on its way, so a sync helps only where that cut
/// was abandoned (the daemon's `cuts` cannot say): once [`ASSIST_AFTER`]
/// has passed since the last sync, and since twice [`ASSIST_AFTER`]
/// after `asked_ms`. It cannot spin: a sync is due at most once per
/// [`ASSIST_AFTER`] here (and per [`ASSIST_PAUSE`] without such a cut),
/// since the engine records each in [`Seen::assisted_ms`] before it plans
/// again, and the request ends, at the latest at its expiry, after which
/// the publish is looked up and asked anew (and a sync may be due again).
fn assist_due(carried: bool, asked_ms: u64, assisted_ms: Option<u64>, now_ms: u64) -> bool {
    let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    let after = |at: u64, d: Duration| now_ms >= at.saturating_add(ms(d));
    if carried {
        let from = asked_ms.saturating_add(ms(ASSIST_AFTER));
        let from = assisted_ms.map_or(from, |last| last.max(from));
        after(from, ASSIST_AFTER)
    } else {
        after(asked_ms, ASSIST_AFTER) && assisted_ms.is_none_or(|last| after(last, ASSIST_PAUSE))
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
    let look = |then: &dyn Fn() -> Action| match seen.snap {
        None => Action::Look,
        Some(SnapSeen::Ours(commit)) => Action::Done(commit),
        Some(SnapSeen::NotOurs | SnapSeen::Unreadable) => give_up(pool, Why::NameTaken),
        Some(SnapSeen::Absent) => then(),
    };
    // Ask again: the snapshot whole and held, with a count read anew.
    let anew = |rewound: bool| {
        if let Some(why) = broken {
            let why = match why {
                Why::Destroyed if rewound => Why::Rewound,
                why => why,
            };
            return give_up(pool, why);
        }
        if !pool.held {
            return Action::Hold;
        }
        Action::AskAnew
    };
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
            if assist_due(carried, asked_ms, seen.assisted_ms, seen.now_ms) {
                Action::Assist
            } else {
                Action::Wait
            }
        }
        Reply::Done(commit) => match seen.snap {
            None => Action::Look,
            Some(SnapSeen::Ours(at)) if at == *commit => Action::Done(at),
            Some(_) => Action::Stop(Stop::DoneNotSeen),
        },
        Reply::Expired | Reply::Withdrawn => look(&|| anew(false)),
        Reply::Unknown => match asked {
            None if pool.held && broken.is_none() && !moved => Action::Ask,
            _ => anew(false),
        },
        Reply::Refused(code, _) => match code {
            Code::Attachment | Code::State | Code::Taken => look(&|| anew(false)),
            Code::Rewound => anew(true),
            Code::Busy => Action::Wait,
            Code::Asked => Action::Withdraw,
            Code::Stopped | Code::Ended | Code::NoAnswer | Code::NoUpload => away(),
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
