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

fn retire() -> Request {
    Request::Retire(Retire {
        name: "pool-7.b_c".into(),
        commit: [0xc0; 32],
    })
}

const EVERY_UNREAD: [Unread; 4] = [
    Unread::NoRow,
    Unread::Version,
    Unread::Length,
    Unread::OtherCommit,
];

/// Mutant: any word of the protocol changed, the greeting's included
/// (it is version 3's: a change is a new version).
#[test]
fn the_lines_are_version_3s_words() {
    let hex_a = "a1".repeat(16);
    let hex_s = "5e".repeat(16);
    assert_eq!(HELLO, "skyzfs-control 3");
    assert_eq!(
        retire().line(),
        format!("retire name=pool-7.b_c commit={}", "c0".repeat(32))
    );
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
            rewound: 5,
        })
        .line()
    };
    assert_eq!(
        cuts(None, None),
        format!("cuts attachment={hex_a} state=none count=7 group=none opens=none rewound=5")
    );
    assert_eq!(
        cuts(Some(S), Some((12, true))),
        format!("cuts attachment={hex_a} state={hex_s} count=7 group=12 opens=itself rewound=5")
    );
    assert_eq!(
        cuts(Some(S), Some((12, false))),
        format!("cuts attachment={hex_a} state={hex_s} count=7 group=12 opens=rewind rewound=5")
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
        (Reply::Retired, "retired"),
        (Reply::Gone, "gone"),
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
            "store",
            "other"
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
        retire(),
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
        Reply::Retired,
        Reply::Gone,
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
                rewound: u64::MAX,
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

/// Each line of version 1, as its daemon and client wrote them, reads
/// the same in version 2: the greeting changed (a reader of version 1
/// does not read version 2's new words as "not said"), and no line of
/// version 1 did. Mutant: a word of version 1 changed.
#[test]
fn each_line_of_version_1_reads_the_same_in_version_2() {
    let hex_a = "a1".repeat(16);
    let hex_s = "5e".repeat(16);
    let requests = [
        ("cuts".to_string(), Request::Cuts),
        (
            format!(
                "publish name=pool-7.b_c attachment={hex_a} state={hex_s} after=3 group=41 \
                 manifest=5"
            ),
            publish(None),
        ),
        (
            format!(
                "publish expiry-s=90 manifest=5 group=41 after=3 state={hex_s} \
                 attachment={hex_a} name=pool-7.b_c"
            ),
            publish(Some(Duration::from_secs(90))),
        ),
        ("progress name=t".into(), Request::Progress("t".into())),
        ("withdraw name=t".into(), Request::Withdraw("t".into())),
    ];
    for (line, request) in requests {
        assert_eq!(Request::parse(&line), Ok(request), "{line}");
    }
    let cuts = |state, newest| {
        Reply::Cuts(CutsReply {
            attachment: A,
            state,
            count: 7,
            newest,
            rewound: 5,
        })
    };
    let mut replies = vec![
        (
            format!("cuts attachment={hex_a} state=none count=7 group=none opens=none rewound=5"),
            cuts(None, None),
        ),
        (
            format!(
                "cuts attachment={hex_a} state={hex_s} count=7 group=12 opens=itself rewound=5"
            ),
            cuts(Some(S), Some((12, true))),
        ),
        (
            format!(
                "cuts attachment={hex_a} state={hex_s} count=7 group=12 opens=rewind rewound=5"
            ),
            cuts(Some(S), Some((12, false))),
        ),
        ("accepted".into(), Reply::Accepted),
        ("pending".into(), Reply::Pending),
        (
            format!("done commit={}", "0f".repeat(32)),
            Reply::Done([0x0f; 32]),
        ),
        ("expired".into(), Reply::Expired),
        ("withdrawn".into(), Reply::Withdrawn),
        ("unknown".into(), Reply::Unknown),
    ];
    let version_1s = [
        (Code::Name, "name"),
        (Code::Attachment, "attachment"),
        (Code::State, "state"),
        (Code::Manifest, "manifest"),
        (Code::Expiry, "expiry"),
        (Code::Asked, "asked"),
        (Code::Busy, "busy"),
        (Code::Taken, "taken"),
        (Code::Stopped, "stopped"),
        (Code::Version, "version"),
        (Code::Malformed, "malformed"),
        (Code::Peer, "peer"),
        (Code::NoUpload, "no-upload"),
        (Code::Ended, "ended"),
        (Code::NoAnswer, "no-answer"),
    ];
    for (code, word) in version_1s {
        replies.push((
            format!("refused {word} it says why"),
            Reply::Refused(code, "it says why".into()),
        ));
    }
    for (line, reply) in replies {
        assert_eq!(Reply::parse(&line), Ok(reply), "{line}");
    }
}

/// Each line that version 2 added reads the same in version 3 (those
/// of version 1 are the test above's): the greeting changed for
/// `retire` and its words, and no line of version 2 did. Mutant: a word
/// of version 2 changed.
#[test]
fn each_line_of_version_2_reads_the_same_in_version_3() {
    let hex_s = "5e".repeat(16);
    let commit = "0f".repeat(32);
    assert_eq!(Request::parse("look name=t"), Ok(Request::Look("t".into())));
    let replies = [
        ("absent".to_string(), Reply::Absent),
        (
            format!(
                "snap commit={commit} state={hex_s} number=3 group=41 manifest-len=19 \
                 manifest-hash={}",
                "40".repeat(32)
            ),
            snap(3, 41, 19),
        ),
        (
            format!("unreadable commit={} why=other-commit", "0e".repeat(32)),
            unreadable(Unread::OtherCommit),
        ),
        (
            "refused rewound it says why".into(),
            Reply::Refused(Code::Rewound, "it says why".into()),
        ),
        (
            "refused store it says why".into(),
            Reply::Refused(Code::Store, "it says why".into()),
        ),
    ];
    for (line, reply) in replies {
        assert_eq!(Reply::parse(&line), Ok(reply), "{line}");
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
        "retire".into(),
        "retire name=t".into(),
        format!("retire commit={}", "c0".repeat(32)),
        format!("retire name=t commit={}", "c0".repeat(31)),
        format!("retire name=t commit={}g", "c0".repeat(31)),
        format!("retire name=t commit={} after=1", "c0".repeat(32)),
        format!("retire name=t name=u commit={}", "c0".repeat(32)),
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
        &format!("cuts attachment={a} state=none count=1 group=3 opens=none rewound=0"),
        // The `cuts` answer of version 2, which has no rewind mark.
        &format!("cuts attachment={a} state=none count=1 group=none opens=none"),
        &format!("cuts attachment={a} state=none count=1 group=none opens=none rewound=x"),
        &format!("cuts attachment={a} state=none count=1 group=none opens=itself rewound=0"),
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

    /// A daemon of an earlier version greets with its own greeting, and
    /// its answer is not read, even one that would parse: the client
    /// gives an error that names both greetings and the deploy order
    /// (version 3, M2.7b-1). Mutant: the greeting not compared.
    #[test]
    fn a_daemon_of_an_earlier_version_is_not_read() {
        use std::io::{BufRead, Write};
        for version in [1, 2] {
            let dir = dir();
            let path = dir.path().join("old.sock");
            let listener = UnixListener::bind(&path).expect("bound");
            let daemon = std::thread::spawn(move || {
                let (stream, _) = listener.accept().expect("accepted");
                let mut reader = io::BufReader::new(stream.try_clone().expect("cloned"));
                let mut greeting = String::new();
                reader.read_line(&mut greeting).expect("read");
                let mut stream = stream;
                let answer = format!(
                    "skyzfs-control {version}\nrefused version this daemon speaks \
                     `skyzfs-control {version}`\n"
                );
                stream.write_all(answer.as_bytes()).expect("written");
                greeting
            });
            let asked = client::ask(&path, &Request::Cuts, &[], Duration::from_secs(10));
            assert_eq!(daemon.join().ok(), Some(format!("{HELLO}\n")));
            let Err(e) = asked else {
                panic!("an answer of version {version} was read: {asked:?}");
            };
            assert_eq!(e.kind(), io::ErrorKind::InvalidData);
            let why = e.to_string();
            assert!(
                why.contains(&format!("`skyzfs-control {version}`"))
                    && why.contains(HELLO)
                    && why.contains("deploy them together"),
                "{why}"
            );
        }
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
        let e =
            connect_counted(&path, in_ms(300), &mut tries).expect_err("no connection was taken");
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
        let stream = connect_counted(&path, in_ms(5000), &mut tries)
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

    /// The instant `ms` milliseconds from now.
    fn in_ms(ms: u64) -> std::time::Instant {
        std::time::Instant::now() + Duration::from_millis(ms)
    }

    /// No daemon at the socket (a socket left by one that has gone, or a
    /// file that is no socket), or no socket at all: the ask fails at
    /// once, after one try at most, and says which. Only a full backlog
    /// is tried again (G2 of the review of #131). A file that is no
    /// socket, and a path with nothing at it, are refused before any try
    /// (L5 of #147). Mutant: every error of the connect tried again until
    /// the limit (CA), which turns "no daemon" into a wait of the whole
    /// limit and the message of a full backlog.
    #[test]
    fn no_daemon_at_the_socket_or_no_socket_fails_the_ask_at_once() {
        let dir = dir();
        let stale = dir.path().join("stale.sock");
        drop(UnixListener::bind(&stale).expect("bound"));
        let plain = dir.path().join("plain.sock");
        std::fs::write(&plain, b"").expect("written");
        let missing = dir.path().join("missing.sock");
        let limit = Duration::from_secs(5);
        for (path, kind, tried) in [
            (&stale, io::ErrorKind::ConnectionRefused, 1),
            (&plain, io::ErrorKind::PermissionDenied, 0),
            (&missing, io::ErrorKind::NotFound, 0),
        ] {
            let mut tries = 0;
            let began = std::time::Instant::now();
            let e = connect_counted(path, in_ms(5000), &mut tries).expect_err("no daemon");
            assert_eq!(e.kind(), kind, "{}: {e}", path.display());
            assert_eq!(tries, tried, "{}", path.display());
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

    /// A daemon that answers a byte at a time, each within the limit:
    /// the ask still ends at its limit, which is one deadline for the
    /// whole request and not one for each read (S4 of the review of
    /// #131). Mutant: a timeout of each read, which waits 17 pauses here
    /// and reads the greeting.
    #[test]
    fn an_ask_ends_at_its_limit_however_the_daemon_spreads_its_answer() {
        use std::io::{BufRead, Write};
        let dir = dir();
        let path = dir.path().join("slow.sock");
        let listener = UnixListener::bind(&path).expect("bound");
        let daemon = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accepted");
            let mut reader = io::BufReader::new(stream.try_clone().expect("cloned"));
            let mut greeting = String::new();
            reader.read_line(&mut greeting).expect("read");
            let mut stream = stream;
            for byte in format!("{HELLO}\n").bytes() {
                std::thread::sleep(Duration::from_millis(100));
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
            }
        });
        let limit = Duration::from_millis(300);
        let began = std::time::Instant::now();
        let e = ask(&path, &Request::Cuts, &[], limit).expect_err("no answer within the limit");
        let took = began.elapsed();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        assert!(
            took >= limit && took < limit + Duration::from_millis(250),
            "{took:?}"
        );
        drop(daemon.join());
    }

    /// A daemon that sends part of its greeting and then nothing: the
    /// read waits out what is left of the deadline, and its timeout
    /// (`WouldBlock` from the socket) is given as `TimedOut` (R14 of the
    /// review of #147). Mutant: the socket's error given as it is.
    #[test]
    fn a_read_cut_by_the_deadline_is_timed_out() {
        use std::io::{BufRead, Write};
        let dir = dir();
        let path = dir.path().join("part.sock");
        let listener = UnixListener::bind(&path).expect("bound");
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let daemon = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accepted");
            let mut reader = io::BufReader::new(stream.try_clone().expect("cloned"));
            let mut greeting = String::new();
            reader.read_line(&mut greeting).expect("read");
            let mut stream = stream;
            stream.write_all(b"skyzfs").expect("written");
            let _ = stopped.recv_timeout(Duration::from_secs(10));
        });
        let limit = Duration::from_millis(300);
        let began = std::time::Instant::now();
        let e = ask(&path, &Request::Cuts, &[], limit).expect_err("no whole greeting");
        let took = began.elapsed();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        assert!(took < limit + Duration::from_millis(250), "{took:?}");
        drop(stop);
        drop(daemon.join());
    }

    /// The wait for room in a full backlog counts in the ask's one
    /// deadline (R3 of the review of #147): room is made 200 ms into a
    /// limit of 300 ms and the answer comes 200 ms later, past the limit,
    /// so the ask ends near 300 ms with no answer. Mutant: the deadline
    /// started after the connect, which reads the answer at about 400 ms.
    #[test]
    fn the_wait_for_a_connection_counts_in_the_deadline() {
        use std::io::{BufRead, Write};
        let dir = dir();
        let path = dir.path().join("late.sock");
        let listener = small_backlog(&path);
        let held = fill_backlog(&path);
        let daemon = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            // Room for the ask: the connections that filled the backlog
            // are taken, and the ask's is the next.
            let mut taken = Vec::new();
            for _ in 0..=held.len() {
                taken.push(listener.accept().expect("accepted").0);
            }
            let stream = taken.pop().expect("the ask's connection");
            let mut reader = io::BufReader::new(stream.try_clone().expect("cloned"));
            let mut greeting = String::new();
            let _ = reader.read_line(&mut greeting);
            std::thread::sleep(Duration::from_millis(200));
            let mut stream = stream;
            let _ = stream.write_all(format!("{HELLO}\nnone\n").as_bytes());
            drop(held);
        });
        let limit = Duration::from_millis(300);
        let began = std::time::Instant::now();
        let answer = ask(&path, &Request::Cuts, &[], limit);
        let took = began.elapsed();
        let e = answer.expect_err("the answer came past the deadline");
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        assert!(took < limit + Duration::from_millis(80), "{took:?}");
        drop(daemon.join());
    }

    /// A limit too long to add to the clock is an error, not a panic (F6
    /// of the review of #147).
    #[test]
    fn a_limit_too_long_is_an_error() {
        let dir = dir();
        let path = dir.path().join("none.sock");
        let e = ask(&path, &Request::Cuts, &[], Duration::MAX).expect_err("too long");
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{e}");
    }

    /// A symbolic link at the socket's path is refused, even to a socket
    /// the daemon would answer on, and nothing is connected (L5 of #147,
    /// F8 of its review). Mutant: the path's metadata read through the
    /// link.
    #[test]
    fn a_link_at_the_sockets_path_is_refused() {
        let dir = dir();
        let real = dir.path().join("real.sock");
        let listener = UnixListener::bind(&real).expect("bound");
        listener.set_nonblocking(true).expect("non-blocking");
        let link = dir.path().join("link.sock");
        std::os::unix::fs::symlink(&real, &link).expect("linked");
        let mut tries = 0;
        let e = connect_counted(&link, in_ms(1000), &mut tries).expect_err("a link");
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
        assert!(e.to_string().contains("symbolic link"), "{e}");
        assert_eq!(tries, 0);
        let e = ask(&link, &Request::Cuts, &[], Duration::from_secs(1)).expect_err("a link");
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
        assert!(
            matches!(listener.accept(), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "the link's socket was connected"
        );
        // The socket itself is asked.
        assert!(connect_counted(&real, in_ms(1000), &mut 0).is_ok());
    }

    /// A socket owned by another user, and a daemon of another user on a
    /// socket owned by root, are refused (L5 of #147). Run as root only:
    /// it needs another user. Mutants: the owner not read, the peer not
    /// read.
    #[test]
    fn a_socket_or_a_daemon_of_another_user_is_refused() {
        use std::io::BufRead as _;
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::process::CommandExt as _;
        if rustix::process::geteuid().as_raw() != 0 {
            eprintln!("not root: another user's daemon is not tried here");
            return;
        }
        const OTHER: u32 = 54_321;
        let dir = dir();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777))
            .expect("open to the other user");
        let path = dir.path().join("other.sock");
        let mut daemon = std::process::Command::new("python3")
            .args([
                "-c",
                "import socket, sys, time\n\
                 s = socket.socket(socket.AF_UNIX)\n\
                 s.bind(sys.argv[1]); s.listen(8)\n\
                 print('ready', flush=True); time.sleep(30)",
                path.to_str().expect("a UTF-8 path"),
            ])
            .uid(OTHER)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("the other user's daemon");
        let mut ready = String::new();
        let out = daemon.stdout.take().expect("its output");
        io::BufReader::new(out)
            .read_line(&mut ready)
            .expect("ready");
        assert_eq!(ready.trim(), "ready");
        let mut tries = 0;
        let e = connect_counted(&path, in_ms(1000), &mut tries).expect_err("its socket");
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
        assert!(e.to_string().contains(&format!("user {OTHER}")), "{e}");
        assert_eq!(tries, 0);
        // The socket handed to root: the daemon is still another's.
        std::os::unix::fs::chown(&path, Some(0), Some(0)).expect("chowned");
        let e = connect_counted(&path, in_ms(1000), &mut tries).expect_err("its daemon");
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
        assert!(
            e.to_string().contains(&format!("answered by user {OTHER}")),
            "{e}"
        );
        let _ = daemon.kill();
        let _ = daemon.wait();
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

