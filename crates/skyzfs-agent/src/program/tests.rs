//! The program's command line, output and exit codes, and the program
//! run on the engine's fakes.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use skyzfs_control::{Code, Reply, Request};

use super::*;
use crate::engine::fake::{
    FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, Restart, World, POOL, VOLUME,
};
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
/// `run` takes the options of `publish` but `--limit-s`, and `--every-s`.
/// Mutant: `--every-s`'s default other than a minute, `--limit-s` taken,
/// or `--expiry-s` not bounded.
#[test]
fn run_is_parsed_with_its_options() {
    let every = |s| Duration::from_secs(s);
    assert_eq!(
        parse(&args(&format!("run {COMMON}")), false),
        Ok(Command::Run {
            run: run(),
            every: every(60)
        })
    );
    assert_eq!(EVERY_DEFAULT, every(60));
    assert_eq!(
        parse(
            &args(&format!("run {COMMON} --every-s 5 --expiry-s=120")),
            false
        ),
        Ok(Command::Run {
            run: Run {
                expiry: Some(every(120)),
                ..run()
            },
            every: every(5)
        })
    );
    let rows = [
        ("--limit-s 30", "run takes no option --limit-s"),
        ("--every-s 0", "at least 1"),
        ("--every-s 1.5", "at least 1"),
        ("--expiry-s 999999", "--expiry-s: at most"),
        ("--name t", "run takes no option --name"),
        ("--give-up", "run takes no option --give-up"),
    ];
    for (extra, why) in rows {
        let got = refused(&format!("run {COMMON} {extra}"), false);
        assert!(got.contains(why), "{extra:?}: {got}");
    }
    assert!(refused(&format!("run {COMMON} --age-max-s 5"), false).contains("test hook"));
    assert!(matches!(
        parse(&args(&format!("run {COMMON} --age-max-s 5")), true),
        Ok(Command::Run { .. })
    ));
    assert!(USAGE.contains("skyzfs-agent run "));
}

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

/// The `not-whole` line (T11, N12 and N13 of #153's review): its word,
/// the datasets that broke the last take, at most
/// [`NOT_WHOLE_NAMED`] of them and the count of the others, each name
/// read as one where it holds a space.
#[test]
fn a_not_whole_report_names_its_datasets() {
    let line = |names: Vec<String>| report_line("skz-t", &Report::NotWhole(names));
    let given_up = "skyzfs-agent: report skz-t not-whole: given up after its takes in a row \
         were not one whole take of the pool or not as read before them";
    assert_eq!(
        outcome_line("skz-t", &Outcome::Abandoned(Why::NotWhole)),
        "abandoned skz-t not-whole\n"
    );
    assert_eq!(
        line(Vec::new()),
        format!("{given_up}; the datasets are not known\n")
    );
    assert_eq!(
        line(vec!["tank/a b".into(), "tank/c".into()]),
        format!("{given_up}; datasets: tank/a b, tank/c\n")
    );
    let names = |n: usize| (0..n).map(|i| format!("tank/d {i}")).collect::<Vec<_>>();
    let shown = names(NOT_WHOLE_NAMED).join(", ");
    assert_eq!(
        line(names(NOT_WHOLE_NAMED)),
        format!("{given_up}; datasets: {shown}\n")
    );
    assert_eq!(
        line(names(NOT_WHOLE_NAMED + 1)),
        format!("{given_up}; datasets: {shown}; and 1 more\n")
    );
    assert_eq!(
        line(names(NOT_WHOLE_NAMED + 37)),
        format!("{given_up}; datasets: {shown}; and 37 more\n")
    );
    assert_eq!(NOT_WHOLE_NAMED, 20);
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
        Why::NotWhole,
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
        (Report::NotWhole(Vec::new()), "not-whole:"),
        (Report::NotWhole(vec!["tank/a b".into()]), "not-whole:"),
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

/// What [`execute`] gave: the exit code, standard output and error. The
/// outcome lines reach standard output by the agent's outcome sink, as
/// [`wired`] sets it, in the order they are made.
fn executed<D: Daemon>(
    agent: &mut Agent<FakeZfs, D, MemKeep, FakeClock>,
    task: &Task,
    limit: Duration,
) -> (i32, String, String) {
    let (mut out, mut err) = (Shared::default(), Shared::default());
    let mut told = out.clone();
    agent.set_outcome_sink(Box::new(move |name, outcome| {
        let _ = told.write_all(outcome_line(name, outcome).as_bytes());
    }));
    let code = execute(agent, task, limit, &mut out, &mut err);
    (code, out.text(), err.text())
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
    // Settled with nothing under way: each outcome was told once, as it
    // was made (S1 of #149's review), and the exit is still by them.
    let (code, out, _) = executed(&mut agent, &Task::Settle, LIMIT);
    assert_eq!((code, out.as_str()), (exit::PUBLISHED, ""));
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

/// The time on the fake world's clock that does not step.
fn mono(world: &Rc<RefCell<World>>) -> u64 {
    world.borrow().now_ms - crate::engine::fake::EPOCH_MS
}

/// What [`schedule`] gave, with the outcome lines told to the same
/// standard output, as [`wired`] tells them: the exit code, standard
/// output and error. `go` is asked as `run` asks it.
fn scheduled<D: Daemon, K: Keep>(
    agent: &mut Agent<FakeZfs, D, K, FakeClock>,
    every: Duration,
    go: &mut dyn FnMut() -> bool,
) -> (i32, String, String) {
    let (mut out, mut err) = (Shared::default(), Shared::default());
    let mut told = out.clone();
    agent.set_outcome_sink(Box::new(move |name, outcome| {
        let _ = told.write_all(outcome_line(name, outcome).as_bytes());
    }));
    let code = schedule(agent, every, go, &mut out, &mut err);
    (code, out.text(), err.text())
}

/// `go` that says yes until the fake clock reaches `until_ms` on the
/// clock that does not step.
fn until(world: &Rc<RefCell<World>>, until_ms: u64) -> impl FnMut() -> bool {
    let world = world.clone();
    move || mono(&world) < until_ms
}

/// The round lines of `out`.
fn rounds(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.starts_with("round ")).collect()
}

/// The value of `key=` in a round line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split(' ')
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("{key} in {line:?}"))
}

