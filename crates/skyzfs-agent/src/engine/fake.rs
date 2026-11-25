//! One fake world for the engine's tests (the plan of M2.6, §7): a pool,
//! the daemon that serves its volume, object storage, the clock, and the
//! agent's directory, behind the engine's traits.
//!
//! **The pool** has three datasets, snapshots with their GUIDs,
//! `createtxg` and holds, its synced group, its layout and `load_guid`.
//! Each change is a sync task that writes one group, as in ZFS, but for
//! the agent's assist, which writes two (its set's, and the sync's with
//! the set's history record, as #145's probe saw); a busy pool also
//! writes a group every five seconds. The snapshots as each
//! group left them are kept, so that a rewind import (`-F`, `-T`) takes
//! the pool back to a group exactly.
//!
//! **The daemon** counts a cut at each group: its number, its group (or a
//! lower one where a stale write marks it, design §5.2), whether it opens
//! as itself, and the pool's snapshots at it, so that "`snap/t` names a
//! cut that holds the snapshot" is checked exactly. It keeps the rewind
//! mark (R) as the store does, holds at most two cuts (the one being
//! uploaded, the one that waits, which a newer cut replaces), uploads
//! them with a lag, and publishes under the contract of
//! `skyzfs_upload::publish`, written again here from its documentation:
//! asking twice is asking once, a request of another attachment or state
//! is refused, a publish has a deadline, a lookup of `snap/t` comes
//! first and ends a publish done or refused `taken`, a rewind ends what
//! waits, and a new uploader knows nothing of the last one's publishes.
//! It can die at each point of a publish, and come back by a take-over,
//! a reboot, or with the pool rewound.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::rc::Rc;
use std::time::Duration;

use skyzfs_control::{Code, CutsReply, Publish as PublishRequest, Reply, Request, SnapReply};

use super::{Clock, Daemon};
use crate::manifest::{self, Kind, Manifest};
use crate::record::{self, Keep, Loaded, Record, SaveError};
use crate::zfs::{Layout, Program, Props, Vdev, VdevKind, Zfs, ZfsError, HOLD_TAG};

/// The pool's name.
pub const POOL: &str = "tank";
/// The volume's device number.
pub const DEVICE: u64 = 0x10_0003;
/// The volume.
pub const VOLUME: [u8; 16] = [0x0f; 16];
/// When the world begins: 2026-11-25T01:00:00Z.
pub const EPOCH_MS: u64 = 1_795_568_400_000;
/// A busy pool's group interval.
pub const TXG_MS: u64 = 5_000;

/// How the pool is written to (the plan, §7, "Pool modes").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A group every five seconds.
    Busy,
    /// Groups only from the agent's own changes and syncs.
    Idle,
    /// Idle, and the first cut that would carry a publish is abandoned
    /// at its upload: the cut that takes its map is published.
    Abandoned,
    /// Busy, and the first cut counted after a publish is accepted has a
    /// stale group, one not above the cut before it.
    ErrLow,
}

/// The daemon's rule (R): which cut marks a rewind, after which the
/// daemon refuses a publish asked with an older count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// None: the matrix must fail without it.
    Off,
    /// As M2.6a built it: a cut whose group is not above the group of
    /// the cut before it.
    Previous,
    /// A cut whose group is not above the highest group written before
    /// it, of any history (the volume's rings show it).
    Highest,
}

/// How many questions of the agent's (the daemon's answers and the pool's
/// properties) the world takes with no time passing before it calls the
/// agent livelocked. A round of the engine asks fewer than ten for each
/// publish, and at most [`UNDER_WAY_MAX`](super::UNDER_WAY_MAX) are under
/// way.
pub const STILL_MAX: u32 = 2_000;

/// The rule the fake daemon has unless a test says otherwise: the
/// daemon's since M2.6a (#140), where a cut is compared with the highest
/// group the store has seen since its start (the lead's decision, made
/// on the three runs that `the_matrices_need_the_rule_r_of_the_daemon`
/// shows [`Rule::Previous`] fails).
pub const DAEMON_RULE: Rule = Rule::Highest;

/// What happens to the world after a loss (the plan, §7, "The losses").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    /// Nothing more.
    None,
    /// A new daemon takes the device over: the same attachment and cut
    /// state, the count it starts with as its rewind mark.
    TakeOver,
    /// A power cut: a new attachment and cut state, the held cuts lost,
    /// the pool imported plainly.
    Reboot,
    /// The pool exported and imported with `-F` back to the group
    /// named; the daemon goes on as it was.
    RewindF(Back),
    /// The same with `-T`.
    RewindT(Back),
}

/// How far back a rewind takes the pool, against the agent's snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Back {
    /// Below its `createtxg`: the snapshot is lost.
    BelowCreated,
    /// Below the group of its hold: the snapshot stays, the hold is lost.
    BelowHold,
    /// One group back, after the hold.
    AfterHold,
}

/// The restarts the matrix crosses each loss with.
pub const RESTARTS: [Restart; 8] = [
    Restart::TakeOver,
    Restart::Reboot,
    Restart::RewindF(Back::BelowCreated),
    Restart::RewindF(Back::BelowHold),
    Restart::RewindF(Back::AfterHold),
    Restart::RewindT(Back::BelowCreated),
    Restart::RewindT(Back::BelowHold),
    Restart::RewindT(Back::AfterHold),
];

