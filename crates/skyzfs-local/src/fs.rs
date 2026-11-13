//! The store's file operations on real files.
//!
//! [`File::sync`] is `fdatasync` on Linux, and on macOS the full flush
//! (`F_FULLFSYNC`) that the standard library issues for it: design §5.1
//! needs a flush that returns only when the data is persistent.
//! [`Dir::sync`] syncs the directory itself, which is what makes a new
//! name survive on Linux and macOS; Windows has no such call and journals
//! the name on its own.
//!
//! Files are created readable and writable by their owner only (mode
//! 0600 on Unix): the log's superblock holds the log's key.
//!
//! [`File::read_device_at`] is a read through a second descriptor opened
//! with `O_DIRECT`, on Linux only. It is opened together with the file,
//! by the same name, and checked to be the same file, so that a later
//! rename or a reuse of the name cannot make the two descriptors
//! disagree. Where the file system refuses that
//! flag (tmpfs did before Linux 6.6), and on the other systems for now,
//! it fails as unsupported; it never falls back to a cached read. A
//! file system that accepts the flag and reads from its cache all the
//! same cannot be told apart here (see the crate's limits).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use std::sync::mpsc;
use std::thread;

use crate::io::{Dir, File, HelperEnded, NotBegun};

/// The size of a page of memory on this system: what its kernel writes
/// back as a whole, and so the least write unit a log on buffered files
/// should have here ([`Identity::write_unit`]).
///
/// Linux: `AT_PAGESZ` of the process's auxiliary vector, through libc's
/// `getauxval`, which every process has and which cannot fail as a read
/// of `/proc/self/auxv` can (where `/proc` is not mounted, a 16 KiB
/// kernel would have been taken for 4 KiB without a word). Apple
/// silicon: 16 KiB, other Macs 4 KiB.
///
/// [`Identity::write_unit`]: crate::format::Identity::write_unit
pub fn page_size() -> u32 {
    #[cfg(target_os = "linux")]
    if let Ok(size) = u32::try_from(rustix::param::page_size()) {
        return size;
    }
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        16384
    } else {
        4096
    }
}

/// The call that starts the writeback of a range of a file
/// ([`File::start_writeback`]): given by whoever opens the store, since
/// it is a system call this crate does not make itself. It answers
/// [`io::ErrorKind::Unsupported`] where the file system has none.
pub type WritebackCall = fn(&fs::File, u64, u64) -> io::Result<()>;

/// The call that counts the pages of a file in the kernel's cache
/// ([`File::cached_pages`]): given by whoever opens the store, since it
/// is a system call this crate does not make itself (Linux: `cachestat`).
/// It answers [`io::ErrorKind::Unsupported`] where the kernel has none.
pub type CacheCountCall = fn(&fs::File) -> io::Result<u64>;

/// A directory on a real file system.
#[derive(Debug, Clone)]
pub struct FsDir {
    path: PathBuf,
    /// Given to every file opened here: see [`FsDir::with_writeback`].
    writeback: Option<WritebackCall>,
    /// Given to every file opened here: see [`FsDir::with_cache_count`].
    cache_count: Option<CacheCountCall>,
}

impl FsDir {
    /// The directory at `path`, which must exist.
    pub fn new(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        if !fs::metadata(&path)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a directory",
            ));
        }
        Ok(Self {
            path,
            writeback: None,
            cache_count: None,
        })
    }

    /// The same directory, whose files start writeback with `call`.
    /// Without it, [`File::start_writeback`] of its files answers
    /// unsupported, and a store's hint is off.
    pub fn with_writeback(self, call: WritebackCall) -> Self {
        Self {
            writeback: Some(call),
            ..self
        }
    }

    /// The same directory, whose files count their cached pages with
    /// `call`. Without it, [`File::cached_pages`] of its files answers
    /// unsupported, and the in-place store refuses to start
    /// ([`crate::inplace`]).
    pub fn with_cache_count(self, call: CacheCountCall) -> Self {
        Self {
            cache_count: Some(call),
            ..self
        }
    }

    /// Where the directory is.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The file the directory's lock is taken on. It holds nothing.
pub const LOCK_NAME: &str = "lock";

