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
//!   was given, not when the store completed the request. The one
//!   commit that `--refuse-one-commit` sends with no buffer, below this
//!   record ([`crate::refuse`]), is written as any other; its refusal
//!   follows as an `ended` line for the tag.
//! - `send`: a wait or a flush of the queue begins with commits not yet
//!   handed on (the result column says how many: the `commit` lines
//!   since the last `sent` or `down`). The queue hands what its ring
//!   holds to the kernel at the start of that wait, before it waits for
//!   anything.
//! - `sent`: that wait or flush returned, so the commits counted in its
//!   `send` have been handed to the kernel.
//! - `down`: that wait or flush failed; those commits may or may not
//!   have reached the kernel.
//! - `label`: the bytes of a write that touches either end of the
//!   volume, written as the queue hands the write's buffer over and so
//!   between its `recv` and the store's receiving it. The ends are the
//!   first 512 KiB, and from 512 KiB before the volume's size rounded
//!   down to 256 KiB to the volume's end: the four labels of a vdev on
//!   the whole volume (`vdev_label_offset` in OpenZFS) and the 512 KiB
//!   at each end. The result column holds every byte of the write, the
//!   part outside the ends too, in lowercase hex, two digits a byte;
//!   the other columns are the `recv` line's.
//! - `applied`: the store has applied write `tag` whole: its direct
//!   write returned whole and the store took the result, before the
//!   store completes it and so before its `commit`. Every write that
//!   succeeds has one; a failed one has none. A write-zeroes has none:
//!   the store applies it when it executes it, in the order it was
//!   received relative to a flush, so its `recv` line places it.
//! - `barrier`: flush `tag` reached its barrier in the store: every
//!   write received before it has ended, and nothing received after it
//!   has begun. The op column says `flush` for the kernel's flush, and
//!   `fua_flush` for the flush the daemon adds after an operation of
//!   that tag that carried FUA.
//! - `cut`: the store counted a cut at that barrier (design §5.2), after
//!   its `barrier` line; the result column is the cut's transaction
//!   group, and the sector column says whether it opens as itself:
//!   `itself` if an import of the volume at its instant, with no
//!   rewind, opens its uberblock's pool state, `rewind` if it opens only
//!   with a rewind to its group. The `cut` lines of an instance are its
//!   cuts in order: their number is that instance's `cuts` in `status`.
//!
//! The store's three lines (`applied`, `barrier`, `cut`) are written on
//! the queue's thread, which drives the store, through a second handle
//! of the same file: they fall in the file in the order of the queue's
//! own events.
//!
//! - `ended`: a command of the queue that the driver ended with a result
//!   other than OK, as the queue's wait or its last flush returned it
//!   (after that wait's or flush's `sent` or `down`, before a wait's
//!   `recv` lines): its tag, the command in
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
//!   queue's next wait or at its last flush, and until then looks here
//!   like one it took. The kernel's own trace (`block_rq_complete`,
//!   `block_rq_requeue`) is the other side of it.
//! - **At a stop** (not at a kill), once the library has seen an abort
//!   it puts no more commits on its ring, while its commit still
//!   returns as done: such a commit has its `commit`, `send` and `sent`
//!   lines and never reached the kernel. This is detach, a deleted
//!   device, a failed volume.
//! - **A request that arrives during the queue's last flush** (at a
//!   stop) is dropped by the queue and has no `recv` line, though the
//!   kernel's trace shows it issued.
//! - **The record costs** three or four `write` calls per request on the
//!   queue's thread, and a write at either end of the volume is written
//!   out whole in hex (with ZFS, a label of 112 KiB and an uberblock to
//!   each of the four labels in every transaction group): a rate
//!   measured with it is not the rate without it.
//!
//! A record that cannot be written does not stop the daemon: the first
//! error is said once on standard error, and the record ends there.

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;

use skyzfs_blk::{Buf, RequestId};
use skyzfs_local::inplace::{Seen, Watcher};
use skyzfs_uberblock::{Labels, LABEL_SIZE};
use skyzfs_ublk::pump::{tag_of, Command, Fetched, Queue, QueueDown, Unfetched, Wake};
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

/// The lines one handle of the record's file writes.
pub struct Lines {
    file: Option<File>,
    pid: u32,
}

impl Lines {
    /// Lines to `file`, if there is one.
    pub fn new(file: Option<File>) -> Self {
        Lines {
            file,
            pid: std::process::id(),
        }
    }

