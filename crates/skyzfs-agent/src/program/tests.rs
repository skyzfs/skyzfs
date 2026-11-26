//! The program's command line, output and exit codes, and the program
//! run on the engine's fakes.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use skyzfs_control::{Code, Reply, Request};

use super::*;
use crate::engine::fake::{FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, World, POOL, VOLUME};
use crate::record::{Abandoned, Count, Enrolled, Member, Publish, Record, Taken};

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(String::from).collect()
}

const COMMON: &str = "--volume /v --run /r --device /dev/ublkb0 --pool tank --dir /d";

fn run() -> Run {
    Run {
        volume: "/v".into(),
        run: "/r".into(),
        device: "/dev/ublkb0".into(),
        pool: "tank".into(),
        dir: "/d".into(),
        limit: LIMIT_DEFAULT,
        expiry: None,
        hooks: Hooks::default(),
    }
}

fn refused(line: &str, hooks: bool) -> String {
    match parse(&args(line), hooks) {
        Err(Usage(why)) => why,
        Ok(command) => panic!("{line:?} parsed: {command:?}"),
    }
}

#[test]
fn each_command_is_parsed_with_its_options() {
    assert_eq!(
        parse(&args(&format!("publish {COMMON}")), false),
        Ok(Command::Publish(run()))
    );
    assert_eq!(
        parse(
            &args(&format!("publish {COMMON} --limit-s 30 --expiry-s=120")),
            false
        ),
        Ok(Command::Publish(Run {
            limit: Duration::from_secs(30),
            expiry: Some(Duration::from_secs(120)),
            ..run()
        }))
    );
    assert_eq!(
        parse(&args(&format!("settle --limit-s 5 {COMMON}")), false),
        Ok(Command::Settle(Run {
            limit: Duration::from_secs(5),
            ..run()
        }))
    );
    for (flag, choice) in [("--give-up", Choice::GiveUp), ("--retry", Choice::Retry)] {
        assert_eq!(
            parse(
                &args(&format!("recover {flag} --name skz-t {COMMON}")),
                false
            ),
            Ok(Command::Recover {
                run: run(),
                name: "skz-t".into(),
                choice
            })
        );
    }
    assert_eq!(
        parse(&args("record --dir /d"), false),
        Ok(Command::Record { dir: "/d".into() })
    );
    for help in ["--help", "-h", "help"] {
        assert_eq!(parse(&args(help), false), Ok(Command::Help));
    }
    // After a command too, and where a value was looked for (S6 of
    // #149's review).
    for line in [
        "record --help",
        "publish -h",
        "record --dir /d --help",
        "record --dir --help",
    ] {
        assert_eq!(parse(&args(line), false), Ok(Command::Help), "{line}");
    }
    // Where a value is looked for, `-h` is the value (nit 3 of the next
    // look at #149).
    assert_eq!(
        parse(&args("record --dir -h"), false),
        Ok(Command::Record { dir: "-h".into() })
    );
    // A value that begins with `--`, given with `=`.
    assert_eq!(
        parse(&args("record --dir=--d"), false),
        Ok(Command::Record { dir: "--d".into() })
    );
}

