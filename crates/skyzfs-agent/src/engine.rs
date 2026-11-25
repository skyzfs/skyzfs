//! The engine (the plan of M2.6, §4): it begins a publish, and for each
//! publish under way it looks at the pool, the daemon and `snap/<t>`,
//! asks the planner ([`crate::plan`]) for the next action, does it, and
//! records what it changed. Every rule is the planner's; the engine only
//! sees and acts, so a crash anywhere is met by planning from what is
//! seen ([`Agent::settle`] after [`Agent::new`]).
//!
//! # The steps of one publish
//!
//! 1. The premise ([`crate::zfs::premise`]), before any snapshot; the
//!    pool and its leaf enrolled at the first publish (the lead's Q4).
//! 2. `cuts`, with `load_guid` read before it (rule (L)).
//! 3. Recorded [`Phase::Intent`].
//! 4. The channel program, with the count read anew and recorded first.
//! 5. The manifest's file, then recorded [`Phase::Taken`].
//! 6. The hold, recorded as believed placed before it is placed.
//! 7. The members, the hold and `load_guid` seen as recorded: recorded
//!    [`Phase::Held`].
//! 8. `publish`, answered `accepted`: recorded [`Phase::Asked`], with
//!    `held_until` from this answer.
//! 9. `progress` and `cuts` once a second, and the agent's sync task
//!    ([`Zfs::assist`]) where the planner says.
//! 10. `done`, read with `look`: recorded [`Phase::Done`].
//! 11. The hold released, then the publish and its manifest removed.
//!
//! [`Action::AskAnew`] reads a count anew in the planner's order ("The
//! rule for AskAnew"): `load_guid`, the snapshot, its hold and its
//! members, `cuts`, `load_guid` again, and records the count (the publish
//! back at [`Phase::Held`]) before it asks.
//!
//! # What the engine remembers between rounds
//!
//! Only within one process, and only as the planner's inputs: the answer
//! to its last request about a publish (a refusal or a withdrawal, kept
//! for the round after a `look` so that the planner sees both, and
//! with what `look` saw for the round after a release, which gives the
//! publish up on them: N8 of #146's review), what `look` saw, when it
//! last helped the publish with a sync, and when the daemon accepted it
//! on the clock that does not step. A restart forgets them, which costs
//! at most a `look`, a sync, or one more hold and ask of a publish
//! refused `stopped`.
//!
//! # Time
//!
//! The record's times are the wall clock's, since they outlive the
//! process. For a publish accepted in this process the planner reads the
//! time as `accepted` plus what the clock that does not step has run
//! since ([`Clock::mono_ms`]): a step of the wall clock neither releases
//! its hold before `held_until` nor holds back its syncs (S4 and S11 of
//! #142's review). A publish's age ([`plan::AGE_MAX`]) is measured the
//! same way from `begun_ms`, for a publish begun in this process, and
//! goes with it when it takes the next name. After a restart the wall
//! clock is all there is to start from: a step forward while the agent
//! was down may release a hold early (the request may still land; its
//! cut holds the snapshot all the same), and a step back keeps it
//! longer. The age of a publish read from the record is anchored on the
//! wall clock once, at the first round that reads it, and runs on the
//! clock that does not step from there (S2 of #144's next look);
//! `held_until` is read on the wall clock until the request is accepted
//! again in this process.
//!
//! # Crash points
//!
//! In its tests the engine can be stopped before and after each effect
//! ([`Point`]): each save of the record, each change to ZFS, each request
//! to the daemon and each file of a manifest. The crash matrix (in
//! `engine/tests.rs`) stops it at every one in turn.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::time::Duration;

use skyzfs_control::{CutsReply, Publish as PublishRequest, Reply, Request, EXPIRY_DEFAULT};

use crate::manifest::{self, Manifest, ManifestError};
use crate::names;
use crate::plan::{self, Action, DaemonSeen, PoolSeen, Report, Seen, SnapSeen, Stop};
use crate::record::{
    Corrupt, Count, Enrolled, Keep, Loaded, Phase, Publish, Record, SaveError, Taken, Why,
    UNDER_WAY_MAX,
};
use crate::zfs::{self, Held, Zfs, ZfsError, HOLD_TAG};

/// How often the engine looks again at a publish that waits (the plan,
/// §4, step 11).
pub const TICK: Duration = Duration::from_secs(1);

/// The daemon, as the agent reaches it: its channel
/// ([`skyzfs_control::ask`]) and its device.
pub trait Daemon {
    /// Ask one request; `manifest` follows a `publish` and is empty
    /// otherwise.
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply>;
    /// The device number (`st_rdev`) of the volume's block device.
    fn device(&mut self) -> io::Result<u64>;
}

