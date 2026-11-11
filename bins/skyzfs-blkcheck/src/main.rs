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
//! - `CMD kill N`: kill the server of volume N with SIGKILL and serve
//!   the volume again. Prints the device path.
//! - `CMD fail N`: make the write log of volume N fail for good. Only
//!   called with `--can fail-log`.
//! - `CMD reopen N`: volume N has failed and its device is closed;
//!   serve it again as the operator of a repaired volume would. Prints
//!   the device path. Only called with `--can reopen-failed`.
//! - `CMD destroy N`: stop serving volume N and remove it.
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
//! - A request reaches the server only after the kernel has had it. The
//!   kernel answers a misaligned or out-of-range request itself and
//!   splits a long one, so the suite's rejection cases test the kernel.
//! - After a kill the kernel issues the requests in flight again by
//!   itself. The calls this tool had outstanding simply return when the
//!   new server answers; their results are handed to the suite when it
//!   resubmits the same ids, and the resubmission is not issued a
//!   second time.

#[cfg(target_os = "linux")]
mod device;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    device::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("skyzfs-blkcheck drives a Linux block device and does nothing elsewhere");
    std::process::ExitCode::from(2)
}
