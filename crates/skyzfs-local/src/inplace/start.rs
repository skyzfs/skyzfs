//! The in-place store's own create, open and start (M1.3t): the volume
//! is its directory's [`volume`](crate::volume) file and its home
//! image, and nothing else.
//!
//! A start has two phases, as it had when the store was entered from
//! the log store, so that the daemon's lifecycle and its rigs see the
//! same steps:
//!
//! 1. **The open** ([`Opened::open`]), under the volume's lock, in the
//!    lifecycle: the volume file is read and checked, the failure
//!    marker is looked at, and the home image is opened and checked
//!    against the volume file. A volume this daemon cannot serve is
//!    refused by reason ([`OpenError`]), and nothing is written but
//!    the removal of an acknowledged marker. None of these refusals
//!    fails the volume.
//! 2. **The start** ([`Opened::start`]), once the device is this
//!    instance's: the home image is synced, the kernel's cache of it
//!    dropped, and the I/O threads started ([`InPlace::assemble`]). A
//!    failed sync there fails the volume ([`StartError::Failed`]).

use std::io;

use skyzfs_blk::{Base, Geometry, BLOCK_SIZE, BLOCK_USIZE, SIZE_UNIT};

use super::{InPlace, IoMode, MarkerWriter, Mutants, Parts, StartError};
use crate::io::{Dir, File};
use crate::store::{stranger, FailureObserver, Waker, Zeroes, HOME_NAME, HOME_NEW_NAME};
use crate::volume::{Volume, VolumeError, LEGACY_NAME, MARKER_NAME, VOLUME_NAME};

/// What a new volume is ([`create`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeConfig {
    /// The volume's id.
    pub volume: [u8; 16],
    /// The volume's size in MiB.
    pub size_mib: u64,
    /// What the volume holds before anything is written.
    pub base: Base,
    /// The write unit: see [`Volume::write_unit`] and
    /// [`crate::volume::write_unit_for`].
    pub write_unit: u32,
}

impl VolumeConfig {
    /// The volume file of a volume of this configuration with
    /// `identity`, if the values can describe one.
    fn volume(&self, identity: [u8; 16]) -> Option<Volume> {
        let size = self.size_mib.checked_mul(SIZE_UNIT).filter(|s| *s > 0)?;
        let volume = Volume {
            id: self.volume,
            size,
            block_size: u32::try_from(BLOCK_SIZE).ok()?,
            write_unit: self.write_unit,
            identity,
        };
        volume.valid().then_some(volume)
    }
}

/// How a volume is opened ([`Opened::open`]).
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// How many requests may be in flight.
    pub queue_depth: u32,
    /// Whether the volume is served read-only.
    pub read_only: bool,
    /// Open a volume that is marked as failed, and remove the marker.
    /// The caller knows that no client is attached that was told more
    /// than the disk holds.
    pub acknowledge_failure: bool,
    /// Called once, at the moment the store decides that the volume
    /// has failed, and before the failure marker is attempted: the
    /// marker needs the disk that has just failed and may take minutes
    /// or never be written, and a transport has to give up its
    /// attachment at once (design §4.2). It is called from the thread
    /// that executes the store's requests and is expected to return
    /// quickly. A failure at the start's own sync calls it too.
    pub on_failure: Option<FailureObserver>,
    /// How the store reads and writes the home image ([`IoMode`]).
    pub io: IoMode,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            queue_depth: 64,
            read_only: false,
            acknowledge_failure: false,
            on_failure: None,
            io: IoMode::default(),
        }
    }
}

