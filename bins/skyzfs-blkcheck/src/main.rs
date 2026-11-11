//! Runs the `skyzfs-blk` conformance suite against a block device, so
//! that a transport is tested through the kernel and not only through
//! its library interface (design §4.1; kernel rig, check c).
//!
//! ```text
//! skyzfs-blkcheck --harness CMD [--can fail-log] [--can reopen-failed]
//!                 [--depth N] [--cases N] [--rounds N]
//!                 [--seed N] [--timeout SECONDS] [--case NAME]...
//! ```
//!
//! `CMD` is a program that knows how to make, kill and fail a served
//! volume; this tool knows nothing about the server. It is called as:
//!
//! - `CMD create N`: make volume number N, empty, and serve it. Prints
//!   the path of its block device on the last line of stdout.
//! - `CMD pid N`: print the pid of the server of volume N. If the
//!   helper has this, the tool sends the SIGKILL itself the moment the
//!   suite asks for a kill, and calls `CMD kill N` for the rest: a
//!   signal sent through the helper lands tens of milliseconds later,
//!   when a fast server has answered every request long before.
//! - `CMD kill N`: kill the server of volume N with SIGKILL, print
//!   `dead R W` when it is gone, and serve the volume again. Prints the
//!   device path on the last line.
//! - `CMD fail N`: make the write log of volume N fail for good. Only
//!   called with `--can fail-log`.
//! - `CMD reopen N`: volume N has failed and its device is closed;
//!   serve it again as the operator of a repaired volume would. Prints
//!   the device path. Only called with `--can reopen-failed`.
//! - `CMD destroy N`: stop serving volume N and remove it.
//!
//! Before each case a line `declared<TAB>name<TAB>STATUS` says how the
//! suite itself declares the case for a harness with these capabilities
//! (`PASS`, `WEAKER` or `NOT_EXERCISED`); `--list` prints only those and
//! runs nothing. A result other than the declared one is a finding.
//!
//! One line per case goes to stdout, `case<TAB>name<TAB>RESULT<TAB>detail`,
//! where RESULT is `PASS`, `FAIL`, `WEAKER` (the rule held in a weaker
//! form than the case has on a harness that can do everything; the
//! detail says what was left out), `NOT_EXERCISED` (the suite's own word
//! for a rule the target or the harness cannot exercise) or
//! `UNSUPPORTED` (the suite asked the harness for something it had said
//! it cannot do, which is a fault of the suite). The harness tells the
//! suite that it has no power cut, no base, no read-only volume and no
//! invalid requests, and a failing log and a reopened failed volume
//! only with `--can`. The exit status is 0 whatever the
//! cases did, 2 for a usage error: judging is the rig's table, not this
//! tool.
//!
//! What a block device changes, and this tool does not hide:
//!
//! - A volume whose log has failed has no server: the server deleted
//!   its device and exited. The answers a failed volume gives are the
//!   kernel's, for a deleted device this tool still holds open. After
//!   `CMD fail N` returned, an I/O error of any kind is reported to the
//!   suite as "the volume has failed"; a request that succeeds is
//!   reported as a success, which the suite then judges.
//!
//! - A request reaches the server only after the kernel has had it. The
//!   kernel answers a misaligned or out-of-range request itself and
//!   splits a long one, so the suite's rejection cases test the kernel.
//! - After a kill the kernel issues the requests in flight again by
//!   itself. The calls this tool had outstanding at the server's death
//!   simply return when the new server answers; their results are
//!   handed to the suite when it resubmits the same ids, and the
//!   resubmission is not issued a second time. A call that had already
//!   returned when the server died, and whose result the suite had not
//!   been given yet, is another matter: the kernel issues nothing of it
//!   again, while the suite's model takes every unacknowledged request
//!   to be applied from the kill on. Its resubmission is issued. To
//!   tell the two apart, `CMD kill N` prints the line `dead R W` when
//!   the old server is gone and before the new one is started: no
//!   request completes in between, so what has returned by that line
//!   was completed before the kill. R and W are the kernel's counts of
//!   reads and writes in flight at the device then. The tool checks
//!   its sorting twice and exits with status 3, an error of the rig
//!   and not a verdict, if either check fails: a call taken as in
//!   flight must not have returned before the sorting, and the calls
//!   still outstanding must not outnumber the requests the kernel
//!   counts. Without that line every outstanding call is taken as in
//!   flight at the death.
//!
//! `skyzfs-blkcheck --replay TRACE VOLUME [SIZE_MIB]` gives a trace
//! written with `BLKCHECK_TRACE=FILE` back to the suite's model, event
//! by event, and prints what the model said to each: for the study of a
//! failed case away from the machine it failed on.

#[cfg(target_os = "linux")]
mod device;
mod replay;

/// `--replay TRACE VOLUME [SIZE_MIB]`: a trace through the model.
fn replay_asked() -> Option<std::process::ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--replay") {
        return None;
    }
    let (Some(path), Some(volume)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: skyzfs-blkcheck --replay TRACE VOLUME [SIZE_MIB]");
        return Some(std::process::ExitCode::from(2));
    };
    let size = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(64);
    Some(replay::replay(path, volume, size))
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    if let Some(code) = replay_asked() {
        return code;
    }
    device::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    if let Some(code) = replay_asked() {
        return code;
    }
    eprintln!("skyzfs-blkcheck drives a Linux block device and does nothing elsewhere");
    std::process::ExitCode::from(2)
}
