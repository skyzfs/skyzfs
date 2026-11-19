use std::io::{self, Cursor, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use skyzfs_upload::publish::{EXPIRY_DEFAULT, EXPIRY_MAX, MANIFEST_MAX};
use skyzfs_upload::{Progress, Refusal};

use super::*;

const A: [u8; 16] = [0xa1; 16];
const S: [u8; 16] = [0x5e; 16];

fn publish(expiry: Option<Duration>) -> Request {
    Request::Publish(Publish {
        name: "pool-7.b_c".into(),
        attachment: A,
        state: S,
        after: 3,
        group: 41,
        manifest: 5,
        expiry,
    })
}

fn every_code() -> Vec<Code> {
    CODES.iter().map(|(code, _)| *code).collect()
}

/// Mutant: any word of the protocol changed (it is version 1's: a
/// change is a new version).
#[test]
fn the_lines_are_version_1s_words() {
    let hex_a = "a1".repeat(16);
    let hex_s = "5e".repeat(16);
    assert_eq!(Request::Cuts.line(), "cuts");
    assert_eq!(
        publish(None).line(),
        format!(
            "publish name=pool-7.b_c attachment={hex_a} state={hex_s} after=3 group=41 manifest=5"
        )
    );
    assert_eq!(
        publish(Some(Duration::from_secs(90))).line(),
        format!(
            "publish name=pool-7.b_c attachment={hex_a} state={hex_s} after=3 group=41 \
             manifest=5 expiry-s=90"
        )
    );
    assert_eq!(Request::Progress("t".into()).line(), "progress name=t");
    assert_eq!(Request::Withdraw("t".into()).line(), "withdraw name=t");
    let cuts = |state, newest| {
        Reply::Cuts(CutsReply {
            attachment: A,
            state,
            count: 7,
            newest,
        })
        .line()
    };
    assert_eq!(
        cuts(None, None),
        format!("cuts attachment={hex_a} state=none count=7 group=none opens=none")
    );
    assert_eq!(
        cuts(Some(S), Some((12, true))),
        format!("cuts attachment={hex_a} state={hex_s} count=7 group=12 opens=itself")
    );
    assert_eq!(
        cuts(Some(S), Some((12, false))),
        format!("cuts attachment={hex_a} state={hex_s} count=7 group=12 opens=rewind")
    );
    assert_eq!(
        Reply::Done([0x0f; 32]).line(),
        format!("done commit={}", "0f".repeat(32))
    );
    for (reply, line) in [
        (Reply::Accepted, "accepted"),
        (Reply::Pending, "pending"),
        (Reply::Expired, "expired"),
        (Reply::Withdrawn, "withdrawn"),
        (Reply::Unknown, "unknown"),
        (
            Reply::Refused(Code::NoAnswer, "late".into()),
            "refused no-answer late",
        ),
    ] {
        assert_eq!(reply.line(), line);
    }
    let words: Vec<&str> = every_code().into_iter().map(Code::word).collect();
    assert_eq!(
        words,
        [
            "name",
            "attachment",
            "state",
            "manifest",
            "expiry",
            "asked",
            "busy",
            "taken",
            "stopped",
            "version",
            "malformed",
            "peer",
            "no-upload",
            "ended",
            "no-answer"
        ]
    );
}

/// Mutant: a field dropped or swapped in either direction.
#[test]
fn every_line_reads_back_as_written() {
    let requests = [
        Request::Cuts,
        publish(None),
        publish(Some(EXPIRY_MAX)),
        Request::Progress("x".into()),
        Request::Withdraw("y".into()),
    ];
    for request in requests {
        assert_eq!(Request::parse(&request.line()), Ok(request));
    }
    let mut replies = vec![
        Reply::Accepted,
        Reply::Pending,
        Reply::Done([7; 32]),
        Reply::Expired,
        Reply::Withdrawn,
        Reply::Unknown,
    ];
    for state in [None, Some(S)] {
        for newest in [None, Some((0, true)), Some((u64::MAX, false))] {
            replies.push(Reply::Cuts(CutsReply {
                attachment: A,
                state,
                count: u64::MAX,
                newest,
            }));
        }
    }
    for code in every_code() {
        replies.push(Reply::Refused(code, format!("why {}", code.word())));
        replies.push(Reply::Refused(code, String::new()));
    }
    for reply in replies {
        assert_eq!(Reply::parse(&reply.line()), Ok(reply));
    }
}

/// Mutant: a key taken twice, a key of another verb, or a value parsed
/// loosely.
#[test]
fn a_line_that_is_not_exactly_a_request_does_not_parse() {
    let a = "a1".repeat(16);
    let good = format!("publish name=t attachment={a} state={a} after=1 group=2 manifest=0");
    assert!(Request::parse(&good).is_ok());
    for bad in [
        String::new(),
        "cut".into(),
        "CUTS".into(),
        "cuts ".into(),
        " cuts".into(),
        "cuts name=t".into(),
        "progress".into(),
        "progress name=t name=u".into(),
        "progress  name=t".into(),
        "progress name".into(),
        "withdraw name=t after=1".into(),
        good.replace(" group=2", ""),
        good.replace("after=1", "after=+1"),
        good.replace("after=1", "after=-1"),
        good.replace("after=1", "after=18446744073709551616"),
        good.replace("after=1", "after="),
        good.replace(&format!("state={a}"), &format!("state={}", &a[2..])),
        good.replace(&format!("state={a}"), &format!("state={}g", &a[1..])),
        good.replace(&format!("state={a}"), "state=none"),
        format!("{good} after=1"),
        format!("{good} expiry-s=x"),
        format!("{good} commit=00"),
    ] {
        assert!(Request::parse(&bad).is_err(), "`{bad}` parsed");
    }
    for bad in [
        "cuts",
        "accepted now",
        "done",
        "done commit=00",
        "refused nope why",
        &format!("cuts attachment={a} state=none count=1 group=3 opens=none"),
        &format!("cuts attachment={a} state=none count=1 group=none opens=itself"),
    ] {
        assert!(Reply::parse(bad).is_err(), "`{bad}` parsed");
    }
}

/// Mutant: a refusal's text written as it came (a `\n` in it would end
/// the answer early; one over the limit would not be read).
#[test]
fn a_refusals_text_stays_on_one_line_within_the_limit() {
    let line = Reply::Refused(Code::Taken, "one\ntwo\rthree".into()).line();
    assert_eq!(line, "refused taken one two three");
    let line = Reply::Refused(Code::Taken, "é".repeat(LINE_MAX)).line();
    assert!(line.len() < LINE_MAX, "{}", line.len());
    assert!(line.starts_with("refused taken é"));
    let back = read_line(&mut Cursor::new(format!("{line}\n"))).map(|l| Reply::parse(&l));
    assert!(matches!(back, Ok(Ok(Reply::Refused(Code::Taken, _)))));
}

/// Mutant: a publish's refusal given another code.
#[test]
fn a_publishs_refusal_keeps_its_own_code() {
    let cases = [
        (Refusal::Name, Code::Name),
        (Refusal::Attachment, Code::Attachment),
        (Refusal::State, Code::State),
        (Refusal::Manifest(9), Code::Manifest),
        (Refusal::Expiry(Duration::ZERO), Code::Expiry),
        (Refusal::Asked, Code::Asked),
        (Refusal::Busy, Code::Busy),
        (Refusal::Taken, Code::Taken),
        (Refusal::Stopped, Code::Stopped),
    ];
    for (refusal, code) in cases {
        assert_eq!(Code::of(&refusal), code);
        assert_eq!(
            Reply::of(&Progress::Refused(refusal.clone())),
            Reply::Refused(code, refusal.to_string())
        );
    }
    assert_eq!(Reply::of(&Progress::Pending), Reply::Pending);
    assert_eq!(Reply::of(&Progress::Done([3; 32])), Reply::Done([3; 32]));
    assert_eq!(Reply::of(&Progress::Expired), Reply::Expired);
    assert_eq!(Reply::of(&Progress::Withdrawn), Reply::Withdrawn);
    assert_eq!(Reply::of(&Progress::Unknown), Reply::Unknown);
}

/// One end of a connection, in memory: what the peer sent, and what is
/// written back.
struct Duplex {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What a handler was asked, and how often.
#[derive(Default)]
struct Asked {
    count: AtomicU64,
    manifest: std::sync::Mutex<Vec<u8>>,
}

fn recording(asked: &Arc<Asked>, reply: Reply) -> Handler {
    let asked = Arc::clone(asked);
    Box::new(move |_: &Request, manifest: Vec<u8>| {
        asked.count.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut kept) = asked.manifest.lock() {
            *kept = manifest;
        }
        reply.clone()
    })
}

/// Answer `sent` as one connection, with a handler that answers
/// `accepted`: the answer, what was written back, and how often the
/// handler was asked.
fn answered(sent: &[u8]) -> (io::Result<Reply>, String, Arc<Asked>) {
    let asked = Arc::new(Asked::default());
    let mut handler = recording(&asked, Reply::Accepted);
    let mut stream = Duplex {
        input: Cursor::new(sent.to_vec()),
        output: Vec::new(),
    };
    let reply = answer(&mut stream, &mut handler);
    let written = String::from_utf8_lossy(&stream.output).into_owned();
    (reply, written, asked)
}

/// Mutant: the manifest not read, read short, or the answer written
/// without the greeting.
#[test]
fn a_publish_hands_its_manifest_to_the_handler() {
    let sent = format!("{HELLO}\n{}\nhello", publish(None).line());
    let (reply, written, asked) = answered(sent.as_bytes());
    assert_eq!(reply.ok(), Some(Reply::Accepted));
    assert_eq!(written, format!("{HELLO}\naccepted\n"));
    assert_eq!(asked.count.load(Ordering::SeqCst), 1);
    assert_eq!(
        asked.manifest.lock().map(|m| m.clone()).ok(),
        Some(b"hello".to_vec())
    );
}

/// Mutant: a request taken without the greeting, or one cut short
/// taken, or a name the uploader would refuse handed on (a minor point
/// of the review of #127: the request log writes names in columns
/// separated by tabs).
#[test]
fn a_request_that_cannot_be_taken_is_refused_without_asking() {
    let long = format!("progress name={}", "n".repeat(LINE_MAX));
    let over = Request::Publish(Publish {
        manifest: MANIFEST_MAX + 1,
        ..match publish(None) {
            Request::Publish(p) => p,
            _ => unreachable!(),
        }
    });
    let expiry = |e| publish(Some(e)).line().replace("manifest=5", "manifest=0");
    for (sent, code) in [
        (format!("{HELLO}x\ncuts\n"), Code::Version),
        ("skyzfs-control 2\ncuts\n".to_string(), Code::Version),
        (format!("cuts\n{HELLO}\n"), Code::Version),
        (format!("{}\ncuts\n", "h".repeat(LINE_MAX)), Code::Version),
        (format!("{HELLO}\n{long}\n"), Code::Malformed),
        (format!("{HELLO}\ncuts please\n"), Code::Malformed),
        (format!("{HELLO}\n\u{7f}\u{0}\n"), Code::Malformed),
        (format!("{HELLO}\n{}\n", over.line()), Code::Manifest),
        (format!("{HELLO}\nprogress name=a\tb\n"), Code::Name),
        (format!("{HELLO}\nwithdraw name=a/b\n"), Code::Name),
        (
            format!(
                "{HELLO}\n{}\n",
                publish(None).line().replace("pool-7.b_c", "a\tb")
            ),
            Code::Name,
        ),
        (
            format!("{HELLO}\n{}\n", expiry(Duration::ZERO)),
            Code::Expiry,
        ),
        (
            format!("{HELLO}\n{}\n", expiry(EXPIRY_MAX + Duration::from_secs(1))),
            Code::Expiry,
        ),
    ] {
        let (reply, written, asked) = answered(sent.as_bytes());
        let Ok(Reply::Refused(got, _)) = reply else {
            panic!("{sent:?}: {reply:?}");
        };
        assert_eq!(got, code, "{sent:?}");
        assert!(written.starts_with(&format!("{HELLO}\nrefused {} ", code.word())));
        assert_eq!(asked.count.load(Ordering::SeqCst), 0, "{sent:?}");
    }
    let bytes = [HELLO.as_bytes(), b"\n\xff\xfe\n"].concat();
    let (reply, _, asked) = answered(&bytes);
    assert!(matches!(reply, Ok(Reply::Refused(Code::Malformed, _))));
    assert_eq!(asked.count.load(Ordering::SeqCst), 0);
    // An expiry of exactly the most is taken.
    let sent = format!("{HELLO}\n{}\n", expiry(EXPIRY_MAX));
    assert_eq!(answered(sent.as_bytes()).0.ok(), Some(Reply::Accepted));
}

/// Mutant: a manifest cut short handed on as it came, or a connection
/// that ended answered.
#[test]
fn a_connection_that_ends_early_is_not_a_request() {
    for sent in [
        String::new(),
        format!("{HELLO}\n"),
        format!("{HELLO}\ncuts"),
        format!("{HELLO}\n{}\nhel", publish(None).line()),
    ] {
        let (reply, written, asked) = answered(sent.as_bytes());
        assert!(reply.is_err(), "{sent:?}: {reply:?}");
        assert_eq!(written, "", "{sent:?}");
        assert_eq!(asked.count.load(Ordering::SeqCst), 0, "{sent:?}");
    }
}

#[cfg(target_os = "linux")]
mod socket {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a directory")
    }

    /// A daemon whose socket's backlog is full, and that takes no more
    /// connections: an ask gives up within its limit, and is not held by
    /// the connect (Codex on #129). Mutant: a blocking connect, which
    /// waits for room before any timeout applies.
    #[test]
    fn an_ask_of_a_socket_whose_backlog_is_full_ends_within_its_limit() {
        let dir = dir();
        let path = dir.path().join("full.sock");
        let listener = small_backlog(&path);
        let held = fill_backlog(&path);
        let limit = Duration::from_millis(300);
        let (done, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let began = std::time::Instant::now();
            let answer = ask(&path, &Request::Cuts, &[], limit);
            let _ = done.send((answer, began.elapsed()));
        });
        let (answer, took) = ended
            .recv_timeout(Duration::from_secs(10))
            .expect("the ask ends, and is not held by the connect");
        let e = answer.expect_err("no connection was taken");
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        assert!(e.to_string().contains("backlog is full"), "{e}");
        assert!(
            took >= limit && took < limit + Duration::from_secs(2),
            "{took:?}"
        );
        drop(held);
        drop(listener);
    }

    /// While the backlog is full the connect tries again after a pause
    /// that doubles from 1 ms to 50 ms, not in a busy loop: about a dozen
    /// tries in 300 ms (G2 of the review of #131). Mutants: no pause (CB,
    /// thousands of tries), a pause that does not grow (CD, about 300),
    /// no try again (one).
    #[test]
    fn a_full_backlog_is_tried_again_after_pauses_that_grow() {
        let dir = dir();
        let path = dir.path().join("full.sock");
        let listener = small_backlog(&path);
        let held = fill_backlog(&path);
        let mut tries = 0;
        let began = std::time::Instant::now();
        let e = connect_counted(&path, Duration::from_millis(300), &mut tries)
            .expect_err("no connection was taken");
        let took = began.elapsed();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        assert!(took >= Duration::from_millis(300), "{took:?}");
        // 1, 2, 4, 8, 16 and 32 ms, then 50 ms: 13 tries at most in
        // 300 ms, fewer where a pause runs long.
        assert!((2..=13).contains(&tries), "{tries} tries in {took:?}");
        drop(held);
        drop(listener);
    }

    /// No daemon at the socket (a socket left by one that has gone, or a
    /// file that is no socket), or no socket at all: the ask fails at
    /// once, after one try, and says which. Only a full backlog is tried
    /// again (G2 of the review of #131). Mutant: every error of the
    /// connect tried again until the limit (CA), which turns "no daemon"
    /// into a wait of the whole limit and the message of a full backlog.
    #[test]
    fn no_daemon_at_the_socket_or_no_socket_fails_the_ask_at_once() {
        let dir = dir();
        let stale = dir.path().join("stale.sock");
        drop(UnixListener::bind(&stale).expect("bound"));
        let plain = dir.path().join("plain.sock");
        std::fs::write(&plain, b"").expect("written");
        let missing = dir.path().join("missing.sock");
        let limit = Duration::from_secs(5);
        for (path, kind) in [
            (&stale, io::ErrorKind::ConnectionRefused),
            (&plain, io::ErrorKind::ConnectionRefused),
            (&missing, io::ErrorKind::NotFound),
        ] {
            let mut tries = 0;
            let began = std::time::Instant::now();
            let e = connect_counted(path, limit, &mut tries).expect_err("no daemon");
            assert_eq!(e.kind(), kind, "{}: {e}", path.display());
            assert_eq!(tries, 1, "{}", path.display());
            let e = ask(path, &Request::Cuts, &[], limit).expect_err("no daemon");
            assert_eq!(e.kind(), kind, "{}: {e}", path.display());
            assert!(!e.to_string().contains("backlog"), "{e}");
            assert!(
                began.elapsed() < Duration::from_secs(2),
                "{}: {:?}",
                path.display(),
                began.elapsed()
            );
        }
    }

    /// The errors of `accept` after which the channel's thread takes the
    /// next connection at once: a peer that gave up, a signal, nothing
    /// waiting, and a protocol error of the peer's. After the socket's own
    /// errors, such as no descriptor left, it pauses first, and does not
    /// spin while they last. Mutant: every error retried at once (N5 of
    /// the review of #127), or none.
    #[test]
    fn accept_is_tried_again_at_once_only_after_a_peer_or_a_signal() {
        use rustix::io::Errno;
        let error = |errno: Errno| io::Error::from_raw_os_error(errno.raw_os_error());
        for errno in [Errno::CONNABORTED, Errno::INTR, Errno::AGAIN, Errno::PROTO] {
            assert!(server::gave_up(&error(errno)), "{errno:?}: at once");
        }
        for errno in [
            Errno::MFILE,
            Errno::NFILE,
            Errno::NOBUFS,
            Errno::NOMEM,
            Errno::BADF,
            Errno::INVAL,
        ] {
            assert!(!server::gave_up(&error(errno)), "{errno:?}: after a pause");
        }
    }

    /// Mutant: the socket left with the umask's mode, or answers not
    /// counted.
    #[test]
    fn the_channel_answers_through_its_socket() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        let meta = std::fs::symlink_metadata(&path).expect("the socket");
        assert!(meta.file_type().is_socket());
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let reply = ask(&path, &Request::Progress("t".into()), &[], IO_LIMIT).expect("asked");
        assert_eq!(reply, Reply::Pending);
        let reply = ask(&path, &publish(None), b"12345", IO_LIMIT).expect("asked");
        assert_eq!(reply, Reply::Pending);
        assert_eq!(
            asked.manifest.lock().map(|m| m.clone()).ok(),
            Some(b"12345".to_vec())
        );
        let mut raw = UnixStream::connect(&path).expect("connect");
        raw.write_all(b"hello\n").expect("write");
        let mut back = String::new();
        raw.read_to_string(&mut back).expect("read");
        assert!(
            back.starts_with(&format!("{HELLO}\nrefused version ")),
            "{back}"
        );
        assert_eq!(
            channel.figures(),
            ChannelFigures {
                answered: 3,
                peer: 0,
                malformed: 1
            }
        );
        assert_eq!(asked.count.load(Ordering::SeqCst), 2);
        assert!(channel.close());
        assert!(!path.exists());
    }

    /// Mutant: the peer check skipped, or a refused peer's request read
    /// and handed on.
    #[test]
    fn a_peer_of_another_user_is_refused_and_its_request_not_asked() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open_checked(
            &path,
            recording(&asked, Reply::Accepted),
            |_| false,
            quiet(),
        )
        .expect("open");
        let reply = ask(&path, &publish(None), b"12345", IO_LIMIT).expect("asked");
        assert!(matches!(reply, Reply::Refused(Code::Peer, _)), "{reply:?}");
        assert_eq!(asked.count.load(Ordering::SeqCst), 0);
        assert_eq!(channel.figures().peer, 1);
        assert_eq!(channel.figures().answered, 0);
        assert!(channel.close());
    }

    /// A post that goes nowhere.
    fn quiet() -> super::super::server::Post {
        Box::new(|_| {})
    }

    /// The channel posts its figures after each connection it counts,
    /// for the status (M2.5-2): an answer, a malformed request, and a
    /// peer refused. Mutant: a count not posted.
    #[test]
    fn the_channel_posts_its_figures_after_each_connection() {
        use std::io::{Read as _, Write as _};
        let wait = Duration::from_secs(10);
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let (posted, posts) = std::sync::mpsc::channel();
        let channel = Channel::open_checked(
            &path,
            recording(&asked, Reply::Accepted),
            super::super::server::same_user_or_root,
            Box::new(move |figures| {
                let _ = posted.send(figures);
            }),
        )
        .expect("open");
        ask(&path, &Request::Cuts, &[], IO_LIMIT).expect("answered");
        let first = posts.recv_timeout(wait).expect("a post");
        assert_eq!(
            first,
            ChannelFigures {
                answered: 1,
                peer: 0,
                malformed: 0
            }
        );
        let mut stream = UnixStream::connect(&path).expect("connect");
        stream
            .write_all(format!("{HELLO}\nnonsense\n").as_bytes())
            .expect("write");
        let mut answer = String::new();
        stream.read_to_string(&mut answer).expect("read");
        assert!(answer.contains("refused malformed"), "{answer:?}");
        let second = posts.recv_timeout(wait).expect("a post");
        assert_eq!(
            second,
            ChannelFigures {
                answered: 2,
                peer: 0,
                malformed: 1
            }
        );
        assert_eq!(channel.figures(), second);
        assert!(channel.close());

        let (posted, posts) = std::sync::mpsc::channel();
        let refusing = Channel::open_checked(
            &path,
            recording(&asked, Reply::Accepted),
            |_| false,
            Box::new(move |figures| {
                let _ = posted.send(figures);
            }),
        )
        .expect("open");
        ask(&path, &Request::Cuts, &[], IO_LIMIT).expect("answered");
        assert_eq!(
            posts.recv_timeout(wait).expect("a post"),
            ChannelFigures {
                answered: 0,
                peer: 1,
                malformed: 0
            }
        );
        assert!(refusing.close());
    }

    /// The check itself: this test's own user and root pass it, and
    /// another user does not (T1 of the review of #127).
    ///
    /// Mutant: the check that lets every user ask.
    #[test]
    fn the_daemons_own_user_and_root_may_ask() {
        let own = rustix::process::geteuid().as_raw();
        assert!(super::super::server::same_user_or_root(0));
        assert!(super::super::server::same_user_or_root(own));
        let other = if own == 1 {
            2
        } else {
            own.wrapping_add(1).max(1)
        };
        assert!(!super::super::server::same_user_or_root(other), "{other}");
    }

    /// Mutant: a stale socket not removed (the bind fails), or anything
    /// at the path removed.
    #[test]
    fn a_stale_socket_is_replaced_and_anything_else_left() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        drop(UnixListener::bind(&path).expect("a socket left behind"));
        assert!(UnixStream::connect(&path).is_err());
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Unknown)).expect("open");
        let reply = ask(&path, &Request::Cuts, &[], IO_LIMIT).expect("asked");
        assert_eq!(reply, Reply::Unknown);
        assert!(channel.close());

        std::fs::write(&path, b"not a socket").expect("a file");
        let opened = Channel::open(&path, recording(&asked, Reply::Unknown));
        assert!(opened.is_err());
        assert_eq!(std::fs::read(&path).ok(), Some(b"not a socket".to_vec()));
    }

    /// Mutant: the client gives up on its write's error (the daemon
    /// refused before reading the manifest, and closed).
    #[test]
    fn a_refusal_before_the_manifest_is_read_reaches_the_client() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Accepted)).expect("open");
        let over = Request::Publish(Publish {
            manifest: 4 * MANIFEST_MAX,
            ..match publish(None) {
                Request::Publish(p) => p,
                _ => unreachable!(),
            }
        });
        let manifest = vec![0u8; 4 * MANIFEST_MAX];
        let reply = ask(&path, &over, &manifest, IO_LIMIT).expect("asked");
        assert!(
            matches!(reply, Reply::Refused(Code::Manifest, _)),
            "{reply:?}"
        );
        assert_eq!(asked.count.load(Ordering::SeqCst), 0);
        assert!(channel.close());
    }

    /// Mutant: close() that does not wake the thread (it would wait the
    /// whole limit and say false).
    #[test]
    fn close_ends_the_thread_at_once() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let channel = Channel::open(&path, handler::no_upload("none".into())).expect("open");
        let began = std::time::Instant::now();
        assert!(channel.close());
        assert!(began.elapsed() < server::CLOSE_LIMIT);
        assert!(!path.exists());
        assert!(ask(&path, &Request::Cuts, &[], IO_LIMIT).is_err());
    }

    /// A listener at `path` with a backlog of one, answered by nobody
    /// until it is given to a channel.
    fn small_backlog(path: &std::path::Path) -> UnixListener {
        use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
        let socket = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .expect("a socket");
        let address = SocketAddrUnix::new(path).expect("an address");
        rustix::net::bind(&socket, &address).expect("bound");
        rustix::net::listen(&socket, 1).expect("listening");
        UnixListener::from(socket)
    }

    /// Connections to `path` that wait in its backlog until it is full:
    /// the next connect would wait (or, made not to, fails at once).
    fn fill_backlog(path: &std::path::Path) -> Vec<rustix::fd::OwnedFd> {
        use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
        let address = SocketAddrUnix::new(path).expect("an address");
        let mut waiting = Vec::new();
        for _ in 0..64 {
            let socket = rustix::net::socket_with(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
                None,
            )
            .expect("a socket");
            match rustix::net::connect(&socket, &address) {
                Ok(()) => waiting.push(socket),
                Err(rustix::io::Errno::AGAIN) => return waiting,
                Err(e) => panic!("a connect: {e}"),
            }
        }
        panic!("the backlog took 64 connections");
    }

    /// The wake does not wait where the backlog is full (F2 of the review
    /// of #127, Codex's P2 on `server.rs`).
    ///
    /// Mutant: the wake's connect made blocking (it waits until a
    /// connection is taken, which nobody does here).
    #[test]
    fn the_wake_does_not_wait_for_a_full_backlog() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let listener = small_backlog(&path);
        let waiting = fill_backlog(&path);
        assert!(!waiting.is_empty());
        let (woke, woken) = std::sync::mpsc::channel();
        let waker = path.clone();
        std::thread::spawn(move || {
            server::wake(&waker);
            let _ = woke.send(());
        });
        assert!(
            woken.recv_timeout(Duration::from_secs(2)).is_ok(),
            "the wake waits"
        );
        drop(listener);
    }

    /// A channel whose thread is held in a request, and whose backlog is
    /// full, closes within its limit and says the thread did not end
    /// (F2 of the review of #127).
    ///
    /// Mutant: the wake's connect made blocking (the close waits until
    /// the request ends, 60 s here).
    #[test]
    fn close_does_not_wait_on_a_full_backlog() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let (entered, inside) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let handler: Handler = Box::new(move |_: &Request, _: Vec<u8>| {
            let _ = entered.send(());
            let _ = held.recv_timeout(Duration::from_secs(60));
            Reply::Pending
        });
        let channel = Channel::answering(
            small_backlog(&path),
            &path,
            handler,
            server::same_user_or_root,
            quiet(),
            REQUEST_LIMIT,
        )
        .expect("open");
        let asker = path.clone();
        let first =
            std::thread::spawn(move || ask(&asker, &Request::Cuts, &[], Duration::from_secs(70)));
        inside
            .recv_timeout(Duration::from_secs(10))
            .expect("the request is in the handler");
        let waiting = fill_backlog(&path);
        let (closed, close) = std::sync::mpsc::channel();
        let began = std::time::Instant::now();
        std::thread::spawn(move || {
            let _ = closed.send(channel.close());
        });
        let ended = close.recv_timeout(server::CLOSE_LIMIT + Duration::from_secs(3));
        let took = began.elapsed();
        drop(release);
        assert_eq!(ended, Ok(false), "the close after {took:?}");
        assert!(
            took < server::CLOSE_LIMIT + Duration::from_secs(2),
            "{took:?}"
        );
        drop(waiting);
        let _ = first.join();
    }

    /// A peer that sends its request a byte at a time, each well within
    /// the limit of one read, holds the thread no longer than the
    /// request's own limit: the next peer is answered (F3 of the review
    /// of #127).
    ///
    /// Mutant: each read given its own limit (the trickle holds the
    /// thread for as long as it goes on, 20 s here).
    #[test]
    fn a_request_that_trickles_holds_the_thread_no_longer_than_its_limit() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::answering(
            UnixListener::bind(&path).expect("bound"),
            &path,
            recording(&asked, Reply::Unknown),
            server::same_user_or_root,
            quiet(),
            Duration::from_millis(500),
        )
        .expect("open");
        let mut slow = UnixStream::connect(&path).expect("connect");
        let trickle = std::thread::spawn(move || {
            let mut sent = format!("{HELLO}\nprogress name=").into_bytes();
            sent.extend(std::iter::repeat_n(b'a', 400));
            for byte in sent {
                if slow.write_all(&[byte]).is_err() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        });
        std::thread::sleep(Duration::from_millis(100));
        let began = std::time::Instant::now();
        let reply = ask(&path, &Request::Cuts, &[], Duration::from_secs(5));
        let took = began.elapsed();
        assert_eq!(reply.ok(), Some(Reply::Unknown), "after {took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
        assert!(
            trickle.join().expect("the trickle"),
            "the trickling peer was cut off"
        );
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }

    /// A peer that sends part of its request and then nothing holds the
    /// thread no longer than the request's limit either: the read that
    /// waits for the rest is given the time left, not a read's whole
    /// limit (F3 of the review of #127).
    ///
    /// Mutant: each read given `IO_LIMIT` (the stalled peer holds the
    /// thread for 10 s).
    #[test]
    fn a_request_that_stops_part_way_holds_the_thread_no_longer_than_its_limit() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::answering(
            UnixListener::bind(&path).expect("bound"),
            &path,
            recording(&asked, Reply::Unknown),
            server::same_user_or_root,
            quiet(),
            Duration::from_millis(500),
        )
        .expect("open");
        let mut stalled = UnixStream::connect(&path).expect("connect");
        stalled
            .write_all(format!("{HELLO}\nprogress name=").as_bytes())
            .expect("the first part");
        std::thread::sleep(Duration::from_millis(100));
        let began = std::time::Instant::now();
        let reply = ask(&path, &Request::Cuts, &[], Duration::from_secs(15));
        let took = began.elapsed();
        assert_eq!(reply.ok(), Some(Reply::Unknown), "after {took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
        stalled
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a timeout");
        let mut rest = Vec::new();
        assert!(
            stalled.read_to_end(&mut rest).is_ok(),
            "the stalled peer's connection was closed"
        );
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }
}