/// Where the daemon dies in a publish (the plan, §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonPoint {
    /// The request received, not answered.
    BeforeAccept,
    /// Answered `accepted`.
    AfterAccept,
    /// A cut that carries it counted, not uploaded.
    CutTaken,
    /// That cut's commit landed, the publish's transaction not begun.
    Committed,
    /// The publish's transaction landed, its end not recorded.
    Landed,
    /// Its end recorded `done`.
    AfterDone,
}

/// The points at which the daemon dies.
pub const DAEMON_POINTS: [DaemonPoint; 6] = [
    DaemonPoint::BeforeAccept,
    DaemonPoint::AfterAccept,
    DaemonPoint::CutTaken,
    DaemonPoint::Committed,
    DaemonPoint::Landed,
    DaemonPoint::AfterDone,
];

/// What a person does to the pool (the plan, §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum User {
    /// `zfs destroy -r pool@t`; refused while held.
    Destroy,
    /// That, and `zfs snapshot -r pool@t` again: new GUIDs.
    Retake,
    /// `zfs destroy pool/db@t`; refused while held.
    DestroyChild,
    /// `zpool attach`: the leaf becomes a mirror.
    Attach,
    /// `zpool add`: a second data vdev.
    Add,
}

/// The people's actions the matrix tries at each point.
pub const USERS: [User; 5] = [
    User::Destroy,
    User::Retake,
    User::DestroyChild,
    User::Attach,
    User::Add,
];

/// A snapshot of one dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snap {
    /// Its GUID.
    pub guid: u64,
    /// Its group.
    pub createtxg: u64,
    /// Its holds, by tag.
    pub holds: BTreeSet<String>,
    /// The group its agent's hold was placed in.
    pub held_in: Option<u64>,
}

/// The fake pool.
#[derive(Debug, Clone)]
pub struct Pool {
    /// `guid`.
    pub guid: u64,
    /// `load_guid`.
    pub load_guid: u64,
    /// The last group synced.
    pub txg: u64,
    /// The highest group of a history a rewind abandoned: until the pool
    /// passes it, its groups do not open as themselves (`-T`).
    pub abandoned_top: u64,
    /// The datasets, the root first, with their GUIDs.
    pub datasets: Vec<(String, Kind, u64)>,
    /// The snapshots, by `dataset@t`.
    pub snaps: BTreeMap<String, Snap>,
    /// The snapshots as each group left them.
    pub history: BTreeMap<u64, BTreeMap<String, Snap>>,
    /// The layout.
    pub layout: Layout,
    /// The next group's number where a rewind chose it (`-F`).
    pub next_txg: Option<u64>,
}

/// A cut the daemon counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    /// Its number in the state.
    pub number: u64,
    /// Its group.
    pub group: u64,
    /// Whether it opens as itself.
    pub itself: bool,
    /// The pool's snapshots at it, `dataset@t` → GUID.
    pub snaps: BTreeMap<String, u64>,
}

/// A cut committed in object storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The cut state.
    pub state: [u8; 16],
    /// The cut.
    pub cut: Cut,
}

/// `snap/<t>` in object storage: the commit it names and the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapRef {
    /// The commit.
    pub commit: [u8; 32],
    /// The row's cut state.
    pub state: [u8; 16],
    /// The row's cut number.
    pub number: u64,
    /// The row's group.
    pub group: u64,
    /// The ZFS manifest stored with it.
    pub manifest: Vec<u8>,
}

/// A publish that pends at the daemon.
#[derive(Debug, Clone)]
struct Pending {
    request: PublishRequest,
    manifest: Vec<u8>,
    deadline: u64,
    withdrawn: bool,
    looked: bool,
}

/// How a publish ended at the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    Done([u8; 32]),
    Expired,
    Withdrawn,
    Refused(Code),
}

/// The daemon.
#[derive(Debug, Clone)]
pub struct FakeDaemonState {
    /// The attachment.
    pub attachment: [u8; 16],
    /// The cut state.
    pub state: [u8; 16],
    /// The cuts counted in it.
    pub cuts: Vec<Cut>,
    /// The rewind mark.
    pub rewound: u64,
    uploading: Option<(u64, u64)>,
    waiting: Option<u64>,
    known: Vec<(Cut, [u8; 32])>,
    pending: Vec<Pending>,
    ended: BTreeMap<String, Ended>,
}

impl FakeDaemonState {
    /// Whether any publish was asked of this daemon (it pends or ended).
    pub fn asked(&self) -> bool {
        !self.pending.is_empty() || !self.ended.is_empty()
    }

    fn new(attachment: [u8; 16], state: [u8; 16], count: u64, cuts: Vec<Cut>) -> Self {
        FakeDaemonState {
            attachment,
            state,
            cuts,
            rewound: count,
            uploading: None,
            waiting: None,
            known: Vec::new(),
            pending: Vec::new(),
            ended: BTreeMap::new(),
        }
    }

    fn count(&self) -> u64 {
        self.cuts.last().map_or(0, |c| c.number)
    }

    fn cut(&self, number: u64) -> Option<&Cut> {
        self.cuts.iter().find(|c| c.number == number)
    }
}

/// Whether `cut` carries `request` (`Request::carried_by`).
fn carries(state: [u8; 16], cut: &Cut, request: &PublishRequest) -> bool {
    state == request.state && cut.number > request.after && cut.group >= request.group && cut.itself
}

