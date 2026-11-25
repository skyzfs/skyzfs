//! Publishing a cut as a snapshot (design §5.2, "A snapshot is published
//! by a cut of the pool's present history", and §5.4; work package M2.5).
//!
//! The agent reads the daemon's count of cuts *C*, takes `pool@t`, reads
//! its `createtxg` *G*, and asks the daemon to publish `snap/t`
//! ([`Request`]). The daemon publishes the first cut **counted after
//! *C***, **of group *G* or later**, **that opens as itself**, of the cut
//! state the request names ([`Request::carried_by`]): such a cut was
//! taken after the snapshot was asked for, on the history that holds it.
//! "First" is the first such cut this uploader knows: the cuts it has
//! committed, or found committed, since it began ([`KNOWN_CUTS`] of them
//! at most), then each cut it commits after the request. A cut that is
//! abandoned is never committed, so the publish waits for the next
//! committed cut, which took its map (the lead's Q2 of M2.5: across a
//! restart, a publish can so land on a later cut than the first counted
//! after *C*; that cut qualifies all the same).
//!
//! **A publish is its own refs transaction**, made once the cut's commit
//! has landed: the ZFS manifest ([`manifest_key`]) and SkyNBD's framed
//! snapshot manifest are PUT, then one transaction creates `snap/t` on the
//! commit (expecting no such ref) and puts the snapshot's row
//! ([`snap_row_key`], [`SnapRow`]). The ref so names only a commit known
//! to be whole in object storage, and a crash before the transaction
//! leaves no `snap/t` (design §5.2, "Atomic visibility"). The transaction
//! goes through the same refs as the uploader's commits, fenced by the
//! lease's epoch. (The plan put it in the head swap's own transaction;
//! that needs the commit's hash before the commit is made, and a name
//! taken would fail the head swap itself. M2.5-1's report.)
//!
//! **Pending publishes are kept in memory** (the lead's Q4 of M2.5): they
//! are lost with the uploader, and the agent asks again. Asking twice is
//! asking once: a request the same as one pending is accepted again, and
//! one asked again after it ended is looked up anew, so that `snap/t`
//! deleted since is not answered done from memory (F5 of #122's review).
//! A lookup that finds `snap/t` named by a row that records the request
//! (its commit, a cut that carries it, its manifest) ends it as done.
//!
//! **The attachment and the cut state.** A request names the attachment
//! and the cut state it read *C* in. One of another attachment, or of
//! another cut state than the store's, is refused: *C* counts in an
//! attachment's state, and a publish asked on one attachment is not
//! carried out on the next. Both are known before any request is taken
//! ([`Publishing::new`]); a request asked while either is not known is
//! refused (F2 of #122's review: a request of another state taken before
//! the uploader knew its own would wait for ever). Within one uploader
//! neither changes, and a new uploader has nothing pending; a setter that
//! changes one ends what pends of the other ([`Publishing::set_state`],
//! [`Publishing::set_attachment`]).
//!
//! **A stopped uploader publishes nothing** ([`Publishing::stop`]): once
//! its lease is lost for good, or its store kept a committed cut, every
//! publish pending ends refused ([`Refusal::Stopped`]), and so does every
//! request after (F4 of #122's review).
//!
//! **A publish has a deadline** (the lead's decision on Q1 of #122's
//! review): the agent holds `pool@t` while its publish pends, and a
//! publish made after the agent gave up and let the hold go could name a
//! cut taken after `pool@t` was destroyed. So a request names how long it
//! may pend ([`Request::expiry`]: at most [`EXPIRY_MAX`], and
//! [`EXPIRY_DEFAULT`] where the agent names none), counted from when it
//! is accepted. Once that time is up no transaction of it is made: it
//! ends expired ([`Progress::Expired`]). [`Publishing::expire`], given
//! the time, ends it: the driver calls it before each publish step, so a
//! transaction is not begun after the deadline, and the uploader at each
//! turn, so `publish?` reads expired while nothing else is done (before
//! the lease is held, or idle). A
//! publish whose transaction failed, and so may have landed, is looked up
//! first: it ends done if the transaction landed, and expired if not, and
//! the transaction is not made again. A transaction begun before the
//! deadline can land after it; its cut was given to the publish before
//! the deadline, so it was taken while the agent held the snapshot. The
//! agent lets its hold go once `publish?` says how the publish ended, or
//! once the expiry has passed by its own clock with a margin for the
//! channel (M2.6). The agent can also withdraw a publish
//! ([`Publishing::withdraw`]): it ends the same way, at once.
//!
//! **A rewind ends a publish that waits** (work package M2.6a, rule (R)
//! of its plan). The store counts as a rewind's every cut whose group is
//! not above the group of the cut before it, and the count it started
//! with (`skyzfs_local::inplace::Cuts::rewound`): after `zpool import
//! -F` the first group of the new history has the abandoned best's
//! number and opens as itself, so without the rule a publish asked
//! before the rewind would be carried by a cut of a history that may
//! lack `pool@t`. A request asked after a lower count than the mark is
//! refused ([`Refusal::Rewound`]), and a publish pending that has no cut
//! yet (to be looked up, or waiting) ends so once the mark passes its
//! count ([`Publishing::set_rewound`]). The agent then checks `pool@t`
//! and asks again with a count read anew (M2.6). The driver gives the
//! mark at each check of a cut, before that cut can be committed, so no
//! cut is given to a publish with a rewind counted between the
//! publish's count and the cut; a publish given its cut before a later
//! rewind goes on, as that cut was counted on the history it was asked
//! on. Between two checks the mark here can be behind the store's: a
//! request asked then is accepted and ends `rewound` at the next check,
//! before any cut after the rewind is committed. With the mark in memory,
//! a request asked of the daemon before a restart is refused by the
//! next (the lead's answer to the plan's Q1, in place of M2.5's new
//! deadline there).

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use tokio::time::Instant;