mod uploader {
    use std::sync::Weak;

    use skyzfs_blk::Buf;
    use skyzfs_local::inplace::{Cut, CutSlice};
    use skyzfs_upload::publish::Request as Ask;
    use skyzfs_upload::queued::Desk;
    use skyzfs_upload::{Answer, Control, CutsNow, Released, Store, Uploadable};
    use tokio::sync::mpsc;

    use super::*;

    const LIMIT: Duration = Duration::from_secs(10);

    /// The count of cuts the store of [`answered_desk`] holds.
    const NOW: CutsNow = CutsNow {
        state: Some(S),
        count: 4,
        newest: Some(Cut {
            number: 4,
            group: 9,
            itself: false,
        }),
    };

    /// A store that holds no cut to upload, and counts [`NOW`].
    struct Counting;

    impl Store for Counting {
        fn uploadable(&mut self) -> Option<Uploadable> {
            None
        }
        fn read(&mut self, _: u64, _: u64, _: &mut Buf) -> io::Result<CutSlice> {
            Err(io::Error::other("no cut"))
        }
        fn begin_sync_copies(&mut self, _: u64) -> io::Result<()> {
            Err(io::Error::other("no cut"))
        }
        fn copies_synced(&mut self, _: bool) -> Option<io::Result<()>> {
            None
        }
        fn release(&mut self, _: u64) -> Released {
            Released::Gone
        }
        fn name_committed(&mut self, _: u64, _: u64) -> bool {
            false
        }
        fn abandon_all(&mut self) -> Vec<u64> {
            Vec::new()
        }
        fn cuts(&mut self) -> CutsNow {
            NOW
        }
    }