/// Why a volume could not be created or opened. None of these fails
/// the volume: nothing was served, and nothing was written but what
/// each says.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The parameters cannot describe a volume.
    #[error("the volume's parameters are not valid")]
    InvalidConfig,
    /// The system gave no random bytes for the volume's identity.
    #[error("the system gave no random bytes for the volume's identity")]
    NoRandomness,
    /// Another instance holds the volume's lock. Nothing was read or
    /// written.
    #[error("the volume is open in another instance")]
    Locked,
    /// `create`: the directory holds a file that no creation leaves
    /// ([`crate::store::CREATE_LEFTOVERS`]). Nothing was removed or
    /// written, not even the lock file.
    #[error("the directory holds a file that is not a volume's")]
    NotEmpty,
    /// `create`: the directory holds a volume.
    #[error("the directory holds a volume already")]
    Exists,
    /// The volume file is missing, of a version this daemon does not
    /// know, damaged, or the directory holds a volume of the log store.
    #[error("{0}")]
    Volume(#[from] VolumeError),
    /// The volume is marked as failed ([`MARKER_NAME`]), and the open
    /// was not told to acknowledge it.
    #[error("the volume is marked as failed")]
    MarkedFailed,
    /// The home image does not fit the volume file.
    #[error("the volume is damaged: {0}")]
    Damaged(&'static str),
    /// A call on the volume's files failed.
    #[error("{during} failed ({kind:?})")]
    Io {
        /// What the store was doing.
        during: &'static str,
        /// The error's kind.
        kind: io::ErrorKind,
    },
    /// A file system under the volume has blocks larger than its write
    /// unit. Nothing was written. The write unit is what a create and
    /// every open hold the system to (the volume file's
    /// [`Volume::write_unit`]): a volume made where pages are 4 KiB and
    /// opened where they are 16 or 64 KiB, or moved to a file system
    /// with larger blocks, is refused.
    #[error(
        "{what} is on a file system with blocks of {block} bytes, larger than the volume's \
         write unit of {unit} bytes"
    )]
    Blocks {
        /// What is there.
        what: Under,
        /// The file system's block size.
        block: u64,
        /// The volume's write unit.
        unit: u64,
    },
    /// The system's page is larger than the volume's write unit.
    /// Nothing was written.
    #[error(
        "this system's pages are {page} bytes, larger than the volume's write unit of {unit} \
         bytes"
    )]
    Pages {
        /// The system's page size.
        page: u64,
        /// The volume's write unit.
        unit: u64,
    },
    /// The same, where the block size could not be read.
    #[error(
        "the block size of the file system under {what} could not be read ({kind:?}): \
         whether a write there stays within the volume's write unit cannot be checked"
    )]
    BlocksUnknown {
        /// What is there.
        what: Under,
        /// The error's kind.
        kind: io::ErrorKind,
    },
}

/// What of a volume a file system's block size was read for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Under {
    /// The volume's directory: where every file of the volume is made.
    Directory,
    /// The home image: checked for itself at every open, since it can
    /// lie on another file system than its directory (a mount point, a
    /// link elsewhere). A create makes it in the directory, so the
    /// directory's check covers it there.
    Home,
}

impl std::fmt::Display for Under {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Directory => "the volume's directory",
            Self::Home => "the volume's home image (home.img)",
        })
    }
}

fn io_error(during: &'static str) -> impl Fn(io::Error) -> OpenError {
    move |e| OpenError::Io {
        during,
        kind: e.kind(),
    }
}

/// Refuse a system whose page is larger than `unit`.
fn pages_within_unit(page: u64, unit: u64) -> Result<(), OpenError> {
    if page <= unit {
        Ok(())
    } else {
        Err(OpenError::Pages { page, unit })
    }
}

/// Refuse a block size that is not at most `unit`.
fn within_unit(what: Under, block: io::Result<u64>, unit: u64) -> Result<(), OpenError> {
    match block {
        Ok(block) if block <= unit => Ok(()),
        Ok(block) => Err(OpenError::Blocks { what, block, unit }),
        Err(e) => Err(OpenError::BlocksUnknown {
            what,
            kind: e.kind(),
        }),
    }
}

/// Take the lock of the volume in `dir` and nothing else: nothing is
/// read or written. While another instance has the lock this fails
/// with [`OpenError::Locked`]. Dropping the lock releases it.
pub fn lock<D: Dir>(dir: &D) -> Result<D::Lock, OpenError> {
    match dir.try_lock() {
        Ok(lock) => Ok(lock),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(OpenError::Locked),
        Err(e) => Err(io_error("locking the directory")(e)),
    }
}