/// The fake world.
#[derive(Debug)]
pub struct World {
    /// The wall clock.
    pub now_ms: u64,
    /// How the pool is written to.
    pub mode: Mode,
    /// The pool; `None` while exported.
    pub pool: Option<Pool>,
    /// The daemon.
    pub daemon: FakeDaemonState,
    /// Object storage's `snap/<t>`.
    pub refs: BTreeMap<String, SnapRef>,
    /// Object storage's commits.
    pub commits: BTreeMap<[u8; 32], Commit>,
    /// The upload's lag.
    pub lag_ms: u64,
    /// Where the daemon dies, and how it comes back.
    pub die: Option<(DaemonPoint, Restart)>,
    /// The world restarts as the daemon is asked for its cuts for the
    /// `k`-th time (from 0), before it answers: between two of the
    /// agent's reads, where none of its points falls.
    pub restart_at_cuts: Option<(usize, Restart)>,
    /// How many times the daemon was asked for its cuts.
    pub cuts_read: usize,
    /// Questions asked of the world since its clock last moved.
    still: u32,
    /// The channel answers nothing until then (the daemon away).
    pub away_until: u64,
    /// A person destroys `pool@t` of this name as soon as it is not held.
    pub destroyer: Option<String>,
    /// The daemon's rule (R).
    pub rule: Rule,
    /// The highest group written to the volume, of any history: what
    /// its rings show.
    pub highest_written: u64,
    /// Each program run, by name: the manifest it read.
    pub programs: BTreeMap<String, Manifest>,
    /// What happened, for a failure's message.
    pub log: Vec<String>,
    /// How far the agent's wall clock is from the world's (a step of it;
    /// the daemon's deadlines are on the world's).
    pub agent_skew_ms: i64,
    /// Each value the agent's sync task set `skyzfs:assist` to, in order.
    pub assists: Vec<u64>,
    /// The world's time of each sync task, whether it wrote a group or not.
    pub synced_at: Vec<u64>,
    /// What the agent's last read of a snapshot's members found.
    pub members_read: Option<Option<Vec<record::Member>>>,
    /// The agent's reads of the pool and requests of the daemon, in order
    /// (`props`, `members`, `held`, and the request's kind).
    pub calls: Vec<&'static str>,
    /// The agent's sync task is a plain `zpool sync`: it writes no group
    /// (N1 of #142's review), for a test that shows the property is
    /// needed.
    pub plain_sync: bool,
    abandon_next: bool,
    err_low_next: bool,
    erred_low: bool,
    next_busy: u64,
    seed: u64,
}

impl World {
    /// A world with a pool of three datasets, written to as `mode` says.
    pub fn new(mode: Mode) -> Rc<RefCell<World>> {
        let mut world = World {
            now_ms: EPOCH_MS,
            mode,
            pool: None,
            daemon: FakeDaemonState::new([0xa1; 16], [0x5e; 16], 0, Vec::new()),
            refs: BTreeMap::new(),
            commits: BTreeMap::new(),
            lag_ms: if mode == Mode::Busy || mode == Mode::ErrLow {
                7_000
            } else {
                2_000
            },
            die: None,
            restart_at_cuts: None,
            cuts_read: 0,
            still: 0,
            away_until: 0,
            destroyer: None,
            rule: DAEMON_RULE,
            highest_written: 0,
            programs: BTreeMap::new(),
            log: Vec::new(),
            agent_skew_ms: 0,
            assists: Vec::new(),
            synced_at: Vec::new(),
            members_read: None,
            calls: Vec::new(),
            plain_sync: false,
            abandon_next: mode == Mode::Abandoned,
            err_low_next: false,
            erred_low: false,
            next_busy: EPOCH_MS + TXG_MS,
            seed: 0x5eed,
        };
        let datasets = vec![
            (POOL.to_string(), Kind::Filesystem, world.guid()),
            (format!("{POOL}/db"), Kind::Filesystem, world.guid()),
            (format!("{POOL}/vol"), Kind::Volume, world.guid()),
        ];
        let leaf = Vdev {
            kind: VdevKind::Disk,
            guid: 0x1eaf,
            rdev: Some(DEVICE),
            children: Vec::new(),
        };
        world.pool = Some(Pool {
            guid: 0x9001,
            load_guid: world.guid(),
            txg: 4,
            abandoned_top: 0,
            datasets,
            snaps: BTreeMap::new(),
            history: BTreeMap::from([(4, BTreeMap::new())]),
            layout: Layout {
                data: vec![leaf],
                special: 0,
                dedup: 0,
                other: 0,
                busy: None,
                removed: None,
            },
            next_txg: None,
        });
        world.sync("created");
        Rc::new(RefCell::new(world))
    }