    /// A desk whose queue's thread answers it from [`Counting`] at each
    /// wake, until the desk closes or is dropped.
    fn answered_desk() -> Arc<Desk> {
        let (wake, woken) = std::sync::mpsc::channel::<()>();
        let desk = Desk::new(move || {
            let _ = wake.send(());
        });
        let weak: Weak<Desk> = Arc::downgrade(&desk);
        std::thread::spawn(move || {
            while woken.recv().is_ok() {
                let Some(desk) = weak.upgrade() else {
                    return;
                };
                if desk.closed() {
                    return;
                }
                desk.answer(&mut Counting);
            }
        });
        desk
    }

    /// An uploader on a thread of its own that answers each control with
    /// `answer`, after keeping what it was asked; the count of cuts is
    /// the desk's.
    fn uploader_with(
        answer: impl Fn(Control, &mut Vec<String>) + Send + 'static,
    ) -> (Handler, std::thread::JoinHandle<Vec<String>>) {
        let (tx, mut rx) = mpsc::channel(handler::INBOX);
        let thread = std::thread::spawn(move || {
            let mut seen = Vec::new();
            while let Some(control) = rx.blocking_recv() {
                answer(control, &mut seen);
            }
            seen
        });
        let handler = handler::uploader(A, tx, answered_desk(), LIMIT).expect("a handler");
        (handler, thread)
    }

