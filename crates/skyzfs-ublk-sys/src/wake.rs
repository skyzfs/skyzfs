//! The event descriptor that wakes a queue's thread from another
//! thread: for a completion of the target, and for a stop.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;

/// An event descriptor. Cloning it gives another handle on the same.
#[derive(Debug, Clone)]
pub struct Wake(Arc<File>);

impl Wake {
    /// A new one.
    pub fn new() -> io::Result<Self> {
        // SAFETY: `eventfd` takes no pointer. It returns a new
        // descriptor or -1.
        let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is an open descriptor that `eventfd` just made
        // and that nothing else owns.
        let owned = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Self(Arc::new(File::from(owned))))
    }

    /// Wake whoever waits on it. It never blocks, and an error is
    /// nothing to act on: the counter is full only when a wake is
    /// already pending.
    pub fn wake(&self) {
        let _ = (&*self.0).write(&1u64.to_ne_bytes());
    }

    /// Take the pending wakes away, so that the descriptor is quiet
    /// until the next [`Wake::wake`].
    pub(crate) fn drain(&self) {
        let mut counter = [0u8; 8];
        let _ = (&*self.0).read(&mut counter);
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}