/// Mutant: an option of one command taken by another, a missing one
/// taken as empty, or a value out of range let through.
#[test]
fn a_command_line_the_program_does_not_take_is_a_usage_error() {
    let rows = [
        ("", "no command"),
        ("snapshot --dir /d", "no command"),
        ("publish", "--volume is needed"),
        (
            "publish --volume /v --run /r --device /d --pool tank",
            "--dir is needed",
        ),
        ("record", "--dir is needed"),
        ("record --dir", "--dir needs a value"),
        ("record --dir=", "--dir needs a value"),
        ("record --dir /d --dir /e", "--dir is given twice"),
        ("record --dir /d stray", "is not an option"),
        // S3 of #149's review: the next option is not a value.
        (
            "record --dir --d",
            "--dir needs a value, not the option --d",
        ),
        (
            "record --dir /d --pool tank",
            "record takes no option --pool",
        ),
    ];
    for (line, want) in rows {
        let why = refused(line, false);
        assert!(why.contains(want), "{line:?}: {why:?}, not {want:?}");
    }
    let with = |rest: &str| format!("{rest} {COMMON}");
    let rows = [
        (
            with("settle --expiry-s 60"),
            "settle takes no option --expiry-s",
        ),
        (with("publish --expiry-s 0"), "at least 1"),
        (with("publish --expiry-s 3601"), "at most 3600 s"),
        (with("publish --limit-s -1"), "at least 1"),
        (with("publish --limit-s 1.5"), "at least 1"),
        (with("recover --name t"), "one of --give-up and --retry"),
        (
            with("recover --name t --give-up --retry"),
            "one of --give-up and --retry",
        ),
        (with("recover --give-up"), "--name is needed"),
        (
            with("recover --name t --retry=yes"),
            "--retry takes no value",
        ),
        (with("publish --name t"), "publish takes no option --name"),
        (
            with("recover --name --give-up --retry"),
            "--name needs a value, not the option --give-up",
        ),
        (
            "publish --volume --run /r --device /d --pool tank --dir /a".into(),
            "--volume needs a value, not the option --run",
        ),
    ];
    for (line, want) in rows {
        let why = refused(&line, false);
        assert!(why.contains(want), "{line:?}: {why:?}, not {want:?}");
    }
    assert_eq!(
        parse(&args(&format!("publish {COMMON} --expiry-s 3600")), false).map(|c| match c {
            Command::Publish(run) => run.expiry,
            _ => None,
        }),
        Ok(Some(EXPIRY_MAX))
    );
}

/// Mutant: a test hook taken by a build without the feature, or refused
/// by one with it.
#[test]
fn the_test_hooks_are_taken_only_by_a_build_with_them() {
    let hooks = [
        "--kill-at asked:after",
        "--trace-points /t",
        "--assist-after-s 2",
        "--age-max-s 60",
    ];
    for hook in hooks {
        let line = format!("settle {COMMON} {hook}");
        let why = refused(&line, false);
        assert!(why.contains("test hook"), "{line:?}: {why:?}");
    }
    let line = format!("settle {COMMON} {}", hooks.join(" "));
    assert_eq!(
        parse(&args(&line), true),
        Ok(Command::Settle(Run {
            hooks: Hooks {
                kill_at: Some(KillAt {
                    point: Point {
                        effect: Effect::Asked,
                        after: true
                    },
                    nth: 1
                }),
                trace_points: Some("/t".into()),
                assist_after: Some(Duration::from_secs(2)),
                age_max: Some(Duration::from_secs(60)),
            },
            ..run()
        }))
    );
    assert!(refused("record --dir /d --trace-points /t", true).contains("record takes no option"));
}

#[test]
fn a_kill_point_names_an_effect_a_side_and_a_reach() {
    let at = |effect, after, nth| KillAt {
        point: Point { effect, after },
        nth,
    };
    assert_eq!(kill_at("asked:after"), Ok(at(Effect::Asked, true, 1)));
    assert_eq!(
        kill_at("Release:before:3"),
        Ok(at(Effect::Release, false, 3))
    );
    assert_eq!(kill_at("STOP:after:1"), Ok(at(Effect::Stop, true, 1)));
    for bad in [
        "",
        "asked",
        "asked:during",
        "unwind:after",
        "asked:after:0",
        "asked:after:x",
        "asked:after:2:1",
    ] {
        assert!(kill_at(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        kill_line(&at(Effect::Asked, true, 2)),
        "skyzfs-agent: test hook --kill-at: killed at asked after (2)\n"
    );
}

/// A writer whose bytes a test reads, shared with the program.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);

impl Shared {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.borrow()).into_owned()
    }
}