    /// Mutant: a field of a request or an answer lost on the way (the
    /// attachment answered is the daemon's, not the request's), or the
    /// count of cuts asked of the uploader.
    #[test]
    fn each_request_goes_to_the_uploader_and_its_answer_back() {
        let (mut handler, thread) = uploader_with(|control, seen| match control {
            Control::Ask(ask, reply) => {
                seen.push(format!("{ask:?}"));
                let _ = reply.send(match ask.name.as_str() {
                    "taken" => Answer::Refused(Refusal::Taken),
                    _ => Answer::Accepted,
                });
            }
            Control::Progress(name, reply) => {
                seen.push(format!("progress {name}"));
                let _ = reply.send(Progress::Done([2; 32]));
            }
            Control::Withdraw(name, reply) => {
                seen.push(format!("withdraw {name}"));
                let _ = reply.send(Progress::Withdrawn);
            }
        });
        assert_eq!(
            handler(&Request::Cuts, Vec::new()),
            Reply::Cuts(CutsReply {
                attachment: A,
                state: Some(S),
                count: 4,
                newest: Some((9, false)),
            })
        );
        assert_eq!(handler(&publish(None), b"12345".to_vec()), Reply::Accepted);
        let taken = Request::Publish(Publish {
            name: "taken".into(),
            expiry: Some(Duration::from_secs(5)),
            ..match publish(None) {
                Request::Publish(p) => p,
                _ => unreachable!(),
            }
        });
        assert_eq!(
            handler(&taken, b"12345".to_vec()),
            Reply::Refused(Code::Taken, Refusal::Taken.to_string())
        );
        assert_eq!(
            handler(&Request::Progress("t".into()), Vec::new()),
            Reply::Done([2; 32])
        );
        assert_eq!(
            handler(&Request::Withdraw("u".into()), Vec::new()),
            Reply::Withdrawn
        );
        drop(handler);
        let seen = thread.join().expect("the uploader");
        let ask = |name: &str, expiry| {
            format!(
                "{:?}",
                Ask {
                    name: name.into(),
                    attachment: A,
                    state: S,
                    after: 3,
                    group: 41,
                    manifest: b"12345".to_vec(),
                    expiry,
                }
            )
        };
        assert_eq!(
            seen,
            [
                ask("pool-7.b_c", EXPIRY_DEFAULT),
                ask("taken", Duration::from_secs(5)),
                "progress t".into(),
                "withdraw u".into(),
            ]
        );
    }

