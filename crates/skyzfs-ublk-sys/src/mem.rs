//! Locking memory so that it is never paged out. Nothing here unlocks:
//! see [`lock`].

use std::io;

use skyzfs_blk::Buf;

/// Lock the whole of `buf`'s memory (its capacity must be its length)
/// so that it is never paged out.
///
/// The range is never unlocked. The queue's buffers are never freed
/// (see the queue's documentation), so there is no moment for it; and
/// an `munlock` of a range takes the lock away from that range also
/// when the whole process was locked with [`lock_all`], which would
/// leave pages of the daemon free to be paged out.
pub(crate) fn lock(buf: &Buf) -> io::Result<()> {
    let len = buf.len();
    if len == 0 {
        return Ok(());
    }
    // SAFETY: `mlock` reads and writes nothing through the pointer: it
    // changes how the kernel keeps the pages. The range is the live
    // memory of `buf`.
    let result = unsafe { libc::mlock(buf.as_ptr().cast(), len) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