impl Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer that fails.
struct Broken;

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("the disk is full"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Mutant: the trace written after the kill, a reach counted at other
/// points, a kill at a reach other than the Nth, or a trace that fails
/// let the engine go on.
#[test]
fn the_points_trace_then_kill_at_the_nth_reach() {
    let point = |effect, after| Point { effect, after };
    let trace = Shared::default();
    let died = Shared::default();
    let clock = Rc::new(RefCell::new(100u64));
    let mut hooked = points(
        Some(KillAt {
            point: point(Effect::Hold, false),
            nth: 2,
        }),
        Some(Box::new(trace.clone())),
        {
            let clock = clock.clone();
            move || {
                *clock.borrow_mut() += 1;
                *clock.borrow()
            }
        },
        {
            let (died, trace) = (died.clone(), trace.clone());
            move |line: &str| {
                // The point is in the trace before the kill.
                let traced = trace.text().lines().count();
                let _ = write!(died.clone(), "{traced} {line}");
            }
        },
        Box::new(io::sink()),
    )
    .expect("points");
    assert!(!hooked(point(Effect::Enrol, false), ""));
    assert!(!hooked(point(Effect::Hold, false), "skz-a"));
    assert!(!hooked(point(Effect::Hold, true), "skz-a"));
    assert_eq!(died.text(), "");
    assert!(!hooked(point(Effect::Hold, false), "skz-b"));
    assert_eq!(
        died.text(),
        "4 skyzfs-agent: test hook --kill-at: killed at hold before (2)\n"
    );
    assert!(!hooked(point(Effect::Hold, false), "skz-b"));
    assert_eq!(died.text().lines().count(), 1, "killed once");
    assert_eq!(
        trace.text(),
        "101\tenrol\tbefore\t\n\
         102\thold\tbefore\tskz-a\n\
         103\thold\tafter\tskz-a\n\
         104\thold\tbefore\tskz-b\n\
         105\thold\tbefore\tskz-b\n"
    );
    let err = Shared::default();
    let mut broken = points(
        None,
        Some(Box::new(Broken)),
        || 0,
        |_: &str| {},
        Box::new(err.clone()),
    )
    .expect("points");
    assert!(broken(point(Effect::Ask, false), "skz-a"));
    assert!(err.text().contains("the disk is full"), "{}", err.text());
    assert!(points(None, None, || 0, |_: &str| {}, Box::new(io::sink())).is_none());
}

/// Mutant: two outcomes or reasons printed alike, or two exit codes the
/// same.
#[test]
fn each_outcome_report_and_exit_is_told_apart() {
    let whys = [
        Why::ManifestLost,
        Why::ManifestTooLarge,
        Why::Destroyed,
        Why::Replaced,
        Why::Premise,
        Why::NameTaken,
        Why::Rewound,
        Why::NoProgress,
        Why::TooOld,
        Why::Stopped,
        Why::Person,
    ];
    let words: std::collections::BTreeSet<&str> = whys.iter().map(|&w| why_word(w)).collect();
    assert_eq!(words.len(), whys.len());
    assert_eq!(
        outcome_line("skz-t", &Outcome::Published([0xc0; 32])),
        format!("snap/skz-t {}\n", "c0".repeat(32))
    );
    assert_eq!(
        outcome_line("skz-t", &Outcome::Abandoned(Why::TooOld)),
        "abandoned skz-t too-old\n"
    );
    assert_eq!(
        outcome_line("skz-t", &Outcome::Stopped("why".into())),
        "stopped skz-t why\n"
    );
    let reports = [
        (Report::SnapshotGone, "snapshot-gone:"),
        (Report::NoProgress(3), "no-progress 3:"),
        (Report::Refused(Code::Rewound), "refused rewound:"),
        (Report::HeldAway, "held-away:"),
        (
            Report::Unreadable("zfs get: exit 1:\ncannot open 'x'\n\nmore".into()),
            "unreadable:",
        ),
    ];
    for (report, word) in reports {
        let line = report_line("skz-t", &report);
        assert!(
            line.starts_with(&format!("skyzfs-agent: report skz-t {word}")),
            "{line:?}"
        );
        assert!(
            line.ends_with('\n') && line.lines().count() == 1,
            "{line:?}"
        );
    }
    let codes = [
        exit::PUBLISHED,
        exit::ERROR,
        exit::USAGE,
        exit::ABANDONED,
        exit::STOPPED,
        exit::PREMISE,
        exit::RECORD,
        exit::UNSETTLED,
        exit::HELD,
    ];
    let distinct: std::collections::BTreeSet<i32> = codes.into_iter().collect();
    assert_eq!(distinct.len(), codes.len());
    for code in codes {
        assert!(
            USAGE.contains(&format!("{code} ")),
            "{code} is not in the usage"
        );
    }
}

#[test]
fn settle_exits_by_the_worst_outcome() {
    let o = |outcome: Outcome| ("skz-t".to_string(), outcome);
    let published = o(Outcome::Published([0; 32]));
    let abandoned = o(Outcome::Abandoned(Why::Person));
    let stopped = o(Outcome::Stopped(String::new()));
    assert_eq!(worst(&[], false), exit::PUBLISHED);
    assert_eq!(
        worst(std::slice::from_ref(&published), false),
        exit::PUBLISHED
    );
    assert_eq!(
        worst(&[published.clone(), abandoned.clone()], false),
        exit::ABANDONED
    );
    assert_eq!(
        worst(
            &[stopped.clone(), abandoned.clone(), published.clone()],
            false
        ),
        exit::STOPPED
    );
    assert_eq!(
        worst(&[published, abandoned, stopped], true),
        exit::UNSETTLED
    );
    assert_eq!(exit_of(None), exit::UNSETTLED);
    let corrupt = crate::record::Corrupt::Hash;
    assert_eq!(exit_of_error(&Error::Corrupt(corrupt)), exit::RECORD);
    assert_eq!(exit_of_error(&Error::Volume([1; 16])), exit::RECORD);
    assert_eq!(exit_of_error(&Error::Unmarked(vec![])), exit::RECORD);
    assert_eq!(
        exit_of_error(&Error::Premise(crate::zfs::Refusal::Checkpoint)),
        exit::PREMISE
    );
    assert_eq!(exit_of_error(&Error::Unsettled(vec![])), exit::UNSETTLED);
    assert_eq!(exit_of_error(&Error::Full), exit::ERROR);
    assert_eq!(exit_of_error(&Error::NotUnderWay("t".into())), exit::ERROR);
}

const REMOTE: &str = "skyzfs-remote 1\nendpoint http://127.0.0.1:9000\nbucket volumes\n\
                      volume pg-data\nid 01ARZ3NDEKTSV4RRFFQ69G5FAV\n";

/// The id's bytes were computed apart from this code, from the ULID's
/// alphabet. Mutant: the bytes least significant first, a character
/// misread, or a file of another shape read.
#[test]
fn the_volume_s_id_is_its_ulid_most_significant_first() {
    let want = skyzfs_control::unhex::<16>("01563e3ab5d3d6764c61efb99302bd5b").expect("hex");
    assert_eq!(volume_id(REMOTE.as_bytes()), Ok(want));
    let lower = REMOTE.replace("01ARZ3NDEKTSV4RRFFQ69G5FAV", "01arz3ndektsv4rrffq69g5fav");
    assert_eq!(volume_id(lower.as_bytes()), Ok(want));
    let most = REMOTE.replace("01ARZ3NDEKTSV4RRFFQ69G5FAV", "7ZZZZZZZZZZZZZZZZZZZZZZZZZ");
    assert_eq!(volume_id(most.as_bytes()), Ok([0xff; 16]));
    let bad = [
        REMOTE.replace("skyzfs-remote 1", "skyzfs-remote 2"),
        REMOTE.trim_end().to_string(),
        format!("{REMOTE}extra\n"),
        REMOTE.replace("bucket volumes\n", ""),
        REMOTE.replace("bucket", "pail"),
        REMOTE.replace("01ARZ3NDEKTSV4RRFFQ69G5FAV", "01ARZ3NDEKTSV4RRFFQ69G5FA"),
        REMOTE.replace("01ARZ3NDEKTSV4RRFFQ69G5FAV", "01ARZ3NDEKTSV4RRFFQ69G5FAU"),
        REMOTE.replace("01ARZ3NDEKTSV4RRFFQ69G5FAV", "81ARZ3NDEKTSV4RRFFQ69G5FAV"),
        REMOTE.replace("id 01ARZ3NDEKTSV4RRFFQ69G5FAV", "id "),
    ];
    for file in bad {
        assert!(volume_id(file.as_bytes()).is_err(), "{file:?}");
    }
    assert!(volume_id(&[0xff, b'\n']).is_err());
}

fn taken(group: u64) -> Taken {
    Taken {
        group,
        members: vec![Member {
            name: "tank".into(),
            snapshot_guid: 7,
        }],
        manifest_len: 99,
        manifest_hash: [0x4d; 32],
    }
}

/// The dump of a record with a publish in each phase, written out
/// whole: the rigs parse these lines.
#[test]
fn the_record_is_dumped_one_line_a_fact() {
    let publish = |name: &str, phase: Phase, person| Publish {
        name: name.into(),
        count: Count {
            attachment: [1; 16],
            state: [2; 16],
            after: 5,
            load_guid: 9,
        },
        held: !matches!(phase, Phase::Intent),
        phase,
        attempts: 2,
        stalls: 1,
        begun_ms: 1000,
        person,
    };
    let record = Record {
        volume: [0xab; 16],
        enrolled: Some(Enrolled {
            pool: "tank".into(),
            pool_guid: 11,
            leaf_guid: 12,
        }),
        under_way: vec![
            publish("skz-a", Phase::Intent, None),
            publish("skz-b", Phase::Held(taken(40)), Some(Person::GiveUp)),
            publish(
                "skz-c",
                Phase::Asked {
                    taken: taken(41),
                    asked_ms: 2000,
                    held_until_ms: 3000,
                },
                Some(Person::Stopped),
            ),
            publish(
                "skz-d",
                Phase::Done {
                    taken: taken(42),
                    commit: [0xc0; 32],
                },
                None,
            ),
        ],
        abandoned: vec![Abandoned {
            name: "skz-z".into(),
            why: Why::NameTaken,
            at_ms: 500,
        }],
    };
    let h = "4d".repeat(32);
    let c = "c0".repeat(32);
    let want = format!(
        "volume {}\n\
         enrolled tank pool_guid=11 leaf_guid=12\n\
         publish skz-a phase=intent count=5 group=- stalls=1 attempts=2 held=no asked_ms=- held_until_ms=- manifest=- commit=- person=- begun_ms=1000\n\
         publish skz-b phase=held count=5 group=40 stalls=1 attempts=2 held=yes asked_ms=- held_until_ms=- manifest={h} commit=- person=give-up begun_ms=1000\n\
         publish skz-c phase=asked count=5 group=41 stalls=1 attempts=2 held=yes asked_ms=2000 held_until_ms=3000 manifest={h} commit=- person=stopped begun_ms=1000\n\
         publish skz-d phase=done count=5 group=42 stalls=1 attempts=2 held=yes asked_ms=- held_until_ms=- manifest={h} commit={c} person=- begun_ms=1000\n\
         abandoned skz-z why=name-taken at_ms=500\n",
        "ab".repeat(16)
    );
    assert_eq!(dump(&Loaded::Present(record)), Ok(want));
    let empty = Record::new([0; 16]);
    assert_eq!(
        dump(&Loaded::Present(empty)),
        Ok(format!("volume {}\nenrolled -\n", "00".repeat(16)))
    );
    assert_eq!(dump(&Loaded::Absent), Ok("absent\n".into()));
    assert!(dump(&Loaded::Corrupt(crate::record::Corrupt::Hash))
        .is_err_and(|why| why.contains("corrupt")));
}

/// The fake daemon with `look` blind: a `done` is never confirmed.
struct Blind(FakeDaemon);

impl Daemon for Blind {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        let reply = self.0.ask(request, manifest)?;
        Ok(match request {
            Request::Look(_) => Reply::Absent,
            _ => reply,
        })
    }
    fn device(&mut self) -> io::Result<u64> {
        self.0.device()
    }
}

