//! The file and directory operations the local store uses.
//!
//! The store keeps a few files in one directory. Everything it asks of
//! the operating system is here, so that it can be run against
//! [`SimDisk`](crate::sim::SimDisk), which loses what was not made
//! durable, and against real files ([`FsDir`](crate::fs::FsDir)).
//!
//! What is durable, and when:
//!
//! - Data written to a file survives the loss of the process as soon as
//!   [`File::write_at`] returns. It survives a power cut only once a later
//!   [`File::sync`] on that file has returned `Ok`.
//! - A name (a created, renamed or removed file) survives a power cut
//!   only once a later [`Dir::sync`] has returned `Ok`. Syncing a file
//!   does not make its name durable.
//! - After a sync has failed, nothing written since the last successful
//!   sync can be relied on, power cut or not, and a later sync that
//!   succeeds proves nothing about it. The operating system may go on
//!   showing the data to [`File::read_at`] from its cache although it
//!   never reached the disk, also to a new process. Only
//!   [`File::read_device_at`] says what the disk holds.

use std::io;

/// The size of the unit a disk may keep or lose on its own at a power
/// cut. A write that covers several sectors can survive in any subset of
/// them.
pub const SECTOR: u64 = 512;

/// A directory holding the store's files.
pub trait Dir {
    /// An open file of this directory.
    type File: File;

    /// Create a new, empty file and open it. Fails if the name exists.
    fn create(&self, name: &str) -> io::Result<Self::File>;

    /// Open an existing file.
    fn open(&self, name: &str) -> io::Result<Self::File>;

    /// Remove a file. Handles that are open on it keep working.
    fn remove(&self, name: &str) -> io::Result<()>;

    /// Rename a file, replacing `to` if it exists. Durable, like every
    /// change of names, only after [`Dir::sync`].
    fn rename(&self, from: &str, to: &str) -> io::Result<()>;

    /// The names in the directory, sorted.
    fn list(&self) -> io::Result<Vec<String>>;

    /// Make every creation, removal and rename so far durable.
    fn sync(&self) -> io::Result<()>;

    /// What holds the directory's lock: dropping it releases the lock.
    type Lock;

    /// The block size of the file system the directory is on, in
    /// bytes: the least that file system allocates, deallocates and
    /// rewrites at once, and so how far a write into a file created
    /// here can reach beside what it writes. Linux: `f_bsize` of
    /// `statfs`. An error where the platform has no such call or the
    /// file system does not say: a caller that depends on the size
    /// must then refuse, not assume.
    fn block_size(&self) -> io::Result<u64>;

    /// The page size of the system that writes this directory's
    /// files, in bytes: what its cache writes back as a whole,
    /// whatever the file system. It belongs to the running kernel and
    /// not to the files: the same directory can be opened under
    /// another.
    fn page_size(&self) -> u64;

    /// Take the directory's exclusive lock, without waiting. While the
    /// lock is held a second call fails with
    /// [`io::ErrorKind::WouldBlock`] and changes nothing. The lock is
    /// advisory: it keeps out whoever asks for it, and it is gone when
    /// its holder's process is, however that ends.
    fn try_lock(&self) -> io::Result<Self::Lock>;
}

/// An open file, read and written at explicit offsets.
pub trait File {
    /// Fill `buf` from `offset`. Reading beyond the end of the file is an
    /// error ([`io::ErrorKind::UnexpectedEof`]).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    /// Fill `buf` from `offset` with what the device holds, bypassing
    /// the operating system's cache. Data written and not yet synced is
    /// included (the device has been handed it, durable or not); data the
    /// cache still shows after a failed sync is not, unless it did reach
    /// the device.
    ///
    /// Fails with [`io::ErrorKind::Unsupported`] where the file system
    /// offers no such read. A caller that needs it must then say that it
    /// could not check, and must not use [`File::read_at`] in its place.
    fn read_device_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;

    /// Write `data` at `offset`, extending the file if it ends beyond the
    /// current length. After an error, any part of `data` may have been
    /// written.
    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()>;

    /// Make the file's content and length durable: return only when
    /// everything written so far would survive a power cut.
    fn sync(&mut self) -> io::Result<()>;

    /// Begin a sync of the file that ends at a later
    /// [`File::sync_end`], so that the caller can do other things
    /// meanwhile. What it makes durable is what was written to the file
    /// before this call, as [`File::sync`] would; what is written after
    /// it may or may not be. A caller writes nothing to the file until
    /// the sync has ended, and begins no other on it.
    ///
    /// `wake` is called once the sync has ended, if the file says that
    /// it is ([`File::sync_wakes`]). By default nothing begins here:
    /// the whole sync is the end's, and `wake` is never called.
    ///
    /// An error that [`NotBegun::is`] says so means that nothing was
    /// begun and nothing is wrong with the file: whatever the sync needs
    /// beside the file could not be had (a thread, a descriptor), and
    /// the caller makes the sync itself with [`File::sync`]. Any other
    /// error is the file's, as a failed sync's.
    fn sync_begin(&mut self, wake: Box<dyn FnOnce() + Send>) -> io::Result<()> {
        drop(wake);
        Ok(())
    }

