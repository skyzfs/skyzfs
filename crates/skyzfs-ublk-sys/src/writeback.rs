//! Starting the writeback of a range of a file early: the call behind
//! the store's hint (`skyzfs_local::writeback`), which the daemon gives
//! to the store's directory as its hook.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

/// Ask the kernel to start writing the dirty pages of `len` bytes of
/// `file` at `offset` to the device, and return without waiting for
/// them: `sync_file_range` with `SYNC_FILE_RANGE_WRITE` alone.
///
/// It makes nothing durable and does not wait for the writeback it
/// starts, nor take the file's writeback error, which the next
/// `fdatasync` of the file still reports. It can wait to submit the
/// pages (for the block layer's queue, or for a page that is already
/// under writeback), but not for the device to finish them.
///
/// The answers the kernel gives where it has no such call for the file
/// (`ENOSYS`, `EINVAL`, `EOPNOTSUPP`, `ESPIPE`) are
/// [`io::ErrorKind::Unsupported`]; any other is the kernel's error.
pub fn start_writeback(file: &File, offset: u64, len: u64) -> io::Result<()> {
    let (Ok(offset), Ok(len)) = (i64::try_from(offset), i64::try_from(len)) else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    if offset.checked_add(len).is_none() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: `sync_file_range` takes a descriptor and three integers
    // and no pointer. The descriptor is `file`'s, open for as long as
    // the borrow lasts.
    let result = unsafe {
        libc::sync_file_range(file.as_raw_fd(), offset, len, libc::SYNC_FILE_RANGE_WRITE)
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP | libc::ESPIPE) => {
            Err(io::ErrorKind::Unsupported.into())
        }
        _ => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// On a regular file the call is made and returns: on a file system
    /// that has it, at once and without error; on one that has not, as
    /// unsupported. Nothing the file holds changes.
    #[test]
    fn a_files_writes_can_be_handed_over() {
        let path = std::env::temp_dir().join(format!(
            "skyzfs-writeback-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .expect("a file");
        file.write_all(&[7u8; 1 << 20]).expect("written");
        match start_writeback(&file, 0, 1 << 20) {
            Ok(()) => {}
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}"),
        }
        assert_eq!(std::fs::read(&path).expect("read"), vec![7u8; 1 << 20]);
        assert_eq!(
            start_writeback(&file, u64::MAX, 1).map_err(|e| e.kind()),
            Err(io::ErrorKind::InvalidInput)
        );
        drop(file);
        let _ = std::fs::remove_file(&path);
    }
}