fn agent_on<D: Daemon>(
    world: &Rc<RefCell<World>>,
    daemon: D,
    keep: &MemKeep,
) -> Agent<FakeZfs, D, MemKeep, FakeClock> {
    Agent::new(
        FakeZfs(world.clone()),
        daemon,
        keep.clone(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
}

const LIMIT: Duration = Duration::from_secs(3 * 3600);

/// What [`execute`] gave: the exit code, standard output and error.
fn executed<D: Daemon>(
    agent: &mut Agent<FakeZfs, D, MemKeep, FakeClock>,
    task: &Task,
    limit: Duration,
) -> (i32, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = execute(agent, task, limit, &mut out, &mut err);
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    )
}

/// Mutant: a publish's exit code taken from another publish, an
/// unsettled publish exiting 0, or an outcome line not printed.
#[test]
fn publish_prints_snap_t_and_its_commit_and_exits_zero() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let (code, out, err) = executed(&mut agent, &Task::Publish, LIMIT);
    assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
    let (t, commit) = {
        let world = world.borrow();
        let (t, row) = world.refs.iter().next().expect("published");
        (t.clone(), row.commit)
    };
    let line = format!("snap/{t} {}\n", hex(&commit));
    assert_eq!(out, line);
    // Settled with nothing under way: the outcomes of the process again.
    let (code, out, _) = executed(&mut agent, &Task::Settle, LIMIT);
    assert_eq!((code, out), (exit::PUBLISHED, line));
}

