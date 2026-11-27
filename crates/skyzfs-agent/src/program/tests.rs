//! The program's command line, output and exit codes, and the program
//! run on the engine's fakes.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use skyzfs_control::{Code, Reply, Request};

use super::*;
use crate::engine::fake::{
    Back, FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, Restart, User, World, POOL, VOLUME,
};
use crate::engine::Noted;
use crate::record::{Abandoned, Count, Enrolled, Member, Publish, Published, Record, Stuck, Taken};

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
            every: every(60),
            kept: Kept::default(),
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
            every: every(5),
            kept: Kept::default(),
        })
    );
    assert_eq!(
        parse(
            &args(&format!("run {COMMON} --keep 1000 --abandoned-keep-s 7")),
            false
        ),
        Ok(Command::Run {
            run: run(),
            every: every(60),
            kept: Kept {
                keep: 1000,
                abandoned_keep: every(7),
            },
        })
    );
    assert_eq!(Kept::default().keep, 60);
    assert_eq!(Kept::default().abandoned_keep, every(24 * 60 * 60));
    let rows = [
        ("--limit-s 30", "run takes no option --limit-s"),
        ("--every-s 0", "at least 1"),
        ("--every-s 1.5", "at least 1"),
        ("--expiry-s 999999", "--expiry-s: at most"),
        ("--name t", "run takes no option --name"),
        ("--give-up", "run takes no option --give-up"),
        (
            "--keep 0",
            "--keep: \"0\" is not a whole number from 1 to 1000",
        ),
        ("--keep 1001", "from 1 to 1000"),
        ("--keep x", "from 1 to 1000"),
        ("--abandoned-keep-s 0", "at least 1"),
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
        abandoned: vec![
            Abandoned {
                name: "skz-z".into(),
                why: Why::NameTaken,
                at_ms: 500,
                left: Left::Nothing,
            },
            Abandoned {
                name: "skz-y".into(),
                why: Why::Person,
                at_ms: 501,
                left: Left::Unknown,
            },
            Abandoned {
                name: "skz-x".into(),
                why: Why::TooOld,
                at_ms: 502,
                left: Left::Members(vec![member("tank", 1), member("tank/a", 2)]),
            },
        ],
        published: [
            None,
            Some(Retire::Ref),
            Some(Retire::Members),
            Some(Retire::Other),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, retire)| Published {
            name: format!("skz-p{i}"),
            commit: [0xc1; 32],
            group: 60 + i as u64,
            members: vec![member("tank", 3); i],
            at_ms: 600 + i as u64,
            retire,
        })
        .collect(),
        stuck: vec![Stuck {
            name: "skz-s".into(),
            members: vec![member("tank", 4), member("tank/a", 5)],
            at_ms: 700,
        }],
    };
    let h = "4d".repeat(32);
    let c = "c0".repeat(32);
    let p = "c1".repeat(32);
    let want = format!(
        "volume {}\n\
         enrolled tank pool_guid=11 leaf_guid=12\n\
         publish skz-a phase=intent count=5 group=- stalls=1 attempts=2 held=no asked_ms=- held_until_ms=- manifest=- commit=- person=- begun_ms=1000\n\
         publish skz-b phase=held count=5 group=40 stalls=1 attempts=2 held=yes asked_ms=- held_until_ms=- manifest={h} commit=- person=give-up begun_ms=1000\n\
         publish skz-c phase=asked count=5 group=41 stalls=1 attempts=2 held=yes asked_ms=2000 held_until_ms=3000 manifest={h} commit=- person=stopped begun_ms=1000\n\
         publish skz-d phase=done count=5 group=42 stalls=1 attempts=2 held=yes asked_ms=- held_until_ms=- manifest={h} commit={c} person=- begun_ms=1000\n\
         abandoned skz-z why=name-taken at_ms=500 left=nothing\n\
         abandoned skz-y why=person at_ms=501 left=unknown\n\
         abandoned skz-x why=too-old at_ms=502 left=members:2\n\
         published skz-p0 commit={p} group=60 members=0 at_ms=600 retire=-\n\
         published skz-p1 commit={p} group=61 members=1 at_ms=601 retire=ref\n\
         published skz-p2 commit={p} group=62 members=2 at_ms=602 retire=members\n\
         published skz-p3 commit={p} group=63 members=3 at_ms=603 retire=other\n\
         stuck skz-s members=2 at_ms=700\n",
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

/// A member `name` whose snapshot's GUID is `guid`.
fn member(name: &str, guid: u64) -> Member {
    Member {
        name: name.into(),
        snapshot_guid: guid,
    }
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

    fn failure(&mut self) -> io::Result<Option<Noted>> {
        self.0.failure()
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
        synced: 4,
        bound: false,
        early_ms: None,
        reopen_held: false,
    };
    assert_eq!(
        round_line(&round),
        "round 3 ok took_ms=3456 late_ms=12 skipped=2 begun=skz-20261126T120000Z under_way=1 \
         synced=4 bound=0\n"
    );
    let failed = Round {
        failed: true,
        begun: None,
        bound: true,
        ..round
    };
    assert_eq!(
        round_line(&failed),
        "round 3 failed took_ms=3456 late_ms=12 skipped=2 begun=- under_way=1 synced=4 bound=1\n"
    );
    // A round begun early, and one whose reopen the cap held back: the
    // two fields come last, only where they apply.
    let early = Round {
        late_ms: 0,
        early_ms: Some(55_000),
        reopen_held: true,
        ..failed
    };
    assert_eq!(
        round_line(&early),
        "round 3 failed took_ms=3456 late_ms=0 skipped=2 begun=- under_way=1 synced=4 bound=1 \
         early_ms=55000 reopen=held\n"
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
    fn failure(&mut self) -> io::Result<Option<Noted>> {
        self.daemon.failure()
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

/// The agent's directory in memory, whose first save that `fails` picks
/// (by its number from 1 and the record it saves) is written and then
/// fails, as a save whose rename landed and whose directory's sync
/// failed: the record on the disk is the new one, and the engine was
/// told it is not. `failed` is the record of that save.
struct SaveFails {
    keep: MemKeep,
    saves: std::cell::Cell<usize>,
    fails: fn(usize, &Record) -> bool,
    failed: RefCell<Option<Record>>,
}

impl SaveFails {
    fn new(fails: fn(usize, &Record) -> bool) -> Self {
        SaveFails {
            keep: MemKeep::default(),
            saves: std::cell::Cell::new(0),
            fails,
            failed: RefCell::new(None),
        }
    }
}

impl Keep for SaveFails {
    fn load(&self) -> io::Result<Loaded> {
        self.keep.load()
    }
    fn save(&self, record: &Record) -> Result<(), crate::record::SaveError> {
        self.keep.save(record)?;
        self.saves.set(self.saves.get() + 1);
        if self.failed.borrow().is_none() && (self.fails)(self.saves.get(), record) {
            *self.failed.borrow_mut() = Some(record.clone());
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
    let keep = SaveFails::new(|n, _| n == 2);
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

/// A begin whose save of the new intent failed after it was written,
/// while another publish is under way, restarts the engine at once,
/// before the round settles the other: settled from the engine's own
/// copy, the other's next save would write the new intent out of the
/// record on the disk, and the next round's restart would not find it
/// (P2 of Codex on #167). Each publish recorded is published. Mutant:
/// the restart left to the next round, after the settling.
#[test]
fn a_begin_whose_save_failed_restarts_before_the_round_settles() {
    let world = World::new(Mode::Busy);
    // Round 1's publish is still under way when round 2 begins its own.
    world.borrow_mut().lag_ms = 90_000;
    let keep = SaveFails::new(|_, record| record.under_way.len() == 2);
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
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 400_000));
    assert_eq!(code, exit::PUBLISHED, "{out}{err}");
    let (_, _, keep, _) = agent.parts();
    let failed = keep.failed.borrow().clone().expect("a save failed");
    let rounds = rounds(&out);
    assert!(rounds[1].starts_with("round 2 failed "), "{out}");
    assert!(
        err.starts_with("skyzfs-agent: round 2: the record: ")
            && err.contains("the directory's sync failed"),
        "{err}"
    );
    // Both publishes of the failed save were published.
    for publish in &failed.under_way {
        let told = format!("snap/{} ", publish.name);
        assert!(
            out.lines().any(|line| line.starts_with(&told)),
            "{} not told: {out}",
            publish.name
        );
    }
}

/// A Zfs whose count takes `slow` on the clock, as on a large pool.
struct SlowCount {
    zfs: FakeZfs,
    slow: Duration,
}

impl crate::zfs::Zfs for SlowCount {
    fn layout(&mut self, pool: &str) -> Result<crate::zfs::Layout, crate::zfs::ZfsError> {
        self.zfs.layout(pool)
    }
    fn props(&mut self, pool: &str) -> Result<crate::zfs::Props, crate::zfs::ZfsError> {
        self.zfs.props(pool)
    }
    fn nocacheflush(&mut self) -> Result<bool, crate::zfs::ZfsError> {
        self.zfs.nocacheflush()
    }
    fn count(&mut self, pool: &str) -> Result<u64, crate::zfs::ZfsError> {
        self.zfs.0.borrow_mut().advance(self.slow);
        self.zfs.count(pool)
    }
    fn snapshot(&mut self, pool: &str, t: &str) -> Result<(), crate::zfs::ZfsError> {
        self.zfs.snapshot(pool, t)
    }
    fn datasets(
        &mut self,
        pool: &str,
        t: &str,
    ) -> Result<Vec<crate::zfs::Dataset>, crate::zfs::ZfsError> {
        self.zfs.datasets(pool, t)
    }
    fn destroy(&mut self, pool: &str, t: &str) -> Result<(), crate::zfs::ZfsError> {
        self.zfs.destroy(pool, t)
    }
    fn destroy_one(
        &mut self,
        pool: &str,
        dataset: &str,
        t: &str,
    ) -> Result<(), crate::zfs::ZfsError> {
        self.zfs.destroy_one(pool, dataset, t)
    }
    fn members(
        &mut self,
        pool: &str,
        t: &str,
    ) -> Result<Option<Vec<Member>>, crate::zfs::ZfsError> {
        self.zfs.members(pool, t)
    }
    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), crate::zfs::ZfsError> {
        self.zfs.hold(pool, t, tag)
    }
    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), crate::zfs::ZfsError> {
        self.zfs.release(pool, t, tag)
    }
    fn held(
        &mut self,
        pool: &str,
        t: &str,
        tag: &str,
        members: Option<&[Member]>,
    ) -> Result<crate::zfs::Held, crate::zfs::ZfsError> {
        self.zfs.held(pool, t, tag, members)
    }
    fn tagged(
        &mut self,
        pool: &str,
        tag: &str,
    ) -> Result<std::collections::BTreeSet<String>, crate::zfs::ZfsError> {
        self.zfs.tagged(pool, tag)
    }
    fn assist(&mut self, pool: &str, value: u64) -> Result<(), crate::zfs::ZfsError> {
        self.zfs.assist(pool, value)
    }

    fn reopen(&mut self, pool: &str) -> Result<(), crate::zfs::ZfsError> {
        self.zfs.reopen(pool)
    }

    fn pool_state(
        &mut self,
        pool: &str,
    ) -> Result<Option<crate::zfs::PoolState>, crate::zfs::ZfsError> {
        self.zfs.pool_state(pool)
    }
}

/// A round settles at least one round of the engine however long the
/// count before it took: with `--every-s 1` and a count of 2 s, the
/// limit has passed before the publishes are looked at, and they would
/// never move (P2 of Codex on #167). Mutant: the limit asked before the
/// first pass.
#[test]
fn a_round_settles_once_however_long_the_count_took() {
    let world = World::new(Mode::Busy);
    let keep = MemKeep::default();
    let mut agent = Agent::new(
        SlowCount {
            zfs: FakeZfs(world.clone()),
            slow: Duration::from_secs(2),
        },
        FakeDaemon(world.clone()),
        keep,
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent");
    let start = mono(&world);
    let (mut out, mut err) = (Shared::default(), Shared::default());
    let code = schedule(
        &mut agent,
        Duration::from_secs(1),
        &mut until(&world, start + 120_000),
        &mut out,
        &mut err,
    );
    let (out, err) = (out.text(), err.text());
    assert_eq!(code, exit::PUBLISHED, "{out}{err}");
    assert!(
        world.borrow().refs.len() >= 2,
        "{} published: {out}{err}",
        world.borrow().refs.len()
    );
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

/// An end asked for while a begin works waits for that begin and no
/// more: `go` is asked before the settle's count and sweep, not after
/// them, so that neither runs once the begin is done (n1 of the next
/// look at #167; the lead's ruling). Mutant: `go` asked only after the
/// count and the sweep that open a pass.
#[test]
fn an_end_during_a_begin_makes_no_read_after_it() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().lag_ms = 365 * 24 * 3_600_000;
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    // Yes at the round's start, no from then on: as a SIGTERM that
    // comes while the begin works.
    let mut asked = 0u32;
    let mut go = || {
        asked += 1;
        asked == 1
    };
    let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
    assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 1, "{out}");
    assert_ne!(field(rounds[0], "begun"), "-", "{out}");
    let calls = world.borrow().calls.clone();
    let counts = calls.iter().filter(|c| **c == "count").count();
    let sweeps = calls.iter().filter(|c| **c == "tagged").count();
    assert_eq!((counts, sweeps), (1, 1), "the begin's own only: {calls:?}");
    assert_eq!(agent.record().under_way.len(), 1);
}

/// An idle pool imported with `-T`, its abandoned history's top `gap`
/// groups above the group the import wrote.
fn rewound_t(gap: u64) -> Rc<RefCell<World>> {
    let world = World::new(Mode::Idle);
    {
        let mut world = world.borrow_mut();
        for _ in 0..4 {
            world.sync("before the rewind");
        }
        world.restart(Restart::RewindT(Back::AfterHold), None);
        let pool = world.pool.as_mut().expect("the pool");
        pool.abandoned_top = pool.txg + gap;
    }
    world
}

/// After a rewind import, a round syncs the pool past the abandoned
/// groups before it begins its publish, which is then published within
/// the round; the round line says how many syncs (M2.7d). Mutants: no
/// syncs before the begin; `synced` not in the line.
#[test]
fn a_round_after_a_rewind_import_syncs_past_it_before_it_begins() {
    let world = rewound_t(5);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 90_000));
    assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 2, "{out}");
    assert!(rounds[0].starts_with("round 1 ok "), "{out}");
    assert_eq!(field(rounds[0], "synced"), "3", "{out}");
    assert_ne!(field(rounds[0], "begun"), "-", "{out}");
    assert_eq!(field(rounds[1], "synced"), "0", "{out}");
    for round in &rounds {
        assert_eq!(field(round, "bound"), "0", "{out}");
    }
    assert_eq!(world.borrow().refs.len(), 2, "{out}");
}