use skynbd_types::{CommitHash, SnapId, VolumeId};
use skyzfs_local::inplace::CommittedCut;

use crate::backoff::Backoff;

/// The largest ZFS manifest a request carries: 1 MiB. Its format is the
/// agent's (M2.6); the daemon stores it as it came.
pub const MANIFEST_MAX: usize = 1 << 20;

/// The cuts committed, or found committed, that the uploader keeps in
/// view for a request that comes after their commit: the newest 64. A
/// request whose first qualifying cut has left the view goes to a later
/// one, which qualifies too.
pub const KNOWN_CUTS: usize = 64;

/// The publishes that may be pending at once. One more is refused
/// ([`Refusal::Busy`]): the agent asks for one a minute (M2.6), so the
/// bound is reached only when no cut has been committed for a long
/// while, and a request refused is asked again.
pub const PENDING_MAX: usize = 64;

/// The outcomes kept for [`Publishing::progress`]: the newest 1024.
pub const OUTCOMES_KEPT: usize = 1024;

/// How long a publish may pend where the agent names no expiry: ten
/// minutes. The channel (M2.5-2) fills it in.
pub const EXPIRY_DEFAULT: Duration = Duration::from_secs(10 * 60);

/// The longest a publish may pend: an hour.
pub const EXPIRY_MAX: Duration = Duration::from_secs(60 * 60);

/// The prefix of a snapshot's row among the refs object's meta rows.
pub const SNAP_ROW_PREFIX: &str = "skyzfs/snap/";

/// The version of the snapshot's row this uploader writes and reads.
pub const SNAP_ROW_VERSION: u16 = 1;

/// The length of a snapshot's row of version 1.
pub const SNAP_ROW_LEN: usize = 2 + 16 + 8 + 8 + 32 + 16 + 4 + 32;

/// The ref of snapshot `name`, as SkyNBD names a snapshot's ref.
pub fn snap_ref(name: &str) -> String {
    format!("snap/{name}")
}

/// The meta row of snapshot `name`.
pub fn snap_row_key(name: &str) -> String {
    format!("{SNAP_ROW_PREFIX}{name}")
}

/// The object that holds the ZFS manifest of snapshot `snap` of volume
/// `vol`, beside SkyNBD's own objects of the volume. Garbage collection
/// does not know it (M4): a deleted snapshot leaves it behind.
pub fn manifest_key(vol: VolumeId, snap: SnapId) -> String {
    format!(
        "{}skyzfs/manifest/{snap}",
        skynbd_engine::keys::volume_prefix(vol)
    )
}

/// Whether `name` may name a snapshot: 1 to 128 of `[A-Za-z0-9._-]`, as
/// SkyNBD's engine requires of a snapshot's name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

