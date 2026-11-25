//! Review of #147, scratch tests of the runner (`System::run`): what
//! happens to the child, its own children, the threads and the zombies
//! after a call past its limit. Planted as `host/scratch147.rs` with
//! `#[cfg(test)] mod scratch147;` in `host.rs`; run alone, one thread:
//! `cargo test -p skyzfs-agent --lib host::scratch147 -- --test-threads=1 --nocapture`.
//! A process in the kernel's uninterruptible wait (D) cannot be made in
//! a container; a child of the child that keeps the pipes and is not
//! killed stands in for "a process the kill does not end".

use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::*;

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").map(|d| d.count()).unwrap_or(0)
}

/// The state letter of `pid` (`R`, `S`, `Z`...), or `None` if it is gone.
fn state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.trim_start().chars().next()
}

fn pid_in(path: &Path) -> u32 {
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        sleep(Duration::from_millis(10));
    }
    panic!("no pid in {}", path.display());
}

/// Zombies whose parent is this process.
fn zombies() -> Vec<u32> {
    let me = std::process::id();
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").expect("proc").flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some((_, after)) = stat.rsplit_once(')') else {
            continue;
        };
        let fields: Vec<&str> = after.split_whitespace().collect();
        if fields.first() == Some(&"Z") && fields.get(1).and_then(|p| p.parse::<u32>().ok()) == Some(me)
        {
            out.push(pid);
        }
    }
    out
}

fn sh(script: &str, stdin: &[u8], limit: Duration) -> io::Result<Output> {
    System.run("sh", &["-c", script], stdin, limit)
}

/// R1: the child past its limit is killed (it never reaches its next
/// step) and reaped (no zombie), and the call returns at the limit.
/// A plant that drops the kill (`abandon` not called, the child only
/// dropped) passes every test of the PR; this one fails it.
#[test]
fn r1_the_child_is_killed_and_reaped() {
    let dir = tempfile::tempdir().expect("dir");
    let pidf = dir.path().join("pid");
    let mark = dir.path().join("mark");
    let script = format!(
        "echo $$ > {}; sleep 2; touch {}",
        pidf.display(),
        mark.display()
    );
    let began = Instant::now();
    let e = sh(&script, &[], Duration::from_millis(300)).expect_err("past its limit");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    let pid = pid_in(&pidf);
    sleep(Duration::from_millis(300));
    println!("r1: returned after {took:?}; child {pid} state 300 ms later: {:?}", state(pid));
    assert_eq!(state(pid), None, "the child is still there (alive or a zombie)");
    sleep(Duration::from_millis(2200));
    assert!(!mark.exists(), "the child was not killed: it reached its next step");
}

/// R2: a child that ignores SIGTERM, SIGINT and SIGHUP is ended all the
/// same (SIGKILL), and reaped.
#[test]
fn r2_a_child_that_ignores_term_is_killed() {
    let dir = tempfile::tempdir().expect("dir");
    let pidf = dir.path().join("pid");
    let script = format!(
        "trap '' TERM INT HUP; echo $$ > {}; while :; do sleep 0.1; done",
        pidf.display()
    );
    let began = Instant::now();
    let e = sh(&script, &[], Duration::from_millis(300)).expect_err("past its limit");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    let pid = pid_in(&pidf);
    sleep(Duration::from_millis(300));
    println!("r2: returned after {took:?}; child state 300 ms later: {:?}", state(pid));
    assert!(took < Duration::from_millis(800), "{took:?}");
    assert_eq!(state(pid), None);
}

/// R3: a child stopped by SIGSTOP is past its limit like any other, and
/// SIGKILL ends it.
#[test]
fn r3_a_stopped_child_is_killed() {
    let dir = tempfile::tempdir().expect("dir");
    let pidf = dir.path().join("pid");
    let script = format!("echo $$ > {}; kill -STOP $$", pidf.display());
    let began = Instant::now();
    let e = sh(&script, &[], Duration::from_millis(300)).expect_err("past its limit");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    let pid = pid_in(&pidf);
    sleep(Duration::from_millis(300));
    println!("r3: returned after {took:?}; child state: {:?}", state(pid));
    assert_eq!(state(pid), None);
}

