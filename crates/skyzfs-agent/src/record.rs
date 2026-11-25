//! The agent's record of a volume (the plan of M2.6, §2): what it has
//! enrolled, the publishes it has under way and how far each has come,
//! and the names it has given up and why. The agent acts on what it
//! sees of the pool and the daemon; the record is what it cannot see
//! there: which snapshots are its own, what it asked, and what it holds.
//!
//! # The file
//!
//! One file, [`RECORD`] in the agent's directory of the volume. Its
//! bytes are the magic [`MAGIC`], the version [`VERSION`] (u16, little
//! endian), flags (u16, zero), the body's length (u32), the body
//! (postcard), and the BLAKE3 hash of everything before it. It is
//! written as a file beside it, synced, renamed over it, and the
//! directory synced: a crash leaves the old record or the new one.
//!
//! A file that does not read as a record of this version, whose hash
//! does not match, or whose body breaks a rule of the record (more than
//! [`UNDER_WAY_MAX`] under way, a name given twice, a hold believed
//! placed before the snapshot was taken) loads [`Loaded::Corrupt`]. The
//! agent then publishes nothing and reports, and [`Store::save`] does
//! not write over it: what it held (which snapshots are held, and by
//! whom) is for a person to judge. A record of another version is
//! corrupt to this agent, as the plan says (no reader of other versions
//! yet).
//!
//! # The manifests
//!
//! A publish's ZFS manifest (design §5.4, [`crate::manifest`]) is in
//! [`MANIFESTS`]`/<t>` beside the record, written and synced before the
//! record names it (phase [`Phase::Taken`]), and removed once the
//! publish has left the record.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The record's magic.
pub const MAGIC: [u8; 4] = *b"SZAG";

/// The record's version.
pub const VERSION: u16 = 1;

/// The publishes that may be under way at once (the lead's Q11).
pub const UNDER_WAY_MAX: usize = 8;

/// The names given up that the record keeps: the newest 64. They are
/// for M2.7's retention, which decides what becomes of their snapshots.
pub const ABANDONED_KEPT: usize = 64;

/// The record's file in the agent's directory of a volume.
pub const RECORD: &str = "record";

/// The directory of the manifests in the agent's directory of a volume.
pub const MANIFESTS: &str = "manifests";

/// The magic, the version, the flags and the body's length.
const HEAD: usize = 4 + 2 + 2 + 4;

/// The BLAKE3 hash at the end.
const SUM: usize = 32;

/// The agent's record of a volume ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The volume's id in object storage.
    pub volume: [u8; 16],
    /// The pool and the leaf the agent enrolled at its first publish
    /// (the lead's Q4); `None` before it.
    pub enrolled: Option<Enrolled>,
    /// The publishes under way, oldest first.
    pub under_way: Vec<Publish>,
    /// The names given up, oldest first.
    pub abandoned: Vec<Abandoned>,
}

/// What the agent enrolled at its first publish: the pool on the volume
/// and the leaf vdev that is the volume (the lead's Q4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrolled {
    /// The pool's name.
    pub pool: String,
    /// The pool's GUID.
    pub pool_guid: u64,
    /// The data leaf's vdev GUID.
    pub leaf_guid: u64,
}

/// A publish under way: snapshot `pool@<name>` and the cut that holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publish {
    /// The snapshot's name, `t` ([`crate::names`]).
    pub name: String,
    /// The count the publish is asked after.
    pub count: Count,
    /// How far it has come.
    pub phase: Phase,
    /// Whether the agent believes its hold placed on `pool@<name>`.
    pub held: bool,
    /// The requests made of the daemon for it.
    pub attempts: u32,
}

/// The daemon's count of cuts that a publish is asked after, read before
/// the snapshot was taken or after its hold was verified (the plan, §3,
/// "The rule for AskAnew"), and the pool's `load_guid` read with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    /// The daemon's attachment, A.
    pub attachment: [u8; 16],
    /// The store's cut state, S.
    pub state: [u8; 16],
    /// The cuts counted in it, C.
    pub after: u64,
    /// The pool's `load_guid`, which a new import changes (rule (L)).
    pub load_guid: u64,
}

