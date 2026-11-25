use std::io;
use std::time::{Duration, Instant};

use super::*;

const LIMIT: Duration = Duration::from_secs(10);

fn sh(script: &str, stdin: &[u8], limit: Duration) -> io::Result<Output> {
    System.run("sh", &["-c", script], stdin, limit)
}

/// A program's exit code and both its outputs come back, and it runs in
/// the locale `C`. Mutants: the outputs swapped, the code lost, the
/// locale not set.
#[test]
fn a_program_gives_its_code_and_both_outputs_in_the_c_locale() {
    let out = sh("echo out; echo err >&2; echo $LC_ALL; exit 3", &[], LIMIT).expect("ran");
    assert_eq!(out.code, Some(3));
    assert_eq!(out.stdout, b"out\nC\n");
    assert_eq!(out.stderr, b"err\n");
}

/// Its standard input is what was given, and a program given none
/// reads an end at once. Mutant: the input not written.
#[test]
fn a_program_reads_what_it_is_given() {
    let out = System.run("cat", &[], b"a manifest", LIMIT).expect("ran");
    assert_eq!((out.code, out.stdout), (Some(0), b"a manifest".to_vec()));
    let out = System.run("cat", &[], &[], LIMIT).expect("ran");
    assert_eq!((out.code, out.stdout), (Some(0), Vec::new()));
}

/// A program that does not end is killed at the limit, and the call
/// returns at once: its output is not waited for, even where a child of
/// its own holds it open (as a program stuck on a suspended pool would
/// hold everything). Mutants: no limit; the child, or its output,
/// waited for after the kill.
#[test]
fn a_program_past_its_limit_is_killed_and_not_waited_for() {
    let limit = Duration::from_millis(300);
    let began = Instant::now();
    let e = sh("sleep 30 & exec sleep 30", &[], limit).expect_err("past its limit");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    assert!(e.to_string().contains("killed"), "{e}");
    assert!(
        took >= limit && took < limit + Duration::from_secs(1),
        "{took:?}"
    );
}

/// A program that ends and leaves its output open to a child of its own
/// is not waited for past the limit either. Mutant: the output read
/// with no limit.
#[test]
fn an_output_held_open_past_the_end_is_waited_for_only_to_the_limit() {
    let limit = Duration::from_millis(300);
    let began = Instant::now();
    let e = sh("sleep 30 &", &[], limit).expect_err("its output stays open");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    assert!(e.to_string().contains("stayed open"), "{e}");
    assert!(took < limit + Duration::from_secs(1), "{took:?}");
}

/// More output than [`OUTPUT_MAX`] is an error, not held in memory.
/// Mutant: no bound.
#[test]
fn more_output_than_the_bound_is_an_error() {
    let script = format!("head -c {} /dev/zero", OUTPUT_MAX + 1);
    let e = sh(&script, &[], LIMIT).expect_err("too much");
    assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{e}");
    let out = sh(&format!("head -c {OUTPUT_MAX} /dev/zero"), &[], LIMIT).expect("at the bound");
    assert_eq!(out.stdout.len(), OUTPUT_MAX);
}

/// A program that cannot be started is an error of the run.
#[test]
fn a_program_not_found_is_an_error() {
    let e = System
        .run("skyzfs-agent-no-such-program", &[], &[], LIMIT)
        .expect_err("not found");
    assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}");
}

/// `stat` tells a regular file from anything else, following links; a
/// block device is not here to stat (no such device in a container),
/// and is read on the rigs.
#[test]
fn stat_tells_a_file_from_anything_else() {
    let dir = tempfile::tempdir().expect("a directory");
    let file = dir.path().join("f");
    std::fs::write(&file, b"x").expect("written");
    let link = dir.path().join("l");
    std::os::unix::fs::symlink(&file, &link).expect("linked");
    assert_eq!(System.stat(&file).expect("read"), Node::File);
    assert_eq!(System.stat(&link).expect("read"), Node::File);
    assert_eq!(System.stat(dir.path()).expect("read"), Node::Other);
    assert_eq!(
        System.stat(Path::new("/dev/null")).expect("read"),
        Node::Other
    );
    let e = System.stat(&dir.path().join("none")).expect_err("none");
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
}
