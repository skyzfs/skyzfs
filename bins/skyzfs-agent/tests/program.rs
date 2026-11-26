//! The program as a process: what it prints and how it exits where no
//! pool is needed, and its test hooks on a fake `zpool` and `zfs`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const AGENT: &str = env!("CARGO_BIN_EXE_skyzfs-agent");

const REMOTE: &str = "skyzfs-remote 1\nendpoint http://127.0.0.1:9000\nbucket volumes\n\
                      volume pg-data\nid 01ARZ3NDEKTSV4RRFFQ69G5FAV\n";

fn agent(args: &[&str], path: Option<&Path>) -> Output {
    let mut command = Command::new(AGENT);
    command.args(args);
    if let Some(dir) = path {
        let path = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{path}", dir.display()));
    }
    command.output().expect("the agent runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A volume's directory with its `remote`, and the agent's directory.
fn dirs(top: &Path) -> (String, String, String) {
    let volume = top.join("vol");
    fs::create_dir(&volume).expect("vol");
    fs::write(volume.join("remote"), REMOTE).expect("remote");
    let run = top.join("run");
    fs::create_dir(&run).expect("run");
    let dir = top.join("agent");
    let s = |p: &Path| p.display().to_string();
    (s(&volume), s(&run), s(&dir))
}

#[test]
fn record_of_a_directory_with_none_prints_absent() {
    let top = tempfile::tempdir().expect("tmp");
    let out = agent(
        &["record", "--dir", &top.path().display().to_string()],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "absent\n");
    fs::write(top.path().join("record"), b"not a record").expect("write");
    let out = agent(
        &["record", "--dir", &top.path().display().to_string()],
        None,
    );
    assert_eq!(out.status.code(), Some(6), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("corrupt"),
        "{}",
        text(&out.stderr)
    );
}

/// A path that is not a directory is an error, not `absent` (S2 of
/// #149's review); a closed pipe on standard output is no panic (S4).
#[test]
fn record_of_what_is_not_a_directory_is_an_error() {
    let top = tempfile::tempdir().expect("tmp");
    let missing = top.path().join("missing");
    let out = agent(&["record", "--dir", &missing.display().to_string()], None);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "");
    assert!(
        text(&out.stderr).contains("missing"),
        "{}",
        text(&out.stderr)
    );
    let file = top.path().join("file");
    fs::write(&file, b"").expect("a file");
    let out = agent(&["record", "--dir", &file.display().to_string()], None);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("not a directory"),
        "{}",
        text(&out.stderr)
    );
}

/// `record` and `--help` into a pipe whose reader has gone: no panic,
/// and the exit is the command's (S4 of #149's review).
#[test]
fn a_closed_pipe_on_standard_output_is_no_panic() {
    let top = tempfile::tempdir().expect("tmp");
    let dir = top.path().display().to_string();
    for args in [&["record", "--dir", &dir][..], &["--help"][..]] {
        let (reader, writer) = std::io::pipe().expect("a pipe");
        drop(reader);
        let out = Command::new(AGENT)
            .args(args)
            .stdout(writer)
            .stderr(std::process::Stdio::piped())
            .output()
            .expect("the agent runs");
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    }
}

/// A second agent on the same directory exits 8 and touches nothing
/// (T2 of #149's review).
#[test]
fn an_agent_whose_directory_another_holds_exits_8() {
    let top = tempfile::tempdir().expect("tmp");
    let (volume, run, dir) = dirs(top.path());
    let _held = skyzfs_agent::record::Store::open(&dir).expect("the first agent's store");
    let args = [
        "settle",
        "--volume",
        &volume,
        "--run",
        &run,
        "--device",
        "/dev/null",
        "--pool",
        "tank",
        "--dir",
        &dir,
    ];
    let out = agent(&args, None);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(8), "{stderr}");
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn a_usage_error_exits_2_with_the_usage() {
    for args in [&[][..], &["publish"][..], &["record", "--dir"][..]] {
        let out = agent(args, None);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("usage:"), "{args:?}");
    }
    let out = agent(&["--help"], None);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).starts_with("usage:"));
}

#[test]
fn a_volume_with_no_place_in_object_storage_publishes_nothing() {
    let top = tempfile::tempdir().expect("tmp");
    let (volume, run, dir) = dirs(top.path());
    fs::remove_file(Path::new(&volume).join("remote")).expect("rm");
    let args = [
        "publish",
        "--volume",
        &volume,
        "--run",
        &run,
        "--device",
        "/dev/null",
        "--pool",
        "tank",
        "--dir",
        &dir,
    ];
    let out = agent(&args, None);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("remote"),
        "{}",
        text(&out.stderr)
    );
}

