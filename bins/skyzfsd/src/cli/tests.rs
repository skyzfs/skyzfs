//! The command line as the kernel rig depends on it.

use std::path::Path;

use super::*;

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(String::from).collect()
}

fn hooked(line: &str) -> Result<Command, UsageError> {
    parse_with(args(line), true)
}

fn plain(line: &str) -> Result<Command, UsageError> {
    parse_with(args(line), false)
}

fn serve(line: &str) -> Serve {
    match hooked(line) {
        Ok(Command::Serve(serve)) => serve,
        other => panic!("{line}: {other:?}"),
    }
}

/// The exit statuses are the numbers the rig expects: 0 after SIGTERM,
/// 1 for anything else, 2 for usage, 3 for a volume that failed while
/// serving, 4 for one refused because it failed in this boot, 5 for one
/// another instance holds, 6 for a detach of a device a client holds,
/// 7 for a kernel driver that broke its rule. The help's table names
/// each.
#[test]
fn the_exit_statuses_are_the_agreed_numbers() {
    let all = [
        (Exit::Done, 0),
        (Exit::Error, 1),
        (Exit::Usage, 2),
        (Exit::Failed, 3),
        (Exit::RefusedFailed, 4),
        (Exit::RefusedHeld, 5),
        (Exit::DeletedHeld, 6),
        (Exit::BrokenRule, 7),
        (Exit::CommitRefused, 8),
    ];
    let table = HELP
        .split_once("Exit statuses:")
        .map(|(_, table)| table)
        .expect("the help has a table of exit statuses");
    for (exit, status) in all {
        assert_eq!(exit.status(), status);
        assert!(
            table.contains(&format!(" {status} ")) || table.contains(&format!("\n{status} ")),
            "the help's table lacks {status}"
        );
    }
    assert!(HELP.contains("7 the kernel's driver broke its rule"));
    assert!(HELP.contains("8 the kernel's driver refused an answer of serve's"));
}

/// `create --dir DIR --size BYTES`: the size is in bytes, decimal, a
/// multiple of 1 MiB above zero; both options are required, in either
/// order and in either spelling.
#[test]
fn create_takes_a_directory_and_a_size_in_bytes() {
    let want = Command::Create(Create {
        dir: "/v/a".into(),
        size: 64 << 20,
        run_dir: DEFAULT_RUN_DIR.into(),
        base_pattern: None,
        zero_fill: false,
    });
    assert_eq!(plain("create --dir /v/a --size 67108864"), Ok(want.clone()));
    assert_eq!(plain("create --size=67108864 --dir=/v/a"), Ok(want));
    // The run directory is looked at for a failure note.
    let elsewhere = plain("create --dir /v/a --size 1048576 --run-dir /r");
    assert!(matches!(
        elsewhere,
        Ok(Command::Create(Create { ref run_dir, .. })) if run_dir.as_os_str() == "/r"
    ));
    assert_eq!(
        plain("create --size 1048576"),
        Err(UsageError::Missing("--dir"))
    );
    assert_eq!(
        plain("create --dir /v/a"),
        Err(UsageError::Missing("--size"))
    );
    for size in [
        "0",
        "1",
        "1048575",
        "1048577",
        "64M",
        "-1048576",
        "0x100000",
        "1e6",
        "",
        "99999999999999999999999",
    ] {
        let line = format!("create --dir /v/a --size={size}");
        assert!(
            matches!(plain(&line), Err(UsageError::Value(name, _)) if name == "--size"),
            "{size}"
        );
    }
}

/// `serve --dir DIR` with its defaults: the run directory `/run/skyzfsd`,
/// one queue of depth 64, no acknowledgement, the deployment rules on,
/// no hook. Each option changes its own field.
#[test]
fn serve_has_the_agreed_defaults_and_options() {
    let default = Serve {
        dir: "/v/a".into(),
        run_dir: "/run/skyzfsd".into(),
        acknowledge_failure: false,
        async_delete_asserted: false,
        queues: 1,
        depth: 64,
        in_place_io: skyzfs_local::inplace::IoMode::default(),
        allow_unlocked_memory: false,
        allow_any_filesystem: false,
        hooks: Hooks::default(),
    };
    assert_eq!(
        plain("serve --dir /v/a"),
        Ok(Command::Serve(default.clone()))
    );
    let full = serve(
        "serve --run-dir /tmp/run --acknowledge-failure --dir /v/a --queues 1 --depth 128 \
         --unsafe-allow-unlocked-memory --unsafe-allow-any-filesystem --kernel-has-async-delete",
    );
    assert_eq!(
        full,
        Serve {
            run_dir: "/tmp/run".into(),
            acknowledge_failure: true,
            async_delete_asserted: true,
            queues: 1,
            depth: 128,
            allow_unlocked_memory: true,
            allow_any_filesystem: true,
            ..default
        }
    );
    assert_eq!(plain("serve"), Err(UsageError::Missing("--dir")));
    for bad in [
        "--depth 0",
        "--depth 4097",
        "--queues 0",
        "--queues 2",
        "--queues 65536",
        "--depth x",
    ] {
        assert!(
            matches!(
                plain(&format!("serve --dir /v/a {bad}")),
                Err(UsageError::Value(..))
            ),
            "{bad}"
        );
    }
    assert_eq!(serve("serve --dir /v/a --depth 4096").depth, 4096);
}

