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

use crate::io::{Dir, File};

/// The size of a page of memory on this system: what its kernel writes
/// back as a whole, and so the least write unit a log on buffered files
/// should have here ([`Identity::write_unit`]). 4096 where it cannot be
/// found out.
///
/// [`Identity::write_unit`]: crate::format::Identity::write_unit
pub fn page_size() -> u32 {
    // Linux tells every process in its auxiliary vector: pairs of
    // native words, the pair with key 6 (AT_PAGESZ) holding the size.
    #[cfg(target_os = "linux")]
    if let Ok(auxv) = fs::read("/proc/self/auxv") {
        const WORD: usize = std::mem::size_of::<usize>();
        let (pairs, _) = auxv.as_chunks::<{ 2 * WORD }>();
        for pair in pairs {
            let (key, value) = pair.split_at(WORD);
            let word = |bytes: &[u8]| {
                let mut raw = [0u8; WORD];
                raw.copy_from_slice(bytes);
                usize::from_ne_bytes(raw)
            };
            if word(key) == 6 {
                if let Ok(size) = u32::try_from(word(value)) {
                    return size;
                }
            }
        }
    }
    // Apple silicon has 16 KiB pages, other Macs 4 KiB.
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        16384
    } else {
        4096
    }
}

/// A directory on a real file system.
#[derive(Debug, Clone)]
pub struct FsDir {
    path: PathBuf,
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
        Ok(Self { path })
    }

    /// Where the directory is.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The file the directory's lock is taken on. It holds nothing.
pub const LOCK_NAME: &str = "lock";

/// The directory's lock, held: see [`Dir::try_lock`].
#[derive(Debug)]
pub struct FsLock {
    file: fs::File,
}

impl Drop for FsLock {
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

impl Dir for FsDir {
    type File = FsFile;
    type Lock = FsLock;

    /// An advisory lock (`flock`, `LockFileEx`) on the file `lock`,
    /// which is created if it is not there. Dropping the [`FsLock`]
    /// unlocks. The operating system drops the lock when the process
    /// ends, with one exception: if the process dies while a child of
    /// its own is between its fork and its exec, the child holds the
    /// lock until the exec, and the next instance may see the
    /// directory locked for that moment.
    fn try_lock(&self) -> io::Result<FsLock> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(self.path.join(LOCK_NAME))?;
        match file.try_lock() {
            Ok(()) => Ok(FsLock { file }),
            Err(fs::TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),
            Err(fs::TryLockError::Error(e)) => Err(e),
        }
    }

    fn create(&self, name: &str) -> io::Result<FsFile> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let path = self.path.join(name);
        let file = options.open(&path)?;
        FsFile::new(file, &path)
    }

    fn open(&self, name: &str) -> io::Result<FsFile> {
        let path = self.path.join(name);
        let file = fs::OpenOptions::new().read(true).write(true).open(&path)?;
        FsFile::new(file, &path)
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
            if let Ok(name) = entry?.file_name().into_string() {
                names.push(name);
            }
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

/// An open real file.
#[derive(Debug)]
pub struct FsFile {
    file: fs::File,
    /// A second descriptor on the same file that bypasses the cache.
    /// `None`: the file system, or this system, has none to give.
    direct: Option<fs::File>,
}

/// The alignment `O_DIRECT` wants of offsets, lengths and memory. 4 KiB
/// satisfies every device the store will meet.
const DIRECT_ALIGN: usize = 4096;

impl FsFile {
    /// `file`, which was just opened as `path`.
    fn new(file: fs::File, path: &Path) -> io::Result<Self> {
        let direct = open_direct(&file, path)?;
        Ok(Self { file, direct })
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
}
