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

    /// The names in the directory, sorted.
    fn list(&self) -> io::Result<Vec<String>>;

    /// Make every creation, removal and rename so far durable.
    fn sync(&self) -> io::Result<()>;
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

    /// Set the file's length to `len` with real, zero-filled space behind
    /// it, so that a later write inside it allocates nothing.
    fn preallocate(&mut self, len: u64) -> io::Result<()>;

    /// The file's length in bytes.
    fn len(&self) -> io::Result<u64>;

    /// Whether the file is empty.
    fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }
}