/// A round line's text. Mutant: a field left out or misnamed.
#[test]
fn a_round_line_says_what_the_round_did() {
    let round = Round {
        n: 3,
        failed: false,
        late_ms: 12,
        took_ms: 3_456,
        skipped: 2,
        begun: Some("skz-20261126T120000Z".into()),
        under_way: 1,
    };
    assert_eq!(
        round_line(&round),
        "round 3 ok took_ms=3456 late_ms=12 skipped=2 begun=skz-20261126T120000Z under_way=1\n"
    );
    let failed = Round {
        failed: true,
        begun: None,
        ..round
    };
    assert_eq!(
        round_line(&failed),
        "round 3 failed took_ms=3456 late_ms=12 skipped=2 begun=- under_way=1\n"
    );
}

/// A round at each tick, each begins a publish that is published within
/// it, and its outcome line comes before its round line. Mutant: the
/// first round not at once, ticks paced by the time a round took (not
/// by the tick), a publish not begun each round, or the round line
/// before the outcome.
#[test]
fn a_round_at_each_tick_begins_a_publish_and_settles_it() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 150_000));
    assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 6, "{out}");
    for (i, pair) in lines.chunks(2).enumerate() {
        let round = pair[1];
        assert!(round.starts_with(&format!("round {} ok ", i + 1)), "{out}");
        let t = field(round, "begun");
        assert!(pair[0].starts_with(&format!("snap/{t} ")), "{out}");
        assert_eq!(field(round, "late_ms"), "0", "{out}");
        assert_eq!(field(round, "skipped"), "0", "{out}");
        assert_eq!(field(round, "under_way"), "0", "{out}");
        assert!(field(round, "took_ms").parse::<u64>().expect("ms") < 60_000);
    }
    assert_eq!(world.borrow().refs.len(), 3);
    // The rounds began a tick apart: the names are of their seconds.
    let names: Vec<&str> = rounds(&out).iter().map(|l| field(l, "begun")).collect();
    let seconds: Vec<u64> = names
        .iter()
        .map(|t| {
            let (_, hms) = t.split_once('T').expect("time");
            let hms = &hms[..6];
            let n = |r: std::ops::Range<usize>| hms[r].parse::<u64>().expect("digits");
            n(0..2) * 3600 + n(2..4) * 60 + n(4..6)
        })
        .collect();
    assert_eq!(seconds[1] - seconds[0], 60, "{names:?}");
    assert_eq!(seconds[2] - seconds[1], 60, "{names:?}");
    // It ended while it waited for the fourth tick, within a wake of
    // being told.
    let ended = mono(&world);
    assert!(
        (start + 150_000..=start + 150_000 + 250).contains(&ended),
        "{}",
        ended - start
    );
}

