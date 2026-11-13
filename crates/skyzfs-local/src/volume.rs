//! The volume file: what makes a directory a volume of the in-place
//! store, and what the store must know of it before it touches the
//! image.
//!
//! # The file
//!
//! [`VOLUME_NAME`] in the volume's directory. It is written once, by
//! [`Volume::create`]: under another name, synced, renamed, and the
//! directory synced. Nothing writes it again. Little endian.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZVOL1` |
//! | 8 | 16 | the volume's id |
//! | 24 | 8 | the volume's size in bytes |
//! | 32 | 4 | the volume's block size in bytes |
//! | 36 | 4 | the write unit: what the layers below the store rewrite as a whole when a part of it is written ([`write_unit_for`]) |
//! | 40 | 16 | the store's identity: random at create, and how a record outside the directory names this volume |
//! | 56 | 4 | CRC-32C of bytes 0 to 55 |
//!
//! **Versions.** The magic names the version, and a file is read only if
//! its length and its magic are exactly a version's, as the daemon's
//! status file is. This is the first. A later version (M2's) adds its
//! fields after these with a magic and a length of its own, and keeps
//! reading this one. A daemon that meets a version it does not know
//! refuses the volume and says so; it does not guess.
//!
//! A directory that holds the log store's superblock ([`LEGACY_NAME`])
//! was made by a daemon from before M1.3t and is refused: the volume is
//! to be created again. There is no conversion.

use std::io;

use crate::io::{Dir, File};

/// The volume file's name in the volume's directory.
pub const VOLUME_NAME: &str = "volume";

/// What the volume file is made under before it is renamed.
pub const VOLUME_NEW_NAME: &str = "volume.new";

/// The failure marker's name: written when the volume fails, so that a
/// later start finds it. Its content is nothing; its presence is all.
pub const MARKER_NAME: &str = "volume.failed";

/// The log store's superblock: a directory that holds it was made by a
/// daemon with a log store, before M1.3t.
pub const LEGACY_NAME: &str = "log.super";

const MAGIC: &[u8; 8] = b"SKYZVOL1";
const LEN: usize = 60;

/// The smallest write unit: no page and no physical sector is smaller.
pub const MIN_WRITE_UNIT: u32 = 4096;
/// The largest write unit a volume can have.
pub const MAX_WRITE_UNIT: u32 = 65536;

/// What the volume file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    /// The volume's id.
    pub id: [u8; 16],
    /// The volume's size in bytes.
    pub size: u64,
    /// The volume's block size in bytes.
    pub block_size: u32,
    /// The write unit: a power of two from [`MIN_WRITE_UNIT`] to
    /// [`MAX_WRITE_UNIT`].
    pub write_unit: u32,
    /// The store's identity.
    pub identity: [u8; 16],
}

/// Why a directory's volume file cannot be read. None of these fails
/// the volume: the start is refused and nothing is written.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VolumeError {
    /// The directory holds no volume file.
    #[error("the directory holds no volume file (`{VOLUME_NAME}`)")]
    Missing,
    /// The directory holds the log store's superblock.
    #[error(
        "the directory holds a volume created by a daemon with a log store (before M1.3t), \
         which this daemon does not serve; create the volume again"
    )]
    Legacy,
    /// A volume file of a version this daemon does not know.
    #[error(
        "the volume file is of a version this daemon does not know (magic {magic:?}, {len} \
         bytes): written by a newer daemon, or not a volume file"
    )]
    Unknown {
        /// Its first bytes, as far as they go.
        magic: String,
        /// Its length.
        len: usize,
    },
    /// A volume file of this version whose checksum or values do not
    /// hold.
    #[error("the volume file is damaged: {0}")]
    Damaged(&'static str),
    /// It could not be read.
    #[error("reading the volume file: {0:?}")]
    Io(io::ErrorKind),
}

/// The write unit of the system the daemon runs on: its page, within
/// what a volume can have.
pub fn system_write_unit() -> u32 {
    crate::fs::page_size().clamp(MIN_WRITE_UNIT, MAX_WRITE_UNIT)
}

/// The write unit to give a new volume in a directory whose file system
/// has blocks of `block` bytes ([`crate::io::Dir::block_size`]): the
/// larger of [`system_write_unit`] and the block. A file system writes
/// back whole blocks, so a write unit below its block would not be how
/// far a write reaches. `None` if that is no unit a volume can have: a
/// block that is not a power of two, or one above [`MAX_WRITE_UNIT`].
pub fn write_unit_for(block: u64) -> Option<u32> {
    let block = u32::try_from(block).ok().filter(|b| b.is_power_of_two())?;
    let unit = block.max(system_write_unit());
    (unit <= MAX_WRITE_UNIT).then_some(unit)
}

