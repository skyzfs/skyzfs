//! Retention (M2.7b-2): the names published past `--keep`, and the
//! names given up past `--abandoned-keep-s`, leave the record, and so do
//! what they left: `snap/<t>` in object storage and each member's `@t`
//! on the pool.
//!
//! The rulings cited as ?1 to ?4 are the lead's answers to the
//! questions so numbered in this work package's plan, listed in the
//! description of skyzfs/skyzfs#179: ?1 a member's `@t` destroyed by
//! itself, by GUID; ?2 `snap/<t>` naming another commit left for a
//! person; ?3 a name a person gave up looked up first; ?4 a name with
//! no member GUIDs forgotten and nothing destroyed.
//!
//! **A name published** is kept while it is among the newest
//! [`Retention::keep`] ([`KEEP_DEFAULT`], at most
//! [`crate::record::KEEP_MAX`]). An older one is marked to be retired
//! ([`Retire::Ref`]), at most [`RETIRING_MAX`] at a time, in one save.
//! Then, each recorded before the next:
//!
//! 1. `retire name=<t> commit=<c>` asks the daemon to retire `snap/<t>`
//!    where it names the publish's commit. `retired` or `gone` (no
//!    `snap/<t>`, or retired before) moves it on ([`Retire::Members`]).
//!    `refused other`, `snap/<t>` names another commit, stops it for a
//!    person: the local snapshot is kept and no ref is touched
//!    ([`Retire::Other`], the lead's ruling ?2). Any other answer, or
//!    none, is told and asked again at the next round.
//! 2. Each member's `@t` that the pool has with the GUID recorded is
//!    destroyed, one by one, with no `-r`, `-R` or `-d` (the ruling ?1):
//!    an `@t` of another GUID (a snapshot a person made under the name,
//!    or of a dataset made since) is left. The children go before their
//!    parents, `pool@t` last, so that a stop half way leaves `pool@t`,
//!    through which the rest is read again. A destroy that fails is
//!    told, with its reason, once in a process and again only when the
//!    reason changes, and the name moves to the names stuck (below).
//!    Only ZFS's own answer counts as a failed destroy: a destroy, or a
//!    read of the members, that the host does not run to its end
//!    ([`ZfsError::NotRun`]: killed at its limit, [`Limits::change`]
//!    for a destroy and [`Limits::read`] for the read, both scaled by
//!    the pool's count, or held back by commands left running) is told
//!    and tried again at the next round, and the name stays where it is
//!    (the lead's ruling on decision 4 of #179's review).
//! 3. The outcome `retired` is told, and then the name leaves the
//!    record. A kill between the two tells it again: a monitor treats
//!    `retired` as repeatable.
//!
//! **A name stuck** (the lead's ruling on #179): a name whose ref is
//! retired, or a name given up past its time, whose members' destroy
//! fails (a clone of the snapshot, a hold of another's) leaves the
//! names published or given up for [`Record::stuck`], in one save, at
//! its first failed destroy, so that it counts for no room
//! ([`Record::room`]) and cannot stop the publishes. At most
//! [`STUCK_STEPS`] names stuck are tried again in a round, besides the
//! round's steps, in turn by name from the one after the last tried
//! (round-robin from the one tried longest ago, the lead's ruling), so
//! that at the cap every name is tried within
//! `STUCK_MAX / STUCK_STEPS` (8) rounds and a slow pool does not
//! stretch a round far past its tick. The turn is kept in the process:
//! a new process begins it again at the first name, since the record
//! does not keep when each was tried (no save at each round; this
//! module's reading, confirmed by the lead). [`Agent::restart`] within
//! the process keeps it. So with `n` names stuck, a process that is
//! started anew every `k` rounds retries only the first `min(n, 8k)`
//! by name, and the later ones wait while the first stay stuck: their
//! snapshots stay longer, and one can reach the cap's forget untried.
//! The room and the publishes are not touched by it. A name leaves
//! with `retired` once its destroy succeeds. At most [`STUCK_MAX`] are
//! kept: past that the oldest is forgotten with the line
//! `forgotten <t> (destroy failed; destroy by hand)`, told before the
//! save that drops it. A name given up moves there too, though the
//! ruling names the retired ones: it holds no ref, and a destroy of its
//! that keeps failing would fill the names given up as surely (this
//! module's reading).
//!
//! **A name given up** is kept for [`Retention::abandoned_keep`]
//! ([`ABANDONED_KEEP_DEFAULT`]), and then, by what it left
//! ([`Left`]):
//!
//! - [`Left::Nothing`]: it leaves the record, with no line.
//! - [`Left::Members`]: each member's `@t` of the GUID recorded is
//!   destroyed as above, `retired` is told, and the name leaves. One a
//!   person gave up ([`Why::Person`]) is looked up first, since its
//!   request may have landed after the give-up: where `snap/<t>` is
//!   there (or there and unreadable), nothing is destroyed, no ref is
//!   touched, and the name leaves with the line `forgotten <t>` saying
//!   so, for a person (the ruling ?3). The look's commit retires
//!   nothing.
//! - [`Left::Unknown`], a give-up with no GUIDs recorded (from a record
//!   of version 2, or before a take was read): told once in a process
//!   while it is kept; then it leaves with the line
//!   `forgotten <t> (no member guids; destroy by hand)`: nothing is
//!   destroyed (the ruling ?4).
//!
//! **Room before its time** (the lead's ruling on M1 of #179's review):
//! where the names given up alone keep the next begin from its room
//! ([`Record::abandoned_over`]), as many of the oldest leave at once as
//! if past their keep, each looked up first as one a person gave up: a
//! `snap/<t>` that is there keeps the local snapshot and leaves the name
//! with a `forgotten` line, for a person; one that is absent has its
//! members destroyed by GUID. So the uploader stopped for hours, every
//! publish given up, does not stop the publishes after it is back, even
//! with names marked to be retired: the first `retire` refused `stopped`
//! ends the round's retires, so they cost the round one step and the
//! early leave keeps the rest (N1 of #179's review; the lead's ruling).
//! [`Left::Nothing`] leaves with no line and [`Left::Unknown`] with its
//! `forgotten` line, as past their keep.
//!
//! A round does at most [`STEPS_MAX`] steps (an ask of the daemon, or
//! the destroys of one name), so that a backlog after a long stop is
//! worked off over rounds and does not hold one round up. A `retire`
//! or a `look` that the daemon does not answer (`no-answer`, or no
//! answer at all) ends the round's asks of the daemon: the next round
//! asks again, so that one round waits for at most one answer limit
//! (L2 of #179's review). A `retire` refused `stopped` ends the round's
//! retires the same way, but not its looks (N1). Before each step, each name stuck tried and
//! each call to ZFS, the round asks whether an end was asked and
//! whether the failure note has news, and ends quietly if so: a volume
//! lost during retention is stepped on after the call under way, not
//! after the round's steps (M2 of #179's review, and Codex's P1).
//!
//! **A name stopped as [`Retire::Other`]** stays in the record, counted
//! among those being retired, until a person settles it; it is not asked
//! again. `snap/<t>` names a commit that this agent did not publish
//! under the name, so a person decides which is right (the lead's ruling
//! on decision 3 of #179's review):
//!
//! - the line `stopped <t> retire refused other: ...` says so, and the
//!   daemon's request log line `<t> refused other commit=H found=H2`
//!   names both commits;
//! - to keep `snap/<t>` as it is, the local `pool@<t>` and its members'
//!   `@<t>` are destroyed by hand where they are not wanted;
//! - to drop it, `snap/<t>` is deleted with the tools of object storage;
//! - either way the name then leaves the record with
//!   `skyzfs-agent forget --name <t>`, which follows in a later pull
//!   request: one-shot, refused while `run` holds the directory (exit
//!   8), it drops the name from the record and touches neither the pool
//!   nor the ref. Until it is there the name stays, one of the
//!   [`RETIRING_MAX`] being retired.
//!
//! **Names published before the record's version 3** (an agent of
//! version 2, upgraded) are not in the record as published, so they
//! are never retired: a person destroys them, their refs and their
//! snapshots, by hand; a later package may adopt them (the lead's
//! ruling on decision 6 of #179's review).
//!
//! **Conservative readings** where the rulings are silent: a member's
//! `@t` left by a person who destroyed `pool@t` by hand is not seen
//! (the read of the members goes through `pool@t`), so it is left; and
//! the names published and given up are no longer dropped to make
//! room: a publish is not begun without room for its end
//! ([`crate::record::Record::room`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use skyzfs_control::{Code, Reply, Request, Retire as RetireRequest};

