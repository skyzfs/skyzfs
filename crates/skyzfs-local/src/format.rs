//! A position of the log store's write log ([`Lsn`]): what is left of
//! the log's format since M1.3t. The daemon's record of what was
//! acknowledged still names one; the in-place store reports none, and
//! that record goes with the pump's position plumbing (M1.3t-b).

use std::fmt;

/// A position in the log: a byte offset in an unbounded space, always on
/// a record boundary. Segment `k` of a log with segments of `S` bytes
/// holds positions `k * S .. (k + 1) * S`, and its first record is one
/// write unit in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lsn(pub u64);

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}
