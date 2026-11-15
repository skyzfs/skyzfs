//! A region of 64-bit words that the kernel keeps while the machine is
//! up, mapped into the daemon and locked in memory (design §5.2): what
//! the cut state lives in, so that a death of the daemon leaves every
//! word it stored, and the next daemon takes them up.
//!
//! **This crate and `skyzfs-ublk-sys` are the only SkyZFS crates that
//! may hold `unsafe`.** Here it is one module, [`Mapped`]'s, over
//! `rustix`; every `unsafe` block says why it is sound, and the API is
//! safe.
//!
//! # What it promises
//!
//! - A region is named by the caller and is there, word for word, for
//!   the next process that opens the name while the machine is up,
//!   whatever became of the process that stored into it: a store is in
//!   the kernel's pages when it returns, and a process that dies after
//!   it leaves it there. The stores of one thread are made in their
//!   order (each is a release store), so the next process never sees a
//!   store without every one the same thread made before it.
//! - The region is allocated in full before it is mapped, and locked
//!   in memory once mapped: no store can fault for lack of space, and
//!   no page of it is written out or read back while it is in use. An
//!   open that cannot do either fails, and says what limit stood in the
//!   way.
//! - Nothing is promised across a reboot or a crash of the machine.
//!   What the caller writes into the region to tell a later boot's
//!   region from its own (the cut state's boot id, M2.2b) is the
//!   caller's.
//!
//! # Linux: a file on tmpfs
//!
//! The name is a file's path, and the file system is the caller's
//! choice; the daemon's is its run directory, `/run/skyzfsd/<volume>/`,
//! a tmpfs. On tmpfs the pages are memory charged to the cgroup of
//! whoever touches them first (the allocation here, so the daemon's),
//! so the region counts against a memory limit on the daemon's group;
//! and the file system's own size limit can refuse the allocation, in
//! which case the error names that limit. The lock is limited by
//! `RLIMIT_MEMLOCK` unless the process has `CAP_IPC_LOCK` (root has); an
//! error names the limit too.
//!
//! A machine-check on a page of the region (a media error of the
//! memory) is a `SIGBUS` when the page is touched, which kills the
//! daemon; the next start finds the region as the kernel has it then.
//! Nothing here handles a signal.
//!
//! On the other operating systems the name is theirs to say (a named
//! shared-memory object, say); [`Mapped::open`] fails there with
//! [`io::ErrorKind::Unsupported`] until a client needs it.

#![deny(unsafe_op_in_unsafe_fn)]

use std::io;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Mapped;

#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(not(target_os = "linux"))]
pub use other::Mapped;

/// The bytes of a word.
pub const WORD: usize = 8;

/// The bytes of `words` words, or an error if that overflows.
fn bytes_of(words: usize) -> io::Result<usize> {
    if words == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a mapped region needs at least one word",
        ));
    }
    words.checked_mul(WORD).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a region of {words} words is more bytes than an address holds"),
        )
    })
}

/// What [`Mapped::open`] did with the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// There was no region of that name: it is new, all zeros.
    Created,
    /// The region was there, with as many words as asked: its words are
    /// what was stored in it.
    Kept,
    /// The region was there with another length, and now has the
    /// length asked: what it held is not to be trusted as a whole.
    Resized,
}

#[cfg(test)]
fn temp_dir() -> tempfile::TempDir {
    match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => panic!("a temporary directory: {e}"),
    }
}

#[cfg(test)]
fn open_or_panic(path: &std::path::Path, words: usize) -> (Mapped, Opened) {
    match Mapped::open(path, words) {
        Ok(opened) => opened,
        Err(e) => panic!("open {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_of_no_words_is_refused() {
        let dir = temp_dir();
        let Err(e) = Mapped::open(&dir.path().join("r"), 0) else {
            panic!("a region of no words was mapped");
        };
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_name_in_no_directory_is_an_error_that_names_it() {
        let dir = temp_dir();
        let path = dir.path().join("missing").join("r");
        let Err(e) = Mapped::open(&path, 4) else {
            panic!("a region in no directory was mapped");
        };
        assert!(e.to_string().contains("missing"), "{e}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stores_are_there_for_the_next_open() {
        let dir = temp_dir();
        let path = dir.path().join("r");
        let (mut region, opened) = open_or_panic(&path, 1000);
        assert_eq!(opened, Opened::Created);
        assert_eq!(region.words(), 1000);
        assert!((0..1000).all(|at| region.load(at) == 0));
        region.store(0, 7);
        region.store(999, u64::MAX);
        drop(region);

        let (region, opened) = open_or_panic(&path, 1000);
        assert_eq!(opened, Opened::Kept);
        assert_eq!((region.load(0), region.load(999)), (7, u64::MAX));
        assert_eq!(region.load(1), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_word_past_the_end_reads_as_zero_and_takes_no_store() {
        let dir = temp_dir();
        let (mut region, _) = open_or_panic(&dir.path().join("r"), 2);
        region.store(2, 5);
        assert_eq!(region.load(2), 0);
        assert_eq!((region.load(0), region.load(1)), (0, 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn another_length_is_resized_and_said() {
        let dir = temp_dir();
        let path = dir.path().join("r");
        let (mut region, _) = open_or_panic(&path, 10);
        region.store(3, 3);
        drop(region);
        let (region, opened) = open_or_panic(&path, 20);
        assert_eq!(opened, Opened::Resized);
        assert_eq!(region.words(), 20);
        // The file is allocated in full at its new length.
        let Ok(meta) = std::fs::metadata(&path) else {
            panic!("the region's file is gone");
        };
        assert_eq!(meta.len(), 20 * WORD as u64);
        assert!(
            std::os::unix::fs::MetadataExt::blocks(&meta) * 512 >= 20 * WORD as u64,
            "{} blocks of 512 bytes for {} bytes",
            std::os::unix::fs::MetadataExt::blocks(&meta),
            20 * WORD
        );
    }

    /// What the child of the next test does: open the region its parent
    /// names, store, and be killed, as `kill -9` kills the daemon.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "run by a_death_leaves_every_store_made_before_it, in a child"]
    fn child_that_stores_and_dies() {
        let Some(path) = std::env::var_os("SKYZFS_MAPPED_CHILD") else {
            return;
        };
        let (mut region, _) = open_or_panic(std::path::Path::new(&path), 4096);
        for at in 0..4096 {
            region.store(at, at as u64 + 1);
        }
        let killed =
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL);
        panic!("the child was not killed: {killed:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_death_leaves_every_store_made_before_it() {
        let dir = temp_dir();
        let path = dir.path().join("r");
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => panic!("the test's own program: {e}"),
        };
        let status = std::process::Command::new(exe)
            .args([
                "--exact",
                "tests::child_that_stores_and_dies",
                "--ignored",
                "--nocapture",
            ])
            .env("SKYZFS_MAPPED_CHILD", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let status = match status {
            Ok(status) => status,
            Err(e) => panic!("the child did not start: {e}"),
        };
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(9),
            "the child was not killed: {status}"
        );
        let (region, opened) = open_or_panic(&path, 4096);
        assert_eq!(opened, Opened::Kept);
        for at in 0..4096 {
            assert_eq!(region.load(at), at as u64 + 1, "word {at}");
        }
    }
}
