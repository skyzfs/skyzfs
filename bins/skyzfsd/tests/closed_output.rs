//! The binary with its output closed under it: a write that fails is
//! ignored, and the exit status is the command's own. With `println!`
//! the process panicked instead (status 101), which on the path of a
//! failed volume would have replaced status 3.

use std::process::{Command, Stdio};

/// A pipe's write end whose read end is closed: every write fails.
fn closed() -> Stdio {
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    Stdio::from(writer)
}

fn skyzfsd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_skyzfsd"))
}

#[test]
fn help_into_a_closed_pipe_exits_0() {
    let status = skyzfsd()
        .arg("help")
        .stdout(closed())
        .stderr(closed())
        .status()
        .expect("run");
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn a_usage_error_with_standard_error_closed_exits_with_the_usage_status() {
    let with_output = skyzfsd().arg("no-such-command").output().expect("run");
    let usage = with_output.status.code();
    assert!(
        usage.is_some_and(|code| code != 0 && code != 101),
        "{usage:?}"
    );
    assert!(!with_output.stderr.is_empty());
    let status = skyzfsd()
        .arg("no-such-command")
        .stdout(closed())
        .stderr(closed())
        .status()
        .expect("run");
    assert_eq!(status.code(), usage);
}

#[test]
fn an_error_of_a_command_with_its_output_closed_keeps_its_status() {
    let dir = tempfile::tempdir().expect("a directory");
    let missing = dir.path().join("missing").join("volume");
    let run = |closed_output: bool| {
        let mut command = skyzfsd();
        command
            .args(["create", "--size", "1048576", "--run-dir"])
            .arg(dir.path().join("run"))
            .arg("--dir")
            .arg(&missing);
        if closed_output {
            command.stdout(closed()).stderr(closed());
        } else {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        command.output().expect("run")
    };
    let open = run(false);
    assert_eq!(open.status.code(), Some(1), "{open:?}");
    assert_eq!(run(true).status.code(), Some(1));
}