/// The directory's lock, held: see [`Dir::try_lock`].
///
/// The lock can be shared inside the process ([`FsLock::share`]): it
/// is then held until the last share is dropped. That is for a caller
/// that must keep a directory locked beyond the life of the store that
/// took the lock, whatever thread that store ends on.
#[derive(Debug)]
pub struct FsLock {
    file: std::sync::Arc<Locked>,
}

impl FsLock {
    /// Another hold on the same lock. The directory stays locked until
    /// this one and every other share are dropped.
    pub fn share(&self) -> Self {
        Self {
            file: self.file.clone(),
        }
    }
}

/// The locked file: unlocked when the last [`FsLock`] on it goes.
#[derive(Debug)]
struct Locked {
    file: fs::File,
}

impl Drop for Locked {
    /// Unlocks, and does not leave it to the close. A lock belongs to
    /// the open file, not to this descriptor, and a child process
    /// started by another thread holds a copy of the descriptor from
    /// its fork to its exec: the close alone would leave the directory
    /// locked for that time. The unlock releases it for every copy.
    fn drop(&mut self) {
        // Nothing to do about a failure: the close follows.
        let _ = self.file.unlock();
    }
}

/// How often [`FsDir::try_lock`] opens the lock file again when its
/// lock landed on a file the name no longer names.
const LOCK_TRIES: u32 = 4;

/// Lock `file`, which was opened as `path`, and make sure that it is
/// still the file `path` names. `None` if it is not (the name is gone,
/// or names another file): the lock is then let go again, with the
/// file. An error if the file is locked by another (`WouldBlock`) or
/// cannot be locked or looked at.
fn finish_lock(file: fs::File, path: &Path) -> io::Result<Option<FsLock>> {
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Err(io::ErrorKind::WouldBlock.into()),
        Err(fs::TryLockError::Error(e)) => return Err(e),
    }
    if !still_named(&file, path)? {
        return Ok(None);
    }
    Ok(Some(FsLock {
        file: std::sync::Arc::new(Locked { file }),
    }))
}