/// An end asked during the syncs begins no publish, even where the last
/// sync made the newest cut open as itself: the begin would be a new
/// step after the end, and would leave an intent for the next process
/// (Astra's P2 on #170). Mutant: no `go` asked between the syncs and the
/// begin.
#[test]
fn an_end_during_the_syncs_begins_no_publish() {
    let world = rewound_t(5);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let watched = world.clone();
    let mut go = move || watched.borrow().synced_at.len() < 3;
    let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
    assert_eq!((code, err.as_str()), (exit::PUBLISHED, ""), "{out}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 1, "{out}");
    assert!(rounds[0].starts_with("round 1 ok "), "{out}");
    assert_eq!(field(rounds[0], "synced"), "3", "{out}");
    assert_eq!(field(rounds[0], "begun"), "-", "{out}");
    assert_eq!(field(rounds[0], "under_way"), "0", "{out}");
    assert_eq!(field(rounds[0], "bound"), "0", "{out}");
    assert_eq!(world.borrow().synced_at.len(), 3, "{out}");
    assert!(world.borrow().programs.is_empty(), "{out}");
    assert!(keep.0.borrow().record.is_none());
}

/// A round whose sync made no cut begins no publish and is `ok`, and its
/// line does not say the bound was met (`bound=0`): only a round stopped
/// at the bound says so (the lead's ruling 3 on #170). Mutant: `bound`
/// set for every cause a round is behind.
#[test]
fn a_round_whose_sync_made_no_cut_is_not_at_the_bound() {
    let world = rewound_t(5);
    world.borrow_mut().plain_sync = true;
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let (code, out, err) = scheduled(
        &mut agent,
        Duration::from_secs(60),
        &mut until(&world, start + 30_000),
    );
    assert_eq!(code, exit::PUBLISHED, "{out}{err}");
    let rounds = rounds(&out);
    assert!(!rounds.is_empty(), "{out}");
    for round in &rounds {
        assert!(round.contains(" ok "), "{out}{err}");
        assert_eq!(field(round, "synced"), "1", "{out}");
        assert_eq!(field(round, "begun"), "-", "{out}");
        assert_eq!(field(round, "bound"), "0", "{out}");
    }
    assert!(err.contains("round 1: no publish begun"), "{err}");
    assert!(world.borrow().programs.is_empty(), "{out}");
}

/// A round whose sync failed after others returned is `failed`, begins
/// nothing, and its line counts the syncs that returned (L2 of the
/// review of #170).
#[test]
fn a_round_whose_sync_failed_counts_the_syncs_before_it() {
    let world = rewound_t(10);
    {
        let mut world = world.borrow_mut();
        world.assist_fails = 1;
        world.assist_fails_after = 2;
    }
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let (_code, out, err) = scheduled(
        &mut agent,
        Duration::from_secs(60),
        &mut until(&world, start + 30_000),
    );
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 1, "{out}");
    assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
    assert!(
        err.contains("round 1: zfs: cannot set property: refused"),
        "{err}"
    );
    assert_eq!(field(rounds[0], "synced"), "2", "{out}");
    assert_eq!(field(rounds[0], "begun"), "-", "{out}");
    assert_eq!(world.borrow().synced_at.len(), 2, "{out}");
}

