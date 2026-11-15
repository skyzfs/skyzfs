//! Linux: the region is a file, allocated in full, mapped shared and
//! locked. This is the crate's one module with `unsafe`.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{fallocate, fstatvfs, FallocateFlags};
use rustix::mm::{mlock, mmap, munmap, MapFlags, ProtFlags};
use rustix::process::{getrlimit, Resource};

use crate::{bytes_of, Opened, WORD};

/// A region of words: a file mapped shared into this process and locked
/// in memory (see the crate's documentation). It is unmapped when
/// dropped; its file stays.
#[derive(Debug)]
pub struct Mapped {
    /// The first word of the mapping: page-aligned, so aligned for an
    /// `AtomicU64`, and `words` words long.
    first: NonNull<AtomicU64>,
    words: usize,
    /// The file under the mapping, kept open for as long as it is
    /// mapped (the mapping does not need it; its name in errors does).
    _file: File,
    path: PathBuf,
}

// SAFETY: the mapping belongs to this value alone and is reached only
// through `AtomicU64`s, which may be used from any thread; nothing in
// it is tied to the thread that made it.
unsafe impl Send for Mapped {}
// SAFETY: as above: every access through `&Mapped` is an atomic load.
unsafe impl Sync for Mapped {}

/// `err` with what was being done and to which file.
fn context(err: io::Error, path: &Path, what: &str) -> io::Error {
    io::Error::new(err.kind(), format!("{}: {what}: {err}", path.display()))
}

/// The memory-lock limit, as a sentence.
fn memlock_limit() -> String {
    match getrlimit(Resource::Memlock).current {
        Some(bytes) => format!("RLIMIT_MEMLOCK is {bytes} bytes"),
        None => "RLIMIT_MEMLOCK is unlimited".to_owned(),
    }
}

/// The file system's room, as a sentence, for an allocation it refused.
fn room(file: &File) -> String {
    match fstatvfs(file) {
        Ok(vfs) => format!(
            "its file system holds {} bytes in all and has {} free (a tmpfs's size limit is the first)",
            vfs.f_blocks.saturating_mul(vfs.f_frsize),
            vfs.f_bavail.saturating_mul(vfs.f_frsize)
        ),
        Err(e) => format!("its file system's room could not be read: {e}"),
    }
}

impl Mapped {
    /// Open the region named `path` with `words` words, creating it
    /// (all zeros, readable and writable by its owner alone) if there
    /// is none. A file of another length is emptied and then given the
    /// length asked ([`Opened::Resized`]): the caller does not trust
    /// what it held, and no word of it survives. Were it only cut or
    /// grown, an open of a shorter length that is then refused (at the
    /// lock, say) would cut the file short and keep its first words,
    /// and an open of the first length would grow it back with zeros
    /// after them: the next open would find the length it asks for and
    /// take the words that survived for a region kept whole (the review
    /// of #100, N1).
    /// The file is allocated in full, mapped shared, and locked in
    /// memory; an open that cannot do one of them fails, and the error
    /// names the file system's room or the lock's limit when that is
    /// what refused.
    pub fn open(path: &Path, words: usize) -> io::Result<(Self, Opened)> {
        let len = bytes_of(words)?;
        let existed = path.exists();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|e| context(e, path, "open the region's file"))?;
        let had = file
            .metadata()
            .map_err(|e| context(e, path, "read the region's length"))?
            .len();
        let opened = match (existed, had == len as u64) {
            (false, _) => Opened::Created,
            (true, true) => Opened::Kept,
            (true, false) => Opened::Resized,
        };
        if had != len as u64 {
            file.set_len(0)
                .and_then(|()| file.set_len(len as u64))
                .map_err(|e| context(e, path, "set the region's length"))?;
        }
        // Every page is allocated before it is mapped: a store never
        // faults for lack of space (on tmpfs, the size limit).
        if let Err(e) = fallocate(&file, FallocateFlags::empty(), 0, len as u64) {
            let e = io::Error::from(e);
            let why = if e.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error()) {
                format!("allocate {len} bytes in full: {e}; {}", room(&file))
            } else {
                format!("allocate {len} bytes in full: {e}")
            };
            return Err(io::Error::new(
                e.kind(),
                format!("{}: {why}", path.display()),
            ));
        }
        // SAFETY: a new mapping (no address asked for, so none of this
        // process's memory is replaced) of `len` bytes of a file that is
        // `len` bytes long and allocated. It is shared, so stores reach
        // the file's pages. Nothing else in this process maps the file;
        // another process that does sees the same pages, and every
        // access here is atomic.
        let at = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &file,
                0,
            )
        }
        .map_err(|e| context(e.into(), path, "map the region"))?;
        let Some(first) = NonNull::new(at.cast::<AtomicU64>()) else {
            return Err(io::Error::other(format!(
                "{}: the region was mapped at address 0",
                path.display()
            )));
        };
        let mapped = Self {
            first,
            words,
            _file: file,
            path: path.to_owned(),
        };
        // SAFETY: the range is the mapping just made, which this value
        // owns; locking it changes no byte of it. On failure the value
        // is dropped, which unmaps it.
        if let Err(e) = unsafe { mlock(at, len) } {
            return Err(io::Error::new(
                io::Error::from(e).kind(),
                format!(
                    "{}: lock {len} bytes in memory: {e}; {}",
                    path.display(),
                    memlock_limit()
                ),
            ));
        }
        Ok((mapped, opened))
    }

    /// The region's length, in words.
    pub fn words(&self) -> usize {
        self.words
    }

    /// The region's name.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The word at `at`, or `None` past the end.
    fn word(&self, at: usize) -> Option<&AtomicU64> {
        if at >= self.words {
            return None;
        }
        // SAFETY: `at` is below `words`, so the word lies in the mapping,
        // which lives as long as `self`; it is aligned (the mapping is
        // page-aligned), and the mapping's bytes are only ever reached
        // as `AtomicU64`s, so a shared reference to one is sound.
        Some(unsafe { &*self.first.as_ptr().add(at) })
    }

    /// The word at `at`; 0 past the end.
    pub fn load(&self, at: usize) -> u64 {
        self.word(at).map_or(0, |word| word.load(Ordering::Acquire))
    }

    /// Store `value` at `at`; nothing past the end. A release store:
    /// whoever reads the region later, this process or the next, sees
    /// it only with every store this thread made before it.
    pub fn store(&mut self, at: usize, value: u64) {
        if let Some(word) = self.word(at) {
            word.store(value, Ordering::Release);
        }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: the mapping is this value's, `words * WORD` bytes from
        // `first`, and no reference into it outlives `&self`. Unmapping
        // also unlocks it. A failure leaves it mapped until the process
        // ends, which is harmless.
        let _ = unsafe { munmap(self.first.as_ptr().cast(), self.words * WORD) };
    }
}
