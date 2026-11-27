//! [`Daemon`] on skyzfsd's channel and its block device
//! ([`DaemonSocket`]).
//!
//! The channel is the socket [`skyzfs_control::SOCKET`] in the volume's
//! own directory under the daemon's run directory
//! ([`skyzfs_control::socket_path`]), asked with [`skyzfs_control::ask`]: one deadline for each
//! request, [`skyzfs_control::ASK_LIMIT`] by default. The device is the
//! volume's `/dev/ublkbN`, as `skyzfsd serve` prints it (`ready
//! /dev/ublkbN`): the agent links nothing of the daemon and does not read
//! its status file, so it is given the path. Its device number is read
//! at each call, since the daemon may have been restarted on another
//! device.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use skyzfs_control::failure::{decode_failure, parse_boot_id, BOOT_ID, FAILED};
use skyzfs_control::{Reply, Request};

use crate::engine::{Daemon, Noted};
use crate::host::{Host, Node};

/// The daemon of one volume ([module docs](self)).
#[derive(Debug)]
pub struct DaemonSocket<H> {
    socket: PathBuf,
    device: PathBuf,
    limit: Duration,
    host: H,
}

impl<H: Host> DaemonSocket<H> {
    /// The daemon whose channel is the socket at `socket` and whose
    /// block device is at `device`, each ask within `limit`; `host`
    /// reads the device.
    pub fn new(socket: PathBuf, device: PathBuf, limit: Duration, host: H) -> Self {
        DaemonSocket {
            socket,
            device,
            limit,
            host,
        }
    }
}

impl<H: Host> Daemon for DaemonSocket<H> {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        skyzfs_control::ask(&self.socket, request, manifest, self.limit)
    }

    fn device(&mut self) -> io::Result<u64> {
        match self.host.stat(&self.device)? {
            Node::Block(rdev) => Ok(rdev),
            Node::File | Node::Other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a block device", self.device.display()),
            )),
        }
    }

    /// The note is [`FAILED`] beside the socket: both are in the volume's
    /// own directory under the run directory
    /// ([`skyzfs_control::volume_run_dir`]). Its boot is compared with
    /// the kernel's ([`BOOT_ID`]), read at each call.
    fn failure(&mut self) -> io::Result<Option<Noted>> {
        let path = self.socket.with_file_name(FAILED);
        let bytes = match self.host.read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(stored) = decode_failure(&bytes) else {
            return Ok(Some(Noted::NotWhole));
        };
        let text = self.host.read(Path::new(BOOT_ID))?;
        let boot = parse_boot_id(&String::from_utf8_lossy(&text)).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{BOOT_ID}: not the kernel's boot id"),
            )
        })?;
        Ok((stored.boot == boot).then_some(Noted::Failed(stored.failure)))
    }
}

#[cfg(test)]
mod tests;