use super::{Agent, Clock, Daemon, Effect, Error, Keep, Outcome};
use crate::plan::Report;
use crate::record::{Left, Member, Record, Retire, Stuck, Why, KEEP_MAX, RETIRING_MAX, STUCK_MAX};
#[cfg(doc)]
use crate::zfs::cli::Limits;
use crate::zfs::{Zfs, ZfsError};

/// The names published kept by default (`--keep`).
pub const KEEP_DEFAULT: usize = 60;
/// How long a name given up is kept by default (`--abandoned-keep-s`).
pub const ABANDONED_KEEP_DEFAULT: Duration = Duration::from_secs(24 * 60 * 60);
/// The steps of retention in one round.
pub const STEPS_MAX: usize = 8;
/// The names stuck tried again in one round, at most.
pub const STUCK_STEPS: usize = 8;
/// Why the oldest name stuck is forgotten past [`STUCK_MAX`].
pub const STUCK_FORGOTTEN: &str = "destroy failed; destroy by hand";

/// Retention's settings, and what it told in this process.
#[derive(Debug, Clone)]
pub(super) struct Retention {
    /// The names published kept.
    pub keep: usize,
    /// How long a name given up is kept.
    pub abandoned_keep: Duration,
    /// What was last told of each name: told again only when it changes.
    /// Forgotten at a restart, and for a name that left the record.
    pub told: BTreeMap<String, String>,
    /// The name stuck last tried in this process: the next round's
    /// turn begins after it.
    pub stuck_last: Option<String>,
    /// In this round, a `retire` or a `look` the daemon did not answer:
    /// the round asks the daemon nothing more (L2 of #179's review).
    pub silent: bool,
    /// In this round, a `retire` refused `stopped`: the uploader steps
    /// no more, so the round asks no more retires, and the names marked
    /// cost it one step and not [`STEPS_MAX`] (N1 of #179's review; the
    /// lead's ruling). Its looks are still asked.
    pub stopped: bool,
    /// In this round, an end was asked or the failure note had news:
    /// the round takes no further step (M2 of #179's review).
    pub halted: bool,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            keep: KEEP_DEFAULT,
            abandoned_keep: ABANDONED_KEEP_DEFAULT,
            told: BTreeMap::new(),
            stuck_last: None,
            silent: false,
            stopped: false,
            halted: false,
        }
    }
}

