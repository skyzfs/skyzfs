//! The client: [`ask`] a daemon through its socket.

use std::io::{self, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::{read_line, Reply, Request, HELLO};

/// Ask the daemon behind the socket at `path`: `request`, with
/// `manifest` after it for `publish`, waiting at most `limit` for the
/// connection and for each read and write. The answer, or why none came.
pub fn ask(path: &Path, request: &Request, manifest: &[u8], limit: Duration) -> io::Result<Reply> {
    let mut stream = connect(path, limit)?;
    stream.set_read_timeout(Some(limit))?;
    stream.set_write_timeout(Some(limit))?;
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

/// A connection to the socket at `path`, made within `limit`. A blocking
/// connect to a socket whose backlog is full waits for room, before any
/// timeout of the stream applies: a daemon that has stopped taking
/// connections would hold it without end (Codex on #129). So the connect
/// is made without blocking, and made again while the backlog is full,
/// at most until the limit; then the stream blocks, and `ask` bounds its
/// reads and writes with timeouts.
fn connect(path: &Path, limit: Duration) -> io::Result<UnixStream> {
    connect_counted(path, limit, &mut 0)
}

/// [`connect`], with its tries counted in `tries`: the tests hold the
/// pauses between them to their growth.
pub(crate) fn connect_counted(
    path: &Path,
    limit: Duration,
    tries: &mut u32,
) -> io::Result<UnixStream> {
    use rustix::io::Errno;
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let socket = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )?;
    let address = SocketAddrUnix::new(path)?;
    let until = Instant::now() + limit;
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
                            limit.as_secs_f64()
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
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// The longest pause between two tries of a connect while the daemon's
/// backlog is full.
const CONNECT_PAUSE: Duration = Duration::from_millis(50);