/// A request to publish a cut as `snap/<name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The snapshot's name, `t`.
    pub name: String,
    /// The attachment the agent read the count of cuts in: the
    /// attachment record's nonce.
    pub attachment: [u8; 16],
    /// The cut state the count is of (`CutState::id`).
    pub state: [u8; 16],
    /// The count of cuts the agent read before it took the snapshot, *C*.
    pub after: u64,
    /// The snapshot's `createtxg`, *G*.
    pub group: u64,
    /// The ZFS manifest of design §5.4: opaque, at most [`MANIFEST_MAX`].
    pub manifest: Vec<u8>,
    /// How long the publish may pend, from when it is accepted: more
    /// than zero and at most [`EXPIRY_MAX`]; [`EXPIRY_DEFAULT`] where the
    /// agent names none. No cut is given to it and no transaction of it
    /// is made after that ([module docs](self)).
    pub expiry: Duration,
}

impl Request {
    /// Whether `cut` carries this publish: of the request's cut state,
    /// counted after *C*, of group *G* or later, and opening as itself.
    pub fn carried_by(&self, cut: &Known) -> bool {
        cut.cut.state == self.state
            && cut.cut.number > self.after
            && cut.cut.group >= self.group
            && cut.itself
    }

    /// Whether `row`, found beside a ref that names `commit`, records this
    /// publish: it names that commit (a ref made anew beside a row left
    /// behind is not this publish, F6 of #122's review), a cut that
    /// carries the request, and this manifest.
    fn recorded_in(&self, row: &SnapRow, commit: CommitHash) -> bool {
        let cut = Known {
            cut: row.cut,
            itself: true,
            commit,
        };
        row.commit == commit
            && self.carried_by(&cut)
            && row.manifest_len as usize == self.manifest.len()
            && row.manifest_hash == *blake3::hash(&self.manifest).as_bytes()
    }
}

/// A cut committed in object storage, as the uploader knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    /// The cut, as its row names it.
    pub cut: CommittedCut,
    /// Whether it opens as itself.
    pub itself: bool,
    /// Its commit.
    pub commit: CommitHash,
}

/// Why a request is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The name is not 1 to 128 of `[A-Za-z0-9._-]`.
    #[error("a snapshot's name is 1 to 128 of [A-Za-z0-9._-]")]
    Name,
    /// The request names another attachment than this daemon's.
    #[error("another attachment")]
    Attachment,
    /// The request names another cut state than the store's, or the
    /// store's is not known.
    #[error("another cut state")]
    State,
    /// The manifest is over [`MANIFEST_MAX`].
    #[error("a manifest of {0} bytes; at most {MANIFEST_MAX}")]
    Manifest(usize),
    /// The expiry is zero or over [`EXPIRY_MAX`].
    #[error("an expiry of {0:?}; more than zero and at most {EXPIRY_MAX:?}")]
    Expiry(Duration),
    /// The name is pending with other parameters.
    #[error("the name is pending with other parameters")]
    Asked,
    /// [`PENDING_MAX`] publishes are pending.
    #[error("{PENDING_MAX} publishes are pending")]
    Busy,
    /// `snap/<name>` exists and does not record this publish: its row
    /// names another commit than the ref, or a cut that does not carry
    /// the request, or another manifest, or it has no row of version 1.
    #[error("the name names another commit")]
    Taken,
    /// The uploader publishes nothing more: its lease is lost for good, or
    /// its store kept a committed cut ([`Publishing::stop`]). A
    /// transaction of the publish that failed may have landed all the
    /// same: a new uploader asked again finds it.
    #[error("this uploader publishes nothing more")]
    Stopped,
    /// A cut that may follow a rewind was counted after the count the
    /// request names, or the daemon started after it ([module
    /// docs](self)): read the count again, and ask anew.
    #[error("a rewind may have been counted after the count asked from")]
    Rewound,
}

/// The answer to a request. A request is answered with no I/O, so it is
/// never answered done: one published already is accepted, and the
/// lookup ends it ([`Publishing::progress`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Pending: [`Publishing::progress`] says how it ends.
    Accepted,
    /// Refused, and nothing is pending.
    Refused(Refusal),
}

/// How a publish stands, by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Accepted and not ended.
    Pending,
    /// Published on this commit.
    Done(CommitHash),
    /// Refused once it was looked up in object storage, or ended by the
    /// uploader's stop.
    Refused(Refusal),
    /// Not made by its deadline ([`Request::expiry`]), and dropped.
    Expired,
    /// Withdrawn by the agent ([`Publishing::withdraw`]), and dropped.
    Withdrawn,
    /// Not asked of this uploader, or its outcome has left the view.
    Unknown,
}