    fn guid(&mut self) -> u64 {
        // SplitMix64: distinct, and the same in every run.
        self.seed = self.seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn id16(&mut self) -> [u8; 16] {
        let mut id = [0; 16];
        id[..8].copy_from_slice(&self.guid().to_le_bytes());
        id[8..].copy_from_slice(&self.guid().to_le_bytes());
        id
    }

    /// Write one group, and count its cut.
    pub fn sync(&mut self, why: &str) {
        let Some(pool) = &mut self.pool else {
            return;
        };
        let txg = pool.next_txg.take().unwrap_or(pool.txg + 1);
        pool.txg = txg;
        pool.history.insert(txg, pool.snaps.clone());
        let itself = txg > pool.abandoned_top;
        let snaps = pool
            .snaps
            .iter()
            .map(|(name, snap)| (name.clone(), snap.guid))
            .collect();
        let previous = self.daemon.cuts.last().map(|c| c.group);
        // A group that errs low is a stale copy two groups behind (the
        // daemon's `cuts.rs`, "A cut's group can err low"): below the
        // newest cut and not the pending mark, so no repeat.
        let group = match previous {
            Some(previous) if self.err_low_next && txg >= 3 && txg - 2 < previous => {
                self.err_low_next = false;
                txg - 2
            }
            _ => txg,
        };
        let number = self.daemon.count() + 1;
        self.log.push(format!(
            "{}: group {txg} ({why}): cut {number} group {group} itself {itself}",
            self.now_ms - EPOCH_MS
        ));
        self.daemon.cuts.push(Cut {
            number,
            group,
            itself,
            snaps,
        });
        let highest_before = self.highest_written;
        self.highest_written = self.highest_written.max(txg);
        let rewound = match self.rule {
            Rule::Off => false,
            Rule::Previous => previous.is_some_and(|previous| group <= previous),
            Rule::Highest => group <= highest_before || group < txg,
        };
        if rewound {
            self.daemon.rewound = number;
            let mark = number;
            self.end_where(|p| p.request.after < mark, Ended::Refused(Code::Rewound));
        }
        match self.daemon.uploading {
            None => self.daemon.uploading = Some((number, self.now_ms + self.lag_ms)),
            Some(_) => self.daemon.waiting = Some(number),
        }
        self.daemon_dies_at_cut(number);
    }

    /// The daemon dies as the cut `number` is counted and held, where it
    /// would carry a publish that pends.
    fn daemon_dies_at_cut(&mut self, number: u64) {
        if !matches!(self.die, Some((DaemonPoint::CutTaken, _))) {
            return;
        }
        let Some(cut) = self.daemon.cut(number).cloned() else {
            return;
        };
        let state = self.daemon.state;
        if self
            .daemon
            .pending
            .iter()
            .any(|p| !p.withdrawn && carries(state, &cut, &p.request))
        {
            self.die_now();
        }
    }

    /// End each publish pending that `which` names, with `how`.
    fn end_where(&mut self, which: impl Fn(&Pending) -> bool, how: Ended) {
        let (ended, kept): (Vec<_>, Vec<_>) = self.daemon.pending.drain(..).partition(|p| which(p));
        self.daemon.pending = kept;
        for p in ended {
            self.daemon.ended.insert(p.request.name, how.clone());
        }
    }

    /// Advance the clock by `d`, a second at a time: the pool's groups,
    /// the uploads and the publishes' steps.
    pub fn advance(&mut self, d: Duration) {
        self.still = 0;
        let end = self.now_ms + u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        while self.now_ms < end {
            self.now_ms = (self.now_ms + 1000).min(end);
            self.turn();
        }
    }

    /// One turn of the world at the clock's time.
    fn turn(&mut self) {
        if let Some(name) = self.destroyer.clone() {
            let _ = self.destroy(&name);
        }
        let busy = matches!(self.mode, Mode::Busy | Mode::ErrLow);
        if busy && self.now_ms >= self.next_busy {
            self.next_busy = self.now_ms + TXG_MS;
            self.sync("busy");
        }
        // Deadlines: a publish whose time is up ends expired.
        let now = self.now_ms;
        self.end_where(|p| now >= p.deadline, Ended::Expired);
        self.end_where(|p| p.withdrawn, Ended::Withdrawn);
        // Each publish pending is looked up first, then waits for a cut.
        let fresh: Vec<PublishRequest> = self
            .daemon
            .pending
            .iter_mut()
            .filter(|p| !p.looked)
            .map(|p| {
                p.looked = true;
                p.request.clone()
            })
            .collect();
        for request in fresh {
            if self.refs.contains_key(&request.name) {
                self.found(&request.name);
                continue;
            }
            let state = self.daemon.state;
            let first = self
                .daemon
                .known
                .iter()
                .find(|(cut, _)| carries(state, cut, &request))
                .map(|(cut, commit)| (cut.clone(), *commit));
            if let Some((cut, commit)) = first {
                if !self.make(&request.name, &cut, commit) {
                    return;
                }
            }
        }
        // The upload.
        if let Some((number, at)) = self.daemon.uploading {
            if now >= at {
                self.uploaded(number);
            }
        }
    }

    /// The cut `number` is uploaded, unless it is the first that carries
    /// a publish and the mode abandons that one.
    fn uploaded(&mut self, number: u64) {
        let next = self.daemon.waiting.take();
        self.daemon.uploading = next.map(|n| (n, self.now_ms + self.lag_ms));
        let Some(cut) = self.daemon.cut(number).cloned() else {
            return;
        };
        let state = self.daemon.state;
        let carrying: Vec<String> = self
            .daemon
            .pending
            .iter()
            .filter(|p| p.looked && !p.withdrawn && carries(state, &cut, &p.request))
            .map(|p| p.request.name.clone())
            .collect();
        if self.abandon_next && !carrying.is_empty() {
            self.abandon_next = false;
            // Its map goes to the cut that waits, or to the open map.
            self.log.push(format!("cut {number} abandoned"));
            return;
        }
        let commit = self.commit_id(number);
        self.commits.insert(
            commit,
            Commit {
                state,
                cut: cut.clone(),
            },
        );
        self.daemon.known.push((cut.clone(), commit));
        self.log.push(format!("cut {number} committed"));
        if matches!(self.die, Some((DaemonPoint::Committed, _))) && !carrying.is_empty() {
            self.die_now();
            return;
        }
        for name in carrying {
            if !self.make(&name, &cut, commit) {
                return;
            }
        }
    }

    fn commit_id(&mut self, number: u64) -> [u8; 32] {
        let mut id = [0; 32];
        id[..8].copy_from_slice(&number.to_le_bytes());
        id[8..16].copy_from_slice(&self.guid().to_le_bytes());
        id
    }

    /// The publish of `name` on `cut`: its transaction, then its end.
    /// Whether the daemon lives on.
    fn make(&mut self, name: &str, cut: &Cut, commit: [u8; 32]) -> bool {
        let Some(i) = self
            .daemon
            .pending
            .iter()
            .position(|p| p.request.name == name)
        else {
            return true;
        };
        if self.refs.contains_key(name) {
            self.found(name);
            return true;
        }
        let pending = self.daemon.pending[i].clone();
        self.refs.insert(
            name.to_string(),
            SnapRef {
                commit,
                state: self.daemon.state,
                number: cut.number,
                group: cut.group,
                manifest: pending.manifest,
            },
        );
        self.log
            .push(format!("snap/{name} made on cut {}", cut.number));
        if matches!(self.die, Some((DaemonPoint::Landed, _))) {
            self.die_now();
            return false;
        }
        self.daemon.pending.remove(i);
        self.daemon
            .ended
            .insert(name.to_string(), Ended::Done(commit));
        if matches!(self.die, Some((DaemonPoint::AfterDone, _))) {
            self.die_now();
            return false;
        }
        true
    }

    /// `snap/<name>` exists: done where its row records the request.
    fn found(&mut self, name: &str) {
        let Some(i) = self
            .daemon
            .pending
            .iter()
            .position(|p| p.request.name == name)
        else {
            return;
        };
        let pending = self.daemon.pending.remove(i);
        let recorded = self.refs.get(name).is_some_and(|r| {
            r.state == pending.request.state
                && r.number > pending.request.after
                && r.group >= pending.request.group
                && r.manifest == pending.manifest
        });
        let how = match self.refs.get(name) {
            Some(r) if recorded => Ended::Done(r.commit),
            _ => Ended::Refused(Code::Taken),
        };
        self.daemon.ended.insert(name.to_string(), how);
    }

    /// The daemon dies where the world says, and comes back.
    fn die_now(&mut self) {
        let Some((point, restart)) = self.die.take() else {
            return;
        };
        self.log
            .push(format!("daemon dies at {point:?}, then {restart:?}"));
        // A daemon that dies is taken over, or lost with the machine.
        if let Restart::RewindF(_) | Restart::RewindT(_) = restart {
            self.take_over();
        }
        self.restart(restart, None);
    }

    /// The world after a loss. `snapshot` names the agent's, for a
    /// rewind's target.
    pub fn restart(&mut self, restart: Restart, snapshot: Option<&str>) {
        let snapshot = snapshot
            .map(str::to_string)
            .or_else(|| self.programs.keys().last().cloned());
        match restart {
            Restart::None => {}
            Restart::TakeOver => self.take_over(),
            Restart::Reboot => {
                let attachment = self.id16();
                let state = self.id16();
                self.daemon = FakeDaemonState::new(attachment, state, 0, Vec::new());
                self.import(None, false);
            }
            Restart::RewindF(back) | Restart::RewindT(back) => {
                // An export and import leaves the daemon as it was: its
                // publishes still pend, and the first group of the new
                // history is counted after the old (the plan's §0).
                let target = self.target(back, snapshot.as_deref());
                self.import(Some(target), matches!(restart, Restart::RewindF(_)));
            }
        }
    }

    fn take_over(&mut self) {
        let old = &self.daemon;
        let mut daemon =
            FakeDaemonState::new(old.attachment, old.state, old.count(), old.cuts.clone());
        // The held cuts outlive the daemon; its publishes do not.
        daemon.uploading = old.uploading;
        daemon.waiting = old.waiting;
        self.daemon = daemon;
    }

    /// The group a rewind takes the pool back to.
    fn target(&self, back: Back, snapshot: Option<&str>) -> u64 {
        let Some(pool) = &self.pool else {
            return 0;
        };
        let snap = snapshot.and_then(|t| pool.snaps.get(&format!("{POOL}@{t}")));
        let top = pool.txg;
        match (back, snap) {
            (Back::BelowCreated, Some(s)) => s.createtxg - 1,
            (Back::BelowHold, Some(s)) => s.held_in.map_or(top - 1, |h| h - 1),
            (Back::AfterHold, Some(s)) => s.held_in.map_or(top - 1, |h| (top - 1).max(h)),
            (Back::BelowCreated, None) => top.saturating_sub(2).max(1),
            (_, None) => top - 1,
        }
    }

    /// The pool exported and imported: plainly, or rewound to `to` (by
    /// `-F` where `f`, else `-T`). A new `load_guid`, and the import's
    /// first group written.
    fn import(&mut self, to: Option<u64>, f: bool) {
        let load_guid = self.guid();
        let Some(pool) = &mut self.pool else {
            return;
        };
        pool.load_guid = load_guid;
        if let Some(to) = to {
            let top = pool.txg;
            let (&at, snaps) = pool
                .history
                .range(..=to)
                .next_back()
                .map(|(at, snaps)| (at, snaps.clone()))
                .unwrap_or((&0, BTreeMap::new()));
            let at = at.min(to);
            pool.snaps = snaps;
            pool.history.retain(|g, _| *g <= at);
            pool.abandoned_top = pool.abandoned_top.max(top);
            pool.txg = at;
            pool.next_txg = Some(if f { top } else { at + 1 });
            if f {
                // The first group of a `-F` import has the abandoned
                // best's number and a later time: it opens as itself.
                pool.abandoned_top = top - 1;
            }
            let lost: Vec<String> = self
                .programs
                .keys()
                .filter(|t| !pool.snaps.contains_key(&format!("{POOL}@{t}")))
                .cloned()
                .collect();
            self.log.push(format!(
                "import -{} to {at} from {top}; lost {lost:?}",
                if f { "F" } else { "T" }
            ));
        } else {
            self.log.push("import".into());
        }
        self.sync("import");
    }

    /// Counts a question of the agent's; panics when the agent asks
    /// [`STILL_MAX`] questions with no time passing between them: a
    /// livelock, which would otherwise run until the test's time limit.
    fn asked_still(&mut self) {
        self.still += 1;
        assert!(
            self.still <= STILL_MAX,
            "a livelock: {STILL_MAX} questions at {} ms with no time passing",
            self.now_ms - EPOCH_MS
        );
    }

    /// A person's action now.
    pub fn user(&mut self, user: User, t: &str) {
        self.log.push(format!("user {user:?} {t}"));
        match user {
            User::Destroy => {
                let _ = self.destroy(t);
            }
            User::Retake => {
                if self.destroy(t).is_ok() {
                    let _ = self.take(t);
                }
            }
            User::DestroyChild => {
                let Some(pool) = &mut self.pool else {
                    return;
                };
                let child = format!("{POOL}/db@{t}");
                if pool.snaps.get(&child).is_some_and(|s| s.holds.is_empty()) {
                    pool.snaps.remove(&child);
                    self.sync("destroy child");
                }
            }
            User::Attach | User::Add => {
                let Some(pool) = &mut self.pool else {
                    return;
                };
                let other = Vdev {
                    kind: VdevKind::Disk,
                    guid: 0x2eaf,
                    rdev: Some(DEVICE + 1),
                    children: Vec::new(),
                };
                if user == User::Attach {
                    let leaf = pool.layout.data[0].clone();
                    pool.layout.data[0] = Vdev {
                        kind: VdevKind::Mirror,
                        guid: 0x3eaf,
                        rdev: None,
                        children: vec![leaf, other],
                    };
                } else {
                    pool.layout.data.push(other);
                }
                self.sync("layout");
            }
        }
    }

    /// `zfs destroy -r pool@t`: refused where any member is held.
    fn destroy(&mut self, t: &str) -> Result<(), ZfsError> {
        let Some(pool) = &mut self.pool else {
            return Err(ZfsError::NoPool);
        };
        let suffix = format!("@{t}");
        let members: Vec<String> = pool
            .snaps
            .keys()
            .filter(|k| k.ends_with(&suffix))
            .cloned()
            .collect();
        if members.is_empty() {
            return Err(ZfsError::NoSnapshot);
        }
        if members.iter().any(|m| !pool.snaps[m].holds.is_empty()) {
            return Err(ZfsError::Other("dataset is busy".into()));
        }
        for m in members {
            pool.snaps.remove(&m);
        }
        self.sync("destroy");
        Ok(())
    }

    /// `@t` of every dataset in one group: what the program reads.
    fn take(&mut self, t: &str) -> Result<Program, ZfsError> {
        if self.pool.is_none() {
            return Err(ZfsError::NoPool);
        }
        let createtxg = self
            .pool
            .as_ref()
            .map_or(0, |p| p.next_txg.unwrap_or(p.txg + 1));
        let datasets = self
            .pool
            .as_ref()
            .map(|p| p.datasets.clone())
            .unwrap_or_default();
        if datasets.iter().any(|(ds, _, _)| {
            self.pool
                .as_ref()
                .is_some_and(|p| p.snaps.contains_key(&format!("{ds}@{t}")))
        }) {
            return Err(ZfsError::Exists);
        }
        let mut members = Vec::new();
        for (ds, kind, guid) in datasets {
            let snapshot_guid = self.guid();
            if let Some(pool) = &mut self.pool {
                pool.snaps.insert(
                    format!("{ds}@{t}"),
                    Snap {
                        guid: snapshot_guid,
                        createtxg,
                        holds: BTreeSet::new(),
                        held_in: None,
                    },
                );
            }
            let local = if kind == Kind::Volume {
                BTreeMap::from([("volsize".to_string(), b"1073741824".to_vec())])
            } else {
                BTreeMap::new()
            };
            members.push(manifest::Member {
                name: ds,
                kind,
                guid,
                snapshot_guid,
                local,
            });
        }
        self.sync("snapshot");
        Ok(Program { createtxg, members })
    }

    /// The channel's answer to `request`.
    fn answer(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        self.asked_still();
        self.calls.push(match request {
            Request::Cuts => "cuts",
            Request::Publish(_) => "publish",
            Request::Progress(_) => "progress",
            Request::Withdraw(_) => "withdraw",
            Request::Look(_) => "look",
        });
        if *request == Request::Cuts {
            let k = self.cuts_read;
            self.cuts_read += 1;
            if let Some((at, restart)) = self.restart_at_cuts {
                if at == k {
                    self.restart_at_cuts = None;
                    self.restart(restart, None);
                }
            }
        }
        if self.now_ms < self.away_until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the daemon is away",
            ));
        }
        let daemon = &self.daemon;
        Ok(match request {
            Request::Cuts => Reply::Cuts(CutsReply {
                attachment: daemon.attachment,
                state: Some(daemon.state),
                count: daemon.count(),
                newest: daemon.cuts.last().map(|c| (c.group, c.itself)),
            }),
            Request::Publish(request) => return self.publish(request, manifest),
            Request::Progress(name) => self.progress(name),
            Request::Withdraw(name) => {
                if let Some(p) = self
                    .daemon
                    .pending
                    .iter_mut()
                    .find(|p| p.request.name == *name)
                {
                    p.withdrawn = true;
                }
                self.end_where(|p| p.withdrawn, Ended::Withdrawn);
                self.progress(name)
            }
            Request::Look(name) => match self.refs.get(name) {
                None => Reply::Absent,
                Some(r) => Reply::Snap(SnapReply {
                    commit: r.commit,
                    state: r.state,
                    number: r.number,
                    group: r.group,
                    manifest_len: u32::try_from(r.manifest.len()).unwrap_or(u32::MAX),
                    manifest_hash: manifest::hash(&r.manifest),
                }),
            },
        })
    }

    fn progress(&self, name: &str) -> Reply {
        if self.daemon.pending.iter().any(|p| p.request.name == name) {
            return Reply::Pending;
        }
        match self.daemon.ended.get(name) {
            Some(Ended::Done(commit)) => Reply::Done(*commit),
            Some(Ended::Expired) => Reply::Expired,
            Some(Ended::Withdrawn) => Reply::Withdrawn,
            Some(Ended::Refused(code)) => Reply::Refused(*code, String::new()),
            None => Reply::Unknown,
        }
    }

    fn publish(&mut self, request: &PublishRequest, manifest: &[u8]) -> io::Result<Reply> {
        if matches!(self.die, Some((DaemonPoint::BeforeAccept, _))) {
            self.die_now();
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "the daemon died",
            ));
        }
        let refused = |code| Ok(Reply::Refused(code, String::new()));
        let daemon = &mut self.daemon;
        if request.attachment != daemon.attachment {
            return refused(Code::Attachment);
        }
        if request.state != daemon.state {
            return refused(Code::State);
        }
        if let Some(p) = daemon
            .pending
            .iter_mut()
            .find(|p| p.request.name == request.name)
        {
            if p.request != *request || p.manifest != manifest {
                return refused(Code::Asked);
            }
            p.withdrawn = false;
            return self.accepted();
        }
        if request.after < daemon.rewound && self.rule != Rule::Off {
            return refused(Code::Rewound);
        }
        let expiry = request.expiry.unwrap_or(skyzfs_control::EXPIRY_DEFAULT);
        daemon.ended.remove(&request.name);
        daemon.pending.push(Pending {
            request: request.clone(),
            manifest: manifest.to_vec(),
            deadline: self.now_ms + u64::try_from(expiry.as_millis()).unwrap_or(u64::MAX),
            withdrawn: false,
            looked: false,
        });
        // The daemon errs low once, at the first acceptance: a daemon
        // that erred at every one would never let a publish through.
        if self.mode == Mode::ErrLow && !self.erred_low {
            self.erred_low = true;
            self.err_low_next = true;
        }
        self.accepted()
    }

    fn accepted(&mut self) -> io::Result<Reply> {
        if matches!(self.die, Some((DaemonPoint::AfterAccept, _))) {
            self.die_now();
        }
        Ok(Reply::Accepted)
    }
}

