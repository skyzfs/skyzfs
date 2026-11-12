//! Writeback started early: a hint that hands what a file has had
//! written to the device before the sync that must wait for it.
//!
//! The store writes its files through the system's cache and syncs them
//! with `fdatasync`. Nothing else starts the writeback of what it wrote
//! before that sync, so a sync waits for the device to write the
//! file's whole backlog since the last one. With a unit set, once that
//! many bytes of a file are written and not yet handed over, the range
//! they lie in is handed over ([`File::start_writeback`]), and the sync
//! that follows has at most about one unit left to wait for, plus what
//! the device has not finished.
//!
//! It is a hint and promises nothing: every sync stays where it is,
//! and nothing about order, durability or what a power cut can leave
//! changes. A file system without the call answers "unsupported", and
//! the hint is then off for that file for as long as it is open. Any
//! other error is a failed write of the file, which its owner treats
//! as such.
//!
//! [`File::start_writeback`]: crate::io::File::start_writeback

use std::io;
use std::time::Instant;

use crate::io::File;

/// The unit a store starts with: hand over a file's writes once a MiB
/// of them is waiting.
pub const DEFAULT_UNIT: u64 = 1 << 20;

/// What the hint did for one kind of file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hinted {
    /// Calls that handed writes over.
    pub count: u64,
    /// The bytes those calls handed over: of the writes in their
    /// ranges, not the ranges' lengths.
    pub bytes: u64,
    /// The time the calls took, in nanoseconds.
    pub ns: u64,
    /// Whether the file system answered "unsupported" and the hint was
    /// switched off for a file of this kind.
    pub off: bool,
}

/// What a file's syncs had to wait for: the bytes written to it since
/// the sync before, summed over its syncs, and of those how many the
/// hint had already handed to the device. Kept by the log for its
/// segment files and its superblock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Backlog {
    /// Bytes written to the segment files before the syncs that
    /// covered them ([`Log::sync`](crate::log::Log::sync) and the seal
    /// of a recovery).
    pub bytes: u64,
    /// Of those, the bytes the hint had handed over.
    pub handed: u64,
    /// The superblock's syncs.
    pub superblock_syncs: u64,
    /// Their time, in nanoseconds.
    pub superblock_ns: u64,
    /// The bytes written to the superblock before them.
    pub superblock_bytes: u64,
}

/// The writes of one file since its last sync, and the hint for them.
#[derive(Debug, Clone)]
pub(crate) struct Writeback {
    /// Hand over once this many bytes are waiting; 0 is never.
    unit: u64,
    /// The file system said it has no such call.
    off: bool,
    /// The range of the writes not handed over yet, and their bytes.
    pending: Option<(u64, u64)>,
    pending_bytes: u64,
    /// Bytes written since the last sync, and how many of them were
    /// handed over.
    written: u64,
    handed: u64,
    /// What the hint has done since the file was opened.
    pub(crate) done: Hinted,
}

impl Writeback {
    pub(crate) fn new(unit: u64) -> Self {
        Self {
            unit,
            off: false,
            pending: None,
            pending_bytes: 0,
            written: 0,
            handed: 0,
            done: Hinted::default(),
        }
    }

    /// From now on hand over once `unit` bytes are waiting; 0 is never.
    pub(crate) fn set_unit(&mut self, unit: u64) {
        self.unit = unit;
    }

    /// `len` bytes were written at `offset` of `file`. Once a unit is
    /// waiting, the range it lies in is handed over. An error other
    /// than "unsupported" is a failed write of the file.
    pub(crate) fn wrote<F: File>(&mut self, file: &mut F, offset: u64, len: u64) -> io::Result<()> {
        self.written += len;
        if self.unit == 0 || self.off || len == 0 {
            return Ok(());
        }
        let end = offset.saturating_add(len);
        self.pending = Some(match self.pending {
            Some((from, to)) => (from.min(offset), to.max(end)),
            None => (offset, end),
        });
        self.pending_bytes += len;
        if self.pending_bytes >= self.unit {
            self.hand(file)
        } else {
            Ok(())
        }
    }

    /// Hand over whatever is waiting, a unit or not: before the file is
    /// left for another (a log's segment at a roll).
    pub(crate) fn hand<F: File>(&mut self, file: &mut F) -> io::Result<()> {
        let Some((from, to)) = self.pending.take() else {
            return Ok(());
        };
        let bytes = std::mem::take(&mut self.pending_bytes);
        let began = Instant::now();
        let started = file.start_writeback(from, to - from);
        let ns = u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX);
        match started {
            Ok(()) => {
                self.done.count += 1;
                self.done.bytes += bytes;
                self.done.ns = self.done.ns.saturating_add(ns);
                self.handed += bytes;
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                self.off = true;
                self.done.off = true;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// The file was synced: what waits is covered, and the bytes the
    /// sync had to have written since the last, with how many of them
    /// had been handed over, are returned and start again from 0.
    pub(crate) fn synced(&mut self) -> (u64, u64) {
        self.pending = None;
        self.pending_bytes = 0;
        (
            std::mem::take(&mut self.written),
            std::mem::take(&mut self.handed),
        )
    }
}
