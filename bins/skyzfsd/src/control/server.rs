//! The channel's socket and the thread that answers it ([module
//! docs](super)).

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use super::{answer, Code, Handler, Reply, HELLO, IO_LIMIT};

/// How long [`Channel::close`] waits for the thread. One that is still
/// in a request after it (waiting on the uploader) is left to end on its
/// own: it answers nothing more.
pub const CLOSE_LIMIT: Duration = Duration::from_secs(5);

/// What the channel has done, for the status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChannelFigures {
    /// Requests answered, refusals included.
    pub answered: u64,
    /// Connections refused for their peer's user, before reading them.
    pub peer: u64,
    /// Requests refused as `version` or `malformed`.
    pub malformed: u64,
}

#[derive(Default)]
struct Counts {
    answered: AtomicU64,
    peer: AtomicU64,
    malformed: AtomicU64,
}

impl Counts {
    fn figures(&self) -> ChannelFigures {
        ChannelFigures {
            answered: self.answered.load(Ordering::Relaxed),
            peer: self.peer.load(Ordering::Relaxed),
            malformed: self.malformed.load(Ordering::Relaxed),
        }
    }
}

/// What the channel's thread is given after each connection it counts:
/// its figures, for the status (M2.5-2).
pub type Post = Box<dyn Fn(ChannelFigures) + Send>;

/// The channel's socket, answered on a thread of its own until
/// [`Channel::close`].
pub struct Channel {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
    ended: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

/// Whether a peer of this user may ask: the daemon's own user or root.
pub(super) fn same_user_or_root(uid: u32) -> bool {
    uid == 0 || uid == rustix::process::geteuid().as_raw()
}

/// The peer's user, read from the connection (`SO_PEERCRED`).
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    Ok(rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw())
}

/// Remove a socket left at `path` by an instance that ended without
/// removing it. The caller holds the volume's lock, so no other daemon
/// of the volume answers it. Anything there that is not a socket is left
/// alone, and the bind that follows fails.
fn remove_stale(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => fs::remove_file(path),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

impl Channel {
    /// Make the socket at `path` and answer it with `handler`, for a peer
    /// of the daemon's own user or root.
    pub fn open(path: &Path, handler: Handler) -> io::Result<Self> {
        Self::open_checked(path, handler, same_user_or_root, Box::new(|_| {}))
    }

    /// As [`Channel::open`], with `post` given the figures after each
    /// connection the thread counts.
    pub fn open_posting(path: &Path, handler: Handler, post: Post) -> io::Result<Self> {
        Self::open_checked(path, handler, same_user_or_root, post)
    }

    /// As [`Channel::open_posting`], with `allowed` saying which peers'
    /// users may ask (the tests refuse one).
    pub fn open_checked(
        path: &Path,
        mut handler: Handler,
        allowed: fn(u32) -> bool,
        post: Post,
    ) -> io::Result<Self> {
        remove_stale(path)?;
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let stop = Arc::new(AtomicBool::new(false));
        let counts = Arc::new(Counts::default());
        let (ended_tx, ended) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("skyzfsd-control".into())
            .spawn({
                let stop = Arc::clone(&stop);
                let counts = Arc::clone(&counts);
                move || {
                    serve(&listener, &mut handler, allowed, &stop, &counts, &post);
                    let _ = ended_tx.send(());
                }
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            stop,
            counts,
            ended,
            thread: Some(thread),
        })
    }

    /// The socket's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What it has done so far.
    pub fn figures(&self) -> ChannelFigures {
        self.counts.figures()
    }

    /// Stop answering and remove the socket. Whether the thread ended
    /// within [`CLOSE_LIMIT`]: one that did not is in a request, and
    /// answers nothing after it.
    pub fn close(mut self) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        // The thread waits in accept(): a connection of our own wakes it,
        // and it sees the flag before it reads anything.
        let _ = UnixStream::connect(&self.path);
        let ended = self.ended.recv_timeout(CLOSE_LIMIT).is_ok();
        // A thread that did not end is left to (dropping its handle).
        let thread = self.thread.take();
        if let (true, Some(thread)) = (ended, thread) {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
        ended
    }
}

impl Drop for Channel {
    /// A channel dropped without [`Channel::close`] stops its thread
    /// without waiting for it, and removes the socket.
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop.store(true, Ordering::SeqCst);
            let _ = UnixStream::connect(&self.path);
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The thread: one connection at a time, one request each.
fn serve(
    listener: &UnixListener,
    handler: &mut Handler,
    allowed: fn(u32) -> bool,
    stop: &AtomicBool,
    counts: &Counts,
    post: &Post,
) {
    loop {
        let accepted = listener.accept();
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok((mut stream, _)) = accepted else {
            // A connection that failed before it was accepted (its peer
            // gave up): nothing to answer. Others still are.
            continue;
        };
        if stream.set_read_timeout(Some(IO_LIMIT)).is_err()
            || stream.set_write_timeout(Some(IO_LIMIT)).is_err()
        {
            continue;
        }
        match peer_uid(&stream) {
            Ok(uid) if allowed(uid) => {}
            _ => {
                counts.peer.fetch_add(1, Ordering::Relaxed);
                post(counts.figures());
                let refusal = Reply::Refused(Code::Peer, "this daemon answers its own user".into());
                let _ = stream.write_all(format!("{HELLO}\n{}\n", refusal.line()).as_bytes());
                continue;
            }
        }
        // An error: the peer went away, or sent nothing in time, and no
        // answer reaches it.
        if let Ok(reply) = answer(&mut stream, handler) {
            counts.answered.fetch_add(1, Ordering::Relaxed);
            if matches!(reply, Reply::Refused(Code::Malformed | Code::Version, _)) {
                counts.malformed.fetch_add(1, Ordering::Relaxed);
            }
            post(counts.figures());
        }
    }
}
