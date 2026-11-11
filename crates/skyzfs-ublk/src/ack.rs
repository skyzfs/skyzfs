//! What was acknowledged (design §4.2): a small file, kept outside the
//! store in the daemon's run directory, with the log position at which
//! the newest record starts that the daemon has acknowledged to the
//! kernel. Every start in the same boot compares it with what the
//! store's recovery found, to notice a log that came back shorter.
//!
//! # The file
//!
//! 64 bytes, little endian.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZACK1` |
//! | 8 | 16 | the store's identity (derived from its log's key, not the key) |
//! | 24 | 16 | the boot the record was made in |
//! | 40 | 8 | the position, or all ones for "nothing acknowledged yet" |
//! | 48 | 16 | zero |
//!
//! # The rules, and where each is kept
//!
//! - **A kill leaves the old value or the new one.** The file is made
//!   whole under another name and renamed into place, and from then on
//!   only the position is written: one write of eight bytes at a fixed
//!   offset. The file is never truncated or rewritten while it is in
//!   use.
//! - **It never moves back.** [`AckFile::advance`] writes only a
//!   position beyond the one it holds. With more than one queue, the
//!   queues share one [`SharedAck`], which compares under its lock, so
//!   two queues that store out of order leave the higher position.
//! - **It belongs to one store in one boot.** [`read`] gives the stored
//!   position together with the identity and the boot it was recorded
//!   for; a record for another store or another boot counts as none
//!   ([`Stored::position_for`]).
//! - **It is not synced.** It has to survive the daemon, not the
//!   machine: after a reboot no client is attached and the record
//!   starts empty. The run directory is expected to be in memory.
//! - **A record that cannot be read is an error**, not "none": a file
//!   that is there and is not a whole record cannot say that nothing
//!   was acknowledged. What the daemon does then is the caller's
//!   decision; the conservative one is to treat the volume as failed.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::pump::AckRecord;

const MAGIC: &[u8; 8] = b"SKYZACK1";
const LEN: usize = 64;
const POSITION_AT: u64 = 40;
const NONE: u64 = u64::MAX;

/// What a record holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stored {
    /// The identity of the store it was recorded for.
    pub identity: [u8; 16],
    /// The boot it was recorded in.
    pub boot: [u8; 16],
    /// The position, if anything was acknowledged.
    pub position: Option<u64>,
}

impl Stored {
    /// The position this record holds for the store `identity` in the
    /// boot `boot`: none if it was recorded for another store or in
    /// another boot.
    pub fn position_for(&self, identity: &[u8; 16], boot: &[u8; 16]) -> Option<u64> {
        (self.identity == *identity && self.boot == *boot)
            .then_some(self.position)
            .flatten()
    }
}

/// Read the record at `path`. `Ok(None)` if there is no file. A file
/// that is not a whole record is an error.
pub fn read(path: &Path) -> io::Result<Option<Stored>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    decode(&bytes).map(Some).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "not a record of what was acknowledged",
        )
    })
}

fn decode(bytes: &[u8]) -> Option<Stored> {
    if bytes.len() != LEN || &bytes[..8] != MAGIC || bytes[48..].iter().any(|b| *b != 0) {
        return None;
    }
    let position = u64::from_le_bytes(bytes[40..48].try_into().ok()?);
    Some(Stored {
        identity: bytes[8..24].try_into().ok()?,
        boot: bytes[24..40].try_into().ok()?,
        position: (position != NONE).then_some(position),
    })
}

fn encode(stored: &Stored) -> [u8; LEN] {
    let mut bytes = [0u8; LEN];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..24].copy_from_slice(&stored.identity);
    bytes[24..40].copy_from_slice(&stored.boot);
    bytes[40..48].copy_from_slice(&stored.position.unwrap_or(NONE).to_le_bytes());
    bytes
}

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// Any position is written, also one below the one held.
    pub(crate) const MOVES_BACK: u32 = 1 << 0;
    /// Opening a record for the same store and boot empties it.
    pub(crate) const OPEN_RESETS: u32 = 1 << 1;
    /// A new record is written in place, over the old one, in pieces.
    pub(crate) const CREATE_IN_PLACE: u32 = 1 << 2;

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// The record, open for writing.
#[derive(Debug)]
pub struct AckFile {
    file: fs::File,
    position: Option<u64>,
    mutants: Mutants,
}

