//! For tests: an allocator that counts the deallocations of memory it
//! was told to watch, and a look at whether a range is still mapped.
//!
//! The queue's buffers are never freed (see [`queue`](crate::queue),
//! "The buffers are never freed"). Whether freed memory is handed out
//! again, and whether the driver then writes into it, depends on the
//! allocator and on a request arriving at the right moment; a test
//! that waits for that damage can pass with the defect in place. What
//! does not depend on either is the deallocation itself: a test binary
//! installs [`Watching`] as its global allocator, tells it the ranges
//! the driver was told ([`watch`]), and asserts that none of them was
//! given back ([`freed`]).
//!
//! Nothing of this is in a daemon: no binary but a test installs the
//! allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The system allocator, counting the deallocations that give back
/// any byte of a watched range.
#[derive(Debug, Clone, Copy, Default)]
pub struct Watching;

/// How many ranges can be watched at once.
pub const ROOM: usize = 1024;

/// The watched ranges: start and end, both 0 for a free slot. A slot's
/// end is written before its start and read after it, so a reader that
/// sees a start sees its end.
static STARTS: [AtomicUsize; ROOM] = [const { AtomicUsize::new(0) }; ROOM];
static ENDS: [AtomicUsize; ROOM] = [const { AtomicUsize::new(0) }; ROOM];
/// The slots in use, so that an allocator nobody asked to watch costs
/// one load per deallocation.
static USED: AtomicUsize = AtomicUsize::new(0);
static FREED: AtomicU64 = AtomicU64::new(0);

/// Watch `len` bytes at `start` from now on. False if there is no room
/// or nothing to watch.
pub fn watch(start: usize, len: usize) -> bool {
    if start == 0 || len == 0 {
        return false;
    }
    let slot = USED.fetch_add(1, Ordering::SeqCst);
    if slot >= ROOM {
        USED.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    ENDS[slot].store(start + len, Ordering::SeqCst);
    STARTS[slot].store(start, Ordering::SeqCst);
    true
}

/// How many deallocations so far gave back memory of a watched range.
pub fn freed() -> u64 {
    FREED.load(Ordering::SeqCst)
}

fn given_back(start: usize, len: usize) {
    let used = USED.load(Ordering::Relaxed).min(ROOM);
    for slot in 0..used {
        let watched = STARTS[slot].load(Ordering::Relaxed);
        if watched != 0 && watched < start + len && start < ENDS[slot].load(Ordering::Relaxed) {
            FREED.fetch_add(1, Ordering::SeqCst);
            return;
        }
    }
}

// SAFETY: every call goes to the system allocator unchanged, with the
// pointer and layout it was given; what is added reads and writes
// atomics only and allocates nothing.
unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is the system allocator's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        given_back(ptr as usize, layout.size());
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as above.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        // A block that moved, or shrank below a watched byte, gave
        // memory back.
        if !new.is_null() && new != ptr {
            given_back(ptr as usize, layout.size());
        } else if !new.is_null() && new_size < layout.size() {
            given_back(ptr as usize + new_size, layout.size() - new_size);
        }
        new
    }
}

/// Whether every page of `len` bytes at `start` is mapped in this
/// process: `mincore` answers `ENOMEM` for a range with a page that is
/// not. False also for an empty range.
pub fn mapped(start: usize, len: usize) -> bool {
    const PAGE: usize = 4096;
    if start == 0 || len == 0 {
        return false;
    }
    let from = start / PAGE * PAGE;
    let bytes = (start + len).div_ceil(PAGE) * PAGE - from;
    let mut pages = vec![0u8; bytes / PAGE];
    // SAFETY: `mincore` reads nothing through `from`: it reports on
    // the mapping of the range and fails for one that is not mapped.
    // It writes one byte per page into `pages`, which has that many.
    let result = unsafe { libc::mincore(from as *mut libc::c_void, bytes, pages.as_mut_ptr()) };
    result == 0
}
