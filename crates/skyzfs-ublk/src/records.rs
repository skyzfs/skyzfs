//! The lifecycle's records as files (design §4.2, "attachment across
//! restarts").
//!
//! # The attachment record
//!
//! One file in the volume's directory, [`ATTACHMENT`]. It says which
//! device was added for the volume, in which boot, and with what. It is
//! written **durably** before the device is added: made whole under
//! another name, synced, renamed into place, and the directory synced,
//! so a kill leaves the old record or the new one, and a power cut
//! after the add cannot leave a device with no record. Reading it does
//! not open the store.
//!
//! 64 bytes, little endian.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZATT1` |
//! | 8 | 4 | the device's number |
//! | 12 | 2 | queues |
//! | 14 | 2 | depth of each queue |
//! | 16 | 16 | the nonce |
//! | 32 | 16 | the boot the record was made in |
//! | 48 | 8 | the device's size in bytes |
//! | 56 | 8 | the driver's flags the device was added with |
//!
//! # The note for a device
//!
//! One file per device number in the run directory, `ublk<N>.note`
//! ([`note_path`]). It repeats the nonce, and is written only after the
//! device was added. A device whose note carries the record's nonce was
//! added for this volume, by this record; a number that was reused by
//! someone else's device has no note or another's. It is made whole
//! under another name and renamed, and not synced: the run directory is
//! expected to be in memory, and after a reboot there is no device.
//!
//! 32 bytes, little endian.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZNOT1` |
//! | 8 | 4 | the device's number |
//! | 12 | 4 | zero |
//! | 16 | 16 | the nonce |
//!
//! # What belongs to a volume in the run directory
//!
//! The run directory is shared by every daemon on the machine, so
//! nothing in it is per run directory. A device note is named by its
//! device number, which is unique on the machine. Everything that
//! belongs to a volume is under `<run dir>/v-<dev>-<ino>/`
//! ([`volume_run_dir`]): the device and inode numbers of the volume's
//! directory, in hex. There is the record of what was acknowledged
//! ([`ACKNOWLEDGED`]). An inode number can be used again within a boot
//! once a volume is removed, which is why that record carries the
//! store's identity as well.
//!
//! # The failure note
//!
//! [`FAILED`] in the volume's directory under the run directory: the
//! volume failed in this boot (design §4.2, "a failed volume gives up
//! its attachment"), its log or the in-place store's image. Made whole under another name and
//! renamed; not synced, because it has to live exactly as long as the
//! boot. A note of another boot counts as none. It has no checksum:
//! it is replaced whole by a rename on a memory file system, and one
//! whose length or magic is not exactly this version's is not a whole
//! note, which counts as a failure while a client held the device.
//!
//! 64 bytes, little endian.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZFAI1` |
//! | 8 | 16 | the boot it was written in |
//! | 24 | 16 | the store's identity, or zeros where it was not known |
//! | 40 | 1 | whether a client held the device: 0 none did, 1 one did, 2 not known when the note was written |
//! | 41 | 1 | the reason: 1 the log failed while serving, 2 the log was short, 3 the recovery failed the log, 4 marked as failed, 5 the acknowledged position could not be read, 6 the device's note could not be read, 7 a write or sync of the in-place store's image failed (while serving, or in its own sync at its start) |
//! | 42 | 6 | zero |
//! | 48 | 8 | when it was written: milliseconds since boot |
//! | 56 | 8 | zero |
//!
//! **Code 7 is newer than the others** (M1.3x). A daemon older than it
//! does not know the code, so a note with it is not a whole note to
//! that daemon: its start refuses the volume as failed while a client
//! held its device, for the rest of the boot and whatever
//! `--acknowledge-failure` says ("the failure note cannot be read"),
//! and deletes a device of the volume that waits for a server. Its
//! `skyzfsd status` says the same. The note lives in the run directory
//! until the next boot: to serve the volume with such a daemon before
//! then, after the failure has been dealt with, remove the note by
//! hand. This version reads all seven codes, and refuses an unknown
//! one in the same way.
//!
//! The syncs are of the record's file and of its directory only. The
//! daemon never syncs a file system or the whole machine: such a sync
//! does not return while any ZFS pool on the machine is suspended.
//!
//! # A file that is not a whole record
//!
//! is an error, never "no record": neither file is ever written in
//! place, so a file that is there and does not decode was not left by a
//! kill of this daemon.
//!
//! # Where the design is silent
//!
//! The file names and the layouts are this module's choice. The boot
//! is the kernel's `boot_id`; the nonce is 16 bytes of the kernel's
//! random source. The volume's directory is examined by its path when
//! the records are made, not through a descriptor the daemon holds.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ack;
use crate::lifecycle::{Attachment, Failure, Held, MustReach, Reason, Records, Shape};