/// There is no choice of a store (M1.3t-d): `--store` is not an
/// option of serve, whatever its value, and neither the usage nor the
/// help offers it. The rigs and the benchmark give an older build
/// `--store in-place` only when its help offers `[--store`, so a build
/// that offered it and refused it would end every serve of theirs. The
/// options of the log and its checkpoints, and its hooks, are options
/// of nothing either: usage errors.
#[test]
fn serve_has_no_choice_of_store() {
    for given in [
        "--store in-place",
        "--store=in-place",
        "--store log",
        "--store=log",
        "--store x",
    ] {
        // An unknown option is named as it was given, value and all
        // when it came with `=`.
        let named = given.split(' ').next().unwrap_or(given);
        assert_eq!(
            plain(&format!("serve --dir /v/a {given}")),
            Err(UsageError::Unknown(named.into())),
            "{given}"
        );
    }
    for text in [USAGE, HELP, HOOKS_USAGE] {
        assert!(!text.contains("--store"), "{text}");
    }
    // The daemon and replay rigs look for it in this file's source.
    assert!(!include_str!("../cli.rs").contains("[--store"));
    for gone in [
        "--log-high 4",
        "--log-low 1",
        "--writeback-kib 0",
        "--home-writeback-kib 0",
        "--readers 1",
    ] {
        let name = gone.split(' ').next().unwrap();
        assert_eq!(
            plain(&format!("serve --dir /v/a {gone}")),
            Err(UsageError::Unknown(name.into())),
            "{gone}"
        );
    }
    for gone in [
        "--recover-short 1",
        "--step-pause-ms 5",
        "--step-pause-until /tmp/go",
        "--step-pause-at saved",
        "--refuse-holes",
    ] {
        let name = gone.split(' ').next().unwrap();
        assert_eq!(
            hooked(&format!("serve --dir /v/a {gone}")),
            Err(UsageError::Unknown(name.into())),
            "{gone}"
        );
    }
}

/// The store's I/O: direct, on 8 threads unless `--io-threads N` (1 to
/// 64) says otherwise; there is no option for buffered I/O, and
/// `--direct-io` (the experiment's) is gone. `create --zero-fill`
/// stays.
#[test]
fn the_in_place_store_does_direct_io_on_its_threads() {
    use skyzfs_local::inplace::IoMode;
    let default = serve("serve --dir /v/a").in_place_io;
    assert_eq!(default, IoMode::default());
    assert!(default.direct());
    assert_eq!(default.threads(), 8);
    for n in [1, 32, 64] {
        let io = serve(&format!("serve --dir /v/a --io-threads {n}")).in_place_io;
        assert_eq!(io, IoMode::with_threads(n).unwrap());
        assert!(io.direct());
    }
    for n in [0, 65] {
        assert!(matches!(
            plain(&format!("serve --dir /v/a --io-threads {n}")),
            Err(UsageError::Value(..))
        ));
    }
    assert_eq!(
        serve("serve --dir /v/a --io-threads 4").in_place_io,
        IoMode::with_threads(4).unwrap()
    );
    assert!(plain("serve --dir /v/a --direct-io").is_err());
    assert!(matches!(
        plain("create --dir /v/a --size 1048576 --zero-fill"),
        Ok(Command::Create(Create {
            zero_fill: true,
            ..
        }))
    ));
}

