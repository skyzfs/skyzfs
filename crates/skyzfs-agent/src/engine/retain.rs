//! Retention (M2.7b-2): the names published past `--keep`, and the
//! names given up past `--abandoned-keep-s`, leave the record, and so do
//! what they left: `snap/<t>` in object storage and each member's `@t`
//! on the pool.
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
//! a restart begins it again at the first name, since the record does
//! not keep when each was tried (no save at each round; this module's
//! reading, confirmed by the lead). So a process restarted more often
//! than every `STUCK_MAX / STUCK_STEPS` rounds retries only the first
//! names by name; this has no other effect. A name leaves with `retired` once its destroy succeeds.
//! At most [`STUCK_MAX`] are
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
//! A round does at most [`STEPS_MAX`] steps (an ask of the daemon, or
//! the destroys of one name), so that a backlog after a long stop is
//! worked off over rounds and does not hold one round up.
//!
//! **Conservative readings** where the rulings are silent: a member's
//! `@t` left by a person who destroyed `pool@t` by hand is not seen
//! (the read of the members goes through `pool@t`), so it is left; a
//! name stopped as [`Retire::Other`] stays in the record, counted
//! among those being retired, until a person settles it; and the names
//! published and given up are no longer dropped to make room: a publish
//! is not begun without room for its end ([`crate::record::Record::room`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use skyzfs_control::{Code, Reply, Request, Retire as RetireRequest};

use super::{Agent, Clock, Daemon, Effect, Error, Keep, Outcome};
use crate::plan::Report;
use crate::record::{Left, Member, Record, Retire, Stuck, Why, KEEP_MAX, RETIRING_MAX, STUCK_MAX};
use crate::zfs::Zfs;

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
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            keep: KEEP_DEFAULT,
            abandoned_keep: ABANDONED_KEEP_DEFAULT,
            told: BTreeMap::new(),
            stuck_last: None,
        }
    }
}

/// What a name given up past its time comes to.
enum Due {
    /// It leaves, with no line.
    Drop,
    /// Its members are destroyed (a person's give-up looked up first).
    Destroy { members: Vec<Member>, look: bool },
    /// It leaves with a `forgotten` line.
    Forget,
}

impl<Z: Zfs, D: Daemon, K: Keep, C: Clock> Agent<Z, D, K, C> {
    /// Keep the newest `keep` names published; at most
    /// [`crate::record::KEEP_MAX`] (`--keep`).
    pub fn set_keep(&mut self, keep: usize) {
        self.retention.keep = keep.min(KEEP_MAX);
    }

    /// Keep a name given up for `keep` (`--abandoned-keep-s`).
    pub fn set_abandoned_keep(&mut self, keep: Duration) {
        self.retention.abandoned_keep = keep;
    }