/// How far a publish has come (the plan, §2). Each is recorded once its
/// step is done and before the next begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// The name chosen and the count read: the snapshot may or may not
    /// have been taken.
    Intent,
    /// The snapshot taken and its manifest on disk.
    Taken(Taken),
    /// The hold placed, and the members, the layout and `load_guid`
    /// verified.
    Held(Taken),
    /// The daemon answered `accepted`; the agent holds the snapshot until
    /// `held_until_ms` while the daemon does not answer. Times are the
    /// wall clock, in ms since 1970.
    Asked {
        /// What was taken.
        taken: Taken,
        /// When the daemon last answered `accepted`.
        asked_ms: u64,
        /// The last `accepted`, plus the expiry, plus the channel's
        /// margin.
        held_until_ms: u64,
    },
    /// Published: `snap/<name>` names `commit`, and `look` read it as this
    /// publish's.
    Done {
        /// What was taken.
        taken: Taken,
        /// The commit `snap/<name>` names.
        commit: [u8; 32],
    },
}

/// What the snapshot is: what its manifest says of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Taken {
    /// `createtxg` of `pool@<name>`: G of the request.
    pub group: u64,
    /// Each member's `@<name>`, as the manifest names it.
    pub members: Vec<Member>,
    /// The manifest's length in bytes.
    pub manifest_len: u32,
    /// The manifest's BLAKE3 hash.
    pub manifest_hash: [u8; 32],
}

/// A member's snapshot: the dataset's name and the GUID of its `@<name>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Member {
    /// The dataset's name.
    pub name: String,
    /// The GUID of its `@<name>`.
    pub snapshot_guid: u64,
}

/// A name given up, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abandoned {
    /// The snapshot's name.
    pub name: String,
    /// Why.
    pub why: Why,
    /// When (the wall clock, in ms since 1970).
    pub at_ms: u64,
}

/// Why a publish was given up (the plan, §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Why {
    /// `pool@<name>` exists and its manifest does not: the agent stopped
    /// between the program and the manifest's file (the lead's Q5). The
    /// snapshot is kept, unpublished.
    ManifestLost,
    /// The manifest is over the protocol's limit.
    ManifestTooLarge,
    /// `pool@<name>` was destroyed before it was published.
    Destroyed,
    /// A member's `@<name>` is not the one the manifest names (destroyed,
    /// or destroyed and taken again).
    Replaced,
    /// The pool's layout or its state no longer allows a publish.
    Premise,
    /// `snap/<name>` names a snapshot that is not this publish's.
    NameTaken,
    /// A rewind of the pool lost `pool@<name>`.
    Rewound,
}

/// Why a record's bytes are not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Corrupt {
    /// Shorter than a record of no body.
    #[error("{0} bytes: shorter than any record")]
    Short(usize),
    /// Another magic.
    #[error("the magic {0:?} is not SZAG")]
    Magic([u8; 4]),
    /// Another version than [`VERSION`].
    #[error("version {0}: this agent reads version {VERSION}")]
    Version(u16),
    /// Flags that are not zero.
    #[error("flags {0:#x}: none are defined")]
    Flags(u16),
    /// A length that is not the body's.
    #[error("a body of {said} bytes said, {is} there")]
    Length {
        /// What the head says.
        said: u64,
        /// What is there.
        is: u64,
    },
    /// The hash does not match.
    #[error("the hash does not match the bytes")]
    Hash,
    /// The body does not decode.
    #[error("the body does not decode: {0}")]
    Body(String),
    /// The body breaks a rule of the record.
    #[error("{0}")]
    Rule(String),
}

impl Record {
    /// A record of `volume` with nothing in it.
    pub fn new(volume: [u8; 16]) -> Self {
        Record {
            volume,
            enrolled: None,
            under_way: Vec::new(),
            abandoned: Vec::new(),
        }
    }

    /// The publish under way of `name`.
    pub fn publish(&self, name: &str) -> Option<&Publish> {
        self.under_way.iter().find(|p| p.name == name)
    }