/// The attachment record's name in the volume's directory.
pub const ATTACHMENT: &str = "attachment";
/// The name, in the volume's directory under the run directory, of the
/// record of what was acknowledged.
pub const ACKNOWLEDGED: &str = "acknowledged";

/// The failure note's name in the volume's directory under the run
/// directory.
pub const FAILED: &str = "failed";

const FAILED_MAGIC: &[u8; 8] = b"SKYZFAI1";
const FAILED_LEN: usize = 64;
const ATTACHMENT_MAGIC: &[u8; 8] = b"SKYZATT1";
const ATTACHMENT_LEN: usize = 64;
const NOTE_MAGIC: &[u8; 8] = b"SKYZNOT1";
const NOTE_LEN: usize = 32;

/// Where the note for device `number` is, under `run_dir`.
pub fn note_path(run_dir: &Path, number: u32) -> PathBuf {
    run_dir.join(format!("ublk{number}.note"))
}

/// Where the volume in `volume_dir` keeps what is its own under
/// `run_dir`: named by the device and inode numbers of the volume's
/// directory.
pub fn volume_run_dir(run_dir: &Path, volume_dir: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(volume_dir)?;
    Ok(run_dir.join(format!("v-{:x}-{:x}", meta.dev(), meta.ino())))
}

fn not_whole(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[..N]);
    out
}

fn encode_attachment(a: &Attachment) -> [u8; ATTACHMENT_LEN] {
    let mut bytes = [0u8; ATTACHMENT_LEN];
    bytes[..8].copy_from_slice(ATTACHMENT_MAGIC);
    bytes[8..12].copy_from_slice(&a.number.to_le_bytes());
    bytes[12..14].copy_from_slice(&a.shape.queues.to_le_bytes());
    bytes[14..16].copy_from_slice(&a.shape.depth.to_le_bytes());
    bytes[16..32].copy_from_slice(&a.nonce);
    bytes[32..48].copy_from_slice(&a.boot);
    bytes[48..56].copy_from_slice(&a.shape.size.to_le_bytes());
    bytes[56..64].copy_from_slice(&a.shape.flags.to_le_bytes());
    bytes
}

fn decode_attachment(bytes: &[u8]) -> Option<Attachment> {
    if bytes.len() != ATTACHMENT_LEN || &bytes[..8] != ATTACHMENT_MAGIC {
        return None;
    }
    Some(Attachment {
        number: u32::from_le_bytes(array(&bytes[8..])),
        nonce: array(&bytes[16..]),
        boot: array(&bytes[32..]),
        shape: Shape {
            size: u64::from_le_bytes(array(&bytes[48..])),
            queues: u16::from_le_bytes(array(&bytes[12..])),
            depth: u16::from_le_bytes(array(&bytes[14..])),
            flags: u64::from_le_bytes(array(&bytes[56..])),
        },
    })
}

fn encode_note(number: u32, nonce: &[u8; 16]) -> [u8; NOTE_LEN] {
    let mut bytes = [0u8; NOTE_LEN];
    bytes[..8].copy_from_slice(NOTE_MAGIC);
    bytes[8..12].copy_from_slice(&number.to_le_bytes());
    bytes[16..].copy_from_slice(nonce);
    bytes
}

