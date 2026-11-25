//! Next look at #147, scratch tests of the runner (planted in a worktree
//! at the head as `crates/skyzfs-agent/src/host/nl147_host.rs` with
//! `#[cfg(test)] mod nl147_host;` in `host.rs`, then removed). They take
//! the tests' lock (`RUNS`) through `super::tests`' helpers being
//! private, so they run with `--test-threads=1` instead.
//!
//! h1: the child ends at once and leaves a child of its own, in its
//!     group, holding the pipes past the limit: the call returns at the
//!     limit; is the grandchild killed? (The group is killed only on the
//!     path where the child itself is past its limit.)
//! h2: the child is past its limit and its child called setsid (a group
//!     of its own): the child is killed and reaped; the grandchild?
//! h3: a child that stops itself (SIGSTOP) is killed and reaped.
//! h4: five calls in a row past their limit: none is refused (each child
//!     is reaped soon after), and the count is back to where it was.
//! h5: threads: after h1 and h2 the process's thread count, then once
//!     the grandchildren have ended.

use std::io;
use std::time::{Duration, Instant};

use super::*;

fn ended(pid: &str) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z')),
    }
}

fn soon(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").map(|d| d.count()).unwrap_or(0)
}

fn pids(path: &std::path::Path) -> Vec<String> {
    soon(Duration::from_secs(2), || path.exists());
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect()
}

#[test]
fn h1_a_grandchild_holding_the_pipes_after_the_child_ended() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("pids");
    let before = threads();
    let script = format!("sleep 3 & echo $! > {}; exit 0", file.display());
    let began = Instant::now();
    let e = System
        .run("sh", &["-c", &script], &[], Duration::from_millis(300))
        .expect_err("the pipes stay open");
    let took = began.elapsed();
    let p = pids(&file);
    let alive = !ended(&p[0]);
    eprintln!(
        "h1: {:?} after {took:?}; grandchild {} alive after the call: {alive}; threads {before} -> {}; stuck {}",
        e.kind(),
        p[0],
        threads(),
        stuck()
    );
    let gone = soon(Duration::from_secs(5), || ended(&p[0]));
    eprintln!("h1: grandchild ended within 5 s: {gone}; threads now {}", threads());
}

#[test]
fn h2_a_setsid_grandchild_escapes_the_group_kill() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("pids");
    let before = threads();
    let s0 = stuck();
    // The grandchild in a session (and group) of its own, holding the
    // pipes; the child past its limit.
    let script = format!(
        "setsid sh -c 'echo $$ > {}; exec sleep 3' & exec sleep 30",
        file.display()
    );
    let began = Instant::now();
    let e = System
        .run("sh", &["-c", &script], &[], Duration::from_millis(300))
        .expect_err("past its limit");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    let p = pids(&file);
    let alive = soon(Duration::from_millis(500), || ended(&p[0]));
    eprintln!(
        "h2: TimedOut after {took:?}; setsid grandchild {} ended within 0.5 s: {alive}; threads {before} -> {}; stuck {s0} -> {}",
        p[0],
        threads(),
        stuck()
    );
    let back = soon(Duration::from_secs(2), || stuck() == s0);
    eprintln!("h2: stuck back to {s0} within 2 s: {back}");
    let gone = soon(Duration::from_secs(5), || ended(&p[0]));
    eprintln!("h2: grandchild ended within 5 s: {gone}; threads now {}", threads());
}

#[test]
fn h3_a_stopped_child_is_killed_and_reaped() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("pids");
    let s0 = stuck();
    let script = format!("echo $$ > {}; kill -STOP $$; echo resumed", file.display());
    let e = System
        .run("sh", &["-c", &script], &[], Duration::from_millis(300))
        .expect_err("stopped past its limit");
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    let p = pids(&file);
    let gone = soon(Duration::from_secs(2), || ended(&p[0]));
    let back = soon(Duration::from_secs(2), || stuck() == s0);
    eprintln!("h3: stopped child {} gone: {gone}; stuck back: {back}", p[0]);
    assert!(gone && back);
}

#[test]
fn h4_five_calls_past_their_limit_in_a_row_are_not_refused() {
    let s0 = stuck();
    for i in 0..5 {
        let e = System
            .run(
                "sh",
                &["-c", "trap '' TERM INT HUP; sleep 30"],
                &[],
                Duration::from_millis(100),
            )
            .expect_err("past its limit");
        assert_eq!(e.kind(), io::ErrorKind::TimedOut, "call {i}: {e}");
    }
    let back = soon(Duration::from_secs(2), || stuck() == s0);
    eprintln!("h4: five TimedOut, none refused; stuck back to {s0}: {back}");
    assert!(back);
}