/// The default and the bound of `--io-threads` are the store's own
/// (`IoMode`), one constant each, so that the default can change after
/// a benchmark round in one place; the help says them.
#[test]
fn the_io_threads_default_is_the_stores() {
    use skyzfs_local::inplace::IoMode;
    assert_eq!(usize::from(DEFAULT_IO_THREADS), IoMode::DEFAULT_THREADS);
    assert_eq!(usize::from(MAX_IO_THREADS), IoMode::MAX_THREADS);
    let said = format!("--io-threads N (1 to {MAX_IO_THREADS}, {DEFAULT_IO_THREADS} unless given)");
    assert!(HELP.contains(&said), "the help does not say {said:?}");
}

/// The fault hooks of the rig, in a build that has them: the volume
/// fails after N requests; one request is held, a read or a write that
/// covers a byte or a flush; the boot id comes from a file; the commits
/// are delayed; the requests are written down.
#[test]
fn the_test_hooks_are_parsed_in_a_test_build() {
    let hooks = serve(
        "serve --dir /v/a --fail-log-after 100 --hold-one-request 300:read:4096 \
         --boot-id-file /tmp/boot --commit-delay-ms 20 --request-log /tmp/requests.tsv",
    )
    .hooks;
    assert_eq!(
        hooks,
        Hooks {
            fail_log_after: Some(100),
            hold: Some(Hold {
                seconds: 300,
                what: Held::Read(4096),
            }),
            boot_id_file: Some("/tmp/boot".into()),
            commit_delay_ms: Some(20),
            request_log: Some("/tmp/requests.tsv".into()),
        }
    );
    let named = serve("serve --dir /v/a --request-log /tmp/r.tsv");
    assert_eq!(named.hooks.request_log, Some("/tmp/r.tsv".into()));
    let held = |spec: &str| {
        serve(&format!("serve --dir /v/a --hold-one-request {spec}"))
            .hooks
            .hold
            .map(|h| (h.seconds, h.what))
    };
    assert_eq!(held("5:write:0"), Some((5, Held::Write(0))));
    assert_eq!(held("300:flush"), Some((300, Held::Flush)));
    for bad in [
        "flush",
        "5:flush:0",
        "5:read",
        "5:write:",
        "x:read:0",
        "5:trim:0",
        "5:read:0:1",
        "5",
        ":flush",
    ] {
        let line = format!("serve --dir /v/a --hold-one-request {bad}");
        assert!(matches!(hooked(&line), Err(UsageError::Value(..))), "{bad}");
    }
    let create = hooked("create --dir /v/a --size 1048576 --base-pattern 47806");
    assert!(matches!(
        create,
        Ok(Command::Create(Create {
            base_pattern: Some(47806),
            ..
        }))
    ));
}

/// The fault hooks are not in a default build: each is a usage error
/// there, so a production daemon cannot be told to fail its log.
#[test]
fn the_test_hooks_are_refused_in_a_default_build() {
    for hook in [
        "serve --dir /v/a --fail-log-after 100",
        "serve --dir /v/a --hold-one-request 300:flush",
        "serve --dir /v/a --boot-id-file /tmp/boot",
        "serve --dir /v/a --commit-delay-ms 20",
        "serve --dir /v/a --request-log /tmp/r.tsv",
        "create --dir /v/a --size 1048576 --base-pattern 1",
    ] {
        assert!(
            matches!(plain(hook), Err(UsageError::TestOnly(_))),
            "{hook}"
        );
        assert!(hooked(hook).is_ok(), "{hook}");
    }
    // The build decides for the public entry point.
    let built = parse(args("serve --dir /v/a --commit-delay-ms 1"));
    assert_eq!(built.is_ok(), cfg!(feature = "test-hooks"));
}

/// `detach` and `status` name a volume and a run directory, and nothing
/// else.
#[test]
fn detach_and_status_name_a_volume() {
    let volume = Volume {
        dir: "/v/a".into(),
        run_dir: "/run/skyzfsd".into(),
    };
    assert_eq!(
        plain("detach --dir /v/a"),
        Ok(Command::Detach(Detach {
            volume: volume.clone(),
            async_delete_asserted: false,
        }))
    );
    assert_eq!(
        plain("detach --kernel-has-async-delete --dir /v/a"),
        Ok(Command::Detach(Detach {
            volume: volume.clone(),
            async_delete_asserted: true,
        }))
    );
    assert_eq!(plain("status --dir /v/a"), Ok(Command::Status(volume)));
    let elsewhere = plain("status --dir /v/a --run-dir /tmp/run");
    let Ok(Command::Status(Volume { run_dir, .. })) = elsewhere else {
        panic!("{elsewhere:?}");
    };
    assert_eq!(run_dir, Path::new("/tmp/run"));
    for bad in [
        "detach",
        "status --dir /v/a --acknowledge-failure",
        "detach --dir /v/a --size 1048576",
        "status --dir /v/a --kernel-has-async-delete",
        "detach --dir /v/a --kernel-has-async-delete=yes",
        "create --dir /v/a --size 1048576 --kernel-has-async-delete",
    ] {
        assert!(plain(bad).is_err(), "{bad}");
    }
}