    /// Lines to another handle of `file`, for the store's events. If
    /// one cannot be had, that is said once on standard error and the
    /// record has no store lines.
    pub fn another(file: &File) -> Self {
        match file.try_clone() {
            Ok(file) => Self::new(Some(file)),
            Err(e) => {
                crate::to_stderr(&format!(
                    "skyzfsd: the request log has no lines of the store: a second handle \
                     of its file cannot be had ({e})\n"
                ));
                Self::new(None)
            }
        }
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
}

/// The store's events as lines of the record, to `lines`: see the
/// [module documentation](self).
pub fn watcher(mut lines: Lines) -> Watcher {
    Box::new(move |seen| lines.line(&store_columns(seen)))
}

/// One line of the store's events, without the time and the pid.
fn store_columns(seen: Seen) -> String {
    let named = |id: RequestId| {
        let (tag, added) = tag_of(id);
        (tag, if added { "fua_flush" } else { "flush" })
    };
    match seen {
        Seen::Applied(id) => format!("applied\t{}\twrite\t-\t-\t-", tag_of(id).0),
        Seen::Barrier(id) => {
            let (tag, op) = named(id);
            format!("barrier\t{tag}\t{op}\t-\t-\t-")
        }
        Seen::Cut(id, cut) => {
            let (tag, op) = named(id);
            let opens = if cut.itself { "itself" } else { "rewind" };
            format!("cut\t{tag}\t{op}\t{opens}\t-\t{}", cut.group)
        }
    }
}

/// The ends of a volume whose writes the record writes out: see the
/// [module documentation](self).
#[derive(Debug, Clone, Copy)]
struct Ends {
    /// The first byte after the first end.
    head: u64,
    /// The first byte of the last end, which runs to the volume's end.
    tail: u64,
}

impl Ends {
    fn of(size: u64) -> Self {
        let two_labels = 2 * LABEL_SIZE;
        Ends {
            head: two_labels,
            tail: Labels::for_size(size).psize().saturating_sub(two_labels),
        }
    }

    /// Whether `len` bytes at `offset` touch either end.
    fn touched(self, offset: u64, len: u64) -> bool {
        len > 0 && (offset < self.head || offset.saturating_add(len) > self.tail)
    }
}

/// `bytes` in lowercase hex, two digits a byte.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

/// A queue whose requests and answers are written down, if it was given
/// a file: see the [module documentation](self).
pub struct Recorded<Q: Queue> {
    inner: Q,
    lines: Lines,
    ends: Ends,
    /// The request each tag holds, as the kernel handed it over.
    held: Vec<Option<Desc>>,
    /// Commits made since the last `sent` or `down` line: the count the
    /// next `send` names.
    unsent: u64,
}

impl<Q: Queue> Recorded<Q> {
    /// `inner`, with its requests and answers written to `file` if
    /// there is one; the `start` line names `device`, a volume of
    /// `size` bytes.
    pub fn new(inner: Q, file: Option<File>, device: u32, size: u64) -> Self {
        let depth = inner.depth();
        let mut recorded = Recorded {
            inner,
            lines: Lines::new(file),
            ends: Ends::of(size),
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
        self.lines.line(columns);
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

    /// An `ended` line for each command of `ended`.
    fn ended(&mut self, ended: &[Unfetched]) {
        for &Unfetched {
            tag,
            command,
            result,
        } in ended
        {
            self.line(&format!(
                "ended\t{tag}\t{}\t-\t-\t{result}",
                command_name(command)
            ));
        }
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
        self.ended(unfetched.get(ended_before..).unwrap_or_default());
        for Fetched { tag, desc } in fetched.get(before..).unwrap_or_default().to_vec() {
            if let Some(slot) = self.held.get_mut(usize::from(tag)) {
                *slot = Some(desc);
            }
            self.line(&columns("recv", &tag.to_string(), Some(&desc), "-"));
        }
        woken
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        let buf = self.inner.take_buf(tag, blocks)?;
        let held = self.held.get(usize::from(tag)).copied().flatten();
        if let Some(desc) = held.filter(|desc| desc.op() == op::WRITE) {
            let offset = desc.start_sector.saturating_mul(512);
            let len = u64::from(desc.nr_sectors) * 512;
            if self.lines.file.is_some() && self.ends.touched(offset, len) {
                let len = usize::try_from(len).unwrap_or(usize::MAX).min(buf.len());
                let bytes = hex(&buf[..len]);
                self.line(&columns("label", &tag.to_string(), Some(&desc), &bytes));
            }
        }
        Some(buf)
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

    fn flush(&mut self, unfetched: &mut Vec<Unfetched>) -> Result<(), QueueDown> {
        let ended_before = unfetched.len();
        self.send();
        let flushed = self.inner.flush(unfetched);
        self.sent(flushed.is_ok());
        self.ended(unfetched.get(ended_before..).unwrap_or_default());
        flushed
    }
}

#[cfg(test)]
mod tests;