/// The fake daemon, whose `cuts` takes `slow` of the fake clock at the
/// `nth` time it is asked (from 0).
struct Slow {
    daemon: FakeDaemon,
    asked: usize,
    nth: usize,
    slow: Duration,
}

impl Daemon for Slow {
    fn ask(&mut self, request: &Request, manifest: &[u8]) -> io::Result<Reply> {
        if *request == Request::Cuts {
            if self.asked == self.nth {
                self.daemon.0.borrow_mut().advance(self.slow);
            }
            self.asked += 1;
        }
        self.daemon.ask(request, manifest)
    }
    fn device(&mut self) -> io::Result<u64> {
        self.daemon.device()
    }
}

/// A round that runs past the next ticks: the next round begins as it
/// ends, for the last of them, and the others are counted skipped in
/// its line. Mutant: rounds overlapped or cut short, the skipped ticks
/// not counted or counted in the next round, or the next round waiting
/// for a tick after the end.
#[test]
fn a_round_past_its_ticks_skips_all_but_the_last() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let slow = Slow {
        daemon: FakeDaemon(world.clone()),
        asked: 0,
        nth: 0,
        slow: Duration::from_secs(130),
    };
    let mut agent = agent_on(&world, slow, &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 170_000));
    assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 2, "{out}");
    // Round 1 ran past the ticks at 60 s and 120 s.
    let took = field(rounds[0], "took_ms").parse::<u64>().expect("ms");
    assert!((130_000..180_000).contains(&took), "{out}");
    assert_eq!(field(rounds[0], "skipped"), "1", "{out}");
    // Round 2 stood for the tick at 120 s, as round 1 ended.
    assert_eq!(
        field(rounds[1], "late_ms"),
        (took - 120_000).to_string(),
        "{out}"
    );
    assert_eq!(field(rounds[1], "skipped"), "0", "{out}");
    // Round 3 would be at the tick at 180 s: the end at 170 s came first.
    assert!(mono(&world) <= start + 170_000 + 250);
    assert_eq!(world.borrow().refs.len(), 2);
}

/// The agent's directory in memory, whose `nth` save (from 1) is written
/// and then fails, as a save whose rename landed and whose directory's
/// sync failed: the record on the disk is the new one, and the engine
/// was told it is not.
struct SaveFails {
    keep: MemKeep,
    saves: std::cell::Cell<usize>,
    nth: usize,
}

impl Keep for SaveFails {
    fn load(&self) -> io::Result<Loaded> {
        self.keep.load()
    }
    fn save(&self, record: &Record) -> Result<(), crate::record::SaveError> {
        self.keep.save(record)?;
        self.saves.set(self.saves.get() + 1);
        if self.saves.get() == self.nth {
            return Err(io::Error::other("the directory's sync failed").into());
        }
        Ok(())
    }
    fn write_manifest(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.keep.write_manifest(name, bytes)
    }
    fn read_manifest(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        self.keep.read_manifest(name)
    }
    fn remove_manifest(&self, name: &str) -> io::Result<()> {
        self.keep.remove_manifest(name)
    }
    fn manifest_names(&self) -> io::Result<Vec<String>> {
        self.keep.manifest_names()
    }
    fn marked(&self) -> io::Result<bool> {
        self.keep.marked()
    }
}

/// A round whose save failed after it was written: the round is
/// reported failed, and the next one starts the engine again from the
/// record, as a new process would, so that the publish the failed save
/// recorded is settled and not lost. Mutant: no restart (the engine goes
/// on from its own copy, and the next save writes the publish out of
/// the record), the round not reported, or `run` ended by the error.
#[test]
fn a_failed_round_is_told_and_the_next_starts_again_from_the_record() {
    let world = World::new(Mode::Busy);
    // The first save enrols the pool, the second records the intent.
    let keep = SaveFails {
        keep: MemKeep::default(),
        saves: std::cell::Cell::new(0),
        nth: 2,
    };
    let files = keep.keep.clone();
    let mut agent = Agent::new(
        FakeZfs(world.clone()),
        FakeDaemon(world.clone()),
        keep,
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent");
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 90_000));
    assert_eq!(code, exit::PUBLISHED, "{out}{err}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 2, "{out}");
    assert!(rounds[0].starts_with("round 1 failed "), "{out}");
    assert_eq!(field(rounds[0], "begun"), "-", "{out}");
    assert!(
        err.starts_with("skyzfs-agent: round 1: the record: ")
            && err.contains("the directory's sync failed"),
        "{err}"
    );
    assert!(rounds[1].starts_with("round 2 ok "), "{out}");
    // The publish recorded by the failed save was read from the record
    // and published, beside round 2's own.
    let published: Vec<String> = world.borrow().refs.keys().cloned().collect();
    assert_eq!(published.len(), 2, "{out}");
    assert_ne!(field(rounds[1], "begun"), "-", "{out}");
    let record = match Record::decode(files.0.borrow().record.as_ref().expect("saved")) {
        Ok(record) => record,
        Err(why) => panic!("{why}"),
    };
    assert!(record.under_way.is_empty(), "{record:?}");
}

