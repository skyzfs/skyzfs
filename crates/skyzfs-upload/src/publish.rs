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
//! one whose `snap/t` names a cut that carries it, with its manifest,
//! answers done ([`Answer::Done`]).
//!
//! **The attachment and the cut state.** A request names the attachment
//! and the cut state it read *C* in. One of another attachment is
//! refused: *C* counts in an attachment's state, and a publish asked on
//! one attachment is not carried out on the next. Within one uploader the
//! attachment does not change, and a new uploader has nothing pending.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use skynbd_types::{CommitHash, SnapId, VolumeId};
use skyzfs_local::inplace::CommittedCut;

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

/// The prefix of a snapshot's row among the refs object's meta rows.
pub const SNAP_ROW_PREFIX: &str = "skyzfs/snap/";

/// The version of the snapshot's row this uploader writes and reads.
pub const SNAP_ROW_VERSION: u16 = 1;

/// The length of a snapshot's row of version 1.
pub const SNAP_ROW_LEN: usize = 2 + 16 + 8 + 8 + 16 + 4 + 32;

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

    /// Whether `row` records this publish: a cut that carries it, and
    /// this manifest.
    fn recorded_in(&self, row: &SnapRow, commit: CommitHash) -> bool {
        let cut = Known {
            cut: row.cut,
            itself: true,
            commit,
        };
        self.carried_by(&cut)
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
    /// The request names another cut state than this daemon's.
    #[error("another cut state")]
    State,
    /// The manifest is over [`MANIFEST_MAX`].
    #[error("a manifest of {0} bytes; at most {MANIFEST_MAX}")]
    Manifest(usize),
    /// The name is pending with other parameters.
    #[error("the name is pending with other parameters")]
    Asked,
    /// [`PENDING_MAX`] publishes are pending.
    #[error("{PENDING_MAX} publishes are pending")]
    Busy,
    /// `snap/<name>` exists and does not record this publish: it names
    /// another commit, or a cut that does not carry the request, or
    /// another manifest, or it has no row of version 1.
    #[error("the name names another commit")]
    Taken,
}

/// The answer to a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Pending: [`Publishing::progress`] says how it ends.
    Accepted,
    /// Published already, on this commit.
    Done(CommitHash),
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
    /// Refused once it was looked up in object storage.
    Refused(Refusal),
    /// Not asked of this uploader, or its outcome has left the view.
    Unknown,
}

