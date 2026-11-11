//! The store's file operations on real files.
//!
//! [`File::sync`] is `fdatasync` on Linux, and on macOS the full flush
//! (`F_FULLFSYNC`) that the standard library issues for it: design §5.1
//! needs a flush that returns only when the data is persistent.
//! [`Dir::sync`] syncs the directory itself, which is what makes a new
//! name survive on Linux and macOS; Windows has no such call and journals
//! the name on its own.
//!
//! [`File::read_device_at`] is a read through a second descriptor opened
//! with `O_DIRECT`, on Linux only. Where the file system refuses that
//! flag (tmpfs does), and on the other systems for now, it fails as
//! unsupported; it never falls back to a cached read.

use std::cell::OnceCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::io::{Dir, File};

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

impl Dir for FsDir {
    type File = FsFile;

    fn create(&self, name: &str) -> io::Result<FsFile> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(self.path.join(name))?;
        Ok(FsFile::new(file, self.path.join(name)))
    }

    fn open(&self, name: &str) -> io::Result<FsFile> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.path.join(name))?;
        Ok(FsFile::new(file, self.path.join(name)))
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

    #[cfg(not(unix))]
    fn sync(&self) -> io::Result<()> {
        Ok(())
    }
}

/// An open real file.
#[derive(Debug)]
pub struct FsFile {
    file: fs::File,
    path: PathBuf,
    /// A second descriptor that bypasses the cache, opened at the first
    /// device read. `None` inside: the file system has none to give.
    direct: OnceCell<Option<fs::File>>,
}

/// The alignment `O_DIRECT` wants of offsets, lengths and memory. 4 KiB
/// satisfies every device the store will meet.
const DIRECT_ALIGN: usize = 4096;

impl FsFile {
    fn new(file: fs::File, path: PathBuf) -> Self {
        Self {
            file,
            path,
            direct: OnceCell::new(),
        }
    }

    #[cfg(target_os = "linux")]
    fn open_direct(&self) -> io::Result<Option<fs::File>> {
        use std::os::unix::fs::OpenOptionsExt;
        match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECT)
            .open(&self.path)
        {
            Ok(file) => Ok(Some(file)),
            // How a file system says that it has no direct I/O.
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => Ok(None),
            Err(e) => Err(e),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn open_direct(&self) -> io::Result<Option<fs::File>> {
        Ok(None)
    }

    fn direct(&self) -> io::Result<&fs::File> {
        if self.direct.get().is_none() {
            let opened = self.open_direct()?;
            // Only this thread can have filled it meanwhile: not at all.
            let _ = self.direct.set(opened);
        }
        match self.direct.get() {
            Some(Some(file)) => Ok(file),
            _ => Err(io::ErrorKind::Unsupported.into()),
        }
    }
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

    fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