/// `go` saying no in a round ends it between two rounds of the engine,
/// its line out and what is under way left in the record; saying no
/// while it waits for a tick ends it within [`WAKE`]. Mutant: `go` not
/// asked within the round (the end waits for the next tick), or not
/// while it waits.
#[test]
fn an_end_comes_between_two_rounds_of_the_engine() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 5_000));
    assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 1, "{out}");
    assert!(rounds[0].starts_with("round 1 ok "), "{out}");
    assert_eq!(field(rounds[0], "under_way"), "1", "{out}");
    // Within one round of the engine (a tick) of being told.
    assert!(
        mono(&world) < start + 5_000 + 2_000,
        "{}",
        mono(&world) - start
    );
    assert_eq!(agent.record().under_way.len(), 1);
    assert!(keep.0.borrow().record.is_some());
}

/// A record that a person must judge ends `run` with 6, after the
/// round's line: here a hold of the agent's tag where the directory has
/// no agent's marker. Mutant: `run` going on, retrying each tick.
#[test]
fn an_error_a_person_judges_ends_run() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let right = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &right);
    let (code, _, _) = executed(&mut agent, &Task::Publish, Duration::from_secs(60));
    assert_eq!(code, exit::UNSETTLED);
    let mistyped = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &mistyped);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 600_000));
    assert_eq!(code, exit::RECORD, "{out}{err}");
    assert_eq!(rounds(&out).len(), 1, "{out}");
    assert!(out.starts_with("round 1 failed "), "{out}");
    assert!(err.contains("no agent's marker"), "{err}");
    assert!(mono(&world) < start + 60_000);
}

/// What the agent keeps of publishes that have left the record is
/// forgotten after each round. Mutant: [`Agent::forget`] not called, or
/// keeping a publish's reports after it left.
#[test]
fn what_left_the_record_is_forgotten_after_each_round() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (_, out, _) = scheduled(&mut agent, every, &mut until(&world, start + 150_000));
    assert_eq!(rounds(&out).len(), 3, "{out}");
    assert!(agent.outcomes().is_empty(), "{:?}", agent.outcomes());
    assert!(agent.reports().is_empty(), "{:?}", agent.reports());
}

/// 8 publishes under way: the round's begin is refused and told, the
/// round still settles, and the engine is not started again for it.
/// Mutant: a refused begin keeping the round from settling.
#[test]
fn a_begin_refused_for_eight_under_way_still_settles() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(
        &mut agent,
        every,
        &mut until(&world, start + 9 * 60_000 + 1),
    );
    assert_eq!(code, exit::PUBLISHED, "{out}{err}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 10, "{out}");
    for line in &rounds[..8] {
        assert!(line.contains(" ok "), "{out}");
    }
    assert!(rounds[8].starts_with("round 9 failed "), "{out}");
    assert_eq!(field(rounds[8], "begun"), "-", "{out}");
    assert_eq!(field(rounds[8], "under_way"), "8", "{out}");
    assert!(
        err.contains("skyzfs-agent: round 9: 8 publishes are under way"),
        "{err}"
    );
    // A refused begin changed nothing: the round after it does not start
    // the engine again, and the sweep ran once in all.
    let sweeps = world
        .borrow()
        .calls
        .iter()
        .filter(|c| **c == "tagged")
        .count();
    assert_eq!(sweeps, 1, "{out}");
    // Once the upload comes back, the next round settles the 8 though
    // its own begin is refused, and the round after begins a ninth.
    {
        let mut world = world.borrow_mut();
        world.lag_ms = 0;
        let now = world.now_ms;
        world.daemon.upload_ends_at(now);
    }
    let now = mono(&world);
    let (_, out, _) = scheduled(&mut agent, every, &mut until(&world, now + 90_000));
    let after = out
        .lines()
        .filter(|l| l.starts_with("round "))
        .collect::<Vec<_>>();
    assert_eq!(after.len(), 2, "{out}");
    assert!(after[0].starts_with("round 1 failed "), "{out}");
    assert_eq!(field(after[0], "under_way"), "0", "{out}");
    assert_eq!(field(after[1], "under_way"), "0", "{out}");
    assert_eq!(out.matches("snap/").count(), 9, "{out}");
}