/// The pool, through `zfs` and `zpool`.
#[derive(Debug, Clone)]
pub struct FakeZfs(pub Rc<RefCell<World>>);

impl FakeZfs {
    fn pool<T>(&self, pool: &str, f: impl FnOnce(&Pool) -> T) -> Result<T, ZfsError> {
        let world = self.0.borrow();
        match &world.pool {
            Some(p) if pool == POOL => Ok(f(p)),
            _ => Err(ZfsError::NoPool),
        }
    }
}

impl Zfs for FakeZfs {
    fn layout(&mut self, pool: &str) -> Result<Layout, ZfsError> {
        self.pool(pool, |p| p.layout.clone())
    }

    fn props(&mut self, pool: &str) -> Result<Props, ZfsError> {
        self.0.borrow_mut().asked_still();
        self.0.borrow_mut().calls.push("props");
        self.pool(pool, |p| Props {
            guid: p.guid,
            load_guid: p.load_guid,
            multihost: false,
            checkpoint: false,
            readonly: false,
        })
    }

    fn nocacheflush(&mut self) -> Result<bool, ZfsError> {
        Ok(false)
    }

    fn snapshot(&mut self, pool: &str, t: &str) -> Result<Program, ZfsError> {
        self.pool(pool, |_| ())?;
        let mut world = self.0.borrow_mut();
        let program = world.take(t)?;
        let manifest = Manifest {
            pool: POOL.into(),
            pool_guid: world.pool.as_ref().map_or(0, |p| p.guid),
            snapshot: t.into(),
            createtxg: program.createtxg,
            members: program.members.clone(),
        };
        world.programs.insert(t.to_string(), manifest);
        Ok(program)
    }

    fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<record::Member>>, ZfsError> {
        self.0.borrow_mut().calls.push("members");
        let members = self.pool(pool, |p| {
            p.snaps.get(&format!("{POOL}@{t}"))?;
            let suffix = format!("@{t}");
            Some(
                p.snaps
                    .iter()
                    .filter_map(|(name, snap)| {
                        name.strip_suffix(&suffix).map(|ds| record::Member {
                            name: ds.to_string(),
                            snapshot_guid: snap.guid,
                        })
                    })
                    .collect(),
            )
        })?;
        self.0.borrow_mut().members_read = Some(members.clone());
        Ok(members)
    }

    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        self.pool(pool, |_| ())?;
        let mut world = self.0.borrow_mut();
        let Some(p) = &mut world.pool else {
            return Err(ZfsError::NoPool);
        };
        if !p.snaps.contains_key(&format!("{POOL}@{t}")) {
            return Err(ZfsError::NoSnapshot);
        }
        let suffix = format!("@{t}");
        let txg = p.next_txg.unwrap_or(p.txg + 1);
        let mut changed = false;
        for (name, snap) in p.snaps.iter_mut() {
            if name.ends_with(&suffix) && snap.holds.insert(tag.to_string()) {
                snap.held_in = Some(txg);
                changed = true;
            }
        }
        if changed {
            world.sync("hold");
        }
        Ok(())
    }

    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        self.pool(pool, |_| ())?;
        let mut world = self.0.borrow_mut();
        let Some(p) = &mut world.pool else {
            return Err(ZfsError::NoPool);
        };
        let suffix = format!("@{t}");
        let mut changed = false;
        for (name, snap) in p.snaps.iter_mut() {
            if name.ends_with(&suffix) && snap.holds.remove(tag) {
                changed = true;
            }
        }
        if changed {
            world.sync("release");
        }
        Ok(())
    }

    fn held(&mut self, pool: &str, t: &str, tag: &str) -> Result<bool, ZfsError> {
        self.0.borrow_mut().calls.push("held");
        let suffix = format!("@{t}");
        self.pool(pool, |p| {
            p.snaps.contains_key(&format!("{POOL}@{t}"))
                && p.snaps
                    .iter()
                    .filter(|(name, _)| name.ends_with(&suffix))
                    .all(|(_, s)| s.holds.contains(tag))
        })
    }

    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
        self.pool(pool, |p| {
            p.snaps
                .iter()
                .filter(|(_, s)| s.holds.contains(tag))
                .filter_map(|(name, _)| name.split_once('@').map(|(_, t)| t.to_string()))
                .collect()
        })
    }

    fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError> {
        self.pool(pool, |_| ())?;
        let mut world = self.0.borrow_mut();
        let now = world.now_ms;
        world.synced_at.push(now);
        // `zfs set` is a sync task: its own group; the `zpool sync` after
        // it writes a second, with the set's history record (the kernel
        // probe of #145, ZFS 2.4.1). A plain sync of an idle pool writes
        // no group. A property set to the value it holds writes nothing
        // here, where ZFS 2.4.1 writes two groups, as for a new value:
        // the conservative model. It is logged ("again"), which the test
        // of the agent's values reads.
        if world.plain_sync {
            world
                .log
                .push(format!("zpool sync alone (value {value}): no group"));
        } else if world.assists.last() == Some(&value) {
            world
                .log
                .push(format!("zpool sync alone (value {value} again): no group"));
        } else {
            world.assists.push(value);
            world.sync("zfs set skyzfs:assist");
            world.sync("zpool sync: the set's history record");
        }
        Ok(())
    }
}