/// How a destroy of a name's members ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destroyed {
    /// Each member's `@t` of the name's GUIDs is gone.
    All,
    /// A read or a destroy failed, and was told: the name is stuck.
    Failed,
    /// A read or a destroy was not run to its end ([`ZfsError::NotRun`]),
    /// and was told: the name stays where it is and is tried again (the
    /// lead's ruling on decision 4 of #179's review: only ZFS's own
    /// error moves a name to the names stuck).
    NotRun,
    /// An end was asked, or the failure note had news, before a call:
    /// nothing was told, and the next round goes on.
    Halted,
}

/// What a name given up past its time comes to.
enum Due {
    /// It leaves, with no line.
    Drop,
    /// Its members are destroyed, `snap/T` looked up first where `look`
    /// (a person's give-up, or one retired early for room); `person`
    /// says which, for the line where `snap/T` is there.
    Destroy {
        members: Vec<Member>,
        look: bool,
        person: bool,
    },
    /// It leaves with a `forgotten` line.
    Forget,
}

impl<Z: Zfs, D: Daemon, K: Keep, C: Clock> Agent<Z, D, K, C> {
    /// Keep the newest `keep` names published; at most
    /// [`crate::record::KEEP_MAX`] (`--keep`).
    ///
    /// `0` is taken here and retires every name published: it is for
    /// tests only. The command line takes `--keep` from 1 (L4 of the
    /// review of #179; the lead ruled to keep it so).
    pub fn set_keep(&mut self, keep: usize) {
        self.retention.keep = keep.min(KEEP_MAX);
    }