/// Make a new volume in `dir`, under its lock, and release the lock.
/// Returns what its volume file says.
///
/// The home image is made under another name, filled with the base,
/// synced and renamed, and the directory synced; the volume file is
/// made last ([`Volume::create`]), so a creation that was cut off
/// leaves no volume, only leftovers that the next one removes. A
/// directory that holds a name no creation leaves is refused with
/// nothing written ([`OpenError::NotEmpty`]), as is one that holds a
/// volume, of this store or of the log store.
pub fn create<D: Dir>(dir: &D, config: &VolumeConfig) -> Result<Volume, OpenError> {
    create_with(dir, config, Mutants::default())
}

/// [`create`], with the whole home image written with zeros
/// (`skyzfsd create --zero-fill`, not the default). A new image is a
/// hole, and every first direct write to a part of it allocates there,
/// which the next sync must make durable as metadata; written with
/// zeros at its creation, it has no hole. Its content does not change.
///
/// The zeros are written into the image under its creation's name,
/// with the lock held and before the volume file is made: no instance
/// can serve the volume while they are written, and a creation cut off
/// or failed among them leaves no volume. With a base every block is
/// written anyway, and this is [`create`].
pub fn create_zero_filled<D: Dir>(dir: &D, config: &VolumeConfig) -> Result<Volume, OpenError> {
    make(dir, config, true, Mutants::default())
}

/// [`create`], with defects planted for the tests.
pub(crate) fn create_with<D: Dir>(
    dir: &D,
    config: &VolumeConfig,
    mutants: Mutants,
) -> Result<Volume, OpenError> {
    make(dir, config, false, mutants)
}

/// [`create`], and with `zero_fill` [`create_zero_filled`].
fn make<D: Dir>(
    dir: &D,
    config: &VolumeConfig,
    zero_fill: bool,
    mutants: Mutants,
) -> Result<Volume, OpenError> {
    let mut identity = [0u8; 16];
    getrandom::fill(&mut identity).map_err(|_| OpenError::NoRandomness)?;
    let volume = config.volume(identity).ok_or(OpenError::InvalidConfig)?;
    let unit = u64::from(volume.write_unit);
    // Before anything is made: every file of the volume is created in
    // this directory, under a name that did not exist, so it is on this
    // file system.
    if !mutants.has(Mutants::ANY_PAGES) {
        pages_within_unit(dir.page_size(), unit)?;
    }
    if !mutants.has(Mutants::ANY_BLOCKS) {
        within_unit(Under::Directory, dir.block_size(), unit)?;
    }
    // Before the lock, which makes a file: a directory with a file of
    // somebody else is refused and left exactly as it was. A directory
    // that holds a volume has its lock file already, and whether that
    // volume is in use is what the lock says first. Then everything
    // again under the lock, for what another instance did in between.
    // A volume of the log store (before M1.3t) is refused here too,
    // before the lock, so that its directory is left exactly as it was:
    // no daemon serves it any more, so there is no use to ask its lock.
    let found = dir.list().map_err(io_error("listing the directory"))?;
    let holds = |names: &[String], name: &str| names.iter().any(|found| found == name);
    if holds(&found, LEGACY_NAME) && !holds(&found, VOLUME_NAME) {
        return Err(VolumeError::Legacy.into());
    }
    if !holds(&found, VOLUME_NAME) && stranger(&found).is_some() {
        return Err(OpenError::NotEmpty);
    }
    let _lock = lock(dir)?;
    let names = dir.list().map_err(io_error("listing the directory"))?;
    if holds(&names, VOLUME_NAME) {
        return Err(OpenError::Exists);
    }
    if holds(&names, LEGACY_NAME) {
        return Err(VolumeError::Legacy.into());
    }
    if stranger(&names).is_some() {
        return Err(OpenError::NotEmpty);
    }
    // What is here under one of a creation's own names is left from a
    // creation that was interrupted; nothing is removed under any
    // other name. (The volume file's own leftover is the volume file's
    // to replace.)
    for name in [HOME_NEW_NAME, HOME_NAME] {
        if holds(&names, name) {
            dir.remove(name).map_err(io_error("removing leftovers"))?;
        }
    }
    let creating = io_error("creating the home image");
    let mut home = dir.create(HOME_NEW_NAME).map_err(&creating)?;
    home.extend(volume.size).map_err(&creating)?;
    if config.base != Base::Empty {
        // The base, block by block, a megabyte at a time.
        let per_chunk = SIZE_UNIT / BLOCK_SIZE;
        let mut chunk = vec![0u8; SIZE_UNIT as usize];
        for mib in 0..config.size_mib {
            for (i, block) in chunk.chunks_mut(BLOCK_USIZE).enumerate() {
                config.base.fill_block(mib * per_chunk + i as u64, block);
            }
            home.write_at(mib * SIZE_UNIT, &chunk).map_err(&creating)?;
        }
    } else if zero_fill {
        crate::store::write_zeros(&mut home, 0, volume.size)
            .map_err(io_error("writing the home image with zeros"))?;
    }
    home.sync().map_err(&creating)?;
    drop(home);
    dir.rename(HOME_NEW_NAME, HOME_NAME).map_err(&creating)?;
    dir.sync().map_err(&creating)?;
    volume
        .create(dir)
        .map_err(io_error("creating the volume file"))?;
    Ok(volume)
}