/// The daemon's channel and device.
#[derive(Debug, Clone)]
pub struct FakeDaemon(pub Rc<RefCell<World>>);

impl Daemon for FakeDaemon {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        self.0.borrow_mut().answer(request, manifest)
    }

    fn device(&mut self) -> io::Result<u64> {
        Ok(DEVICE)
    }
}

/// The clock of the world.
#[derive(Debug, Clone)]
pub struct FakeClock(pub Rc<RefCell<World>>);

impl Clock for FakeClock {
    fn now_ms(&mut self) -> u64 {
        let world = self.0.borrow();
        world.now_ms.saturating_add_signed(world.agent_skew_ms)
    }

    fn mono_ms(&mut self) -> u64 {
        self.0.borrow().now_ms - EPOCH_MS
    }

    fn sleep(&mut self, d: Duration) {
        self.0.borrow_mut().advance(d);
    }
}

/// The agent's directory, in memory: a save that returned is kept.
#[derive(Debug, Clone, Default)]
pub struct MemKeep(pub Rc<RefCell<MemFiles>>);

/// The files of [`MemKeep`].
#[derive(Debug, Default)]
pub struct MemFiles {
    /// The record's bytes.
    pub record: Option<Vec<u8>>,
    /// The manifests.
    pub manifests: BTreeMap<String, Vec<u8>>,
}