/// The wall clock, a clock that does not step, and waiting.
pub trait Clock {
    /// Milliseconds since 1970.
    fn now_ms(&mut self) -> u64;
    /// Milliseconds since a point fixed in this process, on a clock that
    /// the wall clock's steps do not move (`CLOCK_MONOTONIC`).
    fn mono_ms(&mut self) -> u64;
    /// Wait `d`.
    fn sleep(&mut self, d: Duration);
}

/// The system's clocks.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock {
            start: std::time::Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now_ms(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }

    fn mono_ms(&mut self) -> u64 {
        ms(self.start.elapsed())
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// An effect of the engine, at which its tests can stop it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effect {
    /// The record saved with the pool enrolled.
    Enrol,
    /// The record saved with a publish at [`Phase::Intent`].
    Intent,
    /// The channel program.
    Program,
    /// The manifest's file written.
    Manifest,
    /// The record saved at [`Phase::Taken`].
    Taken,
    /// The record saved with the hold believed placed.
    Holding,
    /// `zfs hold`.
    Hold,
    /// The record saved at [`Phase::Held`].
    Held,
    /// The record saved with a count read anew, at [`Phase::Held`].
    Count,
    /// The record saved with the publish's last stall, at
    /// [`Phase::Held`].
    Stalled,
    /// `publish` asked.
    Ask,
    /// The record saved at [`Phase::Asked`].
    Asked,
    /// `withdraw` asked.
    Withdraw,
    /// `look` asked.
    Look,
    /// The agent's sync task ([`Zfs::assist`]).
    Assist,
    /// `zfs release`.
    Release,
    /// The record saved with the hold released.
    Released,
    /// The record saved at [`Phase::Done`].
    Done,
    /// `zfs release` of a hold that no publish under way names, after an
    /// import ([`Agent::settle`]).
    Sweep,
    /// The record saved with the publish removed.
    Forget,
    /// The record saved with the publish given up.
    Abandon,
    /// The manifest's file removed.
    Unlink,
}

/// A point at which the engine's tests can stop it: before or after an
/// effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    /// The effect.
    pub effect: Effect,
    /// After it, or before.
    pub after: bool,
}

/// Called at each [`Point`]; `true` stops the engine there
/// ([`Error::Stopped`]).
pub type Points = Box<dyn FnMut(Point) -> bool>;

/// Why the engine did not do what was asked.
#[derive(Debug)]
pub enum Error {
    /// Its tests stopped it at a [`Point`].
    Stopped,
    /// The record is corrupt: nothing is published, and a person judges.
    Corrupt(Corrupt),
    /// The record is another volume's.
    Volume([u8; 16]),
    /// [`UNDER_WAY_MAX`] publishes are under way.
    Full,
    /// The pool is refused a publish.
    Premise(zfs::Refusal),
    /// The daemon did not answer `cuts`, or holds no cuts yet.
    Daemon(String),
    /// ZFS failed.
    Zfs(ZfsError),
    /// The record was not saved.
    Save(SaveError),
    /// A manifest's file failed.
    Io(io::Error),
    /// [`Agent::settle`] ran out of time with publishes still under way.
    Unsettled(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Stopped => write!(f, "stopped at a crash point"),
            Error::Corrupt(why) => {
                write!(f, "the record is corrupt ({why}); a person must judge it")
            }
            Error::Volume(v) => write!(f, "the record is of volume {}", skyzfs_control::hex(v)),
            Error::Full => write!(f, "{UNDER_WAY_MAX} publishes are under way"),
            Error::Premise(why) => write!(f, "the pool is refused a publish: {why}"),
            Error::Daemon(why) => write!(f, "the daemon: {why}"),
            Error::Zfs(e) => write!(f, "zfs: {e}"),
            Error::Save(e) => write!(f, "the record: {e}"),
            Error::Io(e) => write!(f, "a manifest's file: {e}"),
            Error::Unsettled(names) => write!(f, "still under way: {}", names.join(" ")),
        }
    }
}

impl std::error::Error for Error {}

impl From<SaveError> for Error {
    fn from(e: SaveError) -> Self {
        Error::Save(e)
    }
}

impl From<ZfsError> for Error {
    fn from(e: ZfsError) -> Self {
        Error::Zfs(e)
    }
}

/// How a publish left the record, or stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Published: `snap/<t>` names this commit.
    Published([u8; 32]),
    /// Given up.
    Abandoned(Why),
    /// Stopped for a person to judge; it stays under way.
    Stopped(String),
}

