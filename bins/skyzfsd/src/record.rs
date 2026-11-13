//! The test build's record of the requests the kernel hands the daemon
//! and of the answers it is given: `serve --request-log PATH`, as a
//! queue around the queue (below the delays of [`crate::delay`], next
//! to the kernel).
//!
//! It is there so that a failure of the rig's conformance suite can be
//! read from both sides. The suite's tool records the calls it makes on
//! the device; this records the requests that reached the server. When
//! the block layer cuts one call into several requests, the two records
//! differ, and only this one shows it.
//!
//! # The file
//!
//! Each instance appends to `PATH`, so that the instances of one volume
//! (a take-over after a kill) write one file. Each line is one `write`
//! to a file opened with `O_APPEND`, made when the event happens: a
//! `kill -9` loses nothing that was written, and two instances never
//! interleave inside a line. Lines are tab-separated, eight columns:
//!
//! ```text
//! mono_ns  pid  event  tag  op  sector  sectors  result
//! ```
//!
//! - `mono_ns`: `CLOCK_MONOTONIC` in nanoseconds when the line was
//!   made, so that it lines up with the kernel's trace events when
//!   their clock is `mono`;
//! - `pid`: the instance;
//! - `sector`, `sectors`: as the kernel gave them, in 512-byte units;
//! - a `-` where a column does not apply.
//!
//! The events:
//!
//! - `start`: once per instance, when its queue starts. The tag column
//!   holds the device's number and the result column the queue's depth.
//! - `recv`: a request the kernel handed over, as the queue's wait
//!   returned it: its tag, its operation (`read`, `write`, `flush`,
//!   `discard`, `write_zeroes`, or `op<N>` for another code), sector
//!   and count.
//! - `commit`: the answer for a tag, as handed to the kernel's queue:
//!   the request's operation, sector and count again, and the result
//!   (bytes, or a negated error number). With `--commit-delay-ms` it is
//!   written when the delayed answer goes on, so it is what the kernel
//!   was given, not when the store completed the request.
//! - `sent`: a wait or a flush of the queue returned, so every `commit`
//!   above it has reached the kernel. The result column says how many
//!   commits that covers. A commit that a queue keeps until its next
//!   wait is not with the kernel before this line.
//! - `down`: a wait or a flush of the queue failed; the commits since
//!   the last `sent` (the result column) may not have reached the
//!   kernel.
//!
//! A record that cannot be written does not stop the daemon: the first
//! error is said once on standard error, and the record ends there.

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;

use skyzfs_blk::Buf;
use skyzfs_ublk::pump::{Fetched, Queue, QueueDown, Wake};
use skyzfs_ublk::translate::{op, Desc};

/// Open the record at `path` for appending, creating it if need be.
/// Done before the start, so that a path that cannot be written refuses
/// the serve before any device is touched.
pub fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// The name a line gives an operation code.
fn op_name(code: u8) -> String {
    match code {
        op::READ => "read".into(),
        op::WRITE => "write".into(),
        op::FLUSH => "flush".into(),
        op::DISCARD => "discard".into(),
        op::WRITE_ZEROES => "write_zeroes".into(),
        other => format!("op{other}"),
    }
}

/// `CLOCK_MONOTONIC`, in nanoseconds.
fn mono_ns() -> u128 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let secs = u128::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u128::try_from(now.tv_nsec).unwrap_or(0);
    secs * 1_000_000_000 + nanos
}

/// One line of the record, without the time and the pid.
fn columns(event: &str, tag: &str, desc: Option<&Desc>, result: &str) -> String {
    let (op, sector, sectors) = match desc {
        Some(desc) => (
            op_name(desc.op()),
            desc.start_sector.to_string(),
            desc.nr_sectors.to_string(),
        ),
        None => ("-".into(), "-".into(), "-".into()),
    };
    format!("{event}\t{tag}\t{op}\t{sector}\t{sectors}\t{result}")
}

/// A queue whose requests and answers are written down, if it was given
/// a file: see the [module documentation](self).
pub struct Recorded<Q: Queue> {
    inner: Q,
    file: Option<File>,
    pid: u32,
    /// The request each tag holds, as the kernel handed it over.
    held: Vec<Option<Desc>>,
    /// Commits handed on since the last `sent` or `down` line.
    unsent: u64,
}

impl<Q: Queue> Recorded<Q> {
    /// `inner`, with its requests and answers written to `file` if
    /// there is one; the `start` line names `device`.
    pub fn new(inner: Q, file: Option<File>, device: u32) -> Self {
        let depth = inner.depth();
        let mut recorded = Recorded {
            inner,
            file,
            pid: std::process::id(),
            held: vec![None; usize::from(depth)],
            unsent: 0,
        };
        recorded.line(&columns(
            "start",
            &device.to_string(),
            None,
            &depth.to_string(),
        ));
        recorded
    }

    fn line(&mut self, columns: &str) {
        let Some(file) = &mut self.file else {
            return;
        };
        let text = format!("{}\t{}\t{columns}\n", mono_ns(), self.pid);
        if let Err(e) = file.write_all(text.as_bytes()) {
            crate::to_stderr(&format!(
                "skyzfsd: the request log cannot be written ({e}); it ends here\n"
            ));
            self.file = None;
        }
    }

    /// After a wait or a flush: what the commits since the last line of
    /// this kind came to.
    fn sent(&mut self, ok: bool) {
        if self.unsent == 0 {
            return;
        }
        let event = if ok { "sent" } else { "down" };
        let count = self.unsent.to_string();
        self.unsent = 0;
        self.line(&columns(event, "-", None, &count));
    }
}

impl<Q: Queue> Queue for Recorded<Q> {
    fn depth(&self) -> u16 {
        self.inner.depth()
    }

    fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown> {
        let before = fetched.len();
        let woken = self.inner.wait(fetched);
        self.sent(woken.is_ok());
        for Fetched { tag, desc } in fetched.get(before..).unwrap_or_default().to_vec() {
            if let Some(slot) = self.held.get_mut(usize::from(tag)) {
                *slot = Some(desc);
            }
            self.line(&columns("recv", &tag.to_string(), Some(&desc), "-"));
        }
        woken
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        self.inner.take_buf(tag, blocks)
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        let desc = self.held.get_mut(usize::from(tag)).and_then(Option::take);
        self.line(&columns(
            "commit",
            &tag.to_string(),
            desc.as_ref(),
            &result.to_string(),
        ));
        self.unsent += 1;
        self.inner.commit(tag, result, buf);
    }

    fn pause(&mut self) {
        self.inner.pause();
    }

    fn flush(&mut self) -> Result<(), QueueDown> {
        let flushed = self.inner.flush();
        self.sent(flushed.is_ok());
        flushed
    }
}

#[cfg(test)]
mod tests;
