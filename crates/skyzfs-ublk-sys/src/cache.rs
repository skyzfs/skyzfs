//! How many pages of a file the kernel's cache holds: the call behind
//! the in-place store's check that a start serves only what the disk
//! holds (`skyzfs_local::fs::CacheCountCall`), which the daemon gives
//! to the store's directory.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

/// The number of `cachestat` (Linux 6.5). It is the same on every
/// architecture: the calls numbered from 424 on share one table. The
/// `libc` crate names it only for some targets.
const SYS_CACHESTAT: libc::c_long = 451;

/// `struct cachestat_range` of `<linux/mman.h>`.
#[repr(C)]
struct Range {
    off: u64,
    /// 0: to the end of the file.
    len: u64,
}

/// `struct cachestat` of `<linux/mman.h>`.
#[repr(C)]
#[derive(Default)]
struct Stat {
    nr_cache: u64,
    nr_dirty: u64,
    nr_writeback: u64,
    nr_evicted: u64,
    nr_recently_evicted: u64,
}

/// How many pages of `file`, the whole of it, the kernel's page cache
/// holds now, dirty ones and ones under writeback included: `cachestat`
/// over the file. It asks without mapping the file, which in a process
/// whose memory is locked (`mlockall` with `MCL_FUTURE`) would read the
/// mapped range in and lock it.
///
/// `ENOSYS` (a kernel before 6.5) and `EOPNOTSUPP` are
/// [`io::ErrorKind::Unsupported`]; any other error is the kernel's.
pub fn cached_pages(file: &File) -> io::Result<u64> {
    let range = Range { off: 0, len: 0 };
    let mut stat = Stat::default();
    // SAFETY: `cachestat` reads `range` and writes `stat`, both of the
    // layout the kernel's header gives and alive for the call; the
    // descriptor is `file`'s, open for as long as the borrow lasts. The
    // flags must be 0.
    let result = unsafe {
        libc::syscall(
            SYS_CACHESTAT,
            file.as_raw_fd() as libc::c_uint,
            &range as *const Range,
            &mut stat as *mut Stat,
            0 as libc::c_uint,
        )
    };
    if result == 0 {
        return Ok(stat.nr_cache);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOSYS | libc::EOPNOTSUPP) => Err(io::ErrorKind::Unsupported.into()),
        _ => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileExt;

    /// Pages written are counted; synced and advised away
    /// (`POSIX_FADV_DONTNEED`) they are gone; read, they are back. On a
    /// kernel before 6.5 the call answers unsupported, and that is all
    /// this test can show there.
    #[test]
    fn the_pages_of_a_file_in_the_cache_are_counted() {
        let dir = tempfile_dir();
        let path = dir.join("image");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .expect("a file");
        file.write_all_at(&[7u8; 1 << 20], 0).expect("written");
        let written = match cached_pages(&file) {
            Ok(pages) => pages,
            Err(e) => {
                assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}");
                return;
            }
        };
        assert!(
            written >= 1,
            "{written} pages after a write of 1 MiB: the count does not see this file's pages, \
             as through an overlay file system; set TMPDIR to a directory on ext4 or xfs"
        );
        file.sync_data().expect("synced");
        // SAFETY: `posix_fadvise` takes a descriptor and integers and no
        // pointer; the descriptor is `file`'s.
        let advised =
            unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        assert_eq!(advised, 0);
        assert_eq!(
            cached_pages(&file).expect("counted"),
            0,
            "pages stay cached after a drop, as on tmpfs, where they are the file; set TMPDIR \
             to a directory on ext4 or xfs"
        );
        let mut page = [0u8; 4096];
        file.read_exact_at(&mut page, 0).expect("read");
        assert!(cached_pages(&file).expect("counted") >= 1);
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skyzfs-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory");
        dir
    }
}