/// A round whose syncs met a pool its premise refuses begins nothing
/// and syncs nothing: one error, the premise's (R9 of the review of
/// #170).
#[test]
fn a_round_whose_syncs_were_refused_begins_nothing() {
    let world = rewound_t(5);
    world.borrow_mut().user(User::Attach, "");
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let (_code, out, err) = scheduled(
        &mut agent,
        Duration::from_secs(60),
        &mut until(&world, start + 30_000),
    );
    let rounds = rounds(&out);
    assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
    assert_eq!(field(rounds[0], "begun"), "-", "{out}");
    assert_eq!(
        err.lines().filter(|l| l.contains("round 1: ")).count(),
        1,
        "{err}"
    );
    assert_eq!(world.borrow().synced_at.len(), 0, "{out}");
    assert!(world.borrow().programs.is_empty(), "{out}");
}

/// With the daemon unread at the syncs, the round still begins, and its
/// begin says what is wrong (R3 of the review of #170).
#[test]
fn a_round_with_the_daemon_unread_still_begins() {
    let world = World::new(Mode::Busy);
    world.borrow_mut().away_until = u64::MAX;
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let (_code, out, err) = scheduled(
        &mut agent,
        Duration::from_secs(60),
        &mut until(&world, start + 30_000),
    );
    assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
    assert!(err.contains("round 1: "), "{err}");
    assert!(!err.contains("no publish begun"), "{err}");
}

/// A round still behind settles what is under way (R20 of the review of
/// #170).
#[test]
fn a_round_still_behind_still_settles() {
    let world = rewound_t(10_000);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let (_code, out, _err) = scheduled(
        &mut agent,
        Duration::from_secs(60),
        &mut until(&world, start + 30_000),
    );
    assert_eq!(field(rounds(&out)[0], "begun"), "-", "{out}");
    let calls = world.borrow().calls.clone();
    assert!(
        calls.contains(&"count"),
        "the settle did not run: {calls:?}"
    );
}

/// A round whose syncs stop with the newest cut still behind begins no
/// publish, says why on standard error, and is `ok`: nothing failed, and
/// no cut could carry a publish (M2.7d). The rounds after it sync no
/// more in this process. Mutants: a begin after the syncs stopped; the
/// round `failed`; the bound per round.
#[test]
fn a_round_still_behind_begins_no_publish() {
    let world = rewound_t(10_000);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let every = Duration::from_secs(60);
    let (code, out, err) = scheduled(&mut agent, every, &mut until(&world, start + 90_000));
    assert_eq!(code, exit::PUBLISHED, "{out}{err}");
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 2, "{out}");
    let max = crate::engine::SYNC_PAST_MAX.to_string();
    for (round, synced) in rounds.iter().zip([max.as_str(), "0"]) {
        assert!(round.contains(" ok "), "{out}");
        assert_eq!(field(round, "begun"), "-", "{out}");
        assert_eq!(field(round, "synced"), synced, "{out}");
        assert_eq!(field(round, "under_way"), "0", "{out}");
        assert_eq!(field(round, "bound"), "1", "{out}");
    }
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 2, "{err}");
    assert!(
        lines[0].starts_with("skyzfs-agent: round 1: no publish begun: the newest cut, of group "),
        "{err}"
    );
    assert!(
        lines[0]
            .ends_with("does not open as itself: the syncs reached their bound in this process"),
        "{err}"
    );
    assert!(world.borrow().programs.is_empty(), "{out}");
    assert!(keep.0.borrow().record.is_none());
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
        err.contains("skyzfs-agent: round 9: no room in the record: 8 publishes under way"),
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
    // A give-up is recorded at `Abandoning`, then told, then moved to the
    // names given up, then its manifest goes (M2.7b-2). A stop before it
    // is told leaves it under way at `Abandoning`, and the next run tells
    // it; a stop after it is told and before the move tells it again:
    // at least once, never lost.
    for (effect, after, told) in [
        (Effect::Abandoning, false, false),
        (Effect::Abandoning, true, false),
        (Effect::Abandon, false, true),
        (Effect::Abandon, true, true),
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
        let moved = effect == Effect::Unlink || (effect == Effect::Abandon && after);
        assert_eq!(kept, moved, "{effect:?} {after}");
        // The next run tells it where the record still holds it under
        // way, and not where it has moved.
        let mut next = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let (_, again, _) = executed(&mut next, &Task::Settle, LIMIT);
        assert_eq!(
            again.contains(&format!("abandoned {t} ")),
            !moved,
            "{effect:?} {after}: {again}"
        );
    }
}

/// A round's restart: after a settle's error, at the next round; one
/// that fails; and one that finds a record a person must judge. These
/// came from the review of #167 (its RC, RF, RI and RJ), which found
/// that no test held them.
/// The daemon unread after two syncs that returned, and an end asked
/// during the second (L3 of the review of #172, its tests S1 and S1b):
/// the round line counts both syncs, and nothing of a begin runs after
/// the end was asked. Mutants: `Unseen` taken as a round with no syncs;
/// a begin with no `go` asked after it.
#[test]
fn a_round_whose_daemon_went_away_after_syncs_counts_them_and_begins_nothing_after_an_end() {
    let world = rewound_t(10);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let watched = world.clone();
    let mut asked = 0u32;
    let ended_at = Rc::new(RefCell::new(None));
    let at = ended_at.clone();
    let mut go = move || {
        asked += 1;
        if asked == 3 {
            // Asked before the second sync: the daemon goes away while
            // it runs, and an end is asked during it.
            watched.borrow_mut().away_until = u64::MAX;
            *at.borrow_mut() = Some(watched.borrow().calls.len());
            return true;
        }
        asked < 3
    };
    let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
    let rounds = rounds(&out);
    assert_eq!(world.borrow().synced_at.len(), 2, "{out}{err}");
    assert_eq!(rounds.len(), 1, "{out}{err}");
    assert_eq!(field(rounds[0], "synced"), "2", "{out}{err}");
    assert_eq!(field(rounds[0], "begun"), "-", "{out}{err}");
    let from = (*ended_at.borrow()).expect("asked three times");
    let calls = world.borrow().calls.clone();
    let after = &calls[from..];
    // The second sync's own calls are allowed; a begin's count or sweep
    // is not.
    assert!(
        !after.contains(&"count") && !after.contains(&"tagged"),
        "calls after the end was asked: {after:?}"
    );
}

/// A round stopped by an end before its first sync is not at the bound
/// (the review of #172, S2). Mutant: `bound=1` for `Stuck::Told`.
#[test]
fn a_round_stopped_by_an_end_is_not_at_the_bound() {
    let world = rewound_t(5);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let mut asked = 0u32;
    let mut go = move || {
        asked += 1;
        asked < 2
    };
    let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 1, "{out}{err}");
    assert!(err.contains("an end was asked for"), "{err}");
    assert_eq!(field(rounds[0], "synced"), "0", "{out}");
    assert_eq!(field(rounds[0], "bound"), "0", "{out}");
}

/// A round whose sync failed is not at the bound (the review of #172,
/// S3). Mutant: `bound=1` for a failed sync.
#[test]
fn a_round_whose_sync_failed_is_not_at_the_bound() {
    let world = rewound_t(5);
    world.borrow_mut().assist_fails = 1;
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let mut asked = 0u32;
    let mut go = move || {
        asked += 1;
        asked <= 2
    };
    let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
    let rounds = rounds(&out);
    assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
    assert_eq!(field(rounds[0], "bound"), "0", "{out}");
}