/// A command line that is not understood is a usage error and nothing
/// else: no command, an unknown command or option, an option twice, an
/// option without its value, a flag with one, a stray word.
#[test]
fn anything_else_is_a_usage_error() {
    assert_eq!(plain(""), Err(UsageError::Command));
    assert_eq!(plain("start --dir /v/a"), Err(UsageError::Command));
    assert_eq!(plain("--dir /v/a serve"), Err(UsageError::Command));
    let cases = [
        ("serve --dir /v/a --verbose", "unknown"),
        ("serve --dir /v/a extra", "unknown"),
        ("serve --dir /v/a --dir /v/b", "twice"),
        ("serve --dir=/v/a --dir /v/b", "twice"),
        ("serve --dir", "value"),
        ("serve --dir=", "value"),
        ("serve --dir /v/a --acknowledge-failure=yes", "value"),
        ("serve --dir /v/a --run-dir", "value"),
    ];
    for (line, kind) in cases {
        let got = plain(line);
        let matches = match kind {
            "unknown" => matches!(got, Err(UsageError::Unknown(_))),
            "twice" => matches!(got, Err(UsageError::Twice(_))),
            _ => matches!(got, Err(UsageError::Value(..))),
        };
        assert!(matches, "{line}: {got:?}");
    }
}

#[test]
fn help_is_a_command_and_says_what_the_rig_asks_it_to_say() {
    for line in ["help", "--help", "-h"] {
        assert_eq!(plain(line), Ok(Command::Help));
    }
    assert_eq!(
        plain("help serve"),
        Err(UsageError::Unknown("serve".into()))
    );
    // The lock is tried for 2 s, a live device of the volume's own for
    // 5 s, and a pool is exported before a detach.
    for needle in [
        "2 s",
        "20 times, 100 ms apart",
        "up to 5 s",
        "Export the pool first",
        "stays suspended",
        "ready /dev/ublkbN",
        "--acknowledge-failure",
    ] {
        assert!(HELP.contains(needle), "help does not say: {needle}");
    }
    // A figure that was measured wrongly and is not to be quoted.
    assert!(!HELP.contains("130"));
}

/// The help names the oldest kernel and every flag the start checks,
/// as the one table of them says.
#[test]
fn help_names_the_oldest_kernel_and_the_flags() {
    use skyzfs_ublk::features::{ASYNC_DELETE_OPTION, NEEDED, OLDEST_KERNEL};
    assert!(HELP.contains(&format!("Linux {OLDEST_KERNEL} or later")));
    for flag in NEEDED {
        assert!(HELP.contains(flag.name), "help does not name {}", flag.name);
    }
    for name in ["UBLK_U_CMD_DEL_DEV_ASYNC", "UBLK_U_CMD_GET_FEATURES"] {
        assert!(HELP.contains(name), "help does not name {name}");
    }
    // The override, in the help of both commands that take it, and in
    // the usage.
    assert!(HELP.matches(ASYNC_DELETE_OPTION).count() >= 3, "{HELP}");
    assert!(USAGE.matches(ASYNC_DELETE_OPTION).count() == 2, "{USAGE}");
}

/// Every test hook is in the list that a build with the hooks prints
/// after its usage and its help, and that list is printed exactly in
/// such a build: the kernel rig reads it (`serve --help`) to see which
/// hooks a daemon takes.
#[test]
fn a_build_with_the_hooks_lists_them() {
    for opt in CREATE.iter().chain(SERVE).filter(|opt| opt.hook) {
        assert!(
            HOOKS_USAGE.contains(&format!("[{} ", opt.name))
                || HOOKS_USAGE.contains(&format!("[{}]", opt.name))
                || HOOKS_USAGE.contains(&format!("| {} ", opt.name)),
            "{} is not listed",
            opt.name
        );
    }
    let hooked = cfg!(feature = "test-hooks");
    assert_eq!(usage().contains("--request-log"), hooked);
    assert_eq!(help().contains("--request-log"), hooked);
    assert!(usage().starts_with(USAGE) && help().starts_with(HELP));
    assert!(!USAGE.contains("--request-log") && !HELP.contains("--request-log"));
}