/// The volume's channel is in its own directory under the run directory,
/// named by the device and inode numbers of the volume's directory in
/// hex, whatever path names that directory; another volume's is
/// another; a volume's directory that is not there is an error, and
/// no socket is named (the lead's ruling on the agent rig's first run:
/// the agent finds the socket as the daemon names it).
#[cfg(unix)]
#[test]
fn the_socket_is_in_the_volumes_own_run_directory() {
    use std::os::unix::fs::MetadataExt as _;
    let top = tempfile::tempdir().expect("tmp");
    let run = top.path().join("run");
    let volume = top.path().join("vol");
    let other = top.path().join("other");
    for dir in [&run, &volume, &other] {
        std::fs::create_dir(dir).expect("a directory");
    }
    let meta = std::fs::metadata(&volume).expect("stat");
    let own = run.join(format!("v-{:x}-{:x}", meta.dev(), meta.ino()));
    assert_eq!(volume_run_dir(&run, &volume).expect("the volume"), own);
    assert_eq!(
        socket_path(&run, &volume).expect("the socket"),
        own.join("control")
    );
    let link = top.path().join("link");
    std::os::unix::fs::symlink(&volume, &link).expect("a link");
    for path in [link, top.path().join("other/../vol")] {
        assert_eq!(
            socket_path(&run, &path).expect("the socket"),
            own.join(SOCKET)
        );
    }
    assert_ne!(
        socket_path(&run, &other).expect("the socket"),
        own.join(SOCKET)
    );
    let gone = socket_path(&run, &top.path().join("gone")).expect_err("no volume");
    assert_eq!(gone.kind(), io::ErrorKind::NotFound);
}