/// A snapshot's row (`skyzfs/snap/<t>`), version 1, 118 bytes, little
/// endian:
///
/// | bytes | what |
/// |---|---|
/// | 0..2 | the version, 1 |
/// | 2..18 | the cut state's name |
/// | 18..26 | the cut's number in that state |
/// | 26..34 | the cut's transaction group |
/// | 34..66 | the commit `snap/<t>` was made on: the cut's |
/// | 66..82 | the snapshot's id (a ULID, big endian), which names its manifests |
/// | 82..86 | the ZFS manifest's length |
/// | 86..118 | the ZFS manifest's BLAKE3 hash |
///
/// The cut it names opens as itself: no other is published. The row
/// names its commit so that a ref deleted and made anew on another
/// commit, beside the row left behind (SkyNBD deletes a snapshot's ref
/// and not the row), does not read as this publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapRow {
    /// The cut published.
    pub cut: CommittedCut,
    /// The commit `snap/<t>` was made on.
    pub commit: CommitHash,
    /// The snapshot's id: its SkyNBD manifest and its ZFS manifest are
    /// named by it.
    pub snap: SnapId,
    /// The ZFS manifest's length.
    pub manifest_len: u32,
    /// Its BLAKE3 hash.
    pub manifest_hash: [u8; 32],
}

/// Why a snapshot's row cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapRowError {
    /// Of another version than [`SNAP_ROW_VERSION`].
    #[error("a snapshot's row of version {0}; this uploader reads version {SNAP_ROW_VERSION}")]
    Version(u16),
    /// Of version 1 with another length, or too short to hold a version.
    #[error("a snapshot's row of {0} bytes; version {SNAP_ROW_VERSION} is {SNAP_ROW_LEN}")]
    Length(usize),
}

impl SnapRow {
    /// The row's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut row = Vec::with_capacity(SNAP_ROW_LEN);
        row.extend_from_slice(&SNAP_ROW_VERSION.to_le_bytes());
        row.extend_from_slice(&self.cut.state);
        row.extend_from_slice(&self.cut.number.to_le_bytes());
        row.extend_from_slice(&self.cut.group.to_le_bytes());
        row.extend_from_slice(&self.commit);
        row.extend_from_slice(&self.snap.0.to_bytes());
        row.extend_from_slice(&self.manifest_len.to_le_bytes());
        row.extend_from_slice(&self.manifest_hash);
        row
    }

    /// The row in `bytes`: its version first, then its length.
    pub fn decode(bytes: &[u8]) -> Result<Self, SnapRowError> {
        let version = match bytes {
            [a, b, ..] => u16::from_le_bytes([*a, *b]),
            _ => return Err(SnapRowError::Length(bytes.len())),
        };
        if version != SNAP_ROW_VERSION {
            return Err(SnapRowError::Version(version));
        }
        if bytes.len() != SNAP_ROW_LEN {
            return Err(SnapRowError::Length(bytes.len()));
        }
        let field = |at: usize, len: usize| &bytes[at..at + len];
        let u64_at = |at: usize| {
            let mut word = [0; 8];
            word.copy_from_slice(field(at, 8));
            u64::from_le_bytes(word)
        };
        let mut state = [0; 16];
        state.copy_from_slice(field(2, 16));
        let mut commit = [0; 32];
        commit.copy_from_slice(field(34, 32));
        let mut snap = [0; 16];
        snap.copy_from_slice(field(66, 16));
        let mut len = [0; 4];
        len.copy_from_slice(field(82, 4));
        let mut manifest_hash = [0; 32];
        manifest_hash.copy_from_slice(field(86, 32));
        Ok(SnapRow {
            cut: CommittedCut {
                state,
                number: u64_at(18),
                group: u64_at(26),
            },
            commit,
            snap: SnapId(ulid::Ulid::from_bytes(snap)),
            manifest_len: u32::from_le_bytes(len),
            manifest_hash,
        })
    }
}

/// `snap/<name>` as object storage has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The commit the ref names.
    pub commit: CommitHash,
    /// Its row's bytes, where there is one.
    pub row: Option<Vec<u8>>,
}

/// What a publish's transaction writes ([`crate::Remote::publish`]).
#[derive(Debug, Clone, Copy)]
pub struct Snapshot<'a> {
    /// The snapshot's name.
    pub name: &'a str,
    /// Its id.
    pub snap: SnapId,
    /// Its row.
    pub row: &'a SnapRow,
    /// The ZFS manifest.
    pub manifest: &'a [u8],
}

