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
//! - `send`: a wait or a flush of the queue begins with commits not yet
//!   handed on (the result column says how many: the `commit` lines
//!   since the last `sent` or `down`). The queue hands what its ring
//!   holds to the kernel at the start of that wait, before it waits for
//!   anything.
//! - `sent`: that wait or flush returned, so the commits counted in its
//!   `send` have been handed to the kernel.
//! - `down`: that wait or flush failed; those commits may or may not
//!   have reached the kernel.
//! - `ended`: a command of the queue that the driver ended with a result
//!   other than OK, as the queue's wait returned it (after that wait's
//!   `sent` or `down`, before its `recv` lines): its tag, the command in
//!   the op column (`fetch`, `commit`, or `cmd<N>` for another code), and
//!   the result. `-19` (`-ENODEV`) is the driver's abort: the queue is
//!   being stopped, and each tag's last command ends so. Any other
//!   result is a command that took no effect, which the driver refused
//!   (or io_uring could not hand to it): a refused commit completed
//!   nothing. Whether requests still come on its tag depends on why it
//!   was refused, which the result does not say (see `Queue::wait` in
//!   `skyzfs-ublk`). The daemon counts both (`status`:
//!   `commands_aborted`, `commands_failed`).
//!
//! So a commit reaches the kernel no earlier than its own `commit` line
//! and no later than the `sent` line after it. Mostly that is between
//! the `send` and the `sent`, but not always. The ring has room for as
//! many entries as the queue is deep, and when a commit finds it full
//! the library hands what it holds to the kernel from inside that
//! commit. After a wake, the wake's poll waits in the ring until the
//! next submit, so a turn that then commits on every tag hands all but
//! the last commit over before its `send` line.
//!
//! A wait returns at the next request, a wake, or the library's idle
//! time-out of 20 s, so the gap before `sent` can be long: the kernel
//! may have ended the calls of those commits, and their callers seen
//! them return, well before that line. A `kill -9` inside the wait
//! leaves `commit` lines (and a `send`) with no `sent`, whose commits
//! the kernel may or may not have.
//!
//! # What the record cannot show
//!
//! - **Handed to the kernel is not taken by the driver** until the
//!   driver says so: a commit it refuses has an `ended` line, at the
//!   queue's next wait, and until then looks here like one it took. The
//!   kernel's own trace (`block_rq_complete`, `block_rq_requeue`) is the
//!   other side of it.
//! - **A command the driver ends in the queue's last flush** (at a stop)
//!   has no `ended` line: that flush hands nothing on.
//! - **At a stop** (not at a kill), once the library has seen an abort
//!   it puts no more commits on its ring, while its commit still
//!   returns as done: such a commit has its `commit`, `send` and `sent`
//!   lines and never reached the kernel. This is detach, a deleted
//!   device, a failed volume.
//! - **A request that arrives during the queue's last flush** (at a
//!   stop) is dropped by the queue and has no `recv` line, though the
//!   kernel's trace shows it issued.
//! - **The record costs** two or three `write` calls per request on the
//!   queue's thread: a rate measured with it is not the rate without it.
//!
//! A record that cannot be written does not stop the daemon: the first
//! error is said once on standard error, and the record ends there.

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;

use skyzfs_blk::Buf;
use skyzfs_ublk::pump::{Command, Fetched, Queue, QueueDown, Unfetched, Wake};
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

/// The name a line gives a command of the queue.
fn command_name(command: Command) -> String {
    match command {
        Command::Fetch => "fetch".into(),
        Command::Commit => "commit".into(),
        Command::Other(code) => format!("cmd{code}"),
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
    /// Commits made since the last `sent` or `down` line: the count the
    /// next `send` names.
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

    /// Before a wait or a flush: the commits it hands to the kernel.
    fn send(&mut self) {
        if self.unsent > 0 {
            let count = self.unsent.to_string();
            self.line(&columns("send", "-", None, &count));
        }
    }

    /// After a wait or a flush: what the commits of its `send` came to.
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

    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown> {
        let before = fetched.len();
        let ended_before = unfetched.len();
        self.send();
        let woken = self.inner.wait(fetched, unfetched);
        self.sent(woken.is_ok());
        for Unfetched {
            tag,
            command,
            result,
        } in unfetched.get(ended_before..).unwrap_or_default().to_vec()
        {
            self.line(&format!(
                "ended\t{tag}\t{}\t-\t-\t{result}",
                command_name(command)
            ));
        }
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

    fn flush(&mut self) -> Result<(), QueueDown> {
        self.send();
        let flushed = self.inner.flush();
        self.sent(flushed.is_ok());
        flushed
    }
}

#[cfg(test)]
mod tests;
