//! The SkyZFS local store (design §5.1): the volume's home image,
//! written in place by direct I/O and synced at a flush, and the disk it
//! is tested on.
//!
//! - [`io`]: the few file and directory operations the store uses, as
//!   traits, so that the same code runs on real files and on a disk that
//!   fails on purpose;
//! - [`sim`]: `SimDisk`, that disk, in memory and reproducible from a
//!   seed;
//! - [`fs`]: the same operations on real files;
//! - [`volume`]: the volume file, which makes a directory a volume and
//!   says what the volume is;
//! - [`inplace`]: the store, a block target ([`skyzfs_blk::BlockTarget`])
//!   on the home image alone, with its create, open and start. What it
//!   promises (the volatile-cache disk, rule R10 of the block contract)
//!   is in its documentation;
//! - [`store`]: what the store shares with its owner;
//! - [`mod@format`] and [`writeback`]: what is left of the log store's
//!   types for the daemon's status file and its record of what was
//!   acknowledged.
//!
//! Until M1.3t a write log and a store on it were here as well: the
//! log store, which the in-place store was entered from. It is gone,
//! and a volume it made is refused ([`volume::VolumeError::Legacy`]).
//!
//! # Limits
//!
//! - **The lock keeps out stores, not everything.** It is advisory: a
//!   program that writes the files without asking is not stopped. And
//!   it belongs to the open file, of which a child process holds a copy
//!   from its fork to its exec. Dropping the store unlocks, so a store
//!   that is closed and opened again while another thread starts a
//!   child is not found locked. A process that dies without dropping
//!   the store, at the moment a child of its own is between fork and
//!   exec, leaves the lock to that child until the exec, and the next
//!   instance may see `Locked` for that moment.
//! - **A read error fails the volume**, also where another copy could
//!   have served the read. Nothing is repaired here.
//! - **A full file system fails the volume for good.** The home image
//!   is sparse unless it was made with `--zero-fill`, so a write into it
//!   can run out of space, and that is a failed write like any other.
//!   Design §4.1 holds a condition that can recover and does not fail
//!   on it, and a full file system is one: a known gap, for the package
//!   that accounts for space.
//! - **A disk that lies about a sync.** If a sync returns before the
//!   data is persistent, a power cut can lose what a flush covered. On
//!   macOS the standard library's sync is the full flush
//!   (`F_FULLFSYNC`).
//! - **Platforms other than Linux.** Off Unix, syncing a directory is
//!   not implemented and returns an error rather than success, so a
//!   volume cannot be created there: the Windows port has to supply
//!   it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod format;
pub mod fs;
pub mod inplace;
pub mod io;
pub mod sim;
pub mod store;
pub mod volume;
pub mod writeback;