/// The figures of publishing since the uploader began, for the status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishFigures {
    /// The requests accepted.
    pub accepted: u64,
    /// The publishes made by this uploader.
    pub published: u64,
    /// The requests found published already in object storage.
    pub found: u64,
    /// The requests refused, at once or once looked up.
    pub refused: u64,
    /// The publishes dropped at their deadline.
    pub expired: u64,
    /// The publishes withdrawn.
    pub withdrawn: u64,
    /// The last snapshot published or found: its name, its cut and its
    /// commit.
    pub last: Option<(String, CommittedCut, CommitHash)>,
}

/// Where a pending publish stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// To be looked up in object storage: whether `snap/<name>` exists.
    Look,
    /// Absent: waits for a cut that carries it.
    Wait,
    /// To be published on this cut.
    Ready(Known),
    /// A transaction that failed: to be looked up before it is made
    /// again, since no error says that it did not land.
    Recheck(Known),
}

/// Why a publish ends with nothing made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lapse {
    /// Its deadline passed.
    Expired,
    /// The agent withdrew it.
    Withdrawn,
}

#[derive(Debug, Clone)]
struct Pending {
    request: Request,
    snap: SnapId,
    asked: Instant,
    /// When its time is up: `asked` and its expiry.
    deadline: Instant,
    /// Set when its time is up or it is withdrawn while a transaction of
    /// it may have landed: it ends once looked up.
    lapse: Option<Lapse>,
    stage: Stage,
    /// The waits after its steps that failed ([`Publishing::wait_after`]).
    errors: Backoff,
    /// Before when it takes no step: set after a step of it failed.
    retry_at: Option<Instant>,
}

#[derive(Debug, Clone)]
enum Outcome {
    /// Published, or found so, with this row on this commit.
    Done(SnapRow, CommitHash),
    Refused(Refusal),
    Lapsed(Lapse),
}

impl Outcome {
    /// How a publish that ended so stands.
    fn progress(&self) -> Progress {
        match self {
            Outcome::Done(_, commit) => Progress::Done(*commit),
            Outcome::Refused(why) => Progress::Refused(why.clone()),
            Outcome::Lapsed(Lapse::Expired) => Progress::Expired,
            Outcome::Lapsed(Lapse::Withdrawn) => Progress::Withdrawn,
        }
    }
}

/// The publishes of one uploader ([module docs](self)).
#[derive(Debug, Clone)]
pub struct Publishing {
    attachment: Option<[u8; 16]>,
    state: Option<[u8; 16]>,
    /// The store's rewind mark, as last given ([`Publishing::set_rewound`]).
    rewound: u64,
    stopped: bool,
    pending: Vec<Pending>,
    known: VecDeque<Known>,
    outcomes: BTreeMap<String, Outcome>,
    order: VecDeque<String>,
    figures: PublishFigures,
    /// The publishes ended since [`Publishing::take_ended`] last took
    /// them, in the order they ended: the newest [`OUTCOMES_KEPT`].
    ended: VecDeque<(String, Progress)>,
}

impl Default for Publishing {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl Publishing {
    /// The publishes of an uploader in `attachment`, whose store holds its
    /// cuts in cut state `state` (`InPlace::cut_state_id`). With no
    /// attachment every request is refused ([`Refusal::Attachment`]), and
    /// with no state every request of an attachment ([`Refusal::State`]).
    pub fn new(attachment: Option<[u8; 16]>, state: Option<[u8; 16]>) -> Self {
        Self {
            attachment,
            state,
            rewound: 0,
            stopped: false,
            pending: Vec::new(),
            known: VecDeque::new(),
            outcomes: BTreeMap::new(),
            order: VecDeque::new(),
            figures: PublishFigures::default(),
            ended: VecDeque::new(),
        }
    }

    /// The attachment this uploader serves. A publish pending of another
    /// attachment ends refused ([`Refusal::Attachment`]): it is not
    /// carried out on this one (N4 of #122's review).
    pub fn set_attachment(&mut self, attachment: [u8; 16]) {
        self.attachment = Some(attachment);
        self.end_where(|p| p.request.attachment != attachment, Refusal::Attachment);
    }