    /// Give up the publish of `name`: it leaves those under way, and the
    /// name is kept with `why` among the newest [`ABANDONED_KEPT`].
    pub fn abandon(&mut self, name: &str, why: Why, at_ms: u64) {
        self.under_way.retain(|p| p.name != name);
        self.abandoned.push(Abandoned {
            name: name.to_string(),
            why,
            at_ms,
        });
        let over = self.abandoned.len().saturating_sub(ABANDONED_KEPT);
        self.abandoned.drain(..over);
    }

    /// Whether the record keeps its rules: at most [`UNDER_WAY_MAX`]
    /// under way, each with a name the protocol takes and given once, a
    /// snapshot with members and a manifest within the protocol's limit,
    /// no hold believed placed before the snapshot was taken, and at
    /// most [`ABANDONED_KEPT`] names given up.
    pub fn check(&self) -> Result<(), Corrupt> {
        let rule = |why: String| Err(Corrupt::Rule(why));
        if self.under_way.len() > UNDER_WAY_MAX {
            return rule(format!(
                "{} publishes under way; at most {UNDER_WAY_MAX}",
                self.under_way.len()
            ));
        }
        if self.abandoned.len() > ABANDONED_KEPT {
            return rule(format!(
                "{} names given up kept; at most {ABANDONED_KEPT}",
                self.abandoned.len()
            ));
        }
        for (i, publish) in self.under_way.iter().enumerate() {
            let name = &publish.name;
            if !file_name(name) {
                return rule(format!("`{name}` is not a name the agent gives"));
            }
            if self.under_way[..i].iter().any(|p| p.name == *name) {
                return rule(format!("`{name}` is under way twice"));
            }
            match publish.phase.taken() {
                None if publish.held => {
                    return rule(format!("`{name}` is held before it was taken"));
                }
                None => {}
                Some(taken) if taken.members.is_empty() => {
                    return rule(format!("`{name}` has no members"));
                }
                Some(taken) if taken.manifest_len as usize > skyzfs_control::MANIFEST_MAX => {
                    return rule(format!(
                        "`{name}` has a manifest of {} bytes",
                        taken.manifest_len
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The record's bytes ([module docs](self)), or the rule it breaks.
    pub fn encode(&self) -> Result<Vec<u8>, Corrupt> {
        self.check()?;
        let body = postcard::to_allocvec(self).map_err(|e| Corrupt::Body(e.to_string()))?;
        Ok(frame(&body))
    }

    /// The record in `bytes`, or why it is not read.
    pub fn decode(bytes: &[u8]) -> Result<Self, Corrupt> {
        if bytes.len() < HEAD + SUM {
            return Err(Corrupt::Short(bytes.len()));
        }
        let (head, rest) = bytes.split_at(HEAD);
        let field = |at: usize| u16::from_le_bytes([head[at], head[at + 1]]);
        let magic = [head[0], head[1], head[2], head[3]];
        if magic != MAGIC {
            return Err(Corrupt::Magic(magic));
        }
        if field(4) != VERSION {
            return Err(Corrupt::Version(field(4)));
        }
        if field(6) != 0 {
            return Err(Corrupt::Flags(field(6)));
        }
        let said = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
        let is = rest.len() - SUM;
        if u64::from(said) != is as u64 {
            return Err(Corrupt::Length {
                said: u64::from(said),
                is: is as u64,
            });
        }
        let (body, sum) = rest.split_at(is);
        if blake3::hash(&bytes[..HEAD + is]).as_bytes() != sum {
            return Err(Corrupt::Hash);
        }
        let (record, left): (Record, &[u8]) =
            postcard::take_from_bytes(body).map_err(|e| Corrupt::Body(e.to_string()))?;
        if !left.is_empty() {
            return Err(Corrupt::Body(format!(
                "{} bytes after the body",
                left.len()
            )));
        }
        record.check()?;
        Ok(record)
    }
}

impl Phase {
    /// What was taken, from [`Phase::Taken`] on.
    pub fn taken(&self) -> Option<&Taken> {
        match self {
            Phase::Intent => None,
            Phase::Taken(taken) | Phase::Held(taken) => Some(taken),
            Phase::Asked { taken, .. } | Phase::Done { taken, .. } => Some(taken),
        }
    }
}

/// The bytes of a record whose body is `body`.
pub(crate) fn frame(body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEAD + body.len() + SUM);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    // A body over 4 GiB is no record: its length is written as the most,
    // and decode refuses it.
    let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(body);
    let sum = blake3::hash(&bytes);
    bytes.extend_from_slice(sum.as_bytes());
    bytes
}

/// Whether `name` may name a file of the record's directory: a name the
/// protocol takes, and not `.` or `..` (which it takes).
fn file_name(name: &str) -> bool {
    skyzfs_control::valid_name(name) && !name.starts_with('.')
}

/// What the agent's directory of a volume holds of its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// No record: the agent has published nothing from this directory.
    Absent,
    /// A file that is not a record ([module docs](self)).
    Corrupt(Corrupt),
    /// The record.
    Present(Record),
}

/// Why a record was not saved.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// The file there is not a record, and is not written over.
    #[error("the record there is corrupt ({0}), and is kept for a person to judge")]
    Corrupt(Corrupt),
    /// The record breaks a rule, and is not written.
    #[error("the record breaks a rule: {0}")]
    Rule(Corrupt),
    /// The file system's error.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The agent's directory of a volume: its record and its manifests.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The store in `dir`, which is made at the first save.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Store { dir: dir.into() }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The record there.
    pub fn load(&self) -> io::Result<Loaded> {
        let bytes = match fs::read(self.dir.join(RECORD)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Absent),
            Err(e) => return Err(e),
        };
        Ok(match Record::decode(&bytes) {
            Ok(record) => Loaded::Present(record),
            Err(why) => Loaded::Corrupt(why),
        })
    }

    /// Save `record` over the one there, which must be absent or a
    /// record ([module docs](self)).
    pub fn save(&self, record: &Record) -> Result<(), SaveError> {
        let bytes = record.encode().map_err(SaveError::Rule)?;
        if let Loaded::Corrupt(why) = self.load()? {
            return Err(SaveError::Corrupt(why));
        }
        make_dir(&self.dir)?;
        replace(&self.dir, RECORD, &bytes)?;
        Ok(())
    }

    /// Write the manifest of `name`, synced, before the record names it.
    pub fn write_manifest(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let dir = self.manifests(name)?;
        make_dir(&self.dir)?;
        make_dir(&dir)?;
        replace(&dir, name, bytes)
    }

    /// The manifest of `name`, or `None` where there is none.
    pub fn read_manifest(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        match fs::read(self.manifests(name)?.join(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Remove the manifest of `name`, synced; none there is removed.
    pub fn remove_manifest(&self, name: &str) -> io::Result<()> {
        let dir = self.manifests(name)?;
        match fs::remove_file(dir.join(name)) {
            Ok(()) => File::open(&dir)?.sync_all(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// The names whose manifests are on disk, sorted.
    pub fn manifest_names(&self) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(self.dir.join(MANIFESTS)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut names = Vec::new();
        for entry in entries {
            if let Some(name) = entry?.file_name().to_str() {
                if file_name(name) {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// The manifests' directory, for a file of `name`.
    fn manifests(&self, name: &str) -> io::Result<PathBuf> {
        if !file_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{name}` is not a name the agent gives"),
            ));
        }
        Ok(self.dir.join(MANIFESTS))
    }
}

/// Make `dir` where it is not, and sync its parent so that it stays.
fn make_dir(dir: &Path) -> io::Result<()> {
    match fs::create_dir(dir) {
        Ok(()) => match dir.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => File::open(parent)?.sync_all(),
            _ => Ok(()),
        },
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Put `bytes` at `dir/name` in one step: written beside it, synced,
/// renamed over it, and the directory synced.
fn replace(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp"));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, dir.join(name))?;
    File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests;