    /// The end of the sync that [`File::sync_begin`] began: `None` if
    /// it has not ended yet, which only a call with `wait` false can
    /// answer; else its outcome, as [`File::sync`]'s. A file that syncs
    /// on a thread of its own answers [`HelperEnded`] if that thread
    /// has ended. By default this is the whole sync, done now.
    fn sync_end(&mut self, wait: bool) -> Option<io::Result<()>> {
        let _ = wait;
        Some(self.sync())
    }

    /// Whether a sync begun by [`File::sync_begin`] calls its `wake`
    /// when it ends. A caller of a file that does not must ask
    /// [`File::sync_end`] with `wait` before it waits for anything
    /// else: nothing will tell it that the sync has ended.
    fn sync_wakes(&self) -> bool {
        false
    }

    /// Ask the system to start writing `len` bytes at `offset`, written
    /// and not yet synced, to the device, and return without waiting
    /// for it. A hint: it promises nothing about durability or order,
    /// and only [`File::sync`] makes anything durable. What it changes
    /// is how much a later sync has left to wait for.
    ///
    /// Fails with [`io::ErrorKind::Unsupported`] where the file system
    /// or the platform has no such call, having done nothing: the
    /// caller stops asking for this file. Any other error is a failed
    /// write of the file.
    ///
    /// Linux: `sync_file_range` with `SYNC_FILE_RANGE_WRITE` alone,
    /// which starts the writeback of the dirty pages of the range and
    /// does not wait for it or take the file's writeback error that a
    /// later `fdatasync` reports. Elsewhere: unsupported. This default
    /// is unsupported.
    fn start_writeback(&mut self, offset: u64, len: u64) -> io::Result<()> {
        let _ = (offset, len);
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Set the file's length to `len` with real, zero-filled space behind
    /// it, so that a later write inside it allocates nothing.
    fn preallocate(&mut self, len: u64) -> io::Result<()>;

    /// Set the file's length to `len`, which is not below its present
    /// length. The new part reads as zeros and need not have space
    /// behind it.
    fn extend(&mut self, len: u64) -> io::Result<()>;

    /// Deallocate `len` bytes at `offset`, both multiples of 4096 and
    /// inside the file: the range reads as zeros afterwards and has no
    /// space behind it, and the file's length stays.
    ///
    /// As a write, it is durable only after [`File::sync`]. Until
    /// then a power cut leaves each block of the range as it was or
    /// as zeros. Where the range does not cover a whole block of the
    /// file system (or write unit of the device), the file system
    /// zeroes that part by a write, which rewrites the block around it
    /// as any write does: a caller that protects the neighbours of a
    /// write protects them here in the same way, as far as the larger
    /// of the two reaches ([`File::block_size`]).
    ///
    /// Fails with [`io::ErrorKind::Unsupported`] where the file system
    /// or the platform has no such call, and has then changed nothing:
    /// the caller writes zeros in its place. Any other error is a
    /// failed write: any part of the range may be zero.
    ///
    /// The calls: Linux `fallocate` with `FALLOC_FL_PUNCH_HOLE |
    /// FALLOC_FL_KEEP_SIZE` (built). macOS `fcntl(F_PUNCHHOLE)`, which
    /// wants a range of whole file-system blocks, and Windows
    /// `FSCTL_SET_ZERO_DATA` on a file marked sparse: not built, and
    /// until they are those platforms answer "unsupported".
    fn deallocate(&mut self, offset: u64, len: u64) -> io::Result<()>;

    /// The block size of the file system the file is on, in bytes,
    /// as [`Dir::block_size`] gives it for a directory: a file can be
    /// on another file system than the directory that names it (a
    /// symbolic link, a file mounted over a name). Linux: `f_bsize` of
    /// `fstatfs`.
    fn block_size(&self) -> io::Result<u64>;

    /// The file's length in bytes.
    fn len(&self) -> io::Result<u64>;

    /// Whether the file is empty.
    fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }
}

/// What [`File::sync_begin`] answers when it could not begin the sync
/// for want of something beside the file (a thread, a descriptor):
/// nothing was begun, the file is as it was, and the caller syncs it
/// with [`File::sync`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotBegun(pub String);

impl std::fmt::Display for NotBegun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the sync could not be begun: {}", self.0)
    }
}

impl std::error::Error for NotBegun {}

impl NotBegun {
    /// The error that says so, for `why`.
    pub fn error(why: impl std::fmt::Display) -> io::Error {
        io::Error::other(Self(why.to_string()))
    }

    /// Whether `e` says so.
    pub fn is(e: &io::Error) -> bool {
        e.get_ref().is_some_and(|inner| inner.is::<Self>())
    }
}

/// What [`File::sync_end`] answers when the thread that was syncing
/// the file has ended (it panicked): whether the sync took place, and
/// what the file holds, is not known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelperEnded;

impl std::fmt::Display for HelperEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the thread that was syncing the file has ended")
    }
}

impl std::error::Error for HelperEnded {}

impl HelperEnded {
    /// The error that says so.
    pub fn error() -> io::Error {
        io::Error::other(Self)
    }

    /// Whether `e` says so.
    pub fn is(e: &io::Error) -> bool {
        e.get_ref().is_some_and(|inner| inner.is::<Self>())
    }
}
