//! What the agent's adapters ask of the machine ([`Host`]): run a
//! program with a time limit, read a file, and stat a path. The adapter
//! to `zfs` and `zpool` ([`crate::zfs::cli`]) runs on it, so that its
//! tests run on recorded output with no pool; [`System`] is the
//! program's.
//!
//! # A program that does not end
//!
//! A command on a suspended pool may never return, and cannot be killed
//! while it waits in the kernel (CLAUDE.md, "A command on a suspended
//! pool"). So [`System::run`] never waits for a child without a limit:
//! past it, the child's process group (the child leads one of its own,
//! so that what it started goes too) is sent `SIGKILL`, the child is
//! left to a thread that reaps it whenever it ends, and its output is
//! left to the threads that read it; the call returns `TimedOut` at once.
//! The same is done when the look at the child fails. Such a child holds
//! a process slot and three threads until it ends (the reaper and the
//! two readers), four while its standard input is not all written.
//! `timeout(1)` would not do: it waits for its child.
//!
//! Two cases are not killed (NL-F2 of #147's review). A child that has
//! ended while a process it started still holds its output: the ends
//! are waited for within the limit and the call returns `TimedOut`, but
//! that process is neither killed nor counted, since the group's leader
//! has been reaped and its id is not ours to signal any more. And a
//! process that leaves the group (`setsid`) escapes the kill in either
//! case; only a cgroup would hold it. `zfs` and `zpool` start no helper
//! that outlives them.
//!
//! Children left so are counted while they run. At [`STUCK_MAX`] of
//! them, [`System::run`] refuses to start another with the error
//! [`STUCK`] (kind `WouldBlock`), so that a pool that holds every
//! command does not take a slot and threads per call without end; the
//! agent reports it as a failed call, and a later run tries again (the
//! lead's L3 on #147). The cap is small and fixed: the agent runs one
//! command at a time, so four held means the pool is not answering.
//! The count is of this process only: an agent run by a timer starts
//! anew at each tick, with none counted, while the children an earlier
//! tick left may still run. Keeping the cap across ticks is an item of
//! M2.7's plan (NL-D1 of #147's review).

use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The most children left running past their limit before
/// [`System::run`] refuses to start another ([module docs](self)).
pub const STUCK_MAX: usize = 4;

/// The words of the error [`System::run`] gives at [`STUCK_MAX`].
pub const STUCK: &str = "commands that did not end are still running";

/// The children left running past their limit ([module docs](self)).
static STUCK_NOW: AtomicUsize = AtomicUsize::new(0);

/// The children left running past their limit, now.
pub fn stuck() -> usize {
    STUCK_NOW.load(Ordering::SeqCst)
}

/// The most of a program's standard output, or of its standard error,
/// that is read: more is an error.
pub const OUTPUT_MAX: usize = 64 << 20;

/// What a program gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Its exit code; `None` where a signal ended it.
    pub code: Option<i32>,
    /// Its standard output.
    pub stdout: Vec<u8>,
    /// Its standard error.
    pub stderr: Vec<u8>,
}

/// What a path names, as [`Host::stat`] reads it (links followed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// A block device, with its device number (`st_rdev`).
    Block(u64),
    /// A regular file.
    File,
    /// Anything else.
    Other,
}

/// What the adapters ask of the machine ([module docs](self)).
pub trait Host {
    /// Run `program` with `args`, `stdin` on its standard input, in the
    /// locale `C`, and wait at most `limit` for it to end and close its
    /// output. Past the limit: `TimedOut`, the child killed and not
    /// waited for.
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
    ) -> io::Result<Output>;
    /// The bytes of the file at `path`.
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>>;
    /// What `path` names.
    fn stat(&mut self, path: &Path) -> io::Result<Node>;
}

/// The machine the program runs on.
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

/// The longest pause between two looks at a child that has not ended.
const POLL_MAX: Duration = Duration::from_millis(20);

impl Host for System {
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
    ) -> io::Result<Output> {
        let until = Instant::now().checked_add(limit).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a limit of {} s is too long", limit.as_secs_f64()),
            )
        })?;
        let held = stuck();
        if held >= STUCK_MAX {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("{STUCK}: {held}, the most there may be"),
            ));
        }
        let mut child = Command::new(program)
            .args(args)
            .env("LC_ALL", "C")
            .process_group(0)
            .stdin(if stdin.is_empty() {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Each pipe on a thread of its own, so that a child that fills
        // one while the other is read does not stall, and so that a child
        // that never ends holds only its threads.
        if let Some(mut input) = child.stdin.take() {
            let bytes = stdin.to_vec();
            thread::spawn(move || {
                // A child that exits before reading all of it closes the
                // pipe; its exit code says what it made of it.
                let _ = input.write_all(&bytes);
            });
        }
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let mut pause = Duration::from_millis(1);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    abandon(child);
                    return Err(e);
                }
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                abandon(child);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "`{program}` did not end within {} s: killed, and not waited for",
                        limit.as_secs_f64()
                    ),
                ));
            }
            thread::sleep(pause.min(left));
            pause = (pause * 2).min(POLL_MAX);
        };
        // A child that has ended may have left its pipes to a child of
        // its own: their ends are waited for within the limit too.
        let take = |pipe: mpsc::Receiver<io::Result<Vec<u8>>>, which: &str| {
            let left = until.saturating_duration_since(Instant::now());
            match pipe.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(read) => read,
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("`{program}` ended and its {which} stayed open past the limit"),
                )),
            }
        };
        let stdout = take(stdout, "standard output")?;
        let stderr = take(stderr, "standard error")?;
        Ok(Output {
            code: status.code(),
            stdout,
            stderr,
        })
    }

    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn stat(&mut self, path: &Path) -> io::Result<Node> {
        use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
        let meta = std::fs::metadata(path)?;
        let kind = meta.file_type();
        Ok(if kind.is_block_device() {
            Node::Block(meta.rdev())
        } else if kind.is_file() {
            Node::File
        } else {
            Node::Other
        })
    }
}

/// Read `pipe` to its end on a thread, at most [`OUTPUT_MAX`] bytes.
fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (send, receive) = mpsc::channel();
    match pipe {
        None => {
            let _ = send.send(Ok(Vec::new()));
        }
        Some(pipe) => {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let max = u64::try_from(OUTPUT_MAX).unwrap_or(u64::MAX);
                let read = pipe.take(max + 1).read_to_end(&mut bytes).and_then(|_| {
                    if bytes.len() > OUTPUT_MAX {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("more than {OUTPUT_MAX} bytes of output"),
                        ))
                    } else {
                        Ok(bytes)
                    }
                });
                let _ = send.send(read);
            });
        }
    }
    receive
}

/// Kill `child`'s process group and leave the child to a thread that
/// reaps it when it ends, which a child in the kernel's wait may never
/// do; it is counted in [`stuck`] until then ([module docs](self)).
fn abandon(mut child: Child) {
    let group = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    // The child itself, should the group's kill have failed.
    let _ = child.kill();
    STUCK_NOW.fetch_add(1, Ordering::SeqCst);
    let reaper = thread::Builder::new().spawn(move || {
        let _ = child.wait();
        STUCK_NOW.fetch_sub(1, Ordering::SeqCst);
    });
    if reaper.is_err() {
        // No thread to reap it: it stays counted, as it may never end.
    }
}

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