    /// The cut state the store holds its cuts in. The driver tells it at
    /// each check of a cut, where the store's state can be seen; a store's
    /// state does not change while it runs, so this only confirms the
    /// state given to [`Publishing::new`]. A publish pending of another
    /// state ends refused ([`Refusal::State`]): no cut of this store can
    /// carry it.
    pub fn set_state(&mut self, state: [u8; 16]) {
        self.state = Some(state);
        self.end_where(|p| p.request.state != state, Refusal::State);
    }

    /// The store's rewind mark (`skyzfs_local::inplace::Cuts::rewound`):
    /// the driver gives it at each check of a cut, and the daemon at the
    /// uploader's start. The mark never goes back. Each publish pending
    /// that has no cut yet and that was asked after a lower count ends
    /// refused ([`Refusal::Rewound`]); one given its cut goes on ([module
    /// docs](self)).
    pub fn set_rewound(&mut self, rewound: u64) {
        self.rewound = self.rewound.max(rewound);
        let mark = self.rewound;
        self.end_where(
            |p| p.request.after < mark && matches!(p.stage, Stage::Look | Stage::Wait),
            Refusal::Rewound,
        );
    }

    /// The uploader publishes nothing more: its lease is lost for good, or
    /// its store kept a committed cut. Every publish pending ends refused
    /// ([`Refusal::Stopped`]), and so does every request after. A publish
    /// whose cut is known ends so too: the uploader takes no more steps.
    pub fn stop(&mut self) {
        self.stopped = true;
        self.end_where(|_| true, Refusal::Stopped);
    }

    /// Whether [`Publishing::stop`] was called.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// End every pending publish for which `which` holds, refused `why`.
    fn end_where(&mut self, which: impl Fn(&Pending) -> bool, why: Refusal) {
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(which);
        self.pending = kept;
        for pending in ended {
            self.figures.refused += 1;
            self.end(&pending.request.name, Outcome::Refused(why.clone()));
        }
    }

    /// The figures since the uploader began.
    pub fn figures(&self) -> &PublishFigures {
        &self.figures
    }

    /// The publishes that ended since this was last called, in the order
    /// they ended, each with how it ended: for the daemon's request log
    /// (M2.5-2). A request refused at once never pended and is not here.
    /// The newest [`OUTCOMES_KEPT`] are kept between two calls.
    pub fn take_ended(&mut self) -> Vec<(String, Progress)> {
        self.ended.drain(..).collect()
    }

    /// The publishes pending, and the oldest's name and age.
    pub fn pending(&self) -> (usize, Option<(&str, Duration)>) {
        let oldest = self
            .pending
            .iter()
            .min_by_key(|p| p.asked)
            .map(|p| (p.request.name.as_str(), p.asked.elapsed()));
        (self.pending.len(), oldest)
    }

    /// Ask for a publish ([module docs](self)). Nothing is read or
    /// written here: a request accepted is looked up in object storage at
    /// the driver's next step.
    pub fn ask(&mut self, request: Request) -> Answer {
        let answer = self.answer(request);
        if matches!(answer, Answer::Refused(_)) {
            self.figures.refused += 1;
        }
        answer
    }

    fn answer(&mut self, request: Request) -> Answer {
        if self.stopped {
            return Answer::Refused(Refusal::Stopped);
        }
        if !valid_name(&request.name) {
            return Answer::Refused(Refusal::Name);
        }
        if self.attachment != Some(request.attachment) {
            return Answer::Refused(Refusal::Attachment);
        }
        if self.state != Some(request.state) {
            return Answer::Refused(Refusal::State);
        }
        if request.manifest.len() > MANIFEST_MAX {
            return Answer::Refused(Refusal::Manifest(request.manifest.len()));
        }
        if request.expiry.is_zero() || request.expiry > EXPIRY_MAX {
            return Answer::Refused(Refusal::Expiry(request.expiry));
        }
        if let Some(pending) = self
            .pending
            .iter_mut()
            .find(|p| p.request.name == request.name)
        {
            if pending.request != request {
                return Answer::Refused(Refusal::Asked);
            }
            // Withdrawn while a transaction of it may have landed, and so
            // still pending until it is looked up: the same request asked
            // again undoes the withdrawal, so that `Accepted` is true (D1
            // of the review of #122). A publish whose time is up stays so:
            // asked again while it pends, a publish keeps its deadline.
            if pending.lapse == Some(Lapse::Withdrawn) {
                pending.lapse = None;
            }
            return Answer::Accepted;
        }
        // After the same request pending, which is answered as it
        // stands: one given its cut before the rewind goes on.
        if request.after < self.rewound {
            return Answer::Refused(Refusal::Rewound);
        }
        if self.pending.len() >= PENDING_MAX {
            return Answer::Refused(Refusal::Busy);
        }
        // Asked again after it ended, done or refused: looked up anew, so
        // that a name deleted or taken since is seen as it is now (F5 of
        // #122's review).
        if self.outcomes.remove(&request.name).is_some() {
            self.order.retain(|name| *name != request.name);
        }
        self.figures.accepted += 1;
        let asked = Instant::now();
        self.pending.push(Pending {
            deadline: asked + request.expiry,
            request,
            snap: SnapId(ulid::Ulid::new()),
            asked,
            lapse: None,
            stage: Stage::Look,
            errors: Backoff::new(),
            retry_at: None,
        });
        Answer::Accepted
    }