/// Whether `path` names the open `file`: the same device and inode.
#[cfg(unix)]
fn still_named(file: &fs::File, path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let held = file.metadata()?;
    match fs::metadata(path) {
        Ok(named) => Ok(held.dev() == named.dev() && held.ino() == named.ino()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Not checked off Unix. Whether a lock file that is open can lose its
/// name on Windows depends on how it was opened and on the system's
/// version, and has not been tried: the daemon is not built there yet.
#[cfg(not(unix))]
fn still_named(_file: &fs::File, _path: &Path) -> io::Result<bool> {
    Ok(true)
}

impl Dir for FsDir {
    fn page_size(&self) -> u64 {
        u64::from(page_size())
    }

    #[cfg(target_os = "linux")]
    fn block_size(&self) -> io::Result<u64> {
        said_block_size(retried(|| rustix::fs::statfs(&self.path))?.f_bsize)
    }

    #[cfg(not(target_os = "linux"))]
    fn block_size(&self) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }

    type File = FsFile;
    type Lock = FsLock;

    /// An advisory lock (`flock`, `LockFileEx`) on the file `lock`,
    /// which is created if it is not there. Dropping the [`FsLock`]
    /// unlocks. The operating system drops the lock when the process
    /// ends, with one exception: if the process dies while a child of
    /// its own is between its fork and its exec, the child holds the
    /// lock until the exec, and the next instance may see the
    /// directory locked for that moment.
    ///
    /// **The lock is on the file the name `lock` names.** A lock taken
    /// on a file that was opened under that name and has since been
    /// unlinked, or replaced by another file, excludes nobody: the
    /// next instance opens the name, gets another file and locks that.
    /// So once the lock is taken, the locked file is compared with
    /// what the name names now; if they differ, the
    /// lock is let go and the name is opened again, a few times, and
    /// after that the directory counts as locked.
    fn try_lock(&self) -> io::Result<FsLock> {
        let path = self.path.join(LOCK_NAME);
        for _ in 0..LOCK_TRIES {
            let mut options = fs::OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let file = options.open(&path)?;
            if let Some(lock) = finish_lock(file, &path)? {
                return Ok(lock);
            }
        }
        // The name changed under every try: somebody is at work here.
        Err(io::ErrorKind::WouldBlock.into())
    }

    fn create(&self, name: &str) -> io::Result<FsFile> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let path = self.path.join(name);
        let file = options.open(&path)?;
        FsFile::new(file, &path, self.writeback, self.cache_count)
    }

    fn open(&self, name: &str) -> io::Result<FsFile> {
        let path = self.path.join(name);
        let file = fs::OpenOptions::new().read(true).write(true).open(&path)?;
        FsFile::new(file, &path, self.writeback, self.cache_count)
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        fs::remove_file(self.path.join(name))
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        fs::rename(self.path.join(from), self.path.join(to))
    }

    fn list(&self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            // A name that is not UTF-8 is listed with its bad bytes
            // replaced: it is no name of a store's, and whoever asks
            // what else the directory holds must see that it is there.
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    #[cfg(unix)]
    fn sync(&self) -> io::Result<()> {
        fs::File::open(&self.path)?.sync_all()
    }

    /// Not implemented off Unix, and so an error: a silent success
    /// would let the log count a new file's name as durable.
    #[cfg(not(unix))]
    fn sync(&self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// Make a system call again for as long as a signal interrupts it
/// (`EINTR`): an interrupted call has done nothing, and is no failure
/// of the file. The standard library does this for the reads, writes
/// and syncs above; this is for the calls made through `rustix`,
/// which does not.
#[cfg(target_os = "linux")]
fn retried<T>(
    mut call: impl FnMut() -> Result<T, rustix::io::Errno>,
) -> Result<T, rustix::io::Errno> {
    loop {
        match call() {
            Err(rustix::io::Errno::INTR) => {}
            other => return other,
        }
    }
}

/// A block size as `statfs` gave it: a number above zero.
#[cfg(target_os = "linux")]
fn said_block_size<T: TryInto<u64>>(size: T) -> io::Result<u64> {
    size.try_into()
        .ok()
        .filter(|size| *size > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no block size"))
}

/// An open real file.
#[derive(Debug)]
pub struct FsFile {
    file: fs::File,
    /// See [`FsDir::with_writeback`].
    writeback: Option<WritebackCall>,
    /// See [`FsDir::with_cache_count`].
    cache_count: Option<CacheCountCall>,
    /// A second descriptor on the same file that bypasses the cache.
    /// `None`: the file system, or this system, has none to give.
    direct: Option<fs::File>,
    /// The thread that syncs the file for [`File::sync_begin`], once
    /// one was begun.
    helper: Option<SyncHelper>,
    /// For tests: the helper cannot be started, as when a thread or a
    /// descriptor cannot be had.
    #[cfg(test)]
    no_helper: bool,
}

/// What the helper is asked to do.
enum Job {
    /// Sync the file, then call this.
    Sync(Box<dyn FnOnce() + Send>),
    /// For tests: panic.
    #[cfg(test)]
    Panic,
}

/// A thread that syncs a file, with a descriptor of its own on it, so
/// that the file's owner can go on meanwhile ([`File::sync_begin`]).
/// It lives as long as the file is open: dropping the file ends it,
/// after the sync it is doing, if any.
struct SyncHelper {
    jobs: Option<mpsc::Sender<Job>>,
    results: mpsc::Receiver<io::Result<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for SyncHelper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncHelper").finish_non_exhaustive()
    }
}

impl SyncHelper {
    /// A helper with `file`, a descriptor of its own on the file.
    fn start(file: fs::File) -> io::Result<Self> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, results) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("skyzfs-sync".into())
            .spawn(move || {
                for job in inbox {
                    match job {
                        Job::Sync(wake) => {
                            // `fdatasync` through its own descriptor: a
                            // sync of the file, as `File::sync` is.
                            if outbox.send(file.sync_data()).is_err() {
                                return;
                            }
                            wake();
                        }
                        #[cfg(test)]
                        Job::Panic => panic!("a test asked the sync helper to panic"),
                    }
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            results,
            thread: Some(thread),
        })
    }
}

impl Drop for SyncHelper {
    fn drop(&mut self) {
        // The thread ends when its jobs' sender is gone, after the sync
        // it is doing.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The alignment `O_DIRECT` wants of offsets, lengths and memory. 4 KiB
/// satisfies every device the store will meet.
const DIRECT_ALIGN: usize = 4096;

impl FsFile {
    /// `file`, which was just opened as `path`.
    fn new(
        file: fs::File,
        path: &Path,
        writeback: Option<WritebackCall>,
        cache_count: Option<CacheCountCall>,
    ) -> io::Result<Self> {
        let direct = open_direct(&file, path)?;
        Ok(Self {
            file,
            writeback,
            cache_count,
            direct,
            helper: None,
            #[cfg(test)]
            no_helper: false,
        })
    }

    fn direct(&self) -> io::Result<&fs::File> {
        self.direct
            .as_ref()
            .ok_or_else(|| io::ErrorKind::Unsupported.into())
    }
}

/// A descriptor for direct reads of `file`, opened by the name `file`
/// was just opened by. The name could have been given to another file
/// in between, so the two are compared, and a difference is an error:
/// reads from the device must never come from another file.
#[cfg(target_os = "linux")]
fn open_direct(file: &fs::File, path: &Path) -> io::Result<Option<fs::File>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let direct = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECT)
        .open(path)
    {
        Ok(direct) => direct,
        // How a file system says that it has no direct I/O.
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => return Ok(None),
        Err(e) => return Err(e),
    };
    let (a, b) = (file.metadata()?, direct.metadata()?);
    if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
        return Err(io::Error::other(
            "the file's name was given to another file while it was opened",
        ));
    }
    Ok(Some(direct))
}

#[cfg(not(target_os = "linux"))]
fn open_direct(_: &fs::File, _: &Path) -> io::Result<Option<fs::File>> {
    Ok(None)
}

/// Read as much of `buf` as the file has from `offset`.
#[cfg(unix)]
fn read_some_at(file: &fs::File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        match std::os::unix::fs::FileExt::read_at(file, &mut buf[done..], offset + done as u64) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

#[cfg(not(unix))]
fn read_some_at(_: &fs::File, _: &mut [u8], _: u64) -> io::Result<usize> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(unix)]
fn read_exact_at(file: &fs::File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(unix)]
fn write_all_at(file: &fs::File, data: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, data, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &fs::File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset)? {
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn write_all_at(file: &fs::File, mut data: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !data.is_empty() {
        match file.seek_write(data, offset)? {
            0 => return Err(io::ErrorKind::WriteZero.into()),
            n => {
                data = &data[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

impl File for FsFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_exact_at(&self.file, buf, offset)
    }

    fn read_device_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let direct = self.direct()?;
        // Direct reads take whole aligned blocks into aligned memory, so
        // they go through a buffer of our own. A `Vec` promises no
        // alignment: allocate one block more and start at the first
        // aligned byte.
        const CHUNK: usize = 1 << 20;
        let align = DIRECT_ALIGN as u64;
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let whole = (end.div_ceil(align) * align - offset / align * align) as usize;
        let chunk = whole.min(CHUNK);
        let mut backing = vec![0u8; chunk + DIRECT_ALIGN];
        let skew = backing.as_ptr().align_offset(DIRECT_ALIGN);
        let bounce = &mut backing[skew..skew + chunk];
        let mut at = offset;
        while at < end {
            let from = at / align * align;
            let want = ((end - from).div_ceil(align) * align).min(chunk as u64) as usize;
            let got = read_some_at(direct, &mut bounce[..want], from)?;
            let skip = (at - from) as usize;
            let n = ((end - at) as usize).min(want - skip);
            if got < skip + n {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            buf[(at - offset) as usize..][..n].copy_from_slice(&bounce[skip..skip + n]);
            at += n as u64;
        }
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        write_all_at(&self.file, data, offset)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// The sync is done by a thread of the file's own, started the
    /// first time and kept while the file is open, through a
    /// descriptor of its own on the file: `fdatasync` covers what was
    /// written to the file before it is called, through any descriptor.
    ///
    /// A helper that cannot be started (no descriptor, no thread: the
    /// daemon locks its memory, and a thread's stack with it) begins
    /// nothing and says so ([`NotBegun`]); the next sync tries again.
    fn sync_begin(&mut self, wake: Box<dyn FnOnce() + Send>) -> io::Result<()> {
        if self.helper.is_none() {
            #[cfg(test)]
            if self.no_helper {
                return Err(NotBegun::error("a test refused the helper"));
            }
            let started = self.file.try_clone().and_then(SyncHelper::start);
            self.helper = Some(started.map_err(NotBegun::error)?);
        }
        let sent = self
            .helper
            .as_ref()
            .and_then(|helper| helper.jobs.as_ref())
            .is_some_and(|jobs| jobs.send(Job::Sync(wake)).is_ok());
        if sent {
            Ok(())
        } else {
            Err(HelperEnded::error())
        }
    }

    fn sync_end(&mut self, wait: bool) -> Option<io::Result<()>> {
        let Some(helper) = &self.helper else {
            return Some(self.sync());
        };
        if wait {
            Some(
                helper
                    .results
                    .recv()
                    .unwrap_or_else(|_| Err(HelperEnded::error())),
            )
        } else {
            match helper.results.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(HelperEnded::error())),
            }
        }
    }

    fn sync_wakes(&self) -> bool {
        true
    }

    fn start_writeback(&mut self, offset: u64, len: u64) -> io::Result<()> {
        match self.writeback {
            Some(call) => call(&self.file, offset, len),
            None => Err(io::ErrorKind::Unsupported.into()),
        }
    }

    fn drop_cache(&mut self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            rustix::fs::fadvise(&self.file, 0, None, rustix::fs::Advice::DontNeed)
                .map_err(io::Error::from)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    fn cached_pages(&self) -> io::Result<u64> {
        match self.cache_count {
            Some(call) => call(&self.file),
            None => Err(io::ErrorKind::Unsupported.into()),
        }
    }

    fn preallocate(&mut self, len: u64) -> io::Result<()> {
        let old = self.file.metadata()?.len();
        if len < old {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "preallocate does not shrink a file",
            ));
        }
        // Written zeros, not a reservation: a write into reserved space
        // still changes the file's metadata, and the next sync would pay
        // for that.
        let zeros = vec![0u8; 1 << 20];
        let mut at = old;
        while at < len {
            let n = (len - at).min(zeros.len() as u64) as usize;
            write_all_at(&self.file, &zeros[..n], at)?;
            at += n as u64;
        }
        Ok(())
    }

    fn extend(&mut self, len: u64) -> io::Result<()> {
        if len < self.file.metadata()?.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "extend does not shrink a file",
            ));
        }
        self.file.set_len(len)
    }

    #[cfg(target_os = "linux")]
    fn deallocate(&mut self, offset: u64, len: u64) -> io::Result<()> {
        use rustix::fs::{fallocate, FallocateFlags};
        use rustix::io::Errno;
        if len == 0 {
            return Ok(());
        }
        let flags = FallocateFlags::PUNCH_HOLE | FallocateFlags::KEEP_SIZE;
        match retried(|| fallocate(&self.file, flags, offset, len)) {
            Ok(()) => Ok(()),
            // The file system has no holes, or the kernel no such call.
            Err(Errno::OPNOTSUPP | Errno::NOSYS) => Err(io::ErrorKind::Unsupported.into()),
            Err(e) => Err(e.into()),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn deallocate(&mut self, _offset: u64, _len: u64) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    #[cfg(target_os = "linux")]
    fn block_size(&self) -> io::Result<u64> {
        said_block_size(retried(|| rustix::fs::fstatfs(&self.file))?.f_bsize)
    }

    #[cfg(not(target_os = "linux"))]
    fn block_size(&self) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page size is one a log can take as its write unit.
    #[test]
    fn the_page_size_is_a_valid_write_unit() {
        let size = page_size();
        assert!(size.is_power_of_two());
        assert!((4096..=65536).contains(&size), "{size}");
    }

    /// A call that a signal interrupts is made again, and any other
    /// answer is passed on as it is.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_interrupted_call_is_made_again() {
        use rustix::io::Errno;
        let mut calls = 0;
        let done = retried(|| {
            calls += 1;
            if calls < 4 {
                Err(Errno::INTR)
            } else {
                Ok(calls)
            }
        });
        assert_eq!(done, Ok(4));
        let mut calls = 0;
        let refused: Result<(), Errno> = retried(|| {
            calls += 1;
            Err(Errno::OPNOTSUPP)
        });
        assert_eq!((refused, calls), (Err(Errno::OPNOTSUPP), 1));
    }

    /// A real file says the block size of its file system: a power of
    /// two of at least 512 bytes.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_file_says_its_file_systems_block_size() {
        let keep = tempfile::tempdir().expect("a directory");
        let dir = FsDir::new(keep.path()).expect("the directory");
        let file = dir.create("image").expect("a file");
        let size = file.block_size().expect("a block size");
        assert!(size >= 512 && size.is_power_of_two(), "{size}");
        assert_eq!(dir.block_size().expect("the directory's"), size);
    }

    /// On a real file: a deallocated range reads as zeros, the file
    /// keeps its length, what lies beside the range stays, and the
    /// file holds less space than before. A file system that has no
    /// such call must say "unsupported" and change nothing; the test
    /// then says so and checks only that.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_deallocated_range_of_a_real_file_reads_as_zeros_and_frees_its_space() {
        use std::os::unix::fs::MetadataExt;
        const MIB: usize = 1 << 20;
        let keep = tempfile::tempdir().expect("a directory");
        let dir = FsDir::new(keep.path()).expect("the directory");
        let mut file = dir.create("image").expect("a file");
        file.write_at(0, &vec![7u8; 4 * MIB]).expect("write");
        file.sync().expect("sync");
        let path = keep.path().join("image");
        let before = fs::metadata(&path).expect("metadata").blocks();
        match file.deallocate(MIB as u64, 2 * MIB as u64) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                eprintln!("this file system cannot deallocate: only the refusal is checked");
                let mut all = vec![0u8; 4 * MIB];
                file.read_at(0, &mut all).expect("read");
                assert!(all.iter().all(|b| *b == 7));
                return;
            }
            Err(e) => panic!("deallocate: {e}"),
        }
        file.sync().expect("sync");
        assert_eq!(file.len().expect("len"), 4 * MIB as u64);
        let mut all = vec![0xEEu8; 4 * MIB];
        file.read_at(0, &mut all).expect("read");
        assert!(all[..MIB].iter().all(|b| *b == 7));
        assert!(all[MIB..3 * MIB].iter().all(|b| *b == 0));
        assert!(all[3 * MIB..].iter().all(|b| *b == 7));
        let after = fs::metadata(&path).expect("metadata").blocks();
        // In units of 512 bytes: 2 MiB less, give or take metadata.
        assert!(
            after + 3 * 1024 <= before,
            "the file held {before} blocks and holds {after}"
        );
        // An empty range is nothing.
        file.deallocate(0, 0).expect("an empty range");
    }

    /// The lock is on the file the name `lock` names. A lock that
    /// lands on a file whose name was unlinked after it was opened, or
    /// replaced by a new file, is not taken: such a lock excludes
    /// nobody, since the next instance opens the name and locks
    /// another file. The plain case locks, and a second try finds the
    /// directory locked.
    #[cfg(unix)]
    #[test]
    fn a_lock_on_a_file_the_name_no_longer_names_is_not_taken() {
        let keep = tempfile::tempdir().expect("a directory");
        let path = keep.path().join(LOCK_NAME);
        let open = || {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .expect("the lock file")
        };
        // Opened, then unlinked by whoever cleans up: the name is gone.
        let early = open();
        fs::remove_file(&path).expect("unlink");
        assert!(finish_lock(early, &path).expect("no error").is_none());
        // Opened, unlinked, and the name made again by a third: the
        // name is another file.
        let early = open();
        fs::remove_file(&path).expect("unlink");
        let dir = FsDir::new(keep.path()).expect("the directory");
        let third = dir.try_lock().expect("the third's lock");
        assert!(finish_lock(early, &path).expect("no error").is_none());
        // The third holds the lock, and everybody else sees that.
        let refused = dir.try_lock().expect_err("locked");
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        let refused = finish_lock(open(), &path).expect_err("locked");
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        drop(third);
        // The plain case: the file the name names, locked.
        let lock = finish_lock(open(), &path)
            .expect("no error")
            .expect("the lock");
        assert_eq!(
            dir.try_lock().expect_err("locked").kind(),
            io::ErrorKind::WouldBlock
        );
        drop(lock);
        assert!(dir.try_lock().is_ok());
    }

    /// A shared lock is held until its last share is dropped, whichever
    /// that is, and on whatever thread the others went.
    #[test]
    fn a_shared_lock_is_held_until_the_last_share_goes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let refused =
            |dir: &FsDir| matches!(dir.try_lock(), Err(e) if e.kind() == io::ErrorKind::WouldBlock);
        let first = dir.try_lock().unwrap();
        let share = first.share();
        std::thread::spawn(move || drop(first)).join().unwrap();
        assert!(refused(&dir), "free although a share is held");
        let another = share.share();
        drop(share);
        assert!(refused(&dir));
        drop(another);
        assert!(dir.try_lock().is_ok());
    }

    /// The lock is free as soon as its holder is dropped, also while
    /// other threads start child processes. A child holds a copy of
    /// every descriptor from its fork to its exec, and a lock that was
    /// only closed would stay held through that copy: without the
    /// unlock in `FsLock`'s drop about one round in ten finds the
    /// directory locked. The rounds go on until 300 children have been
    /// started.
    #[cfg(unix)]
    #[test]
    fn a_dropped_lock_is_free_while_children_are_started() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let stop = AtomicBool::new(false);
        let started = AtomicU32::new(0);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        let status = std::process::Command::new("true").status();
                        assert!(status.is_ok_and(|s| s.success()));
                        started.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
            let mut held = 0;
            let mut rounds = 0;
            while started.load(Ordering::Relaxed) < 300 {
                rounds += 1;
                match dir.try_lock() {
                    Ok(lock) => drop(lock),
                    Err(e) => {
                        assert_eq!(e.kind(), io::ErrorKind::WouldBlock, "{e}");
                        held += 1;
                    }
                }
            }
            stop.store(true, Ordering::Relaxed);
            assert_eq!(
                held, 0,
                "rounds of {rounds} that found the dropped lock held"
            );
        });
    }

    /// Real files, device reads: where the file system has them they
    /// return what was written, synced or not, at any offset and length,
    /// across the buffer's chunk size, and fail beyond the end of the
    /// file. Where it has none the answer is "unsupported", which this
    /// test reports and accepts.
    #[test]
    fn real_device_reads_match_what_was_written() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let mut f = dir.create("a").unwrap();
        let len = (1 << 20) + 3 * 4096 + 100;
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        f.write_at(0, &data).unwrap();
        let mut probe = [0u8; 1];
        match f.read_device_at(0, &mut probe) {
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                eprintln!("no direct reads on {}: not exercised", tmp.path().display());
                return;
            }
            other => other.unwrap(),
        }
        for (offset, n) in [
            (0, len),
            (1, 4095),
            (4095, 2),
            (5000, 1 << 20),
            (len - 7, 7),
        ] {
            let mut buf = vec![0xEE; n];
            f.read_device_at(offset as u64, &mut buf).unwrap();
            assert_eq!(buf, data[offset..offset + n], "at {offset} for {n}");
        }
        f.sync().unwrap();
        let mut buf = vec![0u8; 16];
        assert_eq!(
            f.read_device_at(len as u64 - 8, &mut buf)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    /// Real files: a handle's reads from the device stay with its file.
    /// After the file was renamed and its old name given to another
    /// file, a device read through the handle returns the renamed
    /// file's bytes, as its cached reads do; the same after it was
    /// removed.
    #[test]
    fn real_device_reads_follow_the_file_not_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let mut old = dir.create("a").unwrap();
        old.write_at(0, &[1u8; 8192]).unwrap();
        old.sync().unwrap();
        dir.rename("a", "b").unwrap();
        let mut other = dir.create("a").unwrap();
        other.write_at(0, &[2u8; 8192]).unwrap();
        other.sync().unwrap();
        let mut buf = [0u8; 4096];
        match old.read_device_at(4096, &mut buf) {
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                eprintln!("no direct reads on {}: not exercised", tmp.path().display());
                return;
            }
            other => other.unwrap(),
        }
        assert_eq!(buf, [1u8; 4096]);
        other.read_device_at(0, &mut buf).unwrap();
        assert_eq!(buf, [2u8; 4096]);
        dir.remove("b").unwrap();
        old.write_at(0, &[3u8; 4096]).unwrap();
        old.read_device_at(0, &mut buf).unwrap();
        assert_eq!(buf, [3u8; 4096]);
    }

    /// Real files: a new file can be read and written by its owner
    /// only, since one of the log's files holds the log's key.
    #[cfg(unix)]
    #[test]
    fn real_files_are_private_to_their_owner() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        dir.create("a").unwrap();
        let mode = fs::metadata(tmp.path().join("a"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// Real files: create, write, read back, rename, list, reopen and
    /// remove behave as the traits say, and the syncs succeed.
    #[test]
    fn real_files_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        assert_eq!(dir.path(), tmp.path());
        let mut f = dir.create("a").unwrap();
        assert!(f.is_empty().unwrap());
        assert_eq!(
            dir.create("a").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        f.preallocate(8192).unwrap();
        assert_eq!(f.len().unwrap(), 8192);
        assert!(f.preallocate(100).is_err());
        f.write_at(4000, b"hello").unwrap();
        f.sync().unwrap();
        dir.sync().unwrap();
        let mut buf = [0xEE; 7];
        f.read_at(3999, &mut buf).unwrap();
        assert_eq!(&buf, b"\0hello\0");
        assert_eq!(
            f.read_at(8190, &mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        dir.rename("a", "b").unwrap();
        assert_eq!(dir.list().unwrap(), vec!["b".to_owned()]);
        let g = dir.open("b").unwrap();
        g.read_at(3999, &mut buf).unwrap();
        assert_eq!(&buf, b"\0hello\0");
        dir.remove("b").unwrap();
        assert_eq!(dir.open("b").unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(FsDir::new(tmp.path().join("missing")).is_err());
    }

    /// A sync begun is done by the file's helper: it ends, it wakes,
    /// and the helper is used again; a helper that ended is said to
    /// have ended, and its wake is dropped, not left waiting.
    #[test]
    fn a_sync_is_done_by_the_files_helper() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let mut file = dir.create("a").unwrap();
        assert!(file.sync_wakes());
        for round in 0..3u8 {
            file.write_at(0, &[round; 512]).unwrap();
            let (woke, wakes) = mpsc::channel();
            file.sync_begin(Box::new(move || {
                let _ = woke.send(());
            }))
            .unwrap();
            wakes.recv().unwrap();
            assert!(file.sync_end(false).unwrap().is_ok());
        }
        // Panicked: the wake is dropped, and the end says so.
        struct Dropped(mpsc::Sender<()>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        // A helper that cannot be started begins nothing and says so;
        // the file syncs as ever, and the next begin starts one.
        let mut other = dir.create("b").unwrap();
        other.no_helper = true;
        let refused = other.sync_begin(Box::new(|| {})).unwrap_err();
        assert!(
            NotBegun::is(&refused) && !HelperEnded::is(&refused),
            "{refused}"
        );
        assert!(other.helper.is_none());
        other.write_at(0, &[1; 512]).unwrap();
        other.sync().unwrap();
        other.no_helper = false;
        other.sync_begin(Box::new(|| {})).unwrap();
        assert!(other.sync_end(true).unwrap().is_ok());
        let helper = file.helper.as_ref().unwrap();
        helper.jobs.as_ref().unwrap().send(Job::Panic).unwrap();
        let (dropped, gone) = mpsc::channel();
        let guard = Dropped(dropped);
        let begun = file.sync_begin(Box::new(move || drop(guard)));
        if begun.is_ok() {
            gone.recv().unwrap();
        }
        let ended = match begun {
            Ok(()) => file.sync_end(true).unwrap().unwrap_err(),
            Err(e) => e,
        };
        assert!(HelperEnded::is(&ended), "{ended}");
    }
}
