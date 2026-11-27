use std::io;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::*;

const LIMIT: Duration = Duration::from_secs(10);

/// Held by every test that runs a program, as the count of children
/// left running ([`stuck`]) is the process's own.
static RUNS: Mutex<()> = Mutex::new(());

fn runs() -> MutexGuard<'static, ()> {
    RUNS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sh(script: &str, stdin: &[u8], limit: Duration) -> io::Result<Output> {
    let _runs = runs();
    System.run("sh", &["-c", script], stdin, limit)
}

/// Whether process `pid` has ended: gone, or a zombie that nobody here
/// reaps (the container's first process may not).
fn ended(pid: &str) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z')),
    }
}

/// Wait up to five seconds for `done`.
fn soon(mut done: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
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
    let _runs = runs();
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

/// A program killed at its limit is killed with its process group, what
/// it started included, and is reaped: both are gone soon after, and it
/// is no longer counted as left running. Mutants: the child alone
/// killed, the group not its own, no reaper, the count not taken down.
#[test]
fn a_program_past_its_limit_goes_with_what_it_started() {
    let dir = tempfile::tempdir().expect("a directory");
    let pids = dir.path().join("pids");
    let script = format!("sleep 30 & echo $$ $! > {}; exec sleep 30", pids.display());
    let _runs = runs();
    let before = stuck();
    let e = System
        .run("sh", &["-c", &script], &[], Duration::from_millis(300))
        .expect_err("past its limit");
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    let pids = std::fs::read_to_string(&pids).expect("the pids");
    let pids: Vec<&str> = pids.split_whitespace().collect();
    assert_eq!(pids.len(), 2, "{pids:?}");
    for pid in &pids {
        assert!(soon(|| ended(pid)), "{pid} still runs");
    }
    // At most as many as before, not as many: a child of the test before
    // this one (killed at its limit) can still be counted when this one
    // begins, and is reaped while it runs.
    assert!(
        soon(|| stuck() <= before),
        "{} left counted, {before} before",
        stuck()
    );
}

/// At [`STUCK_MAX`] children left running, a call is refused with the
/// error [`STUCK`] and starts nothing; below it, calls run. A limit too
/// long to add to the clock is refused, not a panic. Mutants: no cap,
/// the cap off by one, an overflowing limit.
#[test]
fn at_the_most_children_left_running_a_call_is_refused() {
    let _runs = runs();
    // The children of the other tests, killed, are reaped.
    assert!(soon(|| stuck() == 0), "{} left counted", stuck());
    STUCK_NOW.fetch_add(STUCK_MAX, Ordering::SeqCst);
    let refused = System.run("true", &[], &[], LIMIT);
    STUCK_NOW.fetch_sub(1, Ordering::SeqCst);
    let below = System.run("true", &[], &[], LIMIT);
    STUCK_NOW.fetch_sub(STUCK_MAX - 1, Ordering::SeqCst);
    let e = refused.expect_err("at the cap");
    assert_eq!(e.kind(), io::ErrorKind::WouldBlock, "{e}");
    assert!(e.to_string().contains(STUCK), "{e}");
    assert_eq!(below.expect("below the cap").code, Some(0));
    let e = System
        .run("true", &[], &[], Duration::MAX)
        .expect_err("too long");
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{e}");
}

/// The step's commands are counted apart (the lead's ruling on M2 of
/// #179's review): at [`STUCK_MAX`] other children left running, a
/// command of the step still runs; at [`STUCK_MAX`] of the step's, it is
/// refused while the others run. Mutants: one count for both, the
/// step's count not checked.
#[test]
fn the_step_s_commands_are_counted_apart() {
    let _runs = runs();
    assert!(soon(|| stuck() == 0 && stuck_step() == 0), "left counted");
    STUCK_NOW.fetch_add(STUCK_MAX, Ordering::SeqCst);
    let step = System.run_step("true", &[], &[], LIMIT);
    STUCK_NOW.fetch_sub(STUCK_MAX, Ordering::SeqCst);
    STEP_STUCK_NOW.fetch_add(STUCK_MAX, Ordering::SeqCst);
    let refused = System.run_step("true", &[], &[], LIMIT);
    let other = System.run("true", &[], &[], LIMIT);
    STEP_STUCK_NOW.fetch_sub(STUCK_MAX, Ordering::SeqCst);
    assert_eq!(step.expect("the step beside others held").code, Some(0));
    let e = refused.expect_err("at the step's cap");
    assert_eq!(e.kind(), io::ErrorKind::WouldBlock, "{e}");
    assert_eq!(other.expect("others beside the step's held").code, Some(0));
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
    let _runs = runs();
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
