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
/// 7 for a kernel driver that broke its rule, 10 for a lease another
/// holder has. The help's table names each.
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
        (Exit::NotStarted, 9),
        (Exit::LeaseHeld, 10),
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
    assert!(HELP.contains("9 serve gave the device's start up"));
    assert!(HELP.contains("retry the serve once; a second 9 is for a person"));
    assert!(HELP.contains("10 refused, another holder has the volume's lease"));
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
        remote: None,
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
/// the intent map's header synced, no hook. Each option changes its own
/// field.
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
        hold_cuts: false,
        intent_region_mib: None,
        intent_header_sync: true,
        holder: None,
        lease_ttl_s: None,
        serve_without_upload: false,
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
         --unsafe-allow-unlocked-memory --unsafe-allow-any-filesystem --kernel-has-async-delete \
         --hold-cuts",
    );
    assert_eq!(
        full,
        Serve {
            run_dir: "/tmp/run".into(),
            acknowledge_failure: true,
            async_delete_asserted: true,
            queues: 1,
            depth: 128,
            hold_cuts: true,
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

/// `create --endpoint URL --bucket BUCKET --volume NAME`: all three or
/// none, each a value the file `remote` can hold, and the name one the
/// provisioner takes. The id is the name's.
#[test]
fn create_takes_a_place_in_object_storage_whole_or_not_at_all() {
    let line = "create --dir /v/a --size 1048576 --endpoint http://h:9000 --bucket b --volume pg-1";
    match plain(line) {
        Ok(Command::Create(Create {
            remote: Some(remote),
            ..
        })) => {
            assert_eq!(
                remote,
                Remote::new("http://h:9000", "b", "pg-1").expect("a place")
            );
            assert_eq!(remote.id, crate::remote::volume_id("pg-1"));
        }
        other => panic!("{other:?}"),
    }
    for partial in [
        "--endpoint http://h",
        "--bucket b",
        "--volume v",
        "--endpoint http://h --bucket b",
        "--bucket b --volume v",
        "--endpoint http://h --volume v",
    ] {
        assert_eq!(
            plain(&format!("create --dir /v/a --size 1048576 {partial}")),
            Err(UsageError::Refused(
                "--endpoint, --bucket and --volume: all three or none".into()
            )),
            "{partial}"
        );
    }
    for bad in [
        "--endpoint http://h --bucket b --volume -v",
        "--endpoint http://h --bucket b --volume a/b",
        "--endpoint=http://h --bucket=b --volume=",
        &format!("--endpoint http://h --bucket b --volume {}", "v".repeat(64)),
        &format!(
            "--endpoint http://h --bucket {} --volume v",
            "b".repeat(256)
        ),
    ] {
        assert!(
            matches!(
                plain(&format!("create --dir /v/a --size 1048576 {bad}")),
                Err(UsageError::Refused(_) | UsageError::Value(..))
            ),
            "{bad}"
        );
    }
    assert!(USAGE.contains("[--endpoint URL --bucket BUCKET --volume NAME]"));
}

/// `serve --holder NAME --lease-ttl-s N --serve-without-upload`: the
/// holder is one word of at most 128 bytes, the lease's time 3 to 3600
/// seconds. Whether the volume has a place is known only to the serve.
#[test]
fn serve_takes_the_lease_options() {
    let given = serve("serve --dir /v/a --holder node-a --lease-ttl-s 60 --serve-without-upload");
    assert_eq!(
        (
            given.holder.as_deref(),
            given.lease_ttl_s,
            given.serve_without_upload
        ),
        (Some("node-a"), Some(60), true)
    );
    for ttl in ["3", "3600"] {
        assert!(
            plain(&format!("serve --dir /v/a --lease-ttl-s {ttl}")).is_ok(),
            "{ttl}"
        );
    }
    for ttl in ["0", "2", "3601", "x"] {
        assert!(
            matches!(
                plain(&format!("serve --dir /v/a --lease-ttl-s {ttl}")),
                Err(UsageError::Value(name, _)) if name == "--lease-ttl-s"
            ),
            "{ttl}"
        );
    }
    assert!(plain(&format!("serve --dir /v/a --holder {}", "h".repeat(128))).is_ok());
    assert!(matches!(
        plain(&format!("serve --dir /v/a --holder {}", "h".repeat(129))),
        Err(UsageError::Refused(_))
    ));
    assert!(matches!(
        parse_with(
            ["serve", "--dir", "/v/a", "--holder", "a b"].map(String::from),
            false
        ),
        Err(UsageError::Refused(_))
    ));
    assert!(USAGE.contains("[--holder NAME] [--lease-ttl-s N] [--serve-without-upload]"));
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

/// `--intent-region-mib N` sets the intent map's region, a power of two
/// from 1 to 1024 MiB, and only with `--hold-cuts`; the help says the
/// default, which is the store's.
#[test]
fn the_intent_region_is_a_power_of_two_of_mib_with_held_cuts() {
    assert_eq!(
        serve("serve --dir /v/a --hold-cuts").intent_region_mib,
        None
    );
    for n in [1, 4, 64, 1024] {
        let given = serve(&format!(
            "serve --dir /v/a --hold-cuts --intent-region-mib {n}"
        ));
        assert_eq!(given.intent_region_mib, Some(n));
    }
    for n in ["0", "3", "2048", "x"] {
        assert!(
            matches!(
                plain(&format!(
                    "serve --dir /v/a --hold-cuts --intent-region-mib {n}"
                )),
                Err(UsageError::Value(..))
            ),
            "{n}"
        );
    }
    assert_eq!(
        plain("serve --dir /v/a --intent-region-mib 4"),
        Err(UsageError::Value(
            "--intent-region-mib".into(),
            "only with --hold-cuts"
        ))
    );
    assert_eq!(DEFAULT_INTENT_REGION_MIB, 4);
    assert_eq!(MAX_INTENT_REGION_MIB, 1024);
    let said = format!(
        "--intent-region-mib N (a power of two from {MIN_INTENT_REGION_MIB} to \
         {MAX_INTENT_REGION_MIB}, {DEFAULT_INTENT_REGION_MIB}"
    );
    let help = HELP.split_whitespace().collect::<Vec<_>>().join(" ");
    let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(help.contains(&said), "the help does not say {said:?}");
}

/// The intent map's header is synced at each release (M2.4d) unless
/// `--no-intent-header-sync` is given (Q7, on by default since the bench
/// showed no cost), and that is taken only with `--hold-cuts`, as
/// `--intent-region-mib` is. The option that turned it on is gone. The
/// help names the one that turns it off.
#[test]
fn the_header_sync_is_on_unless_turned_off_with_held_cuts() {
    assert!(serve("serve --dir /v/a").intent_header_sync);
    assert!(serve("serve --dir /v/a --hold-cuts").intent_header_sync);
    assert!(!serve("serve --dir /v/a --hold-cuts --no-intent-header-sync").intent_header_sync);
    assert_eq!(
        plain("serve --dir /v/a --no-intent-header-sync"),
        Err(UsageError::Value(
            "--no-intent-header-sync".into(),
            "only with --hold-cuts"
        ))
    );
    assert!(
        matches!(
            plain("serve --dir /v/a --hold-cuts --no-intent-header-sync 1"),
            Err(UsageError::Unknown(..) | UsageError::Value(..))
        ),
        "a flag takes no value"
    );
    assert!(
        matches!(
            plain("serve --dir /v/a --hold-cuts --intent-header-sync"),
            Err(UsageError::Unknown(name)) if name == "--intent-header-sync"
        ),
        "the option that turned it on is gone"
    );
    assert!(USAGE.contains("[--no-intent-header-sync]"), "{USAGE}");
    assert!(HELP.contains("--no-intent-header-sync"), "{HELP}");
    assert!(!USAGE.contains("[--intent-header-sync]"), "{USAGE}");
    // The old option is named nowhere but inside the new one (Q9 of
    // the review of #133: the help's paragraph was held by nothing).
    for (what, text) in [("usage", USAGE), ("help", HELP)] {
        let rest = text.replace("--no-intent-header-sync", "");
        assert!(
            !rest.contains("--intent-header-sync"),
            "the {what} names the option that is gone"
        );
    }
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
/// are delayed; the requests are written down; one request's commit is
/// sent with no buffer, for the driver to refuse; one first fetch is
/// sent with no buffer, at a start or at a take-over; the uploader's
/// thread kills the daemon at a segment's PUT or at a commit's swap.
#[test]
fn the_test_hooks_are_parsed_in_a_test_build() {
    let hooks = serve(
        "serve --dir /v/a --fail-log-after 100 --hold-one-request 300:read:4096 \
         --boot-id-file /tmp/boot --commit-delay-ms 20 --request-log /tmp/requests.tsv \
         --refuse-one-commit write:8192 --refuse-one-fetch recovery --kill-at-upload put:3",
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
            request_log_whole: false,
            refuse: Some(Held::Write(8192)),
            refuse_fetch: Some(FetchRefusal::Recovery),
            kill_at: Some(KillAt::Put(3)),
        }
    );
    let kill = |spec: &str| hooked(&format!("serve --dir /v/a --kill-at-upload {spec}"));
    for (spec, want) in [
        ("put:1", KillAt::Put(1)),
        ("swap:1", KillAt::Swap(1)),
        ("swap:12", KillAt::Swap(12)),
        ("manifest:1", KillAt::Manifest(1)),
        ("publish:2", KillAt::Publish(2)),
    ] {
        let Ok(Command::Serve(serve)) = kill(spec) else {
            panic!("{spec:?}: {:?}", kill(spec));
        };
        assert_eq!(serve.hooks.kill_at, Some(want), "{spec:?}");
    }
    for bad in [
        "put:0",
        "put",
        "swap:",
        "swap:x",
        "commit:1",
        "put:1:2",
        ":1",
        "put:-1",
        "manifest:0",
        "publish:",
        "publish:x",
        "snap:1",
    ] {
        assert!(matches!(kill(bad), Err(UsageError::Value(..))), "{bad:?}");
    }
    assert_eq!(serve("serve --dir /v/a").hooks.kill_at, None);
    let fetch = |spec: &str| hooked(&format!("serve --dir /v/a --refuse-one-fetch {spec}"));
    assert!(matches!(
        fetch("start"),
        Ok(Command::Serve(Serve {
            hooks: Hooks {
                refuse_fetch: Some(FetchRefusal::Start),
                ..
            },
            ..
        }))
    ));
    for bad in ["begin", "Start", "start:0", "recovery,start", "0"] {
        assert!(matches!(fetch(bad), Err(UsageError::Value(..))), "{bad:?}");
    }
    let refused = |spec: &str| {
        serve(&format!("serve --dir /v/a --refuse-one-commit {spec}"))
            .hooks
            .refuse
    };
    assert_eq!(refused("read:0"), Some(Held::Read(0)));
    assert_eq!(refused("flush"), Some(Held::Flush));
    for bad in [
        "5:flush", "flush:0", "read", "write:", "trim:0", "read:0:1", ":flush", "5:read:0",
    ] {
        let line = format!("serve --dir /v/a --refuse-one-commit {bad}");
        assert!(
            matches!(hooked(&line), Err(UsageError::Value(..))),
            "{bad:?}"
        );
    }
    let named = serve("serve --dir /v/a --request-log /tmp/r.tsv");
    assert_eq!(named.hooks.request_log, Some("/tmp/r.tsv".into()));
    assert!(!named.hooks.request_log_whole);
    let content = |spec: &str| {
        hooked(&format!(
            "serve --dir /v/a --request-log /tmp/r.tsv --request-log-content {spec}"
        ))
    };
    assert!(matches!(
        content("whole"),
        Ok(Command::Serve(Serve {
            hooks: Hooks {
                request_log_whole: true,
                ..
            },
            ..
        }))
    ));
    assert!(matches!(
        content("hash"),
        Ok(Command::Serve(Serve {
            hooks: Hooks {
                request_log_whole: false,
                ..
            },
            ..
        }))
    ));
    assert!(matches!(content("all"), Err(UsageError::Value(..))));
    assert!(matches!(
        hooked("serve --dir /v/a --request-log-content whole"),
        Err(UsageError::Value(..))
    ));
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
        "serve --dir /v/a --refuse-one-commit flush",
        "serve --dir /v/a --refuse-one-fetch start",
        "serve --dir /v/a --kill-at-upload swap:1",
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

/// The channel's commands: each names a volume, as `status` does, and
/// its request. A publish's manifest length is the file's, read when it
/// is sent.
///
/// Mutant: a command mapped to another request, a value dropped, or a
/// name the channel's line could not carry taken.
#[test]
fn the_channels_commands_name_a_volume_and_a_request() {
    use crate::control::{Publish, Request};
    let volume = Volume {
        dir: "/v/a".into(),
        run_dir: "/run/skyzfsd".into(),
    };
    let ask = |request, manifest: Option<&str>| {
        Ok(Command::Ask(Ask {
            volume: volume.clone(),
            request,
            manifest: manifest.map(PathBuf::from),
        }))
    };
    assert_eq!(plain("cuts --dir /v/a"), ask(Request::Cuts, None));
    assert_eq!(
        plain("publish-progress --dir /v/a --name pool.t-1_x"),
        ask(Request::Progress("pool.t-1_x".into()), None)
    );
    assert_eq!(
        plain("withdraw --name t --dir /v/a"),
        ask(Request::Withdraw("t".into()), None)
    );
    let (a, s) = ("a1".repeat(16), "5E".repeat(16));
    let publish = |expiry| Publish {
        name: "t".into(),
        attachment: [0xa1; 16],
        state: [0x5e; 16],
        after: 3,
        group: 41,
        manifest: 0,
        expiry,
    };
    assert_eq!(
        plain(&format!(
            "publish --dir /v/a --name t --attachment {a} --state {s} --after 3 --group 41"
        )),
        ask(Request::Publish(publish(None)), None)
    );
    assert_eq!(
        plain(&format!(
            "publish --dir /v/a --name t --attachment {a} --state {s} --after 3 --group 41 \
             --manifest /m --expiry-s 3600"
        )),
        ask(
            Request::Publish(publish(Some(Duration::from_secs(3600)))),
            Some("/m")
        )
    );
    for (line, name) in [
        ("cuts --dir /v/a", "cuts"),
        ("withdraw --dir /v/a --name t", "withdraw"),
        ("publish-progress --dir /v/a --name t", "publish-progress"),
    ] {
        assert_eq!(plain(line).map(|c| c.name()), Ok(name));
    }
    let full =
        format!("publish --dir /v/a --name t --attachment {a} --state {s} --after 3 --group 41");
    let long = "n".repeat(129);
    for bad in [
        "cuts".to_string(),
        "cuts --dir /v/a --name t".into(),
        "publish-progress --dir /v/a".into(),
        "withdraw --dir /v/a --name a/b".into(),
        format!("withdraw --dir /v/a --name {long}"),
        full.replace(" --group 41", ""),
        full.replace(" --attachment ", " --attachment 0"),
        full.replace(&format!("--state {s}"), &format!("--state {}", &s[2..])),
        full.replace("--after 3", "--after -1"),
        format!("{full} --expiry-s 0"),
        format!("{full} --expiry-s 3601"),
        format!("{full} --kernel-has-async-delete"),
    ] {
        assert!(plain(&bad).is_err(), "{bad}");
    }
    assert!(plain(&format!("withdraw --dir /v/a --name {}", &long[1..])).is_ok());
}