    /// How the publish of `name` stands.
    pub fn progress(&self, name: &str) -> Progress {
        if self.pending.iter().any(|p| p.request.name == name) {
            return Progress::Pending;
        }
        self.outcomes
            .get(name)
            .map_or(Progress::Unknown, Outcome::progress)
    }

    /// End each publish whose time is up at `now` ([`Request::expiry`]):
    /// it is dropped and counted ([`Progress::Expired`]). One whose
    /// transaction failed, and so may have landed, is looked up first:
    /// it ends done if the transaction landed, expired if not, and its
    /// transaction is not made again.
    pub fn expire(&mut self, now: Instant) {
        for pending in &mut self.pending {
            if pending.lapse.is_none() && now >= pending.deadline {
                pending.lapse = Some(Lapse::Expired);
            }
        }
        self.end_lapsed();
    }

    /// Withdraw the publish of `name`: it ends at once
    /// ([`Progress::Withdrawn`]), unless a transaction of it failed and
    /// may have landed; then it is looked up first, and ends done if the
    /// transaction landed, withdrawn if not. A name not pending is left as
    /// it stands. How the name stands after.
    pub fn withdraw(&mut self, name: &str) -> Progress {
        if let Some(i) = self.at(name) {
            self.pending[i].lapse = Some(Lapse::Withdrawn);
            self.end_lapsed();
        }
        self.progress(name)
    }

    /// End each lapsed publish that has no transaction that may have
    /// landed.
    fn end_lapsed(&mut self) {
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.lapse.is_some() && !matches!(p.stage, Stage::Recheck(_)));
        self.pending = kept;
        for pending in ended {
            let Some(lapse) = pending.lapse else { continue };
            match lapse {
                Lapse::Expired => self.figures.expired += 1,
                Lapse::Withdrawn => self.figures.withdrawn += 1,
            }
            self.end(&pending.request.name, Outcome::Lapsed(lapse));
        }
    }

    /// Cut `known` is committed in object storage, by this uploader or
    /// found so: keep it in view, and give it to each publish that waits
    /// and that it carries. Cuts come in the order of their commits, so
    /// the first that carries a publish waiting is this one.
    pub fn committed(&mut self, known: Known) {
        if self.known.len() == KNOWN_CUTS {
            self.known.pop_front();
        }
        self.known.push_back(known);
        for pending in &mut self.pending {
            if pending.stage == Stage::Wait && pending.request.carried_by(&known) {
                pending.stage = Stage::Ready(known);
            }
        }
    }

    /// The publish that has work for the driver: to be looked up, made,
    /// or looked up again, and whose own wait after a step that failed
    /// has passed ([`Publishing::wait_after`]). The oldest asked first, so
    /// that a publish that cannot be made holds up no other (D2 of the
    /// review of #122).
    pub(crate) fn next(&self) -> Option<(String, Stage)> {
        let now = Instant::now();
        self.pending
            .iter()
            .filter(|p| p.stage != Stage::Wait && p.retry_at.is_none_or(|at| at <= now))
            .min_by_key(|p| p.asked)
            .map(|p| (p.request.name.clone(), p.stage))
    }

    /// When the first publish with work may take its step: now where one
    /// has no wait, and none where no publish has work (they wait for a
    /// cut, or none pends).
    pub fn due_at(&self) -> Option<Instant> {
        self.pending
            .iter()
            .filter(|p| p.stage != Stage::Wait)
            .map(|p| p.retry_at.unwrap_or(p.asked))
            .min()
    }

