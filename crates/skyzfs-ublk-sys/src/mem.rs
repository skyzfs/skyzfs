//! Locking a buffer's memory, and unlocking exactly what was locked.

use std::io;

use skyzfs_blk::Buf;

/// A locked range of memory: unlocked when this is dropped.
#[derive(Debug)]
pub(crate) struct Locked {
    addr: usize,
    len: usize,
}

/// Lock the whole of `buf`'s memory (its capacity must be its length)
/// so that it is never paged out.
pub(crate) fn lock(buf: &Buf) -> io::Result<Locked> {
    let (addr, len) = (buf.as_ptr() as usize, buf.len());
    if len == 0 {
        return Ok(Locked { addr, len });
    }
    // SAFETY: `mlock` reads and writes nothing through the pointer: it
    // changes how the kernel keeps the pages. The range is the live
    // memory of `buf`.
    let result = unsafe { libc::mlock(buf.as_ptr().cast(), len) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Locked { addr, len })
}

impl Drop for Locked {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }
        // SAFETY: `munlock` reads and writes nothing through the
        // pointer and frees nothing. If the buffer has been freed since
        // (it went to a target that never gave it back), the call
        // either fails on a range that is no longer mapped or unlocks
        // pages that nothing of ours has locked: this crate locks
        // nothing else. The result is not needed either way.
        unsafe { libc::munlock(self.addr as *const libc::c_void, self.len) };
    }
}

/// Lock all of the process's memory, what it has now and what it maps
/// later, so that the daemon never waits for a page (design §4.1: a
/// daemon that page-faults onto its own pool deadlocks the machine).
pub fn lock_all() -> io::Result<()> {
    // SAFETY: `mlockall` takes no pointer.
    let result = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