fn decode_note(bytes: &[u8], number: u32) -> Option<[u8; 16]> {
    let whole = bytes.len() == NOTE_LEN
        && &bytes[..8] == NOTE_MAGIC
        && bytes[8..12] == number.to_le_bytes()
        && bytes[12..16] == [0; 4];
    whole.then(|| array(&bytes[16..]))
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

fn encode_failure(stored: &StoredFailure) -> [u8; FAILED_LEN] {
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

fn decode_failure(bytes: &[u8]) -> Option<StoredFailure> {
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

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// A record is written in place, over the old one.
    pub(crate) const IN_PLACE: u32 = 1 << 0;
    /// A file that does not decode counts as no record.
    pub(crate) const TORN_IS_NONE: u32 = 1 << 1;
    /// A note is taken for any device's.
    pub(crate) const ANY_NUMBER: u32 = 1 << 2;
    /// What was acknowledged is kept at the top of the run directory,
    /// one file for every volume.
    pub(crate) const ACK_AT_TOP: u32 = 1 << 3;

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Put `bytes` at `path`: whole under another name, then renamed.
/// `durable` syncs the file before the rename and the directory after.
///
/// **No test notices if one of the syncs is taken out.** The tests run
/// on a real file system and never cut its power, so they cannot tell
/// a synced record from one in the cache. That the attachment record
/// is durable before the add (the file's sync and the directory's
/// here, and the directory's sync in `remove_attachment`) rests on
/// reading these lines.
fn put(path: &Path, bytes: &[u8], durable: bool, mutants: Mutants) -> io::Result<()> {
    if mutants.has(Mutants::IN_PLACE) {
        return fs::File::create(path)?.write_all(bytes);
    }
    let mut beside = PathBuf::from(path).into_os_string();
    beside.push(".new");
    let beside = PathBuf::from(beside);
    let mut file = fs::File::create(&beside)?;
    file.write_all(bytes)?;
    if durable {
        file.sync_all()?;
    }
    drop(file);
    fs::rename(&beside, path)?;
    if durable {
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        fs::File::open(dir.unwrap_or(Path::new(".")))?.sync_all()?;
    }
    Ok(())
}

/// Read the attachment record of the volume in `volume_dir`, without
/// opening the store. `Ok(None)` if there is none.
pub fn read_attachment(volume_dir: &Path) -> io::Result<Option<Attachment>> {
    read_attachment_with(volume_dir, Mutants::default())
}

fn read_attachment_with(volume_dir: &Path, mutants: Mutants) -> io::Result<Option<Attachment>> {
    let Some(bytes) = read_if_there(&volume_dir.join(ATTACHMENT))? else {
        return Ok(None);
    };
    match decode_attachment(&bytes) {
        Some(attachment) => Ok(Some(attachment)),
        None if mutants.has(Mutants::TORN_IS_NONE) => Ok(None),
        None => Err(not_whole("not an attachment record")),
    }
}

/// Read the nonce in the note for device `number` under `run_dir`.
/// `Ok(None)` if there is no note.
pub fn read_note(run_dir: &Path, number: u32) -> io::Result<Option<[u8; 16]>> {
    read_note_with(run_dir, number, Mutants::default())
}

fn read_note_with(run_dir: &Path, number: u32, mutants: Mutants) -> io::Result<Option<[u8; 16]>> {
    let Some(bytes) = read_if_there(&note_path(run_dir, number))? else {
        return Ok(None);
    };
    let claimed = if mutants.has(Mutants::ANY_NUMBER) && bytes.len() >= 12 {
        u32::from_le_bytes(array(&bytes[8..]))
    } else {
        number
    };
    match decode_note(&bytes, claimed) {
        Some(nonce) => Ok(Some(nonce)),
        None if mutants.has(Mutants::TORN_IS_NONE) => Ok(None),
        None => Err(not_whole("not a note for this device")),
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

/// This boot, as the kernel names it.
pub fn boot_id() -> io::Result<[u8; 16]> {
    let text = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    parse_boot_id(&text).ok_or_else(|| not_whole("the kernel's boot id is not a UUID"))
}

/// The lifecycle's records, in a volume's directory and a run
/// directory.
#[derive(Debug)]
pub struct Files {
    volume_dir: PathBuf,
    run_dir: PathBuf,
    volume_run_dir: PathBuf,
    boot: [u8; 16],
    pause: Duration,
    mutants: Mutants,
}

impl Files {
    /// The records of the volume in `volume_dir`, with the daemon's
    /// notes in `run_dir`, in the boot `boot`. `pause` is how long
    /// [`Records::pause`] waits before it is asked again. Creates
    /// nothing. An error if the volume's directory cannot be examined.
    pub fn new(
        volume_dir: &Path,
        run_dir: &Path,
        boot: [u8; 16],
        pause: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            volume_dir: volume_dir.into(),
            run_dir: run_dir.into(),
            volume_run_dir: volume_run_dir(run_dir, volume_dir)?,
            boot,
            pause,
            mutants: Mutants::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with(mut self, mutants: u32) -> Self {
        self.mutants = Mutants(mutants);
        self
    }

    /// The volume's own directory under the run directory.
    pub fn volume_run_dir(&self) -> &Path {
        &self.volume_run_dir
    }

    /// Make the run directory and the volume's directory under it, if
    /// they are not there.
    pub fn create_run_dirs(&self) -> io::Result<()> {
        fs::create_dir_all(&self.volume_run_dir)
    }

    /// Where the failure note is.
    pub fn failure_path(&self) -> PathBuf {
        self.volume_run_dir.join(FAILED)
    }

    /// Where the record of what was acknowledged is.
    pub fn acknowledged_path(&self) -> PathBuf {
        if self.mutants.has(Mutants::ACK_AT_TOP) {
            return self.run_dir.join(ACKNOWLEDGED);
        }
        self.volume_run_dir.join(ACKNOWLEDGED)
    }
}

impl Records for Files {
    fn boot(&self) -> [u8; 16] {
        self.boot
    }

    fn nonce(&mut self) -> io::Result<[u8; 16]> {
        let mut nonce = [0u8; 16];
        fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
        Ok(nonce)
    }

    fn attachment(&mut self) -> io::Result<Option<Attachment>> {
        read_attachment_with(&self.volume_dir, self.mutants)
    }

    fn write_attachment(&mut self, attachment: &Attachment) -> io::Result<()> {
        put(
            &self.volume_dir.join(ATTACHMENT),
            &encode_attachment(attachment),
            true,
            self.mutants,
        )
    }

    fn note(&mut self, number: u32) -> io::Result<Option<[u8; 16]>> {
        read_note_with(&self.run_dir, number, self.mutants)
    }

    fn write_note(&mut self, number: u32, nonce: &[u8; 16]) -> io::Result<()> {
        put(
            &note_path(&self.run_dir, number),
            &encode_note(number, nonce),
            false,
            self.mutants,
        )
    }

    fn remove_attachment(&mut self) -> io::Result<()> {
        match fs::remove_file(self.volume_dir.join(ATTACHMENT)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            other => other?,
        }
        fs::File::open(&self.volume_dir)?.sync_all()
    }

    fn remove_note(&mut self, number: u32) -> io::Result<()> {
        match fs::remove_file(note_path(&self.run_dir, number)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn acknowledged(&mut self) -> io::Result<Option<MustReach>> {
        // A record of another boot says nothing: no client holds a
        // device across a reboot. The store it was recorded for is
        // compared inside the store's open, which alone knows its own
        // identity.
        let stored = ack::read(&self.acknowledged_path())?;
        Ok(stored.filter(|s| s.boot == self.boot).and_then(|s| {
            s.position.map(|position| MustReach {
                identity: s.identity,
                position,
            })
        }))
    }

    fn remove_acknowledged(&mut self) -> io::Result<()> {
        // No file is an empty record: the next daemon makes a new one.
        match fs::remove_file(self.acknowledged_path()) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn failure(&mut self) -> io::Result<Option<Failure>> {
        let stored = read_failure(&self.failure_path())?;
        Ok(stored.filter(|s| s.boot == self.boot).map(|s| s.failure))
    }

    fn write_failure(&mut self, failure: &Failure) -> io::Result<()> {
        // The volume's directory under the run directory may not be
        // there yet: a start can fail before anything was served.
        fs::create_dir_all(&self.volume_run_dir)?;
        let stored = StoredFailure {
            boot: self.boot,
            failure: *failure,
            // A note without its time is still the note.
            written_ms: crate::status::uptime_ms().unwrap_or(0),
        };
        put(
            &self.failure_path(),
            &encode_failure(&stored),
            false,
            self.mutants,
        )
    }

    fn remove_failure(&mut self) -> io::Result<()> {
        match fs::remove_file(self.failure_path()) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn pause(&mut self) {
        std::thread::sleep(self.pause);
    }
}

#[cfg(test)]
mod tests;