/// What the engine keeps of a publish between two rounds ([module
/// docs](self)).
#[derive(Debug, Default, Clone)]
struct Memory {
    answer: Option<Reply>,
    snap: Option<SnapSeen>,
    assisted_ms: Option<u64>,
    /// The syncs since the request under way was accepted
    /// ([`Seen::assists`]).
    assists: u32,
    /// `asked_ms` as recorded, and [`Clock::mono_ms`] then.
    accepted: Option<(u64, u64)>,
    /// [`Seen::withdrawn`].
    withdrawn: bool,
    /// The anchor of the publish's age: `begun_ms` as recorded, the time
    /// against which it is measured at the anchor, and [`Clock::mono_ms`]
    /// then. For a publish begun in this process the time is `begun_ms`
    /// itself; for one read from the record, the wall clock at the first
    /// round that read its age.
    begun: Option<(u64, u64, u64)>,
}

/// Whether a round changed anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moved {
    Yes,
    No,
}

/// The agent of one volume and its pool.
pub struct Agent<Z, D, K, C> {
    zfs: Z,
    daemon: D,
    keep: K,
    clock: C,
    pool: String,
    expiry: Duration,
    age_max: Duration,
    record: Record,
    device: Option<u64>,
    memory: BTreeMap<String, Memory>,
    swept: Option<u64>,
    stopped: BTreeSet<String>,
    outcomes: Vec<(String, Outcome)>,
    reports: Vec<(String, Report)>,
    reported: BTreeSet<String>,
    assists: u64,
    points: Option<Points>,
}

impl<Z: Zfs, D: Daemon, K: Keep, C: Clock> Agent<Z, D, K, C> {
    /// The agent of `volume` and `pool`, with its record loaded. A
    /// manifest's file that no publish under way names is removed (a
    /// crash after the publish left the record).
    pub fn new(
        zfs: Z,
        daemon: D,
        keep: K,
        clock: C,
        volume: [u8; 16],
        pool: &str,
    ) -> Result<Self, Error> {
        let record = match keep.load().map_err(Error::Io)? {
            Loaded::Absent => Record::new(volume),
            Loaded::Corrupt(why) => return Err(Error::Corrupt(why)),
            Loaded::Present(record) if record.volume != volume => {
                return Err(Error::Volume(record.volume))
            }
            Loaded::Present(record) => record,
        };
        for name in keep.manifest_names().map_err(Error::Io)? {
            if record.publish(&name).is_none() {
                keep.remove_manifest(&name).map_err(Error::Io)?;
            }
        }
        Ok(Agent {
            zfs,
            daemon,
            keep,
            clock,
            pool: pool.to_string(),
            expiry: EXPIRY_DEFAULT,
            age_max: plan::AGE_MAX,
            record,
            device: None,
            memory: BTreeMap::new(),
            swept: None,
            stopped: BTreeSet::new(),
            outcomes: Vec::new(),
            reports: Vec::new(),
            reported: BTreeSet::new(),
            assists: 0,
            points: None,
        })
    }

    /// Stop at the points `points` chooses (the engine's tests).
    pub fn with_points(mut self, points: Points) -> Self {
        self.points = Some(points);
        self
    }

    /// Give a publish up once it is older than `age_max` instead of
    /// [`plan::AGE_MAX`] (the engine's tests, and the rigs' test hooks).
    pub fn with_age_max(mut self, age_max: Duration) -> Self {
        self.age_max = age_max;
        self
    }

    /// The record as last saved.
    pub fn record(&self) -> &Record {
        &self.record
    }

    /// How each publish that left the record, or stopped, ended, in
    /// order.
    pub fn outcomes(&self) -> &[(String, Outcome)] {
        &self.outcomes
    }

    /// What the agent has told a person while it went on, in order.
    pub fn reports(&self) -> &[(String, Report)] {
        &self.reports
    }

    /// The parts, for a test to look at the world.
    pub fn parts(&mut self) -> (&mut Z, &mut D, &mut K, &mut C) {
        (
            &mut self.zfs,
            &mut self.daemon,
            &mut self.keep,
            &mut self.clock,
        )
    }

    /// Begin a publish: the premise, the pool enrolled at the first,
    /// the count, a name, and the record at [`Phase::Intent`]. Its name.
    pub fn begin(&mut self) -> Result<String, Error> {
        if self.record.under_way.len() >= UNDER_WAY_MAX {
            return Err(Error::Full);
        }
        let props = self.zfs.props(&self.pool)?;
        let leaf = self.premise(&props)?.map_err(Error::Premise)?;
        if self.record.enrolled.is_none() {
            let enrolled = Enrolled {
                pool: self.pool.clone(),
                pool_guid: props.guid,
                leaf_guid: leaf,
            };
            self.commit(Effect::Enrol, |r| r.enrolled = Some(enrolled))?;
        }
        let count = match self.count(props.load_guid) {
            Ok(count) => count,
            Err(why) => return Err(Error::Daemon(why)),
        };
        let name = self.fresh_name()?;
        let begun_ms = self.clock.now_ms();
        let mono = self.clock.mono_ms();
        let publish = Publish {
            name: name.clone(),
            count,
            phase: Phase::Intent,
            held: false,
            attempts: 0,
            stalls: 0,
            begun_ms,
        };
        self.commit(Effect::Intent, |r| r.under_way.push(publish))?;
        self.memory.entry(name.clone()).or_default().begun = Some((begun_ms, begun_ms, mono));
        Ok(name)
    }