/// R4: a child of the child that holds the pipes is not killed (no
/// process group): it outlives the call, and the two threads that read
/// the pipes wait for it. The call itself returns at the limit.
/// Recorded, not judged: the counts are printed.
#[test]
fn r4_a_grandchild_outlives_the_call_and_holds_two_threads() {
    let dir = tempfile::tempdir().expect("dir");
    let gpid = dir.path().join("gpid");
    let before = threads();
    let script = format!("sleep 3 & echo $! > {}; exec sleep 30", gpid.display());
    let began = Instant::now();
    let e = sh(&script, &[], Duration::from_millis(300)).expect_err("past its limit");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    let grandchild = pid_in(&gpid);
    sleep(Duration::from_millis(300));
    let during = threads();
    println!(
        "r4: returned after {took:?}; grandchild {grandchild} state {:?}; threads {before} before, {during} after the call",
        state(grandchild)
    );
    assert!(state(grandchild).is_some(), "the grandchild was killed");
    sleep(Duration::from_millis(3300));
    let later = threads();
    println!("r4: threads once the grandchild has ended: {later}");
}

/// R5: N calls past their limit, each leaving a grandchild that holds
/// the pipes for 4 s: the threads that stay, and the zombies.
#[test]
fn r5_threads_and_zombies_after_many_calls() {
    let before = threads();
    for _ in 0..8 {
        let _ = sh("sleep 4 & exec sleep 30", &[], Duration::from_millis(50));
    }
    sleep(Duration::from_millis(300));
    let during = threads();
    let z = zombies();
    println!("r5: threads {before} before 8 calls, {during} 300 ms after; zombies {z:?}");
    assert!(z.is_empty(), "zombies left: {z:?}");
    sleep(Duration::from_millis(4500));
    println!("r5: threads 4.8 s after: {}", threads());
}

/// R6: a child that ends 0 while a child of its own holds its output is
/// read as `TimedOut`: its exit code and output are lost. For `zfs
/// program` (the snapshot taken) that is `Other`, which the planner
/// meets with `ManifestLost` at the next round (safe).
#[test]
fn r6_an_exit_0_with_output_held_is_timed_out() {
    let e = sh("echo '{\"return\":{}}'; sleep 3 & exit 0", &[], Duration::from_millis(300))
        .expect_err("held");
    println!("r6: {e}");
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
}

/// R7: input larger than a pipe's buffer to a child that never reads
/// it: the call returns at the limit, and the writer's thread ends with
/// the kill (EPIPE, SIGPIPE ignored by Rust's runtime).
#[test]
fn r7_input_to_a_child_that_never_reads() {
    let before = threads();
    let input = vec![b'x'; 1 << 20];
    let began = Instant::now();
    let e = System
        .run("sleep", &["30"], &input, Duration::from_millis(300))
        .expect_err("past its limit");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    sleep(Duration::from_millis(300));
    let after = threads();
    println!("r7: returned after {took:?}; threads {before} before, {after} 300 ms after");
    assert!(after <= before, "a thread stayed: {before} -> {after}");
}

/// R8: output past the bound from a child that then runs on: the call
/// waits for the child to the limit (it does not stop at the bound).
#[test]
fn r8_output_past_the_bound_then_running() {
    let script = format!("head -c {} /dev/zero; sleep 30", OUTPUT_MAX + 4096);
    let began = Instant::now();
    let e = sh(&script, &[], Duration::from_secs(2)).expect_err("too much, then past its limit");
    println!("r8: {:?} after {:?}: {e}", e.kind(), began.elapsed());
}
