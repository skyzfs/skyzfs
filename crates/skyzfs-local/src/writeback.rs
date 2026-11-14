//! What the log store's hint that started writeback early did
//! ([`Hinted`]), as the daemon's status file carries it: zeros since
//! M1.3t, which removed the log store. The field stays until the status
//! file's next version.

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
    /// Calls the system declined this time (short of memory,
    /// interrupted, or busy: `ENOMEM`, `EINTR`, `EAGAIN`): the volume
    /// goes on, and their bytes are offered again with the next unit,
    /// or left to the sync.
    pub refused: u64,
}