#[test]
fn a_publish_still_under_way_at_the_limit_exits_unsettled() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let (code, out, _) = executed(&mut agent, &Task::Publish, Duration::ZERO);
    assert_eq!(code, exit::UNSETTLED);
    let t = agent.record().under_way[0].name.clone();
    assert_eq!(out, format!("unsettled {t}\n"));
    let (code, _, _) = executed(&mut agent, &Task::Settle, Duration::ZERO);
    assert_eq!(code, exit::UNSETTLED);
    let (code, out, _) = executed(&mut agent, &Task::Settle, LIMIT);
    assert_eq!(code, exit::PUBLISHED);
    assert!(out.starts_with(&format!("snap/{t} ")), "{out}");
}

/// Stopped for a person, then recovered: given up by a person, a
/// publish made is still published; retried, it is published. And
/// `recover` of a name not under way is an error.
#[test]
fn a_stop_for_a_person_exits_stopped_until_recovered() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut blind = agent_on(&world, Blind(FakeDaemon(world.clone())), &keep);
    let (code, out, _) = executed(&mut blind, &Task::Publish, LIMIT);
    assert_eq!(code, exit::STOPPED, "{out}");
    let t = blind.record().under_way[0].name.clone();
    assert!(out.starts_with(&format!("stopped {t} ")), "{out}");
    drop(blind);
    // A new publish exits by its own outcome, not by the stop beside it;
    // `settle` exits by the worst.
    let mut again = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let (code, out, _) = executed(&mut again, &Task::Publish, LIMIT);
    assert_eq!(code, exit::PUBLISHED, "{out}");
    assert!(out.starts_with(&format!("stopped {t} ")), "{out}");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("snap/")).count(),
        1,
        "{out}"
    );
    let mut again = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let (code, _, _) = executed(&mut again, &Task::Settle, LIMIT);
    assert_eq!(code, exit::STOPPED);
    let (code, _, err) = executed(
        &mut again,
        &Task::Recover("skz-none".into(), Choice::Retry),
        LIMIT,
    );
    assert_eq!(code, exit::ERROR);
    assert!(err.contains("no publish skz-none is under way"), "{err}");
    let (code, out, _) = executed(&mut again, &Task::Recover(t.clone(), Choice::GiveUp), LIMIT);
    assert_eq!(code, exit::PUBLISHED, "{out}");
    assert!(out.contains(&format!("snap/{t} ")), "{out}");
}

