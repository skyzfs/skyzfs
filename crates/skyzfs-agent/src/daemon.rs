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
use std::path::PathBuf;
use std::time::Duration;

use skyzfs_control::{Reply, Request};

use crate::engine::Daemon;
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
}

#[cfg(test)]
mod tests;