#[cfg(not(feature = "test-hooks"))]
#[test]
fn a_build_without_the_hooks_refuses_them() {
    let out = agent(&["record", "--dir", "/d", "--trace-points", "/t"], None);
    assert_eq!(out.status.code(), Some(2));
    let out = agent(
        &[
            "settle",
            "--volume",
            "/v",
            "--run",
            "/r",
            "--device",
            "/d",
            "--pool",
            "tank",
            "--dir",
            "/d",
            "--kill-at",
            "sweep:before",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("test hook"),
        "{}",
        text(&out.stderr)
    );
}

/// Testing's items 1 and 2 on a pool that `zpool` and `zfs` scripts
/// fake: a hold of the agent's tag that no publish names, which the
/// sweep releases, and a kill just before the release. The trace has
/// the point, named for the snapshot, and the agent dies of `SIGKILL`
/// after its line on standard error. First, in a directory with no
/// agent's marker (a mistyped `--dir`), the sweep releases nothing and
/// the agent exits 6 (L4 of #149's review).
#[cfg(feature = "test-hooks")]
#[test]
fn the_agent_traces_a_point_then_dies_of_sigkill_there() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::ExitStatusExt as _;

    let top = tempfile::tempdir().expect("tmp");
    let (volume, run, dir) = dirs(top.path());
    let bin = top.path().join("bin");
    fs::create_dir(&bin).expect("bin");
    let script = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    };
    script(
        "zpool",
        "[ \"$1\" = get ] || exit 1\n\
         printf 'guid\\t11\\nload_guid\\t12\\nmultihost\\toff\\nreadonly\\toff\\ncheckpoint\\t-\\n'\n",
    );
    // The reads are not logged: the counts (`zfs list`) and the sweep's
    // list of snapshots with their count of holds, then `zfs holds` of
    // the one held, `tank@skz-old`. `zfs.calls` holds what else the
    // agent asked.
    script(
        "zfs",
        "case \"$*\" in\n\
         *name,userrefs*) printf 'tank@skz-old\\t1\\n'; exit 0 ;;\n\
         list*) exit 0 ;;\n\
         holds*) printf 'tank@skz-old\\tskyzfs-publish\\tWed Nov 25 21:24 2026\\n'; exit 0 ;;\n\
         esac\n\
         echo \"zfs $*\" >> \"$0.calls\"\n",
    );
    let trace = top.path().join("trace");
    let trace_arg = trace.display().to_string();
    let args = [
        "settle",
        "--volume",
        &volume,
        "--run",
        &run,
        "--device",
        "/dev/null",
        "--pool",
        "tank",
        "--dir",
        &dir,
        "--limit-s",
        "5",
        "--trace-points",
        &trace_arg,
        "--kill-at",
        "sweep:before:1",
    ];
    let out = agent(&args[..args.len() - 2], Some(&bin));
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(6), "{stderr}");
    assert!(stderr.contains("no agent's marker (`agent`)"), "{stderr}");
    assert!(stderr.contains("skz-old"), "{stderr}");
    assert!(!bin.join("zfs.calls").exists());
    assert_eq!(fs::read_to_string(&trace).unwrap_or_default(), "");
    assert!(!Path::new(&dir).join("record").exists());
    // The agent's directory: its marker, as the first save writes it.
    fs::create_dir_all(&dir).expect("the agent's directory");
    fs::write(Path::new(&dir).join("agent"), b"SZAD\x01\x00").expect("the marker");
    let out = agent(&args, Some(&bin));
    let stderr = text(&out.stderr);
    assert_eq!(out.status.signal(), Some(9), "{:?}: {stderr}", out.status);
    assert!(
        stderr.ends_with("skyzfs-agent: test hook --kill-at: killed at sweep before (1)\n"),
        "{stderr}"
    );
    let traced = fs::read_to_string(&trace).expect("the trace");
    let fields: Vec<Vec<&str>> = traced.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(fields.len(), 1, "{traced:?}");
    assert!(fields[0][0].parse::<u64>().is_ok(), "{traced:?}");
    assert_eq!(fields[0][1..], ["sweep", "before", "skz-old"], "{traced:?}");
    // Killed before the release: `zfs release` never ran.
    assert!(!bin.join("zfs.calls").exists());
    // Without the kill, the sweep releases the hold and the trace says so.
    fs::remove_file(&trace).expect("rm");
    let out = agent(&args[..args.len() - 2], Some(&bin));
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let calls = fs::read_to_string(bin.join("zfs.calls")).expect("calls");
    assert!(
        calls.contains("release -r skyzfs-publish tank@skz-old"),
        "{calls}"
    );
    let traced = fs::read_to_string(&trace).expect("the trace");
    let points: Vec<&str> = traced
        .lines()
        .map(|l| l.split_once('\t').map_or("", |(_, rest)| rest))
        .collect();
    assert_eq!(
        points,
        ["sweep\tbefore\tskz-old", "sweep\tafter\tskz-old"],
        "{traced:?}"
    );
}