impl Keep for MemKeep {
    fn load(&self) -> io::Result<Loaded> {
        Ok(match &self.0.borrow().record {
            None => Loaded::Absent,
            Some(bytes) => match Record::decode(bytes) {
                Ok(record) => Loaded::Present(record),
                Err(why) => Loaded::Corrupt(why),
            },
        })
    }

    fn save(&self, record: &Record) -> Result<(), SaveError> {
        let bytes = record.encode().map_err(SaveError::Rule)?;
        self.0.borrow_mut().record = Some(bytes);
        Ok(())
    }

    fn write_manifest(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.0
            .borrow_mut()
            .manifests
            .insert(name.to_string(), bytes.to_vec());
        Ok(())
    }

    fn read_manifest(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(self.0.borrow().manifests.get(name).cloned())
    }

    fn remove_manifest(&self, name: &str) -> io::Result<()> {
        self.0.borrow_mut().manifests.remove(name);
        Ok(())
    }

    fn manifest_names(&self) -> io::Result<Vec<String>> {
        Ok(self.0.borrow().manifests.keys().cloned().collect())
    }
}

/// The holds on the pool's snapshots, by `dataset@t`.
pub fn holds(world: &World) -> Vec<String> {
    world
        .pool
        .iter()
        .flat_map(|p| p.snaps.iter())
        .filter(|(_, s)| s.holds.contains(HOLD_TAG))
        .map(|(name, _)| name.clone())
        .collect()
}