/// A round that begins late, and does not settle, ends at its next tick
/// and not one `every` after it began, so that the round after begins
/// on time. Mutant: a round's limit of `every` from its begin.
#[test]
fn a_late_round_ends_at_its_next_tick() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let slow = Slow {
        daemon: FakeDaemon(world.clone()),
        asked: 0,
        nth: 0,
        slow: Duration::from_secs(70),
    };
    let mut agent = agent_on(&world, slow, &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (_, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 150_000));
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 3, "{out}{err}");
    let ms = |line: &str, key: &str| field(line, key).parse::<u64>().expect("ms");
    // Round 2 began about 10 s after its tick at 60 s, and ended at the
    // tick at 120 s.
    assert!(
        (10_000..=12_000).contains(&ms(rounds[1], "late_ms")),
        "{out}"
    );
    let ended = ms(rounds[1], "late_ms") + ms(rounds[1], "took_ms");
    assert!((60_000..61_000).contains(&ended), "{out}");
    assert!(ms(rounds[2], "late_ms") < 1_000, "{out}");
    assert_eq!(field(rounds[2], "skipped"), "0", "{out}");
}

/// A report is told once in a process, however many rounds its publish
/// stays under way: what a round forgets is only of publishes that left
/// the record. Mutant: the reports of publishes under way forgotten.
#[test]
fn a_report_is_told_once_while_its_publish_is_under_way() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().unreadable = (0..20_000).map(|read| (read, "guid".to_string())).collect();
    let keep = MemKeep::default();
    let reports = Shared::default();
    let mut sink = reports.clone();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep).with_sink(Box::new(
        move |name, report| {
            let _ = sink.write_all(report_line(name, report).as_bytes());
        },
    ));
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (_, out, _) = scheduled(&mut agent, every, &mut until(&world, start + 150_000));
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 3, "{out}");
    let reports = reports.text();
    for round in &rounds {
        let name = field(round, "begun");
        let told = reports.lines().filter(|l| l.contains(name)).count();
        assert_eq!(told, 1, "{name}: {reports}");
    }
    assert_eq!(field(rounds[2], "under_way"), "3", "{out}");
}

/// The program's wiring ([`wired`]): the run's settings reach the agent,
/// each report goes to standard error and each outcome to standard
/// output, as made, and the record is keyed by the volume's id read
/// from its file `remote` (T2 of #149's review: M2, M4, M5 and M8).
#[test]
fn the_wiring_sets_the_agent_as_the_program_runs_it() {
    let dir = tempfile::tempdir().expect("a directory");
    std::fs::write(dir.path().join("remote"), REMOTE).expect("remote");
    let run = Run {
        volume: dir.path().to_path_buf(),
        expiry: Some(Duration::from_secs(120)),
        ..run()
    };
    let volume = volume_of(&run).expect("the volume's id");
    let want = skyzfs_control::unhex::<16>("01563e3ab5d3d6764c61efb99302bd5b").expect("hex");
    assert_eq!(volume, want);
    let missing = Run {
        volume: dir.path().join("none"),
        ..run.clone()
    };
    assert!(volume_of(&missing).is_err_and(|why| why.contains("none/remote")));
    std::fs::write(dir.path().join("remote"), "skyzfs-remote 2\n").expect("remote");
    assert!(volume_of(&run).is_err_and(|why| why.contains("remote: its first line")));

    // A daemon taken over as each request is accepted: the publish is
    // reported at its third stall and given up at its eighth
    // (`a_publish_whose_daemon_is_taken_over_at_each_request_is_given_up`).
    let world = World::new(Mode::Idle);
    let keep = MemKeep::default();
    let (out, err) = (Shared::default(), Shared::default());
    let parts = (
        FakeZfs(world.clone()),
        FakeDaemon(world.clone()),
        keep.clone(),
        FakeClock(world.clone()),
    );
    let held = Rc::new(RefCell::new(Vec::new()));
    let hook = {
        let (world, keep, held) = (world.clone(), keep.clone(), held.clone());
        move |point: Point, _: &str| {
            let asked = Point {
                effect: Effect::Asked,
                after: true,
            };
            if point == asked {
                if let Ok(Loaded::Present(record)) = keep.load() {
                    if let Phase::Asked {
                        asked_ms,
                        held_until_ms,
                        ..
                    } = record.under_way[0].phase
                    {
                        held.borrow_mut().push(held_until_ms - asked_ms);
                    }
                }
                world.borrow_mut().restart(Restart::TakeOver, None);
            }
            false
        }
    };
    let mut agent = wired(
        &run,
        volume,
        parts,
        Box::new(out.clone()),
        Box::new(err.clone()),
    )
    .expect("the agent")
    .with_points(Box::new(hook));
    let name = agent.begin().expect("begun");
    agent.settle(LIMIT).expect("settled");
    // The expiry (M2).
    let margin = u64::try_from(crate::plan::HOLD_MARGIN.as_millis()).expect("ms");
    assert!(!held.borrow().is_empty());
    assert!(
        held.borrow().iter().all(|&h| h == 120_000 + margin),
        "{:?}",
        held.borrow()
    );
    // The report on standard error, the outcome on standard output (M4,
    // M5).
    let stalls = crate::plan::STALLS_REPORTED;
    assert_eq!(err.text(), report_line(&name, &Report::NoProgress(stalls)));
    assert_eq!(out.text(), format!("abandoned {name} no-progress\n"));
    // The record keyed by the volume's id (M8).
    let record = match Record::decode(keep.0.borrow().record.as_ref().expect("saved")) {
        Ok(record) => record,
        Err(why) => panic!("{why}"),
    };
    assert_eq!(record.volume, want);
}