/// The failure marker's writer: [`MARKER_NAME`] made (or found),
/// synced, and the directory synced (unless `unsynced`, a planted
/// defect). Returns whether that worked.
fn write_marker<D: Dir>(dir: &D, unsynced: bool) -> bool {
    let file = match dir.create(MARKER_NAME) {
        Ok(file) => Some(file),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => dir.open(MARKER_NAME).ok(),
        Err(_) => None,
    };
    let Some(mut file) = file else {
        return false;
    };
    file.sync().is_ok() && (unsynced || dir.sync().is_ok())
}

/// A volume opened under its lock, and not yet started: the first
/// phase of a start (see [`Opened::open`] and [`Opened::start`]).
pub struct Opened<D: Dir> {
    dir: D,
    home: D::File,
    lock: D::Lock,
    volume: Volume,
    geometry: Geometry,
    options: OpenOptions,
    acknowledged: bool,
}

impl<D: Dir> std::fmt::Debug for Opened<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("volume", &self.volume)
            .field("geometry", &self.geometry)
            .field("acknowledged", &self.acknowledged)
            .finish_non_exhaustive()
    }
}

impl<D: Dir> Opened<D> {
    /// Open the volume in `dir` under `lock`, which is held already
    /// ([`lock`], on the same directory), and which the volume takes
    /// over. If the open fails the lock comes back with the error,
    /// still held.
    ///
    /// In order: the volume file is read ([`Volume::read`]); the
    /// system's page, the directory's file system and the home image's
    /// are held to the volume's write unit; the failure marker refuses
    /// the volume unless `options` acknowledge it, and is then removed
    /// and the directory synced; the home image must be of the volume's
    /// size. Nothing else is written, and no refusal fails the volume.
    pub fn open(dir: D, lock: D::Lock, options: OpenOptions) -> Result<Self, (OpenError, D::Lock)> {
        Self::open_with(dir, lock, options, Mutants::default())
    }

    /// [`Opened::open`], with defects planted for the tests.
    pub(crate) fn open_with(
        dir: D,
        lock: D::Lock,
        options: OpenOptions,
        mutants: Mutants,
    ) -> Result<Self, (OpenError, D::Lock)> {
        match Self::open_unlocked(&dir, &options, mutants) {
            Ok((home, volume, geometry, acknowledged)) => Ok(Self {
                dir,
                home,
                lock,
                volume,
                geometry,
                options,
                acknowledged,
            }),
            Err(e) => Err((e, lock)),
        }
    }