    /// Mutant: a wait with no limit (the test would not end), or the
    /// wrong code.
    #[test]
    fn an_uploader_that_does_not_answer_in_time_reads_no_answer() {
        let (tx, rx) = mpsc::channel(handler::INBOX);
        let mut handler = handler::uploader(A, tx, answered_desk(), Duration::from_millis(50))
            .expect("a handler");
        let reply = handler(&Request::Progress("t".into()), Vec::new());
        assert!(
            matches!(reply, Reply::Refused(Code::NoAnswer, _)),
            "{reply:?}"
        );
        drop(rx);
    }

    /// A store whose queue takes no turn leaves `cuts` unanswered: the
    /// channel waits its limit and says `no-answer`, naming the store.
    ///
    /// Mutant: the count waited for with no limit (the test would not
    /// end), or the wrong code.
    #[test]
    fn a_store_whose_queue_takes_no_turn_reads_no_answer_for_cuts() {
        let (tx, rx) = mpsc::channel(handler::INBOX);
        let desk = Desk::new(|| {});
        let mut handler =
            handler::uploader(A, tx, desk, Duration::from_millis(50)).expect("a handler");
        // On a thread of its own, so that a handler that waits for the
        // store with no limit fails the test and does not hang it.
        let (done, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(handler(&Request::Cuts, Vec::new()));
        });
        let reply = answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the handler answers within its limit, not when the store does");
        let Reply::Refused(Code::NoAnswer, why) = reply else {
            panic!("{reply:?}");
        };
        assert!(why.contains("the store did not answer"), "{why}");
        drop(rx);
    }

    /// A closed desk is a store no longer served: `cuts` reads `ended`,
    /// and not a count of nothing (F5 of the review of #127).
    ///
    /// Mutant: a closed desk answered with an empty count.
    #[test]
    fn a_closed_desk_reads_ended_for_cuts() {
        let (tx, rx) = mpsc::channel(handler::INBOX);
        let desk = answered_desk();
        desk.close();
        let mut handler = handler::uploader(A, tx, desk, LIMIT).expect("a handler");
        let reply = handler(&Request::Cuts, Vec::new());
        assert!(
            matches!(&reply, Reply::Refused(Code::Ended, why) if why.contains("store")),
            "the store's queue has ended, not the uploader: {reply:?}"
        );
        drop(rx);
    }

    /// Mutant: a closed inbox, or a request dropped unanswered, read as
    /// anything but `ended`.
    #[test]
    fn an_uploader_that_has_ended_reads_ended() {
        let (tx, rx) = mpsc::channel(handler::INBOX);
        let mut handler = handler::uploader(A, tx, answered_desk(), LIMIT).expect("a handler");
        drop(rx);
        let reply = handler(&Request::Progress("t".into()), Vec::new());
        assert!(matches!(reply, Reply::Refused(Code::Ended, _)), "{reply:?}");

        let (mut handler, thread) = uploader_with(|control, _| drop(control));
        let reply = handler(&Request::Withdraw("t".into()), Vec::new());
        assert!(matches!(reply, Reply::Refused(Code::Ended, _)), "{reply:?}");
        drop(handler);
        let _ = thread.join();
    }

    /// A full inbox is not waited on, and does not hold back the count of
    /// cuts, which the desk gives (F1 of the review of #127).
    ///
    /// Mutant: a full inbox waited on (the channel would hang), or the
    /// count asked through the inbox (`no-answer`).
    #[test]
    fn a_full_inbox_is_not_waited_on() {
        let (tx, rx) = mpsc::channel(handler::INBOX);
        let mut handler = handler::uploader(A, tx.clone(), answered_desk(), Duration::from_secs(5))
            .expect("a handler");
        for _ in 0..handler::INBOX {
            let (reply, _) = tokio::sync::oneshot::channel();
            tx.try_send(Control::Progress("t".into(), reply))
                .expect("room");
        }
        let began = std::time::Instant::now();
        let reply = handler(&Request::Progress("t".into()), Vec::new());
        assert!(
            matches!(reply, Reply::Refused(Code::NoAnswer, _)),
            "{reply:?}"
        );
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(
            handler(&Request::Cuts, Vec::new()),
            Reply::Cuts(CutsReply {
                attachment: A,
                state: Some(S),
                count: 4,
                newest: Some((9, false)),
            })
        );
        drop(rx);
    }

    #[test]
    fn without_an_uploader_every_request_is_refused_no_upload() {
        let mut handler = handler::no_upload("no `remote` file".into());
        for request in [
            Request::Cuts,
            publish(None),
            Request::Progress("t".into()),
            Request::Withdraw("t".into()),
        ] {
            assert_eq!(
                handler(&request, Vec::new()),
                Reply::Refused(Code::NoUpload, "no `remote` file".into())
            );
        }
    }
}