impl Volume {
    /// Whether the values can describe a volume.
    pub(crate) fn valid(&self) -> bool {
        self.check().is_ok()
    }

    fn check(&self) -> Result<(), VolumeError> {
        let unit = self.write_unit;
        if !unit.is_power_of_two() || !(MIN_WRITE_UNIT..=MAX_WRITE_UNIT).contains(&unit) {
            return Err(VolumeError::Damaged(
                "the write unit is not one a volume has",
            ));
        }
        if self.block_size == 0
            || self.size == 0
            || !self.size.is_multiple_of(u64::from(self.block_size))
        {
            return Err(VolumeError::Damaged(
                "the size is not a whole number of blocks",
            ));
        }
        Ok(())
    }

    fn encode(&self) -> [u8; LEN] {
        let mut bytes = [0u8; LEN];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..24].copy_from_slice(&self.id);
        bytes[24..32].copy_from_slice(&self.size.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.block_size.to_le_bytes());
        bytes[36..40].copy_from_slice(&self.write_unit.to_le_bytes());
        bytes[40..56].copy_from_slice(&self.identity);
        let crc = crc32c::crc32c(&bytes[..56]);
        bytes[56..60].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, VolumeError> {
        if bytes.len() != LEN || &bytes[..8] != MAGIC {
            let magic = bytes.get(..8).unwrap_or(bytes);
            return Err(VolumeError::Unknown {
                magic: String::from_utf8_lossy(magic).into_owned(),
                len: bytes.len(),
            });
        }
        let u32_at = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        if u32_at(56) != crc32c::crc32c(&bytes[..56]) {
            return Err(VolumeError::Damaged("its checksum does not match"));
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&bytes[8..24]);
        let mut size = [0u8; 8];
        size.copy_from_slice(&bytes[24..32]);
        let mut identity = [0u8; 16];
        identity.copy_from_slice(&bytes[40..56]);
        let volume = Self {
            id,
            size: u64::from_le_bytes(size),
            block_size: u32_at(32),
            write_unit: u32_at(36),
            identity,
        };
        volume.check()?;
        Ok(volume)
    }

    /// Write the volume file of `self` into `dir`: under another name,
    /// synced, renamed, and the directory synced. An earlier attempt's
    /// file under the other name is replaced. Fails if the directory
    /// holds a volume file already.
    pub fn create<D: Dir>(&self, dir: &D) -> io::Result<()> {
        self.check()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        if dir.list()?.iter().any(|name| name == VOLUME_NAME) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the directory holds a volume file already",
            ));
        }
        match dir.remove(VOLUME_NEW_NAME) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut file = dir.create(VOLUME_NEW_NAME)?;
        file.write_at(0, &self.encode())?;
        file.sync()?;
        drop(file);
        dir.rename(VOLUME_NEW_NAME, VOLUME_NAME)?;
        dir.sync()
    }

    /// Read the volume file of `dir`. Writes nothing and takes no lock.
    pub fn read<D: Dir>(dir: &D) -> Result<Self, VolumeError> {
        let names = dir.list().map_err(|e| VolumeError::Io(e.kind()))?;
        if !names.iter().any(|name| name == VOLUME_NAME) {
            return Err(if names.iter().any(|name| name == LEGACY_NAME) {
                VolumeError::Legacy
            } else {
                VolumeError::Missing
            });
        }
        let file = dir
            .open(VOLUME_NAME)
            .map_err(|e| VolumeError::Io(e.kind()))?;
        let len = file.len().map_err(|e| VolumeError::Io(e.kind()))?;
        // Longer than any version is not one; read enough to say what
        // it starts with.
        let len = usize::try_from(len).unwrap_or(usize::MAX);
        let mut bytes = vec![0u8; len.min(4096)];
        file.read_at(0, &mut bytes)
            .map_err(|e| VolumeError::Io(e.kind()))?;
        if len > bytes.len() {
            return Err(VolumeError::Unknown {
                magic: String::from_utf8_lossy(&bytes[..8]).into_owned(),
                len,
            });
        }
        Self::decode(&bytes)
    }
}

/// The identity of the volume in `dir` ([`Volume::identity`]), read
/// without opening the volume: no lock is taken and nothing is written.
/// For whoever must tell which volume a directory holds while a daemon
/// may be serving it. `None` if the directory holds no volume this
/// daemon can read.
pub fn identity_of<D: Dir>(dir: &D) -> Option<[u8; 16]> {
    Volume::read(dir).ok().map(|volume| volume.identity)
}

#[cfg(test)]
mod tests;