    fn open_unlocked(
        dir: &D,
        options: &OpenOptions,
        mutants: Mutants,
    ) -> Result<(D::File, Volume, Geometry, bool), OpenError> {
        let volume = Volume::read(dir)?;
        let unit = u64::from(volume.write_unit);
        if !mutants.has(Mutants::ANY_PAGES) {
            pages_within_unit(dir.page_size(), unit)?;
        }
        let blocks = !mutants.has(Mutants::ANY_BLOCKS);
        if blocks {
            within_unit(Under::Directory, dir.block_size(), unit)?;
        }
        let home = match dir.open(HOME_NAME) {
            Ok(home) => home,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(OpenError::Damaged("there is no home image"))
            }
            Err(e) => return Err(io_error("opening the home image")(e)),
        };
        // The image is checked for itself: it can lie on another file
        // system than its directory (a mount point, a link elsewhere).
        if blocks && !mutants.has(Mutants::HOME_ANY_BLOCKS) {
            within_unit(Under::Home, home.block_size(), unit)?;
        }
        let size = home.len().map_err(io_error("reading the home image"))?;
        if size != volume.size {
            return Err(OpenError::Damaged(
                "the home image is not of the volume's size",
            ));
        }
        let geometry = Geometry::new(
            volume.size / SIZE_UNIT,
            options.queue_depth,
            options.read_only,
        )
        .ok()
        .filter(|_| volume.size.is_multiple_of(SIZE_UNIT))
        .ok_or(OpenError::InvalidConfig)?;
        let names = dir.list().map_err(io_error("listing the directory"))?;
        let marked = names.iter().any(|name| name == MARKER_NAME);
        if marked && !options.acknowledge_failure {
            return Err(OpenError::MarkedFailed);
        }
        if marked {
            // Acknowledged: the marker goes, durably, before anything
            // is served. Where that fails the volume stays marked, and
            // the start is refused: nothing was served.
            dir.remove(MARKER_NAME)
                .and_then(|()| dir.sync())
                .map_err(io_error("removing the failure marker"))?;
        }
        Ok((home, volume, geometry, marked))
    }

    /// The volume as its file says.
    pub fn volume(&self) -> Volume {
        self.volume
    }

    /// The volume's identity ([`Volume::identity`]).
    pub fn identity(&self) -> [u8; 16] {
        self.volume.identity
    }

    /// The device the volume is served as.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Whether this open found the volume marked as failed and removed
    /// the marker, as it was told to.
    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }

    /// The second phase: sync the home image, drop the kernel's cache of
    /// it, and start the I/O threads. `waker`
    /// is called when a sync that a flush began ends on the file's
    /// helper, and when an I/O thread has a result. Where the sync
    /// fails, the volume has failed: the observer has been called, and
    /// the marker is handed back unwritten with the lock
    /// ([`StartError::Failed`]).
    pub fn start(self, waker: Option<Waker>) -> Result<InPlace<D>, StartError>
    where
        D: Clone + Send + 'static,
        D::Lock: Send + 'static,
        D::File: Send + 'static,
    {
        self.start_with(waker, Mutants::default())
    }

    pub(crate) fn start_with(
        self,
        waker: Option<Waker>,
        mutants: Mutants,
    ) -> Result<InPlace<D>, StartError>
    where
        D: Clone + Send + 'static,
        D::Lock: Send + 'static,
        D::File: Send + 'static,
    {
        let marker: MarkerWriter = {
            let dir = self.dir.clone();
            let unsynced = mutants.has(Mutants::MARKER_UNSYNCED);
            Box::new(move || write_marker(&dir, unsynced))
        };
        InPlace::assemble(
            Parts {
                dir: self.dir,
                home: self.home,
                lock: Some(self.lock),
                geometry: self.geometry,
                identity: self.volume.identity,
                zeroes: Zeroes::default(),
                marker,
                observer: self.options.on_failure,
                io: self.options.io,
            },
            waker.map(Waker::into_call),
            mutants,
        )
    }
}

impl Opened<crate::fs::FsDir> {
    /// Another hold on the volume's lock ([`FsLock::share`](crate::fs::FsLock::share)):
    /// the directory stays locked until the store and every such hold
    /// are gone. For an owner that has work to do under the lock after
    /// the store has ended, on whatever thread it ended.
    pub fn share_lock(&self) -> crate::fs::FsLock {
        self.lock.share()
    }
}

/// The identity of the volume in `dir` ([`Volume::identity`]), read
/// without opening the volume: see [`crate::volume::identity_of`].
pub fn identity_of<D: Dir>(dir: &D) -> Option<[u8; 16]> {
    crate::volume::identity_of(dir)
}