    /// Keep a name given up for `keep` (`--abandoned-keep-s`).
    pub fn set_abandoned_keep(&mut self, keep: Duration) {
        self.retention.abandoned_keep = keep;
    }

    /// One round of retention, with no end asked ([`Agent::retain_while`]).
    pub fn retain(&mut self) -> Result<(), Error> {
        self.retain_while(&mut || true)
    }

    /// One round of retention (the docs of the module `retain`).
    /// `skyzfs-agent run` calls it in each round, after the begin,
    /// whether the begin failed or not, and before the settle; not while
    /// the engine waits to be loaded anew after an error (a nit of
    /// #179's review). Before each step, each name
    /// stuck tried and each call to ZFS, it asks `go` and the failure
    /// note, and it ends the round quietly where an end was asked or
    /// the note had news: the next round reads the note first (M2 of
    /// #179's review, and Codex's P1 there). An error is one of saving
    /// the record, or a stop of the tests: what the daemon and ZFS
    /// refuse is told and tried again.
    pub fn retain_while(&mut self, go: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        self.at.clear();
        self.retention.silent = false;
        self.retention.stopped = false;
        self.retention.halted = false;
        let result = self.retain_steps(go);
        self.at.clear();
        result
    }

    /// Whether the round must take no further step: an end was asked,
    /// or the failure note has news. Once true, true for the round.
    fn halt(&mut self, go: &mut dyn FnMut() -> bool) -> bool {
        if !self.retention.halted && (!go() || self.note_came()) {
            self.retention.halted = true;
        }
        self.retention.halted
    }

    fn retain_steps(&mut self, go: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        // No ask before the marks: they are a save, and each step below
        // asks before it goes.
        self.retry_stuck(go)?;
        self.mark_retiring()?;
        let mut steps = 0;
        let marked: Vec<(String, Retire)> = self
            .record
            .published
            .iter()
            .filter_map(|p| p.retire.map(|r| (p.name.clone(), r)))
            .collect();
        for (name, retire) in marked {
            self.at.clone_from(&name);
            match retire {
                Retire::Other => self.say_other(&name, None),
                _ if steps >= STEPS_MAX => {}
                Retire::Ref if self.retention.silent || self.retention.stopped => {}
                _ if self.halt(go) => return Ok(()),
                Retire::Ref => {
                    steps += 1;
                    self.retire_ref(&name, go)?;
                }
                Retire::Members => {
                    steps += 1;
                    self.retire_members(&name, go)?;
                }
            }
        }
        // No ask here either: the drops of names given up are a save,
        // and each step there asks before it goes.
        self.retain_given_up(&mut steps, go)
    }

    /// Mark the oldest names published past `keep` to be retired, within
    /// [`RETIRING_MAX`], in one save.
    fn mark_retiring(&mut self) -> Result<(), Error> {
        let published = &self.record.published;
        let kept = published.iter().filter(|p| p.retire.is_none()).count();
        let retiring = published.len() - kept;
        let over = kept.saturating_sub(self.retention.keep);
        let n = over.min(RETIRING_MAX.saturating_sub(retiring));
        if n == 0 {
            return Ok(());
        }
        let names: BTreeSet<String> = published
            .iter()
            .filter(|p| p.retire.is_none())
            .take(n)
            .map(|p| p.name.clone())
            .collect();
        self.commit(Effect::Retiring, |r| {
            for p in &mut r.published {
                if names.contains(&p.name) {
                    p.retire = Some(Retire::Ref);
                }
            }
        })
    }