    /// Drive every publish under way until each has left the record or
    /// stopped, waiting [`TICK`] between rounds where none moved; at most
    /// until `limit` has passed. Each that left or stopped is in
    /// [`Agent::outcomes`]. Each round begins with the sweep
    /// (`Agent::sweep`).
    pub fn settle(&mut self, limit: Duration) -> Result<(), Error> {
        let end = self.clock.mono_ms().saturating_add(ms(limit));
        loop {
            self.sweep()?;
            let names: Vec<String> = self
                .record
                .under_way
                .iter()
                .map(|p| p.name.clone())
                .filter(|name| !self.stopped.contains(name))
                .collect();
            if names.is_empty() {
                return Ok(());
            }
            if self.clock.mono_ms() >= end {
                return Err(Error::Unsettled(names));
            }
            let mut moved = Moved::No;
            for name in names {
                if self.round(&name)? == Moved::Yes {
                    moved = Moved::Yes;
                }
            }
            if moved == Moved::No {
                self.clock.sleep(TICK);
            }
        }
    }

    /// Release each hold [`HOLD_TAG`] in the pool that no publish under
    /// way names, once per import of the pool (per `load_guid`) and at
    /// the agent's start.
    ///
    /// Such a hold was brought back by a rewind: `zpool import -F` or
    /// `-T` back to a group after the hold and before its release, when
    /// the publish has since left the record. Nothing else would release
    /// it, and a person could not destroy the snapshot. A hold of the
    /// tag is the agent's alone. The agent places one only for a publish
    /// in the record, and releases it before the publish leaves the
    /// record (done, or given up after its request ended), so a hold no
    /// publish names protects no publish. One agent runs per pool, and
    /// within it the sweep and the steps of a publish do not overlap.
    /// Not in the plan of M2.6: the crash matrix found it (a rewind
    /// back to after the hold, under a publish already done).
    fn sweep(&mut self) -> Result<(), Error> {
        let load_guid = match self.zfs.props(&self.pool) {
            Ok(props) => props.load_guid,
            Err(ZfsError::NoPool) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        if self.swept == Some(load_guid) {
            return Ok(());
        }
        for name in self.zfs.tagged(&self.pool, HOLD_TAG)? {
            if self.record.publish(&name).is_some() {
                continue;
            }
            self.point(Effect::Sweep, false)?;
            self.zfs.release(&self.pool, &name, HOLD_TAG)?;
            self.point(Effect::Sweep, true)?;
        }
        self.swept = Some(load_guid);
        Ok(())
    }

    /// One round for the publish of `name`: see, plan, act.
    fn round(&mut self, name: &str) -> Result<Moved, Error> {
        let Some(publish) = self.record.publish(name).cloned() else {
            return Ok(Moved::No);
        };
        let Some(seen) = self.see(&publish)? else {
            return Ok(Moved::No);
        };
        let action = plan::plan(&publish, &seen);
        // Before the act, as the report of `done` (F1 of #144's review):
        // a stop between the two makes it again in the next process.
        if let Some(report) = plan::report(&publish, &seen, &action) {
            let made = (name.to_string(), report);
            if !self.reports.contains(&made) {
                self.reports.push(made);
            }
        }
        self.act(&publish, action)
    }

    /// What the planner reads, or `None` where the pool cannot be seen
    /// (not imported, or the volume's device not known).
    fn see(&mut self, publish: &Publish) -> Result<Option<Seen>, Error> {
        let pool = &self.pool.clone();
        let name = &publish.name;
        let props = match self.zfs.props(pool) {
            Ok(props) => props,
            Err(ZfsError::NoPool) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let premise = match self.premise(&props) {
            Ok(verdict) => verdict.is_ok(),
            Err(Error::Zfs(ZfsError::NoPool)) => return Ok(None),
            Err(Error::Daemon(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let snapshot = self.zfs.members(pool, name)?;
        let hold = self.zfs.held(pool, name, HOLD_TAG)?;
        let now_ms = self.now_for(publish);
        let too_old = plan::too_old(publish.begun_ms, self.age_now(publish), self.age_max);
        let memory = self.memory.entry(name.clone()).or_default();
        let kept = memory.answer.clone();
        let snap = memory.snap;
        let assisted_ms = memory.assisted_ms;
        let assists = memory.assists;
        let withdrawn = memory.withdrawn;
        let daemon = match self.daemon.ask(&Request::Cuts, &[]) {
            Ok(Reply::Cuts(cuts)) => {
                let answer = match (&publish.phase, kept) {
                    (_, Some(answer)) => Some(answer),
                    (Phase::Intent | Phase::Taken(_), None) => Some(Reply::Unknown),
                    (_, None) => self.daemon.ask(&Request::Progress(name.clone()), &[]).ok(),
                };
                match answer {
                    Some(answer) => DaemonSeen::Here { cuts, answer },
                    None => DaemonSeen::Away,
                }
            }
            _ => DaemonSeen::Away,
        };
        Ok(Some(Seen {
            pool: PoolSeen {
                load_guid: props.load_guid,
                premise,
                snapshot,
                hold,
            },
            daemon,
            snap,
            now_ms,
            assisted_ms,
            assists,
            too_old,
            withdrawn,
        }))
    }

    /// The time against which `publish`'s age is measured: the anchor's
    /// time plus what the clock that does not step has run since. The
    /// anchor is `begun_ms` for a publish begun in this process, else the
    /// wall clock at the first round of this process that reads the age
    /// (S2 of #144's next look; [module docs](self), "Time").
    fn age_now(&mut self, publish: &Publish) -> u64 {
        let mono = self.clock.mono_ms();
        let memory = self.memory.entry(publish.name.clone()).or_default();
        let (at, then) = match memory.begun {
            Some((begun, at, then)) if begun == publish.begun_ms => (at, then),
            _ => {
                let at = self.clock.now_ms();
                memory.begun = Some((publish.begun_ms, at, mono));
                (at, mono)
            }
        };
        at.saturating_add(mono.saturating_sub(then))
    }

    /// The time the planner reads for `publish` ([module docs](self),
    /// "Time").
    fn now_for(&mut self, publish: &Publish) -> u64 {
        let wall = self.clock.now_ms();
        let Phase::Asked { asked_ms, .. } = publish.phase else {
            return wall;
        };
        match self.memory.get(&publish.name).and_then(|m| m.accepted) {
            Some((at, mono)) if at == asked_ms => {
                asked_ms.saturating_add(self.clock.mono_ms().saturating_sub(mono))
            }
            _ => wall,
        }
    }

    /// Do `action` for `publish`.
    fn act(&mut self, publish: &Publish, action: Action) -> Result<Moved, Error> {
        let name = publish.name.clone();
        // The answer and the sight are kept across a look, which acts
        // on them, and across a release, after which the next round
        // gives the publish up on them: asked nothing between, the
        // daemon cannot have changed its answer, and one refused
        // `stopped` to its own `publish` is not kept by the daemon, whose
        // `progress` would then answer `unknown` and have the publish
        // held and asked again (N8 of #146's review).
        if !matches!(action, Action::Look | Action::Release) {
            let memory = self.memory.entry(name.clone()).or_default();
            memory.answer = None;
            memory.snap = None;
        }
        match action {
            Action::Take => self.take(&name),
            Action::Hold => {
                self.commit(Effect::Holding, |r| set(r, &name, |p| p.held = true))?;
                self.point(Effect::Hold, false)?;
                let held = self.zfs.hold(&self.pool, &name, HOLD_TAG);
                self.point(Effect::Hold, true)?;
                Ok(if held.is_ok() { Moved::Yes } else { Moved::No })
            }
            Action::Held => {
                let Some(taken) = publish.phase.taken().cloned() else {
                    return Ok(Moved::No);
                };
                self.commit(Effect::Held, |r| {
                    set(r, &name, |p| {
                        p.held = true;
                        p.phase = Phase::Held(taken);
                    })
                })?;
                Ok(Moved::Yes)
            }
            Action::Ask => self.ask(publish, publish.count),
            Action::AskAnew => self.ask_anew(publish),
            Action::Stalled => {
                let Some(taken) = publish.phase.taken().cloned() else {
                    return Ok(Moved::No);
                };
                let stalls = publish.stalls.saturating_add(1);
                self.commit(Effect::Stalled, |r| {
                    set(r, &name, |p| {
                        p.stalls = stalls;
                        p.phase = Phase::Held(taken);
                    })
                })?;
                Ok(Moved::Yes)
            }
            Action::Withdraw => {
                self.point(Effect::Withdraw, false)?;
                let answer = self.daemon.ask(&Request::Withdraw(name.clone()), &[]);
                self.point(Effect::Withdraw, true)?;
                match answer {
                    Ok(answer) => {
                        let memory = self.memory.entry(name).or_default();
                        if matches!(answer, Reply::Withdrawn | Reply::Unknown | Reply::Expired) {
                            memory.withdrawn = true;
                        }
                        memory.answer = Some(answer);
                        Ok(Moved::Yes)
                    }
                    Err(_) => Ok(Moved::No),
                }
            }
            Action::Look => {
                let Some(taken) = publish.phase.taken().cloned() else {
                    return Ok(Moved::No);
                };
                self.point(Effect::Look, false)?;
                let answer = self.daemon.ask(&Request::Look(name.clone()), &[]);
                self.point(Effect::Look, true)?;
                let snap = answer
                    .ok()
                    .and_then(|reply| plan::snap_seen(&reply, &taken));
                self.memory.entry(name).or_default().snap = snap;
                Ok(if snap.is_some() {
                    Moved::Yes
                } else {
                    Moved::No
                })
            }
            Action::Assist => {
                // A value of its own at each sync: the clock, or one past
                // the last where the clock has not moved on.
                let value = self.clock.now_ms().max(self.assists.saturating_add(1));
                self.assists = value;
                self.point(Effect::Assist, false)?;
                let synced = self.zfs.assist(&self.pool, value);
                self.point(Effect::Assist, true)?;
                let now = self.now_for(publish);
                let memory = self.memory.entry(name).or_default();
                memory.assisted_ms = Some(now);
                memory.assists = memory.assists.saturating_add(1);
                // A sync is a step only for the pool: the cut it makes is
                // seen at the next round, a tick later.
                let _ = synced;
                Ok(Moved::No)
            }
            Action::Wait => Ok(Moved::No),
            Action::Release => {
                self.point(Effect::Release, false)?;
                let released = self.zfs.release(&self.pool, &name, HOLD_TAG);
                self.point(Effect::Release, true)?;
                if released.is_err() {
                    return Ok(Moved::No);
                }
                self.commit(Effect::Released, |r| set(r, &name, |p| p.held = false))?;
                Ok(Moved::Yes)
            }
            Action::Done { commit, report } => {
                let Some(taken) = publish.phase.taken().cloned() else {
                    return Ok(Moved::No);
                };
                // The report before the save: a stop between the two
                // plans `Done` with it again, so it is made at least once
                // (F1 of #144's review). A report is not recorded, and
                // after the save the publish is only released and
                // forgotten. Made once in this process where a save that
                // failed has it planned again.
                if let Some(report) = report {
                    let made = (name.clone(), report);
                    if !self.reports.contains(&made) {
                        self.reports.push(made);
                    }
                }
                self.commit(Effect::Done, |r| {
                    set(r, &name, |p| p.phase = Phase::Done { taken, commit })
                })?;
                Ok(Moved::Yes)
            }
            Action::Forget => {
                let commit = match publish.phase {
                    Phase::Done { commit, .. } => commit,
                    _ => [0; 32],
                };
                self.commit(Effect::Forget, |r| r.under_way.retain(|p| p.name != name))?;
                self.unlink(&name)?;
                self.leave(name, Outcome::Published(commit));
                Ok(Moved::Yes)
            }
            Action::Abandon(why) => {
                self.abandon(&name, why)?;
                Ok(Moved::Yes)
            }
            Action::Stop(stop) => {
                let why = match stop {
                    Stop::Refused(code) => format!("the daemon refused it: {}", code.word()),
                    Stop::DoneNotSeen => {
                        "the daemon said done, and look did not see it".to_string()
                    }
                };
                self.stop(name, why);
                Ok(Moved::No)
            }
        }
    }

    /// Take the snapshot: the count read anew and recorded, the program,
    /// the manifest's file, and the record at [`Phase::Taken`].
    fn take(&mut self, name: &str) -> Result<Moved, Error> {
        let props = self.zfs.props(&self.pool)?;
        let Ok(count) = self.count(props.load_guid) else {
            return Ok(Moved::No);
        };
        self.commit(Effect::Intent, |r| set(r, name, |p| p.count = count))?;
        self.point(Effect::Program, false)?;
        let program = self.zfs.snapshot(&self.pool, name);
        self.point(Effect::Program, true)?;
        let program = match program {
            Ok(program) => program,
            Err(ZfsError::Exists) => {
                // A member's `@t` of this name exists: nothing was taken,
                // and the publish takes the next name.
                let next = self.fresh_name()?;
                self.commit(Effect::Intent, |r| set(r, name, |p| p.name = next.clone()))?;
                // What this process knows of the publish goes with it: its
                // age is still read from its anchor (N2 of #144's next
                // look).
                if let Some(memory) = self.memory.remove(name) {
                    self.memory.insert(next, memory);
                }
                return Ok(Moved::Yes);
            }
            Err(_) => return Ok(Moved::No),
        };
        let manifest = Manifest {
            pool: self.pool.clone(),
            pool_guid: props.guid,
            snapshot: name.to_string(),
            createtxg: program.createtxg,
            members: program.members,
        };
        let bytes = match manifest.encode() {
            Ok(bytes) => bytes,
            Err(ManifestError::TooLarge(_)) => {
                self.abandon(name, Why::ManifestTooLarge)?;
                return Ok(Moved::Yes);
            }
            Err(e) => {
                self.stop(name.to_string(), format!("its manifest: {e}"));
                return Ok(Moved::No);
            }
        };
        self.point(Effect::Manifest, false)?;
        self.keep.write_manifest(name, &bytes).map_err(Error::Io)?;
        self.point(Effect::Manifest, true)?;
        let taken = Taken {
            group: manifest.createtxg,
            members: manifest.snapshots(),
            manifest_len: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            manifest_hash: manifest::hash(&bytes),
        };
        self.commit(Effect::Taken, |r| {
            set(r, name, |p| p.phase = Phase::Taken(taken))
        })?;
        Ok(Moved::Yes)
    }

    /// Ask the daemon to publish with `count`; `accepted` is recorded,
    /// any other answer kept for the next round.
    fn ask(&mut self, publish: &Publish, count: Count) -> Result<Moved, Error> {
        let name = publish.name.clone();
        let Some(taken) = publish.phase.taken().cloned() else {
            return Ok(Moved::No);
        };
        let bytes = match self.keep.read_manifest(&name).map_err(Error::Io)? {
            Some(bytes)
                if bytes.len() == taken.manifest_len as usize
                    && manifest::hash(&bytes) == taken.manifest_hash =>
            {
                bytes
            }
            // The agent's own file gone or changed under it: the hold is
            // kept, and a person judges.
            _ => {
                self.stop(name, "its manifest's file is gone or changed".into());
                return Ok(Moved::No);
            }
        };
        let request = Request::Publish(PublishRequest {
            name: name.clone(),
            attachment: count.attachment,
            state: count.state,
            after: count.after,
            group: taken.group,
            manifest: bytes.len(),
            expiry: Some(self.expiry),
        });
        self.point(Effect::Ask, false)?;
        let answer = self.daemon.ask(&request, &bytes);
        self.point(Effect::Ask, true)?;
        match answer {
            Ok(Reply::Accepted) => {
                let now = self.clock.now_ms();
                let mono = self.clock.mono_ms();
                let held_until_ms = plan::held_until(now, self.expiry);
                self.commit(Effect::Asked, |r| {
                    set(r, &name, |p| {
                        p.count = count;
                        p.attempts = p.attempts.saturating_add(1);
                        p.phase = Phase::Asked {
                            taken,
                            asked_ms: now,
                            held_until_ms,
                        };
                    })
                })?;
                let memory = self.memory.entry(name).or_default();
                memory.accepted = Some((now, mono));
                memory.assists = 0;
                memory.withdrawn = false;
                Ok(Moved::Yes)
            }
            Ok(answer) => {
                self.memory.entry(name).or_default().answer = Some(answer);
                Ok(Moved::Yes)
            }
            Err(_) => Ok(Moved::No),
        }
    }

    /// Read a count anew after the hold is verified ("The rule for
    /// AskAnew"), record it, and ask with it.
    ///
    /// `load_guid` is read first, then the snapshot's members and its
    /// hold are verified, then the count is read, then `load_guid` again:
    /// the count is of the import whose hold was verified, and an import
    /// between the two is seen again at the next round (D5 of #142's
    /// review).
    ///
    /// The count is recorded with the publish's stalls ([`plan::stalls`])
    /// and the publish at [`Phase::Held`]: no request of it is out (the
    /// last one ended, which is why it is asked anew), so that a request
    /// that is not accepted and is asked anew again is not counted twice,
    /// and a crash before `accepted` is recorded asks with this count.
    /// A publish with three stalls or more is reported
    /// ([`plan::stalls_report`]) once in this process, and again if its
    /// stalls fall below three and reach it anew: at least once, since a
    /// report is not recorded and a crash may come between the count's
    /// commit and the report (a restart reports the publish again).
    fn ask_anew(&mut self, publish: &Publish) -> Result<Moved, Error> {
        let name = publish.name.clone();
        let Some(taken) = publish.phase.taken().cloned() else {
            return Ok(Moved::No);
        };
        let before = self.zfs.props(&self.pool)?.load_guid;
        let members = self.zfs.members(&self.pool, &name)?;
        let held = self.zfs.held(&self.pool, &name, HOLD_TAG)? == Held::All;
        let whole = members.is_some_and(|members| plan::same_snapshot(&members, &taken));
        if !whole || !held {
            // Seen again at the next round, which plans from it.
            return Ok(Moved::No);
        }
        let Ok(count) = self.count(before) else {
            return Ok(Moved::No);
        };
        let after = self.zfs.props(&self.pool)?.load_guid;
        if after != before {
            return Ok(Moved::No);
        }
        let stalls = plan::stalls(publish, &count);
        let recorded = taken.clone();
        self.commit(Effect::Count, |r| {
            set(r, &name, |p| {
                p.count = count;
                p.stalls = stalls;
                p.phase = Phase::Held(recorded);
            })
        })?;
        match plan::stalls_report(stalls) {
            Some(report) => {
                if self.reported.insert(name.clone()) {
                    self.reports.push((name.clone(), report));
                }
            }
            None => {
                self.reported.remove(&name);
            }
        }
        let mut publish = publish.clone();
        publish.count = count;
        publish.stalls = stalls;
        publish.phase = Phase::Held(taken);
        self.ask(&publish, count)
    }

    /// The count of cuts now, with `load_guid` read before it.
    fn count(&mut self, load_guid: u64) -> Result<Count, String> {
        match self.daemon.ask(&Request::Cuts, &[]) {
            Ok(Reply::Cuts(CutsReply {
                attachment,
                state: Some(state),
                count,
                ..
            })) => Ok(Count {
                attachment,
                state,
                after: count,
                load_guid,
            }),
            Ok(Reply::Cuts(_)) => Err("it holds no cuts yet".into()),
            Ok(reply) => Err(format!("it answered `cuts` with {reply:?}")),
            Err(e) => Err(e.to_string()),
        }
    }

    /// The premise for the pool now: the leaf's GUID, or why not.
    fn premise(&mut self, props: &zfs::Props) -> Result<Result<u64, zfs::Refusal>, Error> {
        let layout = self.zfs.layout(&self.pool)?;
        let nocacheflush = self.zfs.nocacheflush()?;
        let device = match self.daemon.device() {
            Ok(device) => {
                self.device = Some(device);
                device
            }
            Err(e) => match self.device {
                Some(device) => device,
                None => return Err(Error::Daemon(format!("its device: {e}"))),
            },
        };
        Ok(zfs::premise(
            &layout,
            props,
            nocacheflush,
            device,
            &self.pool,
            self.record.enrolled.as_ref(),
        ))
    }

    /// A name for a new publish: this second's, with `-N` where it is
    /// taken in the record or in the pool.
    fn fresh_name(&mut self) -> Result<String, Error> {
        let second = self.clock.now_ms() / 1000;
        for taken in 0..=u32::from(u16::MAX) {
            let name = names::name(second, taken);
            let recorded = self.record.publish(&name).is_some()
                || self.record.abandoned.iter().any(|a| a.name == name);
            if recorded {
                continue;
            }
            if self.zfs.members(&self.pool, &name)?.is_none() {
                return Ok(name);
            }
        }
        Err(Error::Zfs(ZfsError::Other(format!(
            "no free name in second {second}"
        ))))
    }

    /// Give the publish of `name` up for `why`, and remove its manifest.
    fn abandon(&mut self, name: &str, why: Why) -> Result<(), Error> {
        let now = self.clock.now_ms();
        self.commit(Effect::Abandon, |r| r.abandon(name, why, now))?;
        self.unlink(name)?;
        self.leave(name.to_string(), Outcome::Abandoned(why));
        Ok(())
    }

    /// Remove the manifest's file of a publish that has left the record.
    fn unlink(&mut self, name: &str) -> Result<(), Error> {
        self.point(Effect::Unlink, false)?;
        self.keep.remove_manifest(name).map_err(Error::Io)?;
        self.point(Effect::Unlink, true)
    }

    /// The publish of `name` stops for a person to judge.
    fn stop(&mut self, name: String, why: String) {
        self.stopped.insert(name.clone());
        self.outcomes.push((name, Outcome::Stopped(why)));
    }

    /// The publish of `name` has left the record.
    fn leave(&mut self, name: String, outcome: Outcome) {
        self.memory.remove(&name);
        self.outcomes.push((name, outcome));
    }

    /// Save the record as `change` leaves it, at the points of `effect`.
    fn commit(&mut self, effect: Effect, change: impl FnOnce(&mut Record)) -> Result<(), Error> {
        let mut next = self.record.clone();
        change(&mut next);
        self.point(effect, false)?;
        self.keep.save(&next)?;
        self.record = next;
        self.point(effect, true)
    }

    /// Stop here where the tests say so.
    fn point(&mut self, effect: Effect, after: bool) -> Result<(), Error> {
        let stop = match &mut self.points {
            Some(points) => points(Point { effect, after }),
            None => false,
        };
        if stop {
            Err(Error::Stopped)
        } else {
            Ok(())
        }
    }
}

/// Change the publish of `name` in `record`.
fn set(record: &mut Record, name: &str, change: impl FnOnce(&mut Publish)) {
    if let Some(publish) = record.under_way.iter_mut().find(|p| p.name == name) {
        change(publish);
    }
}

/// `d` in milliseconds.
fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
