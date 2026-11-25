//! The client: [`ask`] a daemon through its socket.
//!
//! # Who answers
//!
//! The agent runs as root and acts on the daemon's answer (it releases
//! its hold and forgets a publish on "done"), so the client asks only a
//! daemon it trusts (the lead's L5 on #147). Before connecting, the path
//! is read without following a link (`lstat`): it must be a socket, not a
//! symbolic link, owned by root or by the client's own effective user.
//! After connecting, the peer's user is read from the connection
//! (`SO_PEERCRED`), and any user but root or the client's own is
//! refused. Both refusals are `PermissionDenied`, and nothing is written
//! to a peer refused. The daemon's own check of its peer is the same
//! rule (`skyzfsd`'s `same_user_or_root`).

use std::io::{self, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::{read_line, Reply, Request, HELLO};

/// Ask the daemon behind the socket at `path`: `request`, with
/// `manifest` after it for `publish`. The answer, or why none came.
///
/// `limit` is one deadline for the whole request: the connection, the
/// write and the answer together (S4 of the review of #131). Each read
/// and write waits only for what is left of it, so a daemon that is slow
/// in each part, or that answers a byte at a time, cannot stretch the
/// ask past `limit` (and a little: a read under way when it runs out
/// ends with its timeout). Past it, the error is `TimedOut`.
///
/// A limit too long to add to the clock is `InvalidInput`.
pub fn ask(path: &Path, request: &Request, manifest: &[u8], limit: Duration) -> io::Result<Reply> {
    let until = deadline(limit)?;
    let stream = connect(path, until)?;
    let mut stream = Deadline { stream, until };
    let mut sent = format!("{HELLO}\n{}\n", request.line()).into_bytes();
    sent.extend_from_slice(manifest);
    // A daemon that refuses a request before reading all of it closes
    // the connection, and the rest of the write fails: its answer is read
    // all the same, and the write's error given only when none came.
    let written = stream.write_all(&sent).and_then(|()| stream.flush());
    let mut reader = BufReader::new(stream);
    let hello = match (read_line(&mut reader), written) {
        (Ok(hello), _) => hello,
        (Err(_), Err(e)) => return Err(e),
        (Err(e), Ok(())) => return Err(e),
    };
    if hello != HELLO {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the daemon greets with `{hello}`, not `{HELLO}`"),
        ));
    }
    let line = read_line(&mut reader)?;
    Reply::parse(&line).map_err(|why| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the answer `{line}`: {why}"),
        )
    })
}

/// A stream whose reads and writes all end by `until`: each waits only
/// for what is left ([`ask`]).
struct Deadline {
    stream: UnixStream,
    until: Instant,
}

impl Deadline {
    /// What is left of the deadline, as the timeout of the next call.
    fn left(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(timed_out())
        } else {
            Ok(left)
        }
    }
}

/// The instant `limit` from now; `InvalidInput` where it is too long to
/// add to the clock.
pub(crate) fn deadline(limit: Duration) -> io::Result<Instant> {
    Instant::now().checked_add(limit).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a limit of {} s is too long", limit.as_secs_f64()),
        )
    })
}

/// Whether `uid` may own the socket or answer on it: root or the
/// client's own effective user ([module docs](self)).
fn trusted(uid: u32) -> bool {
    uid == 0 || uid == rustix::process::geteuid().as_raw()
}

/// The path, read without following a link, is a socket that root or
/// the client's user owns ([module docs](self)).
fn check_path(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
    let meta = std::fs::symlink_metadata(path)?;
    let kind = meta.file_type();
    let refuse = |why: String| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{}: {why}", path.display()),
        ))
    };
    if kind.is_symlink() {
        return refuse("a symbolic link, not the daemon's socket".into());
    }
    if !kind.is_socket() {
        return refuse("not a socket".into());
    }
    if !trusted(meta.uid()) {
        return refuse(format!(
            "a socket of user {}, neither root nor this one",
            meta.uid()
        ));
    }
    Ok(())
}

/// The peer on `stream` is root or the client's user ([module
/// docs](self)).
fn check_peer(path: &Path, stream: &UnixStream) -> io::Result<()> {
    let uid = rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw();
    if trusted(uid) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{}: answered by user {uid}, neither root nor this one",
                path.display()
            ),
        ))
    }
}

/// The error of an ask past its deadline.
fn timed_out() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "the daemon did not answer within the ask's limit",
    )
}

/// A socket's timeout ends a call with `WouldBlock` (`EAGAIN`), which
/// is the ask's deadline here.
fn deadline_error(e: io::Error) -> io::Error {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => timed_out(),
        _ => e,
    }
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.left()?;
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf).map_err(deadline_error)
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let left = self.left()?;
        self.stream.set_write_timeout(Some(left))?;
        self.stream.write(buf).map_err(deadline_error)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// A connection to the socket at `path`, made by `until`, to a peer the
/// client trusts ([module docs](self)). A blocking connect to a socket
/// whose backlog is full waits for room, before any timeout of the
/// stream applies: a daemon that has stopped taking connections would
/// hold it without end (Codex on #129). So the connect is made without
/// blocking, and made again while the backlog is full, at most until the
/// deadline, which is the ask's own (R3 of the review of #147); then the
/// stream blocks, and `ask` bounds its reads and writes with timeouts.
fn connect(path: &Path, until: Instant) -> io::Result<UnixStream> {
    connect_counted(path, until, &mut 0)
}

/// [`connect`], with its tries counted in `tries`: the tests hold the
/// pauses between them to their growth. A path refused is tried no
/// time.
pub(crate) fn connect_counted(
    path: &Path,
    until: Instant,
    tries: &mut u32,
) -> io::Result<UnixStream> {
    use rustix::io::Errno;
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    check_path(path)?;
    let began = Instant::now();
    let socket = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )?;
    let address = SocketAddrUnix::new(path)?;
    let mut pause = Duration::from_millis(1);
    loop {
        *tries += 1;
        match rustix::net::connect(&socket, &address) {
            Ok(()) => break,
            Err(Errno::AGAIN | Errno::INTR) => {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "the daemon's socket took no connection within {} s: \
                             its backlog is full",
                            began.elapsed().as_secs_f64()
                        ),
                    ));
                }
                std::thread::sleep(pause.min(left));
                pause = (pause * 2).min(CONNECT_PAUSE);
            }
            Err(e) => return Err(e.into()),
        }
    }
    let stream = UnixStream::from(socket);
    check_peer(path, &stream)?;
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// The longest pause between two tries of a connect while the daemon's
/// backlog is full.
const CONNECT_PAUSE: Duration = Duration::from_millis(50);
