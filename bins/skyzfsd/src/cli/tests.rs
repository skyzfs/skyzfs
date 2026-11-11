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
/// another instance holds.
#[test]
fn the_exit_statuses_are_the_agreed_numbers() {
    let all = [
        (Exit::Done, 0),
        (Exit::Error, 1),
        (Exit::Usage, 2),
        (Exit::Failed, 3),
        (Exit::RefusedFailed, 4),
        (Exit::RefusedHeld, 5),
    ];
    for (exit, status) in all {
        assert_eq!(exit.status(), status);
    }
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
        queues: 1,
        depth: 64,
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
         --unsafe-allow-unlocked-memory --unsafe-allow-any-filesystem",
    );
    assert_eq!(
        full,
        Serve {
            run_dir: "/tmp/run".into(),
            acknowledge_failure: true,
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

/// The fault hooks of the rig, in a build that has them: the log fails
/// after N requests; one request is held, a read or a write that covers
/// a byte or a flush; the boot id comes from a file; the recovery is
/// short by N records.
#[test]
fn the_test_hooks_are_parsed_in_a_test_build() {
    let hooks = serve(
        "serve --dir /v/a --fail-log-after 100 --hold-one-request 300:read:4096 \
         --boot-id-file /tmp/boot --recover-short 1",
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
            recover_short: 1,
        }
    );
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
        "serve --dir /v/a --recover-short 1",
        "create --dir /v/a --size 1048576 --base-pattern 1",
    ] {
        assert!(
            matches!(plain(hook), Err(UsageError::TestOnly(_))),
            "{hook}"
        );
        assert!(hooked(hook).is_ok(), "{hook}");
    }
    // The build decides for the public entry point.
    let built = parse(args("serve --dir /v/a --recover-short 1"));
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
        Ok(Command::Detach(volume.clone()))
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
