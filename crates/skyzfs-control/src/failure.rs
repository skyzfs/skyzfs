//! The failure note: what a daemon leaves under the run directory when
//! its volume fails, or when a start refuses the volume as failed
//! (design §4.2, "a failed volume gives up its attachment"), and the
//! kernel's boot id, which the note carries.
//!
//! The daemon writes and reads it (`skyzfs_ublk::records`, where the
//! layout is documented, and `skyzfs_ublk::lifecycle`, which says what
//! each field means), and the agent reads it (M2.7c: the daemon is gone
//! by then, so the note is what tells the agent that the volume failed
//! under its pool). The code is here, std only, so that both read one
//! definition and the agent links nothing of the daemon's.

use std::fs;
use std::io;
use std::path::Path;

fn not_whole(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[..N]);
    out
}

/// The failure note's name in the volume's run directory
/// ([`crate::volume_run_dir`]).
pub const FAILED: &str = "failed";

const FAILED_MAGIC: &[u8; 8] = b"SKYZFAI1";
const FAILED_LEN: usize = 64;

/// Why a volume failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The log store's log failed: while it served the volume, or in the
    /// checkpoint that handed the volume to the in-place store at a start.
    /// No longer given since M1.3t removed the log store; kept so that a
    /// note of an older daemon is read (code 1).
    LogFailed,
    /// A start found the log shorter than what was acknowledged. No
    /// longer given since M1.3t (code 2).
    Short,
    /// The recovery of a start failed the log. No longer given since
    /// M1.3t (code 3).
    RecoveryFailed,
    /// A start found the log marked as failed.
    Marked,
    /// A start could not read the record of what was acknowledged. No
    /// longer given since M1.3t-b removed the record (code 5).
    PositionUnreadable,
    /// A start could not read the note for the volume's device.
    DeviceNoteUnreadable,
    /// A write or a sync of the volume's image failed: the in-place
    /// store's failure, while it served or in its own sync at its start.
    /// Both fail the volume (rule R11), and one reason covers both: what
    /// a later start does with the volume is the same for either, and
    /// the daemon's line at the failure says when it happened.
    ImageFailed,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Reason::LogFailed => "the log store's log failed",
            Reason::Short => "the log is shorter than what was acknowledged",
            Reason::RecoveryFailed => "the recovery's sync failed",
            Reason::Marked => "the volume is marked as failed",
            Reason::PositionUnreadable => "the record of what was acknowledged cannot be read",
            Reason::DeviceNoteUnreadable => "the note for the volume's device cannot be read",
            Reason::ImageFailed => "a write or sync of the volume's image failed",
        })
    }
}

/// Whether a client held the volume's device when the volume failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// No client can have held it.
    No,
    /// One did, or it has to be taken that one did.
    Yes,
    /// It could not be told when the failure was noted. A later start
    /// settles it by what the driver then says about the recorded
    /// number, and writes the answer into the note.
    Unknown,
}

impl From<bool> for Held {
    fn from(held: bool) -> Self {
        if held {
            Held::Yes
        } else {
            Held::No
        }
    }
}

/// What the failure note says: see "the failure note" in the
/// documentation of `skyzfs_ublk::lifecycle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    /// Whether a client held the volume's device at the failure.
    pub held: Held,
    /// Why the volume failed.
    pub reason: Reason,
    /// The store's identity, or zeros where it was not known.
    pub identity: [u8; 16],
}

/// A failure note as it is on the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredFailure {
    /// The boot it was written in.
    pub boot: [u8; 16],
    /// What it says.
    pub failure: Failure,
    /// When it was written, in milliseconds since boot.
    pub written_ms: u64,
}

const REASONS: [(u8, Reason); 7] = [
    (1, Reason::LogFailed),
    (2, Reason::Short),
    (3, Reason::RecoveryFailed),
    (4, Reason::Marked),
    (5, Reason::PositionUnreadable),
    (6, Reason::DeviceNoteUnreadable),
    (7, Reason::ImageFailed),
];

/// The note's bytes, as the daemon writes them (whole, under another
/// name, then renamed: `skyzfs_ublk::records`).
pub fn encode_failure(stored: &StoredFailure) -> [u8; FAILED_LEN] {
    let mut bytes = [0u8; FAILED_LEN];
    bytes[..8].copy_from_slice(FAILED_MAGIC);
    bytes[8..24].copy_from_slice(&stored.boot);
    bytes[24..40].copy_from_slice(&stored.failure.identity);
    bytes[40] = match stored.failure.held {
        Held::No => 0,
        Held::Yes => 1,
        Held::Unknown => 2,
    };
    let code = REASONS.iter().find(|(_, r)| *r == stored.failure.reason);
    bytes[41] = code.map_or(0, |(code, _)| *code);
    bytes[48..56].copy_from_slice(&stored.written_ms.to_le_bytes());
    bytes
}

/// The note in `bytes`; `None` where they are not a whole note of
/// this version.
pub fn decode_failure(bytes: &[u8]) -> Option<StoredFailure> {
    let whole = bytes.len() == FAILED_LEN
        && &bytes[..8] == FAILED_MAGIC
        && bytes[40] <= 2
        && bytes[42..48] == [0; 6]
        && bytes[56..] == [0; 8];
    if !whole {
        return None;
    }
    let (_, reason) = REASONS.iter().find(|(code, _)| *code == bytes[41])?;
    Some(StoredFailure {
        boot: array(&bytes[8..]),
        failure: Failure {
            held: match bytes[40] {
                0 => Held::No,
                1 => Held::Yes,
                _ => Held::Unknown,
            },
            reason: *reason,
            identity: array(&bytes[24..]),
        },
        written_ms: u64::from_le_bytes(array(&bytes[48..])),
    })
}

/// Read the failure note at `path`, of whatever boot. `Ok(None)` if
/// there is none. A file that is not a whole note is an error.
pub fn read_failure(path: &Path) -> io::Result<Option<StoredFailure>> {
    let Some(bytes) = read_if_there(path)? else {
        return Ok(None);
    };
    decode_failure(&bytes)
        .map(Some)
        .ok_or_else(|| not_whole("not a failure note"))
}

fn read_if_there(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Parse the kernel's `boot_id`: a UUID in text.
pub fn parse_boot_id(text: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = text.trim().bytes().filter(|b| *b != b'-').collect();
    if hex.len() != 32 || !hex.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut boot = [0u8; 16];
    for (byte, pair) in boot.iter_mut().zip(hex.chunks(2)) {
        let pair = std::str::from_utf8(pair).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(boot)
}

/// Where the kernel names this boot ([`parse_boot_id`]).
pub const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// This boot, as the kernel names it.
pub fn boot_id() -> io::Result<[u8; 16]> {
    let text = fs::read_to_string(BOOT_ID)?;
    parse_boot_id(&text).ok_or_else(|| not_whole("the kernel's boot id is not a UUID"))
}

#[cfg(test)]
mod tests;