/// A round after one at the bound, where the daemon then holds no cut,
/// is not at the bound (the review of #172, S4). Mutant: `bound` kept
/// from the round before.
#[test]
fn a_round_after_the_bound_that_finds_nothing_behind_is_not_at_it() {
    let world = rewound_t(10_000);
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let watched = world.clone();
    let mut cleared = false;
    let mut go = move || {
        let now = mono(&watched);
        if !cleared && now >= start + 60_000 {
            cleared = true;
            watched.borrow_mut().daemon.cuts.clear();
        }
        now < start + 90_000
    };
    let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 2, "{out}{err}");
    assert_eq!(field(rounds[0], "bound"), "1", "{out}");
    assert_eq!(field(rounds[1], "bound"), "0", "{out}{err}");
}

/// A round whose premise is refused before any sync, after a round
/// whose sync failed after two returned, says `synced=0`: the count is
/// the round's, not the round before's (the review of #172, S5).
/// Mutant: `last_synced` reset only where the syncs clear.
#[test]
fn a_refusal_after_a_failed_sync_counts_no_syncs() {
    let world = rewound_t(10);
    {
        let mut world = world.borrow_mut();
        world.assist_fails = 1;
        world.assist_fails_after = 2;
    }
    let keep = MemKeep::default();
    let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
    let start = mono(&world);
    let watched = world.clone();
    let mut attached = false;
    let mut go = move || {
        let now = mono(&watched);
        if !attached && now >= start + 60_000 {
            attached = true;
            watched.borrow_mut().user(User::Attach, "");
        }
        now < start + 90_000
    };
    let (_code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
    let rounds = rounds(&out);
    assert_eq!(rounds.len(), 2, "{out}{err}");
    assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
    assert_eq!(field(rounds[0], "synced"), "2", "{out}");
    assert!(rounds[1].starts_with("round 2 failed "), "{out}{err}");
    assert!(err.contains("round 2: "), "{err}");
    assert_eq!(field(rounds[1], "synced"), "0", "{out}{err}");
    assert_eq!(world.borrow().synced_at.len(), 2, "{out}");
}

mod restarts {
    use std::cell::Cell;
    use std::collections::BTreeSet;

    use super::*;
    use crate::record::{Loaded, SaveError};

    /// A directory in memory that fails the first save matching `when`
    /// after writing it (the directory's sync), and the `fail_load`-th
    /// load (from 1), if any.
    struct Flaky {
        keep: MemKeep,
        when: Box<dyn Fn(&Record) -> bool>,
        failed: Rc<RefCell<Option<Record>>>,
        loads: Cell<usize>,
        fail_load: usize,
        spoil: Cell<Spoil>,
    }

    /// What the save that fails leaves on the disk besides the record it
    /// was given.
    #[derive(Clone, Copy)]
    enum Spoil {
        No,
        /// Its last byte flipped: a corrupt record.
        Corrupt,
        /// Another volume's record in its place.
        Volume,
    }

    impl Keep for Flaky {
        fn load(&self) -> io::Result<Loaded> {
            self.loads.set(self.loads.get() + 1);
            if self.loads.get() == self.fail_load {
                return Err(io::Error::other("the record could not be read"));
            }
            self.keep.load()
        }
        fn save(&self, record: &Record) -> Result<(), SaveError> {
            self.keep.save(record)?;
            if self.failed.borrow().is_none() && (self.when)(record) {
                *self.failed.borrow_mut() = Some(record.clone());
                let mut files = self.keep.0.borrow_mut();
                match self.spoil.get() {
                    Spoil::No => {}
                    Spoil::Corrupt => {
                        if let Some(b) = files.record.as_mut().and_then(|r| r.last_mut()) {
                            *b ^= 0xff;
                        }
                    }
                    Spoil::Volume => {
                        let other = Record::new(OTHER)
                            .encode()
                            .expect("an empty record encodes");
                        files.record = Some(other);
                    }
                }
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

    fn flaky(
        when: Box<dyn Fn(&Record) -> bool>,
        fail_load: usize,
    ) -> (Flaky, MemKeep, Rc<RefCell<Option<Record>>>) {
        let failed = Rc::new(RefCell::new(None));
        let keep = MemKeep::default();
        (
            Flaky {
                keep: keep.clone(),
                when,
                failed: failed.clone(),
                loads: Cell::new(0),
                fail_load,
                spoil: Cell::new(Spoil::No),
            },
            keep,
            failed,
        )
    }

    fn sched_any<Z: Zfs, D: Daemon, K: Keep>(
        agent: &mut Agent<Z, D, K, FakeClock>,
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

    /// A begin whose save failed with another publish
    /// under way, and a restart at once that cannot read the record: the
    /// round settles nothing on what the process holds (a save of it
    /// would write the new intent out), and the next round starts again
    /// and publishes both.
    #[test]
    fn a_restart_that_fails_leaves_the_round_alone() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().lag_ms = 90_000;
        // Loads: 1 at Agent::new, 2 at the restart after the failed save
        // (fails), 3 at the next round's.
        let (keep, _files, failed) = flaky(Box::new(|r: &Record| r.under_way.len() == 2), 2);
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
        let (code, out, err) = sched_any(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 400_000),
        );
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        let failed = failed.borrow().clone().expect("a save failed");
        let rounds = rounds(&out);
        assert!(rounds[1].starts_with("round 2 failed "), "{out}");
        assert_eq!(field(rounds[1], "begun"), "-", "{out}");
        assert!(
            err.contains("round 2: the record's directory: the record could not be read"),
            "{err}"
        );
        for publish in &failed.under_way {
            let told = format!("snap/{} ", publish.name);
            assert!(
                out.lines().any(|l| l.starts_with(&told)),
                "{} not told:\n{out}{err}",
                publish.name
            );
        }
    }

    /// An agent on `world` whose directory fails its `fail_load`-th load
    /// (0: none) and saves always.
    fn loading(
        world: &Rc<RefCell<World>>,
        fail_load: usize,
    ) -> Agent<FakeZfs, FakeDaemon, Flaky, FakeClock> {
        let (keep, _files, _failed) = flaky(Box::new(|_: &Record| false), fail_load);
        Agent::new(
            FakeZfs(world.clone()),
            FakeDaemon(world.clone()),
            keep,
            FakeClock(world.clone()),
            VOLUME,
            POOL,
        )
        .expect("the agent")
    }

    /// An error of the syncs after a rewind import reads the record
    /// again at once, as a begin's does (R8 of the review of #170).
    #[test]
    fn an_error_of_the_syncs_reads_the_record_again() {
        let world = rewound_t(5);
        world.borrow_mut().assist_fails = 1;
        let mut agent = loading(&world, 0);
        assert_eq!(agent.parts().2.loads.get(), 1);
        let mut asked = 0u32;
        let mut go = || {
            asked += 1;
            asked <= 3
        };
        let (_code, out, err) = sched_any(&mut agent, Duration::from_secs(60), &mut go);
        assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
        assert_eq!(agent.parts().2.loads.get(), 2, "{out}{err}");
    }

    /// A refusal of the pool's premise at the syncs reads the record no
    /// more than a begin's refusal does (R12 of the review of #170).
    #[test]
    fn a_refusal_at_the_syncs_reads_the_record_once() {
        let world = rewound_t(5);
        world.borrow_mut().user(User::Attach, "");
        let mut agent = loading(&world, 0);
        let mut asked = 0u32;
        let mut go = || {
            asked += 1;
            asked <= 3
        };
        let (_code, out, err) = sched_any(&mut agent, Duration::from_secs(60), &mut go);
        assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
        assert_eq!(agent.parts().2.loads.get(), 1, "{out}{err}");
    }

    /// An error of the syncs whose reading of the record fails settles
    /// nothing in that round (R10 of the review of #170).
    #[test]
    fn a_failed_reading_after_an_error_of_the_syncs_settles_nothing() {
        let world = rewound_t(5);
        world.borrow_mut().assist_fails = 1;
        let mut agent = loading(&world, 2);
        let mut asked = 0u32;
        let mut go = || {
            asked += 1;
            asked <= 3
        };
        let (_code, out, err) = sched_any(&mut agent, Duration::from_secs(60), &mut go);
        assert!(rounds(&out)[0].starts_with("round 1 failed "), "{out}{err}");
        assert!(
            err.contains("round 1: the record's directory: the record could not be read"),
            "{err}"
        );
        let calls = world.borrow().calls.clone();
        assert!(!calls.contains(&"count"), "the settle ran: {calls:?}");
    }

    /// Another volume's.
    const OTHER: [u8; 16] = [0xee; 16];

    /// A record found corrupt, or another volume's, when the engine
    /// starts again (here at once, after a begin whose save failed) ends
    /// `run` with 6, after the round's line.
    #[test]
    fn a_record_found_corrupt_at_a_restart_ends_run() {
        let err = a_record_a_person_judges_at_a_restart(Spoil::Corrupt);
        assert!(err.contains("round 1: the record is corrupt"), "{err}");
    }

    #[test]
    fn a_record_of_another_volume_at_a_restart_ends_run() {
        let err = a_record_a_person_judges_at_a_restart(Spoil::Volume);
        let told = format!(
            "round 1: the record is of volume {}",
            skyzfs_control::hex(&OTHER)
        );
        assert!(err.contains(&told), "{err}");
    }

    /// The run of those two: its standard error.
    fn a_record_a_person_judges_at_a_restart(spoil: Spoil) -> String {
        let world = World::new(Mode::Busy);
        let (keep, _files, _failed) = flaky(Box::new(|r: &Record| !r.under_way.is_empty()), 0);
        keep.spoil.set(spoil);
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
        let (code, out, err) = sched_any(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 600_000),
        );
        assert_eq!(code, exit::RECORD, "{out}{err}");
        let rounds = rounds(&out);
        assert_eq!(rounds.len(), 1, "{out}{err}");
        assert!(rounds[0].starts_with("round 1 failed "), "{out}");
        err
    }

    /// The first save that forgets a publish (the record empty again
    /// after it held one) fails after it was written.
    fn forget_fails() -> Box<dyn Fn(&Record) -> bool> {
        let seen = Cell::new(false);
        Box::new(move |r: &Record| {
            if r.under_way.is_empty() {
                seen.get()
            } else {
                seen.set(true);
                false
            }
        })
    }

    /// A save in the settling that failed after it was
    /// written (the one that forgets a publish); `fail_load` fails the
    /// next round's restart too. Its line is told once.
    fn settle_save_fails(fail_load: usize) -> (String, String) {
        let world = World::new(Mode::Busy);
        let (keep, _files, failed) = flaky(forget_fails(), fail_load);
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
        let (code, out, err) = sched_any(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 200_000),
        );
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        assert!(failed.borrow().is_some(), "no save failed:\n{out}{err}");
        let rounds = rounds(&out);
        assert!(rounds[0].starts_with("round 1 failed "), "{out}{err}");
        let told: Vec<&str> = out.lines().filter(|l| l.starts_with("snap/")).collect();
        let once: BTreeSet<&str> = told.iter().copied().collect();
        assert_eq!(once.len(), told.len(), "a line told twice:\n{out}{err}");
        (out, err)
    }

    /// The next round restarts after a save of the
    /// settling failed.
    #[test]
    fn a_settle_whose_save_failed_restarts_at_the_next_round() {
        let (out, _) = settle_save_fails(0);
        assert!(rounds(&out)[1].starts_with("round 2 ok "), "{out}");
    }

    /// Where that restart fails, the round does nothing
    /// on what the process holds; the next starts again.
    #[test]
    fn a_failed_restart_at_a_round_start_leaves_the_round_alone() {
        let (out, err) = settle_save_fails(2);
        let rounds = rounds(&out);
        assert!(rounds[1].starts_with("round 2 failed "), "{out}");
        assert_eq!(field(rounds[1], "begun"), "-", "{out}");
        assert!(err.contains("round 2: the record's directory: "), "{err}");
    }
}

/// A volume that failed in this boot (M2.7c), and the end of `run` when
/// its device or the daemon's attachment changes (the lead's D4 on
/// #167).
mod failed_volume {
    use skyzfs_control::failure::{Failure, Held, Reason};

    use super::*;
    use crate::engine::fake::Restart;

    fn noted(held: Held) -> Noted {
        Noted::Failed(Failure {
            held,
            reason: Reason::ImageFailed,
            identity: [7; 16],
        })
    }

    fn lost_world(noted: Noted) -> Rc<RefCell<World>> {
        let world = World::new(Mode::Busy);
        {
            let mut w = world.borrow_mut();
            w.noted = Some(noted);
            w.suspends_after_ms = Some(1_050);
        }
        world
    }

    fn run(world: &Rc<RefCell<World>>) -> (i32, String, String) {
        let keep = MemKeep::default();
        let mut agent = agent_on(world, FakeDaemon(world.clone()), &keep);
        let start = mono(world);
        scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(world, start + 600_000),
        )
    }

    /// `run` on a volume lost in this boot makes the pool read, says so
    /// on a `lost` line, then the round line, and ends with 11 once the
    /// pool reads SUSPENDED; it begins nothing (the lead's answers to Q3
    /// and Q4). Mutants: no reopen; the code of an end; the round line
    /// left out; the lost line after the round line; a begin before the
    /// step; the round line's count of what is under way taken before
    /// the step (A-P5 of the review of #173).
    #[test]
    fn run_on_a_lost_volume_makes_the_pool_read_and_ends_with_11() {
        let world = lost_world(noted(Held::Yes));
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert_eq!(
            lines[0],
            format!(
                "lost tank volume={} note=whole reason=image-failed held=yes step=reopened \
                 reopen=ok state=SUSPENDED looked_ms=1250 verdict=lost",
                hex(&VOLUME)
            )
        );
        assert!(lines[1].starts_with("round 1 ok "), "{out}");
        assert_eq!(field(lines[1], "begun"), "-");
        assert_eq!(field(lines[1], "under_way"), "0");
        assert_eq!(world.borrow().reopens, 1);
        assert!(world.borrow().programs.is_empty(), "nothing begun");
        // The round's only call is the reopen: no sync, count, snapshot
        // or settle (P17 of the review of #173).
        assert_eq!(world.borrow().calls, ["reopen"], "{out}");
        assert_eq!(err, "");
    }

    /// Started again on the same note, `run` finds the pool suspended,
    /// does not reopen it, and ends with 11 again. Mutant: a second
    /// reopen.
    #[test]
    fn run_started_again_on_the_same_note_ends_with_11_and_no_reopen() {
        let world = lost_world(noted(Held::Unknown));
        assert_eq!(run(&world).0, exit::LOST);
        let (code, out, _) = run(&world);
        assert_eq!(code, exit::LOST, "{out}");
        let lost = out.lines().next().expect("a line");
        assert!(lost.contains(" held=unknown step=suspended reopen=- state=SUSPENDED "));
        assert!(lost.ends_with(" verdict=lost"), "{lost}");
        assert_eq!(world.borrow().reopens, 1);
    }

    /// A torn note counts as a failure while a client held the device
    /// (the lead's ruling), and the line names it torn. Mutant: a torn
    /// note taken as none.
    #[test]
    fn a_torn_note_is_a_lost_volume_named_torn() {
        let world = lost_world(Noted::NotWhole);
        let (code, out, _) = run(&world);
        assert_eq!(code, exit::LOST, "{out}");
        assert!(out.starts_with(&format!(
            "lost tank volume={} note=torn reason=- held=yes step=reopened",
            hex(&VOLUME)
        )));
        assert_eq!(world.borrow().reopens, 1);
    }

    /// A pool that is not imported is not reopened, and the volume is not
    /// lost for `run`: nothing is suspended, whatever the note says it
    /// held. Each round says the volume failed, touches nothing else, is
    /// not failed, and `run` goes on at its ticks (the lead's ruling on
    /// F4 of the review of #173). Mutants: a reopen of a pool not there;
    /// an end with 11; the round failed; the round going on to a begin;
    /// the watch taking the round's own note for news (rounds closer than
    /// the tick).
    #[test]
    fn a_lost_note_with_no_pool_imported_is_reported_failed_and_run_goes_on() {
        for noted in [noted(Held::Yes), noted(Held::Unknown), Noted::NotWhole] {
            let world = lost_world(noted);
            world.borrow_mut().pool = None;
            let (code, out, err) = run(&world);
            assert_eq!(code, exit::PUBLISHED, "{noted:?}: {out}{err}");
            let rounds = rounds(&out);
            assert_eq!(rounds.len(), 10, "{out}");
            for (i, round) in rounds.iter().enumerate() {
                assert!(round.starts_with(&format!("round {} ok ", i + 1)), "{out}");
                assert_eq!(field(round, "begun"), "-", "{out}");
                assert_eq!(field(round, "late_ms"), "0", "{out}");
            }
            let lost: Vec<&str> = out.lines().filter(|l| l.starts_with("lost ")).collect();
            assert_eq!(lost.len(), 10, "{out}");
            for line in lost {
                assert!(
                    line.ends_with(
                        " step=not-imported reopen=- state=- looked_ms=- verdict=failed"
                    ),
                    "{line}"
                );
            }
            assert!(
                err.contains("round 1: the volume failed in this boot (note=")
                    && err.contains(
                        "and the pool is not imported: nothing is suspended; run goes on"
                    ),
                "{err}"
            );
            assert_eq!(world.borrow().reopens, 0);
            assert!(world.borrow().programs.is_empty(), "nothing begun");
        }
    }

    /// A pool that does not read SUSPENDED within the look is not shown
    /// lost: the round fails, and the step is taken again, until the pool
    /// suspends; then `run` ends with 11 (the lead's ruling on F2 of the
    /// review of #173). The step is taken again [`QUICK_RETRY`] after a
    /// round that failed so, [`QUICK_RETRIES`] times in a row, each such
    /// round marked early and the round of the tick still on its tick;
    /// then at the ticks (the lead's ruling on D1 of the review of #173).
    /// Mutants: an end with 11 on a pool that reads ONLINE; the round not
    /// failed; no second step; no quick retry; more quick retries than
    /// the bound; a quick retry that uses up its tick (round 5 late, or
    /// none at 60 s); `early_ms` left out.
    #[test]
    fn a_pool_not_shown_suspended_is_tried_again_within_seconds_then_at_the_ticks() {
        let world = lost_world(noted(Held::Yes));
        // The reopen does nothing at first: the pool stays ONLINE.
        world.borrow_mut().suspends_after_ms = None;
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let watched = world.clone();
        let mut go = until(&world, start + 600_000);
        let mut asked = move || {
            // From 120 s on, a reopen makes the pool suspend.
            if mono(&watched) >= start + 120_000 {
                watched.borrow_mut().suspends_after_ms = Some(1_050);
            }
            go()
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut asked);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 12, "{out}");
        // Each look lasts LOOK_MAX (10 s): round 1 at 0 s, the quick
        // retries 5 s after each round ends (15, 30 and 45 s), the round
        // of the tick at 60 s, then 120 s, where the pool suspends.
        let early = [None, Some(45_000), Some(30_000), Some(15_000), None];
        for (i, early) in early.iter().enumerate() {
            assert!(
                lines[2 * i].ends_with(
                    " step=reopened reopen=ok state=ONLINE looked_ms=10000 verdict=retry"
                ),
                "{out}"
            );
            let round = lines[2 * i + 1];
            assert!(
                round.starts_with(&format!("round {} failed ", i + 1)),
                "{out}"
            );
            assert_eq!(field(round, "late_ms"), "0", "{out}");
            match early {
                Some(ms) => assert!(round.ends_with(&format!(" early_ms={ms}")), "{out}"),
                None => assert!(!round.contains("early_ms"), "{out}"),
            }
        }
        assert!(
            lines[10].ends_with(" state=SUSPENDED looked_ms=1250 verdict=lost"),
            "{out}"
        );
        assert!(lines[11].starts_with("round 6 ok "), "{out}");
        assert!(!lines[11].contains("early_ms"), "{out}");
        assert!(
            err.contains(
                "round 1: the volume is lost for this boot (note=whole reason=image-failed \
                 held=yes), and the pool was not shown suspended: the next round tries again"
            ),
            "{err}"
        );
        assert_eq!(world.borrow().reopens, 6);
        assert!(world.borrow().programs.is_empty(), "nothing begun");
    }

    /// After [`REOPEN_CAP`] reopens of one note that leave the pool
    /// unsuspended, the pool is reopened only at every [`REOPEN_CAP`]th
    /// round, and the rounds in between read its state only (`step=held`,
    /// `reopen=held`), with no quick retry; it is said once on standard
    /// error. A pool zed suspends meanwhile is found lost at the next
    /// round, held or not (the lead's ruling on the review of #173).
    /// Mutants: no cap; a cap that never reopens again; a held round that
    /// reads nothing; quick retries past the cap; the cap said each round.
    #[test]
    fn reopens_that_leave_the_pool_online_are_capped() {
        let world = lost_world(noted(Held::Yes));
        world.borrow_mut().suspends_after_ms = None;
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let (code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 990_000),
        );
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        let steps: Vec<&str> = lines.iter().step_by(2).copied().collect();
        let rounds: Vec<&str> = lines.iter().skip(1).step_by(2).copied().collect();
        // 4 rounds by 45 s (one and its three quick retries), then one a
        // tick: the 10th reopen at 360 s, held rounds from 420 s to 900 s,
        // the 10th round after the cap (960 s) reopens.
        assert_eq!(rounds.len(), 20, "{out}");
        for (i, (step, round)) in steps.iter().zip(&rounds).enumerate() {
            assert!(
                round.starts_with(&format!("round {} failed ", i + 1)),
                "{out}"
            );
            if (10..19).contains(&i) {
                assert!(
                    step.ends_with(" step=held reopen=- state=ONLINE looked_ms=- verdict=retry"),
                    "{out}"
                );
                assert!(round.ends_with(" bound=0 reopen=held"), "{out}");
            } else {
                assert!(
                    step.contains(" step=reopened reopen=ok state=ONLINE "),
                    "{out}"
                );
                assert!(!round.contains("reopen=held"), "{out}");
            }
            if i >= 4 {
                assert!(!round.contains("early_ms"), "round {} early: {out}", i + 1);
            }
        }
        assert_eq!(world.borrow().reopens, 11);
        assert_eq!(
            err.matches("10 reopens of this note left the pool unsuspended")
                .count(),
            1,
            "{err}"
        );
        assert!(
            err.contains(
                "round 10: 10 reopens of this note left the pool unsuspended: it is reopened \
                 only every 10th round from now on"
            ),
            "{err}"
        );

        // zed suspends the pool at 500 s, during the held rounds: the next
        // round (13, at 540 s) finds it SUSPENDED with no reopen, and `run`
        // ends with 11.
        let world = lost_world(noted(Held::Yes));
        world.borrow_mut().suspends_after_ms = None;
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let watched = world.clone();
        let mut go = until(&world, start + 990_000);
        let mut asked = move || {
            if mono(&watched) >= start + 500_000 && watched.borrow().suspended_at.is_none() {
                let at = watched.borrow().now_ms;
                watched.borrow_mut().suspended_at = Some(at);
            }
            go()
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut asked);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let last: Vec<&str> = out.lines().rev().take(2).collect();
        assert!(
            last[1].contains(" step=suspended reopen=- state=SUSPENDED "),
            "{out}"
        );
        assert!(last[0].starts_with("round 13 ok "), "{out}");
        assert_eq!(world.borrow().reopens, 10);

        // Past the cap, a round that finds the pool not imported (from 400
        // to 430 s) ends the quick retries' run, and the held rounds after
        // it still come at the ticks: past the cap there is no quick retry.
        let world = lost_world(noted(Held::Yes));
        world.borrow_mut().suspends_after_ms = None;
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let watched = world.clone();
        let pool = world.borrow().pool.clone();
        let mut go = until(&world, start + 700_000);
        let mut asked = move || {
            let now = mono(&watched);
            let gone = (start + 400_000..start + 430_000).contains(&now);
            watched.borrow_mut().pool = if gone { None } else { pool.clone() };
            go()
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut asked);
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        // Rounds 1 to 10 reopen (the last at 360 s), 11 (420 s) finds no
        // pool, then held rounds at 480, 540, 600 and 660 s.
        assert_eq!(lines.len(), 30, "{out}");
        assert!(lines[20].contains(" step=not-imported "), "{out}");
        for i in [22, 24, 26, 28] {
            assert!(lines[i].contains(" step=held "), "{out}");
            assert!(!lines[i + 1].contains("early_ms"), "{out}");
        }
        assert_eq!(world.borrow().reopens, 10);
    }

    /// A volume lost while `run` waits, whose pool does not suspend: the
    /// rounds after the first step are the quick retries and then the
    /// ticks, never back to back: the watch takes each round's own note
    /// as no news (W12 of the review of #173). Mutants: the watch set
    /// only at the first round; no quick retry.
    #[test]
    fn rounds_on_a_lost_note_are_not_back_to_back() {
        let world = World::new(Mode::Idle);
        let failed_at = world.borrow().now_ms + 30_000;
        {
            let mut w = world.borrow_mut();
            w.noted = Some(noted(Held::Unknown));
            w.noted_from_ms = Some(failed_at);
            w.gone_from = Some(failed_at);
            w.suspends_after_ms = None;
        }
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let (code, out, err) = scheduled(
            &mut agent,
            Duration::from_secs(60),
            &mut until(&world, start + 300_000),
        );
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        // Round 1 at 0 s; the note at 30 s and its step at once (round 2,
        // early); three quick retries; the ticks at 120, 180 and 240 s.
        let rounds = rounds(&out);
        assert_eq!(rounds.len(), 8, "{out}");
        assert_eq!(world.borrow().reopens, 7, "{out}");
    }

    /// A volume lost while the syncs after a rewind import have not yet
    /// made the newest cut open as itself: the syncs stop on the news
    /// (`Stuck::Noted`), the round says why it began nothing, its line
    /// counts the syncs that returned and is not `bound`, and the next
    /// round takes the step (P19 and P20 of the review of #173).
    /// Mutants: the syncs not stopped by the note; `bound=1` on a stop by
    /// news; `synced=` not the syncs that returned.
    #[test]
    fn a_volume_lost_before_the_cut_opens_stops_the_syncs() {
        let world = rewound_t(5);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let watched = world.clone();
        let mut failed = false;
        let mut go = move || {
            let mut w = watched.borrow_mut();
            // After the first sync, while the newest cut is still behind.
            if w.synced_at.len() == 1 && !failed {
                failed = true;
                w.advance(Duration::from_secs(2));
                let now = w.now_ms;
                w.noted = Some(noted(Held::Yes));
                w.noted_from_ms = Some(now);
                w.gone_from = Some(now);
                w.suspends_after_ms = Some(1_050);
            }
            w.now_ms - crate::engine::fake::EPOCH_MS < start + 600_000
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        assert!(lines[0].starts_with("round 1 ok "), "{out}");
        assert_eq!(field(lines[0], "synced"), "1", "{out}");
        assert_eq!(field(lines[0], "bound"), "0", "{out}");
        assert_eq!(field(lines[0], "begun"), "-", "{out}");
        assert!(lines[1].starts_with("lost tank "), "{out}");
        assert!(lines[2].starts_with("round 2 ok "), "{out}");
        assert!(
            err.contains("round 1: no publish begun: the newest cut, of group ")
                && err.contains("the volume's failure note came"),
            "{err}"
        );
        assert_eq!(world.borrow().synced_at.len(), 1, "{out}");
        assert!(world.borrow().programs.is_empty(), "{out}");
    }

    /// A reopen refused because zed suspended the pool first is a lost
    /// volume: the look, not the command, is the signal. The reopen's
    /// error is on standard error. Mutant: a refused reopen taken as a
    /// failed step.
    #[test]
    fn a_refused_reopen_of_a_pool_zed_suspended_is_lost() {
        let world = lost_world(noted(Held::Yes));
        {
            // zed suspends the pool 100 ms after the step's first look,
            // which reads ONLINE, and the reopen is refused.
            let mut w = world.borrow_mut();
            w.reopen_refused = true;
            w.suspends_after_ms = None;
            let at = w.now_ms + 100;
            w.suspended_at = Some(at);
        }
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        assert!(
            out.lines().next().is_some_and(|l| l.contains(
                " step=reopened reopen=failed state=SUSPENDED looked_ms=250 verdict=lost"
            )),
            "{out}"
        );
        assert!(err.contains("round 1: the reopen: "), "{err}");
    }

    /// A look at the pool's state that fails fails the round, and the
    /// next takes the step again; a look that fails after the reopen
    /// says the reopen's outcome and `state=unread` (F3 of the review of
    /// #173). Mutants: an end on an unread state; `reopen=-` after a
    /// reopen; the look's error not said.
    #[test]
    fn a_state_that_cannot_be_read_is_tried_again_at_the_next_round() {
        // Before the reopen: no reopen, `step=failed`.
        let world = lost_world(noted(Held::Yes));
        world.borrow_mut().pool_state_fails = 1;
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        assert!(
            lines[0].ends_with(" step=failed reopen=- state=- looked_ms=- verdict=retry"),
            "{out}"
        );
        assert!(lines[1].starts_with("round 1 failed "), "{out}");
        assert!(lines[2].ends_with(" verdict=lost"), "{out}");
        assert!(lines[3].starts_with("round 2 ok "), "{out}");
        assert!(
            err.contains("round 1: ") && err.contains("cannot read the pool's state"),
            "{err}"
        );
        assert_eq!(world.borrow().reopens, 1);

        // After the reopen: the reopen is said, and the pool, which
        // suspended meanwhile, is found SUSPENDED at the next round.
        let world = lost_world(noted(Held::Yes));
        {
            let mut w = world.borrow_mut();
            w.pool_state_fails_after = 1;
            w.pool_state_fails = 1;
        }
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        assert!(
            lines[0].ends_with(" step=reopened reopen=ok state=unread looked_ms=0 verdict=retry"),
            "{out}"
        );
        assert!(lines[1].starts_with("round 1 failed "), "{out}");
        assert!(
            lines[2].contains(" step=suspended reopen=- state=SUSPENDED "),
            "{out}"
        );
        assert!(
            err.contains(
                "round 1: the look at the pool's state after the reopen: \
                 zpool list: exit 1: cannot read the pool's state"
            ),
            "{err}"
        );
        assert_eq!(world.borrow().reopens, 1);
    }

    /// A note that cannot be read fails the round, which touches nothing
    /// else (a nit of the review of #173); the next read that finds it
    /// lost takes the step. Mutants: an unreadable note taken as none
    /// (the round not failed, or a begin); as lost.
    #[test]
    fn a_note_that_cannot_be_read_fails_the_round() {
        let world = lost_world(noted(Held::Yes));
        world.borrow_mut().note_read_fails = 1;
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("round 1 failed "), "{out}");
        assert_eq!(field(lines[0], "begun"), "-", "{out}");
        assert!(
            !world.borrow().calls.contains(&"count"),
            "no ZFS call: {:?}",
            world.borrow().calls
        );
        assert!(lines[1].starts_with("lost tank "), "{out}");
        assert!(lines[2].starts_with("round 2 ok "), "{out}");
        assert!(
            err.contains("round 1: ") && err.contains("its failure note: "),
            "{err}"
        );
        assert_eq!(world.borrow().reopens, 1);
    }

    /// A volume that fails while `run` settles a publish, with the
    /// default tick of a minute, is stepped on within about a second,
    /// not at the next tick (F1 of the review of #173; the lead's
    /// ruling). The round under way ends early, and the next begins at
    /// once. Mutant: the note read only at a round's start (the reopen
    /// about 55 s after the failure).
    #[test]
    fn a_volume_lost_while_a_publish_settles_is_stepped_on_within_seconds() {
        let world = World::new(Mode::Busy);
        let failed_at = world.borrow().now_ms + 5_000;
        {
            // The daemon fails at +5 s: it leaves its note and answers
            // nothing more, so the publish stays under way.
            let mut w = world.borrow_mut();
            w.noted = Some(noted(Held::Yes));
            w.noted_from_ms = Some(failed_at);
            w.gone_from = Some(failed_at);
            w.suspends_after_ms = Some(1_050);
        }
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("round 1 ok "), "{out}");
        assert_ne!(field(lines[0], "begun"), "-", "{out}");
        assert!(lines[1].starts_with("lost tank "), "{out}");
        assert!(lines[2].starts_with("round 2 ok "), "{out}");
        assert_eq!(field(lines[2], "under_way"), "1", "{out}");
        let reopened = world.borrow().suspended_at.expect("reopened") - 1_050;
        let after = reopened - failed_at;
        assert!(after <= 2_000, "the reopen {after} ms after the failure");
        assert_eq!(world.borrow().reopens, 1);
    }

    /// A volume lost during a rewind import's last sync, whose `cuts`
    /// then shows the newest cut opening as itself: the watch is asked
    /// before the begin, so no publish is begun, and the next round takes
    /// the step at once (Codex's P1 on #173). Mutant: the begin with no
    /// look at the watch after `sync_past`.
    #[test]
    fn a_volume_lost_during_the_syncs_begins_no_publish() {
        let world = rewound_t(5);
        let keep = MemKeep::default();
        let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
        let start = mono(&world);
        let watched = world.clone();
        let mut failed = false;
        let mut go = move || {
            let mut w = watched.borrow_mut();
            // The `go` after the third sync, which makes the newest cut
            // open as itself: the volume fails two seconds later, with
            // its note left and its daemon gone.
            if w.synced_at.len() == 3 && !failed {
                failed = true;
                w.advance(Duration::from_secs(2));
                let now = w.now_ms;
                w.noted = Some(noted(Held::Yes));
                w.noted_from_ms = Some(now);
                w.gone_from = Some(now);
                w.suspends_after_ms = Some(1_050);
            }
            w.now_ms - crate::engine::fake::EPOCH_MS < start + 600_000
        };
        let (code, out, err) = scheduled(&mut agent, Duration::from_secs(60), &mut go);
        assert_eq!(code, exit::LOST, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("round 1 ok "), "{out}");
        assert_eq!(field(lines[0], "synced"), "3", "{out}");
        assert_eq!(field(lines[0], "begun"), "-", "{out}");
        assert!(lines[1].starts_with("lost tank "), "{out}");
        assert!(lines[2].starts_with("round 2 ok "), "{out}");
        assert!(world.borrow().programs.is_empty(), "{out}");
        assert!(keep.0.borrow().record.is_none(), "{out}");
        assert_eq!(world.borrow().reopens, 1);
    }

    /// The same while `run` waits for its tick with nothing under way.
    /// Mutant: the note not read in the wait.
    #[test]
    fn a_volume_lost_while_run_waits_is_stepped_on_within_seconds() {
        let world = World::new(Mode::Idle);
        let failed_at = world.borrow().now_ms + 30_000;
        {
            let mut w = world.borrow_mut();
            w.noted = Some(noted(Held::Unknown));
            w.noted_from_ms = Some(failed_at);
            w.gone_from = Some(failed_at);
            w.suspends_after_ms = Some(1_050);
        }
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::LOST, "{out}{err}");
        assert_eq!(rounds(&out).len(), 2, "{out}");
        let reopened = world.borrow().suspended_at.expect("reopened") - 1_050;
        let after = reopened - failed_at;
        assert!(after <= 1_250, "the reopen {after} ms after the failure");
    }

    /// A volume that failed with no client holding its device: said on
    /// standard error at each round, no pool touched (no ZFS call at all:
    /// no sync, begin or settle), each round `ok` with nothing begun, and
    /// `run` goes on (the lead's answer to Q2; Codex's P2 on #173).
    /// Mutants: a reopen; an end with 11; the round going on to the sync,
    /// the begin and the settle.
    #[test]
    fn a_volume_failed_with_no_client_touches_no_pool() {
        let world = lost_world(noted(Held::No));
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
        let rounds = rounds(&out);
        assert!(rounds.len() > 1, "{out}");
        for (i, round) in rounds.iter().enumerate() {
            assert!(round.starts_with(&format!("round {} ok ", i + 1)), "{out}");
            assert_eq!(field(round, "begun"), "-", "{out}");
        }
        assert!(
            world.borrow().calls.is_empty(),
            "no ZFS call: {:?}",
            world.borrow().calls
        );
        assert!(world.borrow().programs.is_empty(), "{out}");
        assert!(!out.contains("lost "), "{out}");
        assert!(
            err.contains(
                "round 1: the volume failed in this boot (a write or sync of the \
                 volume's image failed), and no client held its device: no pool is touched"
            ),
            "{err}"
        );
        assert_eq!(world.borrow().reopens, 0);
    }

    /// `publish` and `settle` refuse a lost volume with 11, and one that
    /// failed with no client with 1, touching no pool and beginning
    /// nothing (the lead's answer to Q5). Mutants: no refusal; a reopen;
    /// one code for both.
    #[test]
    fn publish_and_settle_refuse_a_failed_volume() {
        for (noted, want) in [
            (noted(Held::Yes), exit::LOST),
            (Noted::NotWhole, exit::LOST),
            (noted(Held::No), exit::ERROR),
        ] {
            for task in [Task::Publish, Task::Settle] {
                let world = lost_world(noted);
                let keep = MemKeep::default();
                let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
                let (code, out, err) = executed(&mut agent, &task, LIMIT);
                assert_eq!(code, want, "{task:?} {noted:?}: {out}{err}");
                assert_eq!(out, "");
                assert!(err.contains("nothing is published"), "{err}");
                assert_eq!(world.borrow().reopens, 0);
                assert!(world.borrow().programs.is_empty());
            }
        }
    }

    /// `publish` and `settle` with a note that cannot be read refuse with
    /// 1 and touch nothing (the review of #173). Mutant: the error taken
    /// as no note.
    #[test]
    fn publish_and_settle_refuse_a_note_that_cannot_be_read() {
        for task in [Task::Publish, Task::Settle] {
            let world = lost_world(noted(Held::Yes));
            world.borrow_mut().note_read_fails = 1;
            let keep = MemKeep::default();
            let mut agent = agent_on(&world, FakeDaemon(world.clone()), &keep);
            let (code, out, err) = executed(&mut agent, &task, LIMIT);
            assert_eq!(code, exit::ERROR, "{task:?}: {out}{err}");
            assert_eq!(out, "");
            assert!(err.contains("its failure note: "), "{err}");
            assert_eq!(world.borrow().reopens, 0);
            assert!(
                world.borrow().calls.is_empty(),
                "{:?}",
                world.borrow().calls
            );
        }
    }

    /// `run` ends with 12 after the round in which the daemon's
    /// attachment changed (a new attach: here a power cut), and goes on
    /// through a take-over, which keeps it. Mutants: no end; an end
    /// before the round line; an end at a take-over.
    #[test]
    fn run_ends_with_12_when_the_attachment_changes() {
        let world = World::new(Mode::Busy);
        world.borrow_mut().restart_at_cuts = Some((3, Restart::Reboot));
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::MOVED, "{out}{err}");
        let last = out.lines().last().expect("a line");
        assert!(last.starts_with("round "), "{out}");
        assert!(
            err.contains("the daemon's attachment changed from ")
                && err.contains(": run ends, to be started again on the device as it is now"),
            "{err}"
        );

        let world = World::new(Mode::Busy);
        world.borrow_mut().restart_at_cuts = Some((3, Restart::TakeOver));
        let (code, out, err) = run(&world);
        assert_eq!(code, exit::PUBLISHED, "{out}{err}");
    }

    /// The agent's codes for a lost volume and a move are not
    /// `skyzfsd`'s 9 and 10 (the lead's ruling on F5 of the review of
    /// #173). Mutant: either back at 9 or 10.
    #[test]
    fn the_codes_of_a_lost_volume_and_a_move_are_not_the_daemons() {
        assert_eq!((exit::LOST, exit::MOVED), (11, 12));
        assert!(USAGE.contains("11 the volume lost") && USAGE.contains("12 the device"));
    }

    /// The `lost` line's text. Mutant: a field left out or misnamed.
    #[test]
    fn the_lost_line() {
        use crate::engine::{Note, Step};
        use crate::zfs::PoolState;
        let failure = Failure {
            held: Held::Unknown,
            reason: Reason::Marked,
            identity: [0; 16],
        };
        let step = Step::Reopened {
            error: Some("refused".into()),
            state: Some(PoolState("ONLINE".into())),
            unread: None,
            looked_ms: 10_000,
        };
        assert_eq!(
            lost_line("p", &[0xab; 16], &Note::Whole(failure), Some(&step)),
            format!(
                "lost p volume={} note=whole reason=marked held=unknown step=reopened \
                 reopen=failed state=ONLINE looked_ms=10000 verdict=retry\n",
                "ab".repeat(16)
            )
        );
        let unread = Step::Reopened {
            error: None,
            state: None,
            unread: Some("no".into()),
            looked_ms: 250,
        };
        assert!(lost_line("p", &[0; 16], &Note::Torn, Some(&unread))
            .ends_with(" step=reopened reopen=ok state=unread looked_ms=250 verdict=retry\n"));
        assert!(
            lost_line("p", &[0; 16], &Note::Torn, Some(&Step::NotImported))
                .ends_with(" step=not-imported reopen=- state=- looked_ms=- verdict=failed\n")
        );
        assert_eq!(
            lost_line("p", &[0; 16], &Note::Torn, None),
            format!(
                "lost p volume={} note=torn reason=- held=yes step=failed reopen=- state=- \
                 looked_ms=- verdict=retry\n",
                "00".repeat(16)
            )
        );
    }
}