/// A person's give-up of a publish whose request pends exits abandoned.
#[test]
fn a_person_s_give_up_exits_abandoned() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let (code, _, _) = executed(&mut agent, &Task::Publish, Duration::from_secs(60));
    assert_eq!(code, exit::UNSETTLED);
    let t = agent.record().under_way[0].name.clone();
    let (code, out, _) = executed(&mut agent, &Task::Recover(t.clone(), Choice::GiveUp), LIMIT);
    assert_eq!(code, exit::ABANDONED, "{out}");
    assert_eq!(out, format!("abandoned {t} person\n"));
    assert!(world.borrow().refs.is_empty());
}

/// A publish whose name a person took in one member between the intent
/// and the snapshot is made under the next name, and `publish` exits by
/// the outcome of that name (D2 of #149's review). The rows take one
/// name, and two: the publish is renamed twice. Mutants: the exit looked
/// up by the name begun, which has no outcome (exit 7); one rename
/// followed and not the next.
#[test]
fn a_publish_made_under_its_next_name_exits_by_its_outcome() {
    for names in [1, 2] {
        let world = World::new(Mode::Busy);
        let keep = MemKeep::default();
        let taken = Rc::new(RefCell::new(Vec::<String>::new()));
        let hook = {
            let (world, taken) = (world.clone(), taken.clone());
            move |point: Point, at: &str| {
                let mut taken = taken.borrow_mut();
                if point.effect == Effect::Intent
                    && point.after
                    && taken.len() < names
                    && !taken.iter().any(|t| t == at)
                {
                    taken.push(at.to_string());
                    let mut world = world.borrow_mut();
                    let pool = world.pool.as_mut().expect("the pool");
                    let snap = crate::engine::fake::Snap {
                        guid: 0xdead,
                        createtxg: pool.txg,
                        holds: std::collections::BTreeSet::new(),
                        held_in: None,
                    };
                    pool.snaps.insert(format!("{POOL}/db@{at}"), snap);
                }
                false
            }
        };
        let mut agent =
            agent_on(&world, FakeDaemon(world.clone()), &keep).with_points(Box::new(hook));
        let (code, out, err) = executed(&mut agent, &Task::Publish, LIMIT);
        let taken = taken.borrow().clone();
        assert_eq!(taken.len(), names, "{names} names: {taken:?}");
        assert_eq!(
            (code, err.as_str()),
            (exit::PUBLISHED, ""),
            "{names} names: {out}"
        );
        assert_eq!(out.lines().count(), 1, "{names} names: {out}");
        let made = out
            .strip_prefix("snap/")
            .and_then(|o| o.split(' ').next())
            .expect("a snap/ line");
        assert!(
            !taken.iter().any(|t| t == made),
            "{names} names: {made} {taken:?}"
        );
        assert!(
            world.borrow().refs.contains_key(made),
            "{names} names: {made}"
        );
    }
}

