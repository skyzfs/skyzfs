//! For tests: an allocator that counts the deallocations of memory it
//! was told to watch, and a look at whether a range is still mapped.
//!
//! **In test builds only.** This file is compiled into the crate's
//! unit tests (`#[cfg(test)]` in `lib.rs`) and included by the kernel
//! tests for their own binary (`tests/kernel.rs`). It is not part of
//! the library, and no daemon contains it.
//!
//! The queue's buffers are never freed (see the queue's
//! documentation, "The buffers are never freed"). Whether freed memory is handed out
//! again, and whether the driver then writes into it, depends on the
//! allocator and on a request arriving at the right moment; a test
//! that waits for that damage can pass with the defect in place. What
//! does not depend on either is the deallocation itself: a test binary
//! installs [`Watching`] as its global allocator, tells it the ranges
//! the driver was told ([`watch`]), and asserts that none of them was
//! given back ([`freed`]).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The system allocator, counting the deallocations that give back
/// any byte of a watched range.
#[derive(Debug, Clone, Copy, Default)]
pub struct Watching;

/// How many ranges can be watched at once.
const ROOM: usize = 1024;

/// The watched ranges: start and end, both 0 for a free slot. A slot's
/// end is written before its start and read after it, the start with
/// `Acquire`, so a reader that sees a start sees its end on any
/// architecture, not only on amd64's stronger order.
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
    let used = USED.load(Ordering::Acquire).min(ROOM);
    for slot in 0..used {
        let watched = STARTS[slot].load(Ordering::Acquire);
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
    mapped_in(start, len, page_size())
}

/// This system's page: 4096 on amd64, 16 KiB or 64 KiB on some arm64
/// kernels. 4096 where the system does not say.
fn page_size() -> usize {
    // SAFETY: `sysconf` reads a value of the system and touches no
    // memory of ours.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(page)
        .ok()
        .filter(|p| p.is_power_of_two())
        .unwrap_or(4096)
}

/// [`mapped`] with the page given: `mincore` takes a start aligned to
/// the system's page and fails with `EINVAL` for any other, so the
/// range is widened to whole pages of `page` bytes.
fn mapped_in(start: usize, len: usize, page: usize) -> bool {
    if start == 0 || len == 0 || page == 0 {
        return false;
    }
    let from = start / page * page;
    let bytes = (start + len).div_ceil(page) * page - from;
    let host = page_size();
    let mut pages = vec![0u8; bytes.div_ceil(host)];
    // SAFETY: `mincore` reads nothing through `from`: it reports on
    // the mapping of the range and fails for one that is not mapped.
    // It writes one byte per page of the system into `pages`, which
    // has that many: `bytes` is a multiple of `page`, and a page given
    // smaller than the system's makes `mincore` fail on the start.
    let result = unsafe { libc::mincore(from as *mut libc::c_void, bytes, pages.as_mut_ptr()) };
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mapped_in` widens the range to whole pages of the size it is
    /// given, not of 4096 bytes: on a kernel of 16 KiB pages a start
    /// rounded to 4096 is not aligned, `mincore` fails with `EINVAL`,
    /// and every buffer reads as unmapped. The test is written against
    /// `mapped_in` and not `mapped` because on amd64 the system's page
    /// is 4096, so `mapped` would pass with the size fixed at 4096
    /// too. It maps four pages of the system aligned to four, unmaps
    /// the first, and asks about the second: with pages of four of the
    /// system's, the range reaches the unmapped one; with the system's
    /// own, it does not. Another thread of the test binary can map a
    /// page into the hole meanwhile; the answers count only when the
    /// hole is still there after them, and the test tries again
    /// otherwise.
    #[test]
    fn mapped_in_rounds_to_the_page_it_is_given() {
        for _ in 0..100 {
            if let Some((own, larger, hole)) = with_a_hole() {
                assert!(own, "the page after the hole reads as unmapped");
                assert!(!larger, "a page four times larger did not reach the hole");
                assert!(!hole);
                return;
            }
        }
        panic!("another thread filled the hole every time");
    }

    /// Map, make the hole, ask `mapped_in` about the page after it with
    /// the system's page and with one four times larger, and whether the
    /// hole reads as mapped; `None` where the hole was filled.
    fn with_a_hole() -> Option<(bool, bool, bool)> {
        let host = page_size();
        let large = 4 * host;
        // Room to find a start aligned to `large`.
        let room = 2 * large;
        // SAFETY: a new anonymous private mapping, of no file and at no
        // address of ours; the result is checked.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                room,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED);
        let base = base as usize;
        let aligned = base.div_ceil(large) * large;
        // SAFETY: the first page of `aligned` lies inside the mapping
        // made above, and nothing refers to it.
        let unmapped = unsafe { libc::munmap(aligned as *mut libc::c_void, host) };
        assert_eq!(unmapped, 0);
        let second = aligned + host;
        let answers = (
            mapped_in(second, host, host) && mapped(second, host),
            mapped_in(second, host, large),
            mapped_in(aligned, host, host),
        );
        let filled = answers.2;
        // SAFETY: what is left of the mapping made above, around the
        // page already unmapped; nothing refers to it. A page another
        // thread mapped into the hole is not touched.
        unsafe {
            libc::munmap(base as *mut libc::c_void, aligned - base);
            libc::munmap(second as *mut libc::c_void, base + room - second);
        }
        (!filled).then_some(answers)
    }
}
