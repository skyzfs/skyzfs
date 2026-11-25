//! The file-system calls of the agent's record ([`crate::record::Store`]):
//! each step of a write, so that the tests can cut the power after any of
//! them, or fail any of them, on a model that keeps what was synced apart
//! from what was not (D2 of #142's review; the model is `fs::model`, in
//! the tests only).
//!
//! [`Disk`] is the program's: each call is one system call (or `open`
//! and one), and nothing is synced that is not asked for.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write as _};
use std::path::Path;

/// What the record's store asks of the file system.
pub trait Fs {
    /// What a lock taken with [`Fs::lock`] is held by: dropped, it is
    /// released.
    type Lock;

    /// The bytes of the file at `path` (`NotFound` where there is none).
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// Make the directory `path` (`AlreadyExists` where it is), not
    /// synced: its entry is in its parent, which [`Fs::sync_dir`] keeps.
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Make the file at `path` hold `bytes` (made, or cut to none and
    /// written), not synced.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Sync the file at `path`: its bytes, not its entry.
    fn sync_file(&self, path: &Path) -> io::Result<()>;
    /// Rename `from` over `to`, not synced.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Sync the directory `path`: its entries.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;
    /// Remove the file at `path` (`NotFound` where there is none), not
    /// synced.
    fn unlink(&self, path: &Path) -> io::Result<()>;
    /// The names in the directory `path` (`NotFound` where there is
    /// none), in no order.
    fn list(&self, path: &Path) -> io::Result<Vec<String>>;
    /// Take the lock of the file `path` (made where it is not), or
    /// `WouldBlock` where another holds it.
    fn lock(&self, path: &Path) -> io::Result<Self::Lock>;
}

/// The program's file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct Disk;

impl Fs for Disk {
    type Lock = File;

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?
            .write_all(bytes)
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        // `fsync` of any descriptor of the file syncs its bytes.
        File::open(path)?.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        File::open(path)?.sync_all()
    }

    fn unlink(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn list(&self, path: &Path) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(path)? {
            if let Some(name) = entry?.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    fn lock(&self, path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
}

#[cfg(test)]
pub mod model;

#[cfg(test)]
mod tests {
    use super::*;

    /// `Disk` does what the trait says on a real directory (T3 of #144's
    /// review; whether its syncs reach the disk needs a kernel rig).
    ///
    /// Mutant: `write` that does not cut the file (a shorter record left
    /// with the tail of a longer one), `unlink` or `rename` that does
    /// nothing, `create_dir` that does not refuse a directory that is
    /// there, or `list` that leaves names out.
    #[test]
    fn disk_writes_renames_lists_and_removes_files_in_a_directory() {
        let root = tempfile::tempdir().expect("a directory");
        let dir = root.path().join("d");
        let disk = Disk;
        disk.create_dir(&dir).expect("made");
        assert_eq!(
            disk.create_dir(&dir).map_err(|e| e.kind()),
            Err(io::ErrorKind::AlreadyExists)
        );
        disk.sync_dir(root.path()).expect("parent synced");
        let tmp = dir.join("a.tmp");
        let file = dir.join("a");
        disk.write(&tmp, b"a longer record").expect("written");
        disk.write(&tmp, b"short").expect("written over");
        disk.sync_file(&tmp).expect("synced");
        assert_eq!(disk.read(&tmp).expect("read"), b"short");
        disk.rename(&tmp, &file).expect("renamed");
        disk.sync_dir(&dir).expect("synced");
        assert_eq!(disk.read(&file).expect("read"), b"short");
        assert_eq!(
            disk.read(&tmp).map_err(|e| e.kind()),
            Err(io::ErrorKind::NotFound)
        );
        disk.write(&dir.join("b"), b"").expect("written");
        let mut names = disk.list(&dir).expect("listed");
        names.sort();
        assert_eq!(names, ["a", "b"]);
        disk.unlink(&file).expect("removed");
        assert_eq!(
            disk.unlink(&file).map_err(|e| e.kind()),
            Err(io::ErrorKind::NotFound)
        );
        assert_eq!(disk.list(&dir).expect("listed"), ["b"]);
        assert_eq!(
            disk.list(&root.path().join("none")).map_err(|e| e.kind()),
            Err(io::ErrorKind::NotFound)
        );
    }
}