/// `settle` and `publish` in a mistyped `--dir`, beside a publish held
/// from the right one: each exits 6 and names the marker, and the hold
/// stays (L4 of #149's review).
#[test]
fn a_mistyped_directory_exits_record_and_releases_nothing() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let right = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &right);
    let (code, _, _) = executed(&mut agent, &Task::Publish, Duration::from_secs(60));
    assert_eq!(code, exit::UNSETTLED);
    let held = crate::engine::fake::holds(&world.borrow());
    assert!(!held.is_empty());
    let mistyped = MemKeep::default();
    for task in [Task::Settle, Task::Publish] {
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &mistyped);
        let (code, out, err) = executed(&mut agent, &task, LIMIT);
        assert_eq!((code, out.as_str()), (exit::RECORD, ""), "{task:?}: {err}");
        assert!(err.contains("no agent's marker"), "{err}");
        assert_eq!(crate::engine::fake::holds(&world.borrow()), held);
    }
    assert!(mistyped.0.borrow().record.is_none());
}

/// The test hooks and the expiry reach the engine. Mutant: one dropped.
#[test]
fn the_run_s_settings_reach_the_agent() {
    let asked_at = |world: &Rc<RefCell<World>>, run: &Run| {
        let keep = MemKeep::default();
        let stop = |p: Point, _: &str| {
            p == Point {
                effect: Effect::Asked,
                after: true,
            }
        };
        let mut agent = configured(agent_on(world, FakeDaemon(world.clone()), &keep), run)
            .with_points(Box::new(stop));
        let (code, _, err) = executed(&mut agent, &Task::Publish, LIMIT);
        assert_eq!(code, exit::ERROR, "{err}");
        match agent.record().under_way[0].phase {
            Phase::Asked {
                asked_ms,
                held_until_ms,
                ..
            } => (agent, asked_ms, held_until_ms),
            ref other => panic!("{other:?}"),
        }
    };
    // The expiry: held until the acceptance, plus it, plus the margin.
    let world = World::new(Mode::Busy);
    let expiry = Run {
        expiry: Some(Duration::from_secs(120)),
        ..run()
    };
    let (_, asked, until) = asked_at(&world, &expiry);
    let margin = u64::try_from(crate::plan::HOLD_MARGIN.as_millis()).expect("ms");
    assert_eq!(until - asked, 120_000 + margin);
    // The time before a sync: on an idle pool, the first is that long
    // after the acceptance.
    let world = World::new(Mode::Idle);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let quick = Run {
        hooks: Hooks {
            assist_after: Some(Duration::from_secs(2)),
            ..Hooks::default()
        },
        ..run()
    };
    let (mut agent, asked, _) = asked_at(&world, &quick);
    let agent = agent_on(
        &world,
        FakeDaemon(world.clone()),
        &MemKeep(agent.parts().2 .0.clone()),
    );
    let mut agent = configured(agent, &quick);
    let _ = executed(&mut agent, &Task::Settle, Duration::from_secs(30));
    // Not sooner than it, and well before the default's ten seconds (the
    // restarted agent's rounds are a tick apart).
    let first = world.borrow().synced_at.first().copied().expect("a sync");
    assert!(
        (asked + 2_000..asked + 10_000).contains(&first),
        "{} ms after the acceptance",
        first - asked
    );
    // The bound on age.
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let old = Run {
        hooks: Hooks {
            age_max: Some(Duration::from_secs(1)),
            ..Hooks::default()
        },
        ..run()
    };
    let mut agent = configured(
        agent_on(&world, FakeDaemon(world.clone()), &MemKeep::default()),
        &old,
    );
    // Well within the default bound (two hours): without the setting
    // the publish would still be under way at the limit.
    let (code, out, _) = executed(&mut agent, &Task::Publish, Duration::from_secs(600));
    assert_eq!(code, exit::ABANDONED, "{out}");
    assert!(out.ends_with(" too-old\n"), "{out}");
}