/// A snapshot's row (`skyzfs/snap/<t>`), version 1, 86 bytes, little
/// endian:
///
/// | bytes | what |
/// |---|---|
/// | 0..2 | the version, 1 |
/// | 2..18 | the cut state's name |
/// | 18..26 | the cut's number in that state |
/// | 26..34 | the cut's transaction group |
/// | 34..50 | the snapshot's id (a ULID, big endian), which names its manifests |
/// | 50..54 | the ZFS manifest's length |
/// | 54..86 | the ZFS manifest's BLAKE3 hash |
///
/// The cut it names opens as itself: no other is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapRow {
    /// The cut published.
    pub cut: CommittedCut,
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
        let mut snap = [0; 16];
        snap.copy_from_slice(field(34, 16));
        let mut len = [0; 4];
        len.copy_from_slice(field(50, 4));
        let mut manifest_hash = [0; 32];
        manifest_hash.copy_from_slice(field(54, 32));
        Ok(SnapRow {
            cut: CommittedCut {
                state,
                number: u64_at(18),
                group: u64_at(26),
            },
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

#[derive(Debug, Clone)]
struct Pending {
    request: Request,
    snap: SnapId,
    asked: Instant,
    stage: Stage,
}

#[derive(Debug, Clone)]
enum Outcome {
    /// Published, or found so, with this row on this commit.
    Done(SnapRow, CommitHash),
    Refused(Refusal),
}

/// The publishes of one uploader ([module docs](self)).
#[derive(Debug, Clone)]
pub struct Publishing {
    attachment: Option<[u8; 16]>,
    state: Option<[u8; 16]>,
    pending: Vec<Pending>,
    known: VecDeque<Known>,
    outcomes: BTreeMap<String, Outcome>,
    order: VecDeque<String>,
    figures: PublishFigures,
}

impl Default for Publishing {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Publishing {
    /// The publishes of an uploader in `attachment`: with none, every
    /// request is refused ([`Refusal::Attachment`]).
    pub fn new(attachment: Option<[u8; 16]>) -> Self {
        Self {
            attachment,
            state: None,
            pending: Vec::new(),
            known: VecDeque::new(),
            outcomes: BTreeMap::new(),
            order: VecDeque::new(),
            figures: PublishFigures::default(),
        }
    }

    /// The attachment this uploader serves.
    pub fn set_attachment(&mut self, attachment: [u8; 16]) {
        self.attachment = Some(attachment);
    }

    /// The cut state the store holds its cuts in. Until it is known, a
    /// request of any state is accepted, and waits for a cut of its own.
    pub fn set_state(&mut self, state: [u8; 16]) {
        self.state = Some(state);
    }

    /// The figures since the uploader began.
    pub fn figures(&self) -> &PublishFigures {
        &self.figures
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
        if !valid_name(&request.name) {
            return Answer::Refused(Refusal::Name);
        }
        if self.attachment != Some(request.attachment) {
            return Answer::Refused(Refusal::Attachment);
        }
        if self.state.is_some_and(|state| state != request.state) {
            return Answer::Refused(Refusal::State);
        }
        if request.manifest.len() > MANIFEST_MAX {
            return Answer::Refused(Refusal::Manifest(request.manifest.len()));
        }
        if let Some(pending) = self.pending.iter().find(|p| p.request.name == request.name) {
            return if pending.request == request {
                Answer::Accepted
            } else {
                Answer::Refused(Refusal::Asked)
            };
        }
        match self.outcomes.get(&request.name) {
            Some(Outcome::Done(row, commit)) => {
                // Published by this uploader, or found so: done again
                // where it carries this request with its manifest.
                return if request.recorded_in(row, *commit) {
                    Answer::Done(*commit)
                } else {
                    Answer::Refused(Refusal::Taken)
                };
            }
            // Asked again after a refusal: looked up anew.
            Some(Outcome::Refused(_)) => {
                self.outcomes.remove(&request.name);
                self.order.retain(|name| *name != request.name);
            }
            None => {}
        }
        if self.pending.len() >= PENDING_MAX {
            return Answer::Refused(Refusal::Busy);
        }
        self.figures.accepted += 1;
        self.pending.push(Pending {
            request,
            snap: SnapId(ulid::Ulid::new()),
            asked: Instant::now(),
            stage: Stage::Look,
        });
        Answer::Accepted
    }

    /// How the publish of `name` stands.
    pub fn progress(&self, name: &str) -> Progress {
        if self.pending.iter().any(|p| p.request.name == name) {
            return Progress::Pending;
        }
        match self.outcomes.get(name) {
            Some(Outcome::Done(_, commit)) => Progress::Done(*commit),
            Some(Outcome::Refused(why)) => Progress::Refused(why.clone()),
            None => Progress::Unknown,
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
    /// or looked up again. The oldest asked first.
    pub(crate) fn next(&self) -> Option<(String, Stage)> {
        self.pending
            .iter()
            .filter(|p| p.stage != Stage::Wait)
            .min_by_key(|p| p.asked)
            .map(|p| (p.request.name.clone(), p.stage))
    }

    fn at(&mut self, name: &str) -> Option<usize> {
        self.pending.iter().position(|p| p.request.name == name)
    }

    /// `snap/<name>` is absent: the publish goes on the first known cut
    /// that carries it, or waits for one.
    pub(crate) fn absent(&mut self, name: &str) {
        let Some(i) = self.at(name) else { return };
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