    /// Ask the daemon to retire `snap/<name>`.
    fn retire_ref(&mut self, name: &str, go: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let Some(commit) = self
            .record
            .published
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.commit)
        else {
            return Ok(());
        };
        let request = Request::Retire(RetireRequest {
            name: name.to_string(),
            commit,
        });
        self.point(Effect::Retire, false)?;
        let answer = self.daemon.ask(&request, &[]);
        self.point(Effect::Retire, true)?;
        match answer {
            Ok(Reply::Retired | Reply::Gone) => {
                self.commit(Effect::Retired, |r| set_retire(r, name, Retire::Members))?;
                self.retire_members(name, go)
            }
            Ok(Reply::Refused(Code::Other, text)) => {
                self.commit(Effect::Retired, |r| set_retire(r, name, Retire::Other))?;
                self.say_other(name, Some(&text));
                Ok(())
            }
            Ok(Reply::Refused(code, text)) => {
                match code {
                    Code::NoAnswer => self.retention.silent = true,
                    Code::Stopped => self.retention.stopped = true,
                    _ => {}
                }
                self.say(
                    name,
                    Report::NotRetired(format!("retire refused {}: {text}", code.word())),
                );
                Ok(())
            }
            Ok(other) => {
                self.say(
                    name,
                    Report::NotRetired(format!("retire answered `{}`", other.line())),
                );
                Ok(())
            }
            Err(e) => {
                self.retention.silent = true;
                self.say(
                    name,
                    Report::NotRetired(format!("retire not answered: {e}")),
                );
                Ok(())
            }
        }
    }

    /// Destroy the members' `@<name>` of a name published whose ref is
    /// retired, and let it leave.
    fn retire_members(&mut self, name: &str, go: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let Some(members) = self
            .record
            .published
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.members.clone())
        else {
            return Ok(());
        };
        match self.destroy_members(name, &members, go)? {
            Destroyed::All => {
                self.told(name.to_string(), Outcome::Retired);
                self.commit(Effect::Retain, |r| r.published.retain(|p| p.name != name))?;
            }
            Destroyed::Failed => self.stick(name, members)?,
            Destroyed::NotRun | Destroyed::Halted => {}
        }
        Ok(())
    }

    /// Try again the destroy of at most [`STUCK_STEPS`] names stuck, in
    /// turn by name from the one after the last tried; one that
    /// succeeds is told `retired` and leaves.
    fn retry_stuck(&mut self, go: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        let mut stuck: Vec<(String, Vec<Member>)> = self
            .record
            .stuck
            .iter()
            .map(|s| (s.name.clone(), s.members.clone()))
            .collect();
        stuck.sort_by(|a, b| a.0.cmp(&b.0));
        let first = match &self.retention.stuck_last {
            Some(last) => stuck.iter().position(|(n, _)| n > last).unwrap_or(0),
            None => 0,
        };
        stuck.rotate_left(first);
        stuck.truncate(STUCK_STEPS);
        for (name, members) in stuck {
            // `destroy_members` asks before its first call to ZFS.
            self.at.clone_from(&name);
            match self.destroy_members(&name, &members, go)? {
                Destroyed::All => {
                    self.retention.stuck_last = Some(name.clone());
                    self.told(name.clone(), Outcome::Retired);
                    self.commit(Effect::Retain, |r| r.stuck.retain(|s| s.name != name))?;
                }
                Destroyed::Failed | Destroyed::NotRun => {
                    self.retention.stuck_last = Some(name.clone())
                }
                Destroyed::Halted => break,
            }
        }
        Ok(())
    }

    /// Move `name`, whose members' destroy failed, from the names
    /// published or given up to the names stuck, in one save; past
    /// [`STUCK_MAX`] the oldest stuck are forgotten, told first.
    fn stick(&mut self, name: &str, members: Vec<Member>) -> Result<(), Error> {
        let at_ms = self.clock.now_ms();
        let over = (self.record.stuck.len() + 1).saturating_sub(STUCK_MAX);
        let forgotten: BTreeSet<String> = self
            .record
            .stuck
            .iter()
            .take(over)
            .map(|s| s.name.clone())
            .collect();
        for old in &forgotten {
            self.told(old.clone(), Outcome::Forgotten(STUCK_FORGOTTEN.to_string()));
        }
        let name = name.to_string();
        self.commit(Effect::Retain, |r| {
            r.published.retain(|p| p.name != name);
            r.abandoned.retain(|a| a.name != name);
            r.stuck.retain(|s| !forgotten.contains(&s.name));
            r.stuck.push(Stuck {
                name,
                members,
                at_ms,
            });
        })
    }

    /// Destroy each `@<name>` of the pool whose GUID is one of `members`',
    /// children first and `pool@<name>` last ([`Destroyed`]); a read or a
    /// destroy that fails is told. Before each call to ZFS, `go` and the
    /// failure note are asked ([`Agent::retain_while`]).
    fn destroy_members(
        &mut self,
        name: &str,
        members: &[Member],
        go: &mut dyn FnMut() -> bool,
    ) -> Result<Destroyed, Error> {
        let ours: BTreeSet<u64> = members.iter().map(|m| m.snapshot_guid).collect();
        let pool = self.pool.clone();
        if self.halt(go) {
            return Ok(Destroyed::Halted);
        }
        let read = match self.zfs.members(&pool, name) {
            Ok(read) => read.unwrap_or_default(),
            Err(e) => {
                self.say(
                    name,
                    Report::NotRetired(format!("the members' @{name} not read: {e}")),
                );
                return Ok(not_run_or_failed(&e));
            }
        };
        let mut doomed: Vec<Member> = read
            .into_iter()
            .filter(|m| ours.contains(&m.snapshot_guid))
            .collect();
        doomed.sort_by(|a, b| {
            let depth = |m: &Member| m.name.matches('/').count();
            (Reverse(depth(a)), &b.name).cmp(&(Reverse(depth(b)), &a.name))
        });
        for member in doomed {
            if self.halt(go) {
                return Ok(Destroyed::Halted);
            }
            self.point(Effect::DestroyMember, false)?;
            let destroyed = self.zfs.destroy_one(&pool, &member.name, name);
            self.point(Effect::DestroyMember, true)?;
            if let Err(e) = destroyed {
                self.say(
                    name,
                    Report::NotRetired(format!("zfs destroy {}@{name}: {e}", member.name)),
                );
                return Ok(not_run_or_failed(&e));
            }
        }
        Ok(Destroyed::All)
    }

    /// The names given up past their time, oldest first: those that left
    /// nothing leave in one save; the others, within the round's steps.
    fn retain_given_up(
        &mut self,
        steps: &mut usize,
        go: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let now = self.clock.now_ms();
        let bound = u64::try_from(self.retention.abandoned_keep.as_millis()).unwrap_or(u64::MAX);
        // The oldest names given up that alone keep the next begin from
        // its room leave early, as if past their keep, with `snap/T`
        // looked up first (the lead's ruling on M1 of #179's review): a
        // few hours of give-ups do not stop the publishes for a day.
        let mut oldest: Vec<(u64, usize)> = self
            .record
            .abandoned
            .iter()
            .enumerate()
            .map(|(i, a)| (a.at_ms, i))
            .collect();
        oldest.sort_unstable();
        let early: BTreeSet<usize> = oldest
            .iter()
            .take(self.record.abandoned_over())
            .map(|&(_, i)| i)
            .filter(|&i| {
                self.record
                    .abandoned
                    .get(i)
                    .is_some_and(|a| now.saturating_sub(a.at_ms) < bound)
            })
            .collect();
        let mut due = Vec::new();
        for (i, a) in self.record.abandoned.iter().enumerate() {
            let early = early.contains(&i);
            let past = early || now.saturating_sub(a.at_ms) >= bound;
            match (&a.left, past) {
                (Left::Unknown, false) => due.push((a.name.clone(), None)),
                (_, false) => {}
                (Left::Nothing, true) => due.push((a.name.clone(), Some(Due::Drop))),
                (Left::Members(members), true) => due.push((
                    a.name.clone(),
                    Some(Due::Destroy {
                        members: members.clone(),
                        look: early || a.why == Why::Person,
                        person: a.why == Why::Person,
                    }),
                )),
                (Left::Unknown, true) => due.push((a.name.clone(), Some(Due::Forget))),
            }
        }
        let dropped: BTreeSet<String> = due
            .iter()
            .filter(|(_, d)| matches!(d, Some(Due::Drop)))
            .map(|(name, _)| name.clone())
            .collect();
        if !dropped.is_empty() {
            self.commit(Effect::Retain, |r| {
                r.abandoned.retain(|a| !dropped.contains(&a.name));
            })?;
        }
        for (name, due) in due {
            self.at.clone_from(&name);
            match due {
                None => self.say(&name, Report::NoGuids),
                Some(Due::Drop) => {}
                Some(_) if *steps >= STEPS_MAX => {}
                Some(Due::Destroy { look: true, .. }) if self.retention.silent => {}
                Some(_) if self.halt(go) => return Ok(()),
                Some(Due::Forget) => {
                    *steps += 1;
                    self.forget_given_up(&name, "no member guids; destroy by hand")?;
                }
                Some(Due::Destroy {
                    members,
                    look,
                    person,
                }) => {
                    *steps += 1;
                    if look && !self.looked_absent(&name, person)? {
                        continue;
                    }
                    match self.destroy_members(&name, &members, go)? {
                        Destroyed::All => {
                            self.told(name.clone(), Outcome::Retired);
                            self.commit(Effect::Retain, |r| {
                                r.abandoned.retain(|a| a.name != name)
                            })?;
                        }
                        Destroyed::Failed => self.stick(&name, members)?,
                        Destroyed::NotRun => {}
                        Destroyed::Halted => return Ok(()),
                    }
                }
            }
        }
        Ok(())
    }

    /// Look `snap/<name>` up before the members of a name given up are
    /// destroyed, where a person gave it up (`person`) or it leaves early
    /// for room: whether it is absent. Where it is there, the name leaves
    /// with its local snapshot, for a person; where the look is not
    /// answered, it is told and tried again.
    fn looked_absent(&mut self, name: &str, person: bool) -> Result<bool, Error> {
        self.point(Effect::LookGivenUp, false)?;
        let answer = self.daemon.ask(&Request::Look(name.to_string()), &[]);
        self.point(Effect::LookGivenUp, true)?;
        match answer {
            Ok(Reply::Absent) => Ok(true),
            Ok(Reply::Snap(_) | Reply::Unreadable(_)) => {
                let why = if person {
                    "snap/t exists though given up by a person; the local snapshot is kept, \
                     judge by hand"
                } else {
                    "snap/t exists though given up; the local snapshot is kept, judge by hand"
                };
                self.forget_given_up(name, why)?;
                Ok(false)
            }
            Ok(other) => {
                if matches!(other, Reply::Refused(Code::NoAnswer, _)) {
                    self.retention.silent = true;
                }
                self.say(
                    name,
                    Report::NotRetired(format!("look answered `{}`", other.line())),
                );
                Ok(false)
            }
            Err(e) => {
                self.retention.silent = true;
                self.say(name, Report::NotRetired(format!("look not answered: {e}")));
                Ok(false)
            }
        }
    }

    /// A name given up leaves with a `forgotten` line: told first.
    fn forget_given_up(&mut self, name: &str, why: &str) -> Result<(), Error> {
        self.told(name.to_string(), Outcome::Forgotten(why.to_string()));
        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))
    }

    /// Tell `report` of `name` unless it was the last told of it.
    fn say(&mut self, name: &str, report: Report) {
        let key = format!("{report:?}");
        if self.retention.told.get(name) == Some(&key) {
            return;
        }
        self.retention.told.insert(name.to_string(), key);
        if let Some(sink) = &mut self.sink {
            sink(name, &report);
        }
        self.reports.push((name.to_string(), report));
    }

    /// Stop a name whose `snap/<t>` names another commit for a person:
    /// once in a process, with the daemon's text where it was just said.
    fn say_other(&mut self, name: &str, text: Option<&str>) {
        const KEY: &str = "other";
        if self.retention.told.get(name).map(String::as_str) == Some(KEY) {
            return;
        }
        self.retention
            .told
            .insert(name.to_string(), KEY.to_string());
        let why = match text {
            Some(text) => format!("retire refused other: {text}"),
            None => format!(
                "retire refused other: snap/{name} names another commit than this publish's; \
                 the local snapshot is kept"
            ),
        };
        self.told(name.to_string(), Outcome::Stopped(why));
    }
}

/// Set the retirement of the name published `name`.
fn set_retire(record: &mut Record, name: &str, retire: Retire) {
    if let Some(p) = record.published.iter_mut().find(|p| p.name == name) {
        p.retire = Some(retire);
    }
}

#[cfg(test)]
mod tests;

/// How a destroy that met `e` ended: not run where the host did not run
/// its command to its end, failed where ZFS answered.
fn not_run_or_failed(e: &ZfsError) -> Destroyed {
    match e {
        ZfsError::NotRun(_) => Destroyed::NotRun,
        _ => Destroyed::Failed,
    }
}