    /// One round of retention (the docs of the module `retain`). `skyzfs-agent run`
    /// calls it after a round that touched the pool and was not failed.
    /// An error is one of saving the record, or a stop of the tests:
    /// what the daemon and ZFS refuse is told and tried again.
    pub fn retain(&mut self) -> Result<(), Error> {
        self.at.clear();
        self.retry_stuck()?;
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
                Retire::Ref => {
                    steps += 1;
                    self.retire_ref(&name)?;
                }
                Retire::Members => {
                    steps += 1;
                    self.retire_members(&name)?;
                }
            }
        }
        self.retain_given_up(&mut steps)?;
        self.at.clear();
        Ok(())
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
    fn retire_ref(&mut self, name: &str) -> Result<(), Error> {
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
                self.retire_members(name)
            }
            Ok(Reply::Refused(Code::Other, text)) => {
                self.commit(Effect::Retired, |r| set_retire(r, name, Retire::Other))?;
                self.say_other(name, Some(&text));
                Ok(())
            }
            Ok(Reply::Refused(code, text)) => {
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
    fn retire_members(&mut self, name: &str) -> Result<(), Error> {
        let Some(members) = self
            .record
            .published
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.members.clone())
        else {
            return Ok(());
        };
        if self.destroy_members(name, &members)? {
            self.told(name.to_string(), Outcome::Retired);
            self.commit(Effect::Retain, |r| r.published.retain(|p| p.name != name))?;
        } else {
            self.stick(name, members)?;
        }
        Ok(())
    }

    /// Try again the destroy of at most [`STUCK_STEPS`] names stuck, in
    /// turn by name from the one after the last tried; one that
    /// succeeds is told `retired` and leaves.
    fn retry_stuck(&mut self) -> Result<(), Error> {
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
            self.at.clone_from(&name);
            self.retention.stuck_last = Some(name.clone());
            if self.destroy_members(&name, &members)? {
                self.told(name.clone(), Outcome::Retired);
                self.commit(Effect::Retain, |r| r.stuck.retain(|s| s.name != name))?;
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
    /// children first and `pool@<name>` last. Whether all are gone; a
    /// read or a destroy that fails is told.
    fn destroy_members(&mut self, name: &str, members: &[Member]) -> Result<bool, Error> {
        let ours: BTreeSet<u64> = members.iter().map(|m| m.snapshot_guid).collect();
        let pool = self.pool.clone();
        let read = match self.zfs.members(&pool, name) {
            Ok(read) => read.unwrap_or_default(),
            Err(e) => {
                self.say(
                    name,
                    Report::NotRetired(format!("the members' @{name} not read: {e}")),
                );
                return Ok(false);
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
            self.point(Effect::DestroyMember, false)?;
            let destroyed = self.zfs.destroy_one(&pool, &member.name, name);
            self.point(Effect::DestroyMember, true)?;
            if let Err(e) = destroyed {
                self.say(
                    name,
                    Report::NotRetired(format!("zfs destroy {}@{name}: {e}", member.name)),
                );
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The names given up past their time, oldest first: those that left
    /// nothing leave in one save; the others, within the round's steps.
    fn retain_given_up(&mut self, steps: &mut usize) -> Result<(), Error> {
        let now = self.clock.now_ms();
        let bound = u64::try_from(self.retention.abandoned_keep.as_millis()).unwrap_or(u64::MAX);
        let mut due = Vec::new();
        for a in &self.record.abandoned {
            let past = now.saturating_sub(a.at_ms) >= bound;
            match (&a.left, past) {
                (Left::Unknown, false) => due.push((a.name.clone(), None)),
                (_, false) => {}
                (Left::Nothing, true) => due.push((a.name.clone(), Some(Due::Drop))),
                (Left::Members(members), true) => due.push((
                    a.name.clone(),
                    Some(Due::Destroy {
                        members: members.clone(),
                        look: a.why == Why::Person,
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
                Some(Due::Forget) => {
                    *steps += 1;
                    self.forget_given_up(&name, "no member guids; destroy by hand")?;
                }
                Some(Due::Destroy { members, look }) => {
                    *steps += 1;
                    if look && !self.looked_absent(&name)? {
                        continue;
                    }
                    if self.destroy_members(&name, &members)? {
                        self.told(name.clone(), Outcome::Retired);
                        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))?;
                    } else {
                        self.stick(&name, members)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Look `snap/<name>` up before the members of a name a person gave
    /// up are destroyed: whether it is absent. Where it is there, the
    /// name leaves with its local snapshot, for a person; where the look
    /// is not answered, it is told and tried again.
    fn looked_absent(&mut self, name: &str) -> Result<bool, Error> {
        self.point(Effect::LookGivenUp, false)?;
        let answer = self.daemon.ask(&Request::Look(name.to_string()), &[]);
        self.point(Effect::LookGivenUp, true)?;
        match answer {
            Ok(Reply::Absent) => Ok(true),
            Ok(Reply::Snap(_) | Reply::Unreadable(_)) => {
                self.forget_given_up(
                    name,
                    "snap/t exists though given up by a person; the local snapshot is kept, \
                     judge by hand",
                )?;
                Ok(false)
            }
            Ok(other) => {
                self.say(
                    name,
                    Report::NotRetired(format!("look answered `{}`", other.line())),
                );
                Ok(false)
            }
            Err(e) => {
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