    /// A step of the publish of `name` that ended at `ended` failed on
    /// object storage: it takes no step for its wait, one second doubling
    /// to five minutes at each failure of it, in a row or not
    /// ([`crate::backoff`]). The wait is the publish's own: the other
    /// publishes step meanwhile, and a publish that ends, however it
    /// ends, takes its wait with it. A lookup that succeeds does not
    /// reset it, since the lookup can succeed while the transaction
    /// cannot be made (F3 of the review of #122). The wait, where the
    /// publish still pends.
    pub fn wait_after(&mut self, name: &str, ended: Instant) -> Option<Duration> {
        let i = self.at(name)?;
        let pending = &mut self.pending[i];
        let wait = pending.errors.failed();
        pending.retry_at = Some(ended + wait);
        Some(wait)
    }

    fn at(&mut self, name: &str) -> Option<usize> {
        self.pending.iter().position(|p| p.request.name == name)
    }

    /// `snap/<name>` is absent: the publish goes on the first known cut
    /// that carries it, or waits for one.
    pub(crate) fn absent(&mut self, name: &str) {
        let Some(i) = self.at(name) else { return };
        if self.pending[i].lapse.is_some() {
            // Its time is up, or it was withdrawn, after a transaction
            // that did not land: it ends, and is not made again.
            self.pending[i].stage = Stage::Look;
            self.end_lapsed();
            return;
        }
        let first = self
            .known
            .iter()
            .find(|known| self.pending[i].request.carried_by(known))
            .copied();
        let pending = &mut self.pending[i];
        pending.stage = match (pending.stage, first) {
            // Looked up again after a transaction that did not land: made
            // again on the same cut.
            (Stage::Recheck(known), _) => Stage::Ready(known),
            (_, Some(known)) => Stage::Ready(known),
            (_, None) => Stage::Wait,
        };
    }

    /// `snap/<name>` exists: done where it records the request, refused
    /// otherwise. Ends the publish either way. Whether it was done.
    pub(crate) fn found(&mut self, name: &str, found: &Found) -> Result<SnapRow, Refusal> {
        let Some(i) = self.at(name) else {
            return Err(Refusal::Taken);
        };
        let pending = self.pending.remove(i);
        let row = found
            .row
            .as_deref()
            .and_then(|bytes| SnapRow::decode(bytes).ok())
            .filter(|row| pending.request.recorded_in(row, found.commit));
        match row {
            Some(row) => {
                self.figures.found += 1;
                self.end(name, Outcome::Done(row, found.commit));
                Ok(row)
            }
            None => {
                self.figures.refused += 1;
                self.end(name, Outcome::Refused(Refusal::Taken));
                Err(Refusal::Taken)
            }
        }
    }

    /// The transaction for the publish of `name` on `known`: its id and
    /// its row, and its manifest.
    pub(crate) fn snapshot(&self, name: &str, known: &Known) -> Option<(SnapId, SnapRow, &[u8])> {
        let pending = self.pending.iter().find(|p| p.request.name == name)?;
        let manifest = pending.request.manifest.as_slice();
        let row = SnapRow {
            cut: known.cut,
            commit: known.commit,
            snap: pending.snap,
            manifest_len: manifest.len() as u32,
            manifest_hash: *blake3::hash(manifest).as_bytes(),
        };
        Some((pending.snap, row, manifest))
    }

    /// The transaction for `name` landed, with `row` on `commit`.
    pub(crate) fn published(&mut self, name: &str, row: SnapRow, commit: CommitHash) {
        let Some(i) = self.at(name) else { return };
        self.pending.remove(i);
        self.figures.published += 1;
        self.end(name, Outcome::Done(row, commit));
    }

    /// The transaction for `name` failed: look it up before it is made
    /// again.
    pub(crate) fn failed(&mut self, name: &str) {
        let Some(i) = self.at(name) else { return };
        if let Stage::Ready(known) = self.pending[i].stage {
            self.pending[i].stage = Stage::Recheck(known);
        }
    }

    fn end(&mut self, name: &str, outcome: Outcome) {
        if let Outcome::Done(row, commit) = &outcome {
            self.figures.last = Some((name.to_string(), row.cut, *commit));
        }
        if self.ended.len() == OUTCOMES_KEPT {
            self.ended.pop_front();
        }
        self.ended.push_back((name.to_string(), outcome.progress()));
        if self.outcomes.insert(name.to_string(), outcome).is_none() {
            self.order.push_back(name.to_string());
        }
        while self.order.len() > OUTCOMES_KEPT {
            if let Some(old) = self.order.pop_front() {
                self.outcomes.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests;