/// Each outcome line is out before the effects that follow it: a run
/// stopped (as by a kill) as its publish leaves the record has printed
/// the publish's line, and one stopped before has not (S1 of #149's
/// review: the record holds nothing of a publish that has left it).
/// Mutant: the lines printed at the end of `execute`, or a publish's
/// told after its record's save.
#[test]
fn an_outcome_is_out_before_the_record_forgets_it() {
    for (effect, after, told) in [
        (Effect::Done, true, false),
        (Effect::Forget, false, true),
        (Effect::Forget, true, true),
        (Effect::Unlink, false, true),
    ] {
        let world = World::new(Mode::Busy);
        let keep = MemKeep::default();
        let at = Point { effect, after };
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep)
            .with_points(Box::new(move |point, _| point == at));
        let (code, out, err) = executed(&mut agent, &Task::Publish, LIMIT);
        assert_eq!(code, exit::ERROR, "{effect:?} {after}: {err}");
        assert_eq!(out.starts_with("snap/"), told, "{effect:?} {after}: {out}");
        // A run that follows tells it where the record still holds it,
        // and not again where it has left: at least once.
        let mut next = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let (_, again, _) = executed(&mut next, &Task::Settle, LIMIT);
        let left = !(effect == Effect::Done || (effect == Effect::Forget && !after));
        assert_eq!(
            again.starts_with("snap/"),
            !left,
            "{effect:?} {after}: {again}"
        );
    }
    // A give-up is told once its record is saved, before its manifest
    // goes. A stop between the two (`Abandon:after`) has not told it,
    // and no later run tells it: the record keeps it among the
    // abandoned, which `record` prints (the window the module's docs
    // name in "`run`").
    for (effect, after, told) in [
        (Effect::Abandon, false, false),
        (Effect::Abandon, true, false),
        (Effect::Unlink, false, true),
    ] {
        let world = World::new(Mode::Busy);
        world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
        let keep = MemKeep::default();
        let mut first = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let (code, _, _) = executed(&mut first, &Task::Publish, Duration::from_secs(60));
        assert_eq!(code, exit::UNSETTLED);
        let t = first.record().under_way[0].name.clone();
        drop(first);
        let at = Point { effect, after };
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep)
            .with_points(Box::new(move |point, _| point == at));
        let (code, out, err) =
            executed(&mut agent, &Task::Recover(t.clone(), Choice::GiveUp), LIMIT);
        assert_eq!(code, exit::ERROR, "{effect:?} {after}: {out}{err}");
        assert_eq!(
            out.starts_with("abandoned "),
            told,
            "{effect:?} {after}: {out}{err}"
        );
        let kept = agent.record().abandoned.iter().any(|a| a.name == t);
        assert_eq!(
            kept,
            after || effect == Effect::Unlink,
            "{effect:?} {after}"
        );
    }
}
