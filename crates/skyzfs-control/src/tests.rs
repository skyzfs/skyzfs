use std::io::{self, Cursor};
use std::time::Duration;

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

fn snap(number: u64, group: u64, manifest_len: u32) -> Reply {
    Reply::Snap(SnapReply {
        commit: [0x0f; 32],
        state: S,
        number,
        group,
        manifest_len,
        manifest_hash: [0x40; 32],
    })
}

fn unreadable(why: Unread) -> Reply {
    Reply::Unreadable(UnreadableReply {
        commit: [0x0e; 32],
        why,
    })
}

const EVERY_UNREAD: [Unread; 4] = [
    Unread::NoRow,
    Unread::Version,
    Unread::Length,
    Unread::OtherCommit,
];

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
    assert_eq!(Request::Look("t".into()).line(), "look name=t");
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
    assert_eq!(
        snap(3, 41, 19).line(),
        format!(
            "snap commit={} state={hex_s} number=3 group=41 manifest-len=19 manifest-hash={}",
            "0f".repeat(32),
            "40".repeat(32)
        )
    );
    let whys: Vec<String> = EVERY_UNREAD
        .into_iter()
        .map(|why| unreadable(why).line())
        .collect();
    let commit = "0e".repeat(32);
    assert_eq!(
        whys,
        [
            format!("unreadable commit={commit} why=no-row"),
            format!("unreadable commit={commit} why=version"),
            format!("unreadable commit={commit} why=length"),
            format!("unreadable commit={commit} why=other-commit"),
        ]
    );
    for (reply, line) in [
        (Reply::Accepted, "accepted"),
        (Reply::Pending, "pending"),
        (Reply::Expired, "expired"),
        (Reply::Withdrawn, "withdrawn"),
        (Reply::Unknown, "unknown"),
        (Reply::Absent, "absent"),
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
            "rewound",
            "version",
            "malformed",
            "peer",
            "no-upload",
            "ended",
            "no-answer",
            "store"
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
        Request::Look("z".into()),
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
        Reply::Absent,
        snap(0, 0, 0),
        snap(u64::MAX, u64::MAX, u32::MAX),
        // A number and a group that differ, read each from its own word
        // (T1 of the review of #140): the agent compares both with its
        // request.
        snap(3, 41, 19),
    ];
    replies.extend(EVERY_UNREAD.map(unreadable));
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
        "look".into(),
        "look name=t group=1".into(),
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
    let snap_line = snap(3, 41, 19).line();
    let unreadable_line = unreadable(Unread::NoRow).line();
    for bad in [
        "cuts",
        "accepted now",
        "done",
        "done commit=00",
        "refused nope why",
        &format!("cuts attachment={a} state=none count=1 group=3 opens=none"),
        &format!("cuts attachment={a} state=none count=1 group=none opens=itself"),
        "absent now",
        &snap_line.replace(" number=3", ""),
        &snap_line.replace("state=", "attachment="),
        &snap_line.replace("number=3", "number=x"),
        &snap_line.replace("manifest-len=19", "manifest-len=4294967296"),
        &snap_line.replace("manifest-hash=40", "manifest-hash="),
        &format!("{snap_line} why=no-row"),
        &unreadable_line.replace("no-row", "nope"),
        &unreadable_line.replace(" why=no-row", ""),
        &unreadable_line.replace("commit=0e", "commit="),
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

#[cfg(target_os = "linux")]
mod socket {
    use std::os::unix::net::UnixListener;

    use super::*;
    use crate::client::connect_counted;

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
        // 1, 2, 4, 8, 16 and 32 ms, then 50 ms, the last cut to what is
        // left: 12 tries at most in 300 ms (try 12 comes at the limit),
        // fewer where a pause runs long, since a sleep never ends early.
        assert!((2..=12).contains(&tries), "{tries} tries in {took:?}");
        drop(held);
        drop(listener);
    }

    /// The pause between tries stops growing at 50 ms, so a connect is
    /// made soon after the backlog has room, however long it has waited
    /// (N3 of the review of #133): room is made 1.5 s into a wait of 5 s,
    /// and the connection follows within 200 ms. Mutant: a pause that
    /// keeps doubling (CE), which tries at 1023 ms and then at 2047 ms,
    /// half a second after the room. A pause capped at a few hundred ms
    /// can still try soon after the room by chance (a cap of 500 ms, CE5,
    /// tries at 1511 ms), so the tries are counted too: at 50 ms there
    /// are about 36, the last of which connects, at a cap of 200 ms 16, so at
    /// least 20 are asked for (T1 of the review of #136). Not caught here
    /// or above: a cap of 100 ms (22 tries), which costs a connect 100 ms
    /// at most, and a first pause of 50 ms (CF), which costs it 50 ms.
    #[test]
    fn a_connect_is_made_soon_after_the_backlog_has_room() {
        let dir = dir();
        let path = dir.path().join("room.sock");
        let listener = small_backlog(&path);
        let held = fill_backlog(&path);
        let room = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            let taken = listener.accept().expect("a connection waiting");
            (std::time::Instant::now(), taken, listener)
        });
        let mut tries = 0;
        let stream = connect_counted(&path, Duration::from_secs(5), &mut tries)
            .expect("connected once the backlog had room");
        let connected = std::time::Instant::now();
        let (made, _taken, _listener) = room.join().expect("the thread that made room");
        let late = connected.saturating_duration_since(made);
        assert!(
            late < Duration::from_millis(200),
            "connected {late:?} after the backlog had room, at try {tries}"
        );
        assert!(
            tries >= 20,
            "only {tries} tries, connected {late:?} after the backlog had room"
        );
        drop(stream);
        drop(held);
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
}
