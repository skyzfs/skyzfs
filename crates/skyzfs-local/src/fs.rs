//! The store's file operations on real files.
//!
//! [`File::sync`] is `fdatasync` on Linux, and on macOS the full flush
//! (`F_FULLFSYNC`) that the standard library issues for it: design §5.1
//! needs a flush that returns only when the data is persistent.
//! [`Dir::sync`] syncs the directory itself, which is what makes a new
//! name survive on Linux and macOS; Windows has no such call and journals
//! the name on its own.

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
        Ok(FsFile { file })
    }

    fn open(&self, name: &str) -> io::Result<FsFile> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.path.join(name))?;
        Ok(FsFile { file })
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