impl AckFile {
    /// Open the record at `path` for the store `identity` in the boot
    /// `boot`. If the file holds a record for this store and this boot,
    /// it is kept as it is, position included: a daemon that takes a
    /// volume over has acknowledged nothing yet and must not forget
    /// what the one before it did. Otherwise (no file, another store,
    /// another boot) a new, empty record takes its place.
    ///
    /// Call it only after the stored position has been compared with
    /// what the store recovered.
    pub fn open(path: &Path, identity: [u8; 16], boot: [u8; 16]) -> io::Result<Self> {
        Self::open_with(path, identity, boot, Mutants::default())
    }

    pub(crate) fn open_with(
        path: &Path,
        identity: [u8; 16],
        boot: [u8; 16],
        mutants: Mutants,
    ) -> io::Result<Self> {
        let kept = match read(path)? {
            Some(stored) if stored.identity == identity && stored.boot == boot => {
                (!mutants.has(Mutants::OPEN_RESETS)).then_some(stored.position)
            }
            _ => None,
        };
        let position = match kept {
            Some(position) => position,
            None => {
                Self::replace(path, identity, boot, mutants)?;
                None
            }
        };
        let file = fs::OpenOptions::new().write(true).open(path)?;
        Ok(Self {
            file,
            position,
            mutants,
        })
    }

    /// Put a new, empty record at `path`, whatever is there: for a
    /// volume whose failure was acknowledged while no client held it.
    pub fn reset(path: &Path, identity: [u8; 16], boot: [u8; 16]) -> io::Result<Self> {
        Self::replace(path, identity, boot, Mutants::default())?;
        let file = fs::OpenOptions::new().write(true).open(path)?;
        Ok(Self {
            file,
            position: None,
            mutants: Mutants::default(),
        })
    }

    /// Write a whole new record beside `path` and rename it into place.
    fn replace(
        path: &Path,
        identity: [u8; 16],
        boot: [u8; 16],
        mutants: Mutants,
    ) -> io::Result<()> {
        let record = encode(&Stored {
            identity,
            boot,
            position: None,
        });
        if mutants.has(Mutants::CREATE_IN_PLACE) {
            let mut file = fs::File::create(path)?;
            return file.write_all(&record);
        }
        let mut beside = PathBuf::from(path).into_os_string();
        beside.push(".new");
        let beside = PathBuf::from(beside);
        let mut file = fs::File::create(&beside)?;
        file.write_all(&record)?;
        drop(file);
        fs::rename(&beside, path)
    }

    /// The position the record holds.
    pub fn position(&self) -> Option<u64> {
        self.position
    }

    /// Store `position` unless the record holds that or a higher one
    /// already. One write of eight bytes in place.
    pub fn advance(&mut self, position: u64) -> io::Result<()> {
        if position == NONE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no such position",
            ));
        }
        let back = self.position.is_some_and(|held| position <= held);
        if back && !self.mutants.has(Mutants::MOVES_BACK) {
            return Ok(());
        }
        self.file
            .write_all_at(&position.to_le_bytes(), POSITION_AT)?;
        self.position = Some(position);
        Ok(())
    }
}

/// One record shared by the queues of a device: each queue's pump holds
/// a clone, and the comparison is made under one lock, so the stored
/// position never moves back whatever order the queues store in.
#[derive(Debug, Clone)]
pub struct SharedAck(Arc<Mutex<AckFile>>);

impl SharedAck {
    /// Share `file`.
    pub fn new(file: AckFile) -> Self {
        Self(Arc::new(Mutex::new(file)))
    }
}

impl AckRecord for SharedAck {
    fn advance(&mut self, position: u64) -> io::Result<()> {
        // A queue that panicked while it held the lock had either
        // written its position or not: the record is whole either way.
        let mut file = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        file.advance(position)
    }
}

#[cfg(test)]
mod tests;
