//! A test program for the binding's control commands, one step per
//! call, for a rig that sets up devices of other servers and looks at
//! what this binding says and does about them. It is not part of the
//! daemon and decides nothing: each step prints the binding's own
//! answer and leaves the judging to the caller.
//!
//! ```text
//! skyzfs-ublk-control add N [--size BYTES] [--depth D]
//! skyzfs-ublk-control info N
//! skyzfs-ublk-control delete N
//! skyzfs-ublk-control begin-recovery N [--retry]
//! skyzfs-ublk-control client-holds N
//! skyzfs-ublk-control set-params N --size BYTES
//! skyzfs-ublk-control serve N --file PATH [--size BYTES] [--recovery-begun] [--recovery-wait-ms T] [--hold-one-request SPEC] [--delete-after-ms T]
//! skyzfs-ublk-control take-over N --file PATH [--recovery-wait-ms T] [--hold-one-request SPEC] [--delete-after-ms T]
//! ```
//!
//! # Output
//!
//! Every answer is one line on standard output, `STEP<tab>N<tab>ANSWER`:
//!
//! - `add`: `added`, `taken`, or `refused: errno E (text)`. A device's
//!   size is not part of the driver's add: it is given with the first
//!   start. `--size` is accepted and repeated in the answer so that a
//!   rig's lines read alike; the size a device gets is the size of the
//!   file `serve` is given.
//! - `info`: `nothing`, or `state=live|waiting|dead size=BYTES queues=Q
//!   depth=D flags=0x..` as the binding reports the device (the flags
//!   under the mask of the bits the daemon asks for), then the driver's
//!   own values beside: `raw_state=`, `raw_flags=`, `max_io=`,
//!   `server_pid=`. A state the binding does not know reads
//!   `state=unknown`.
//! - `delete`: `returned after T ms`, `nothing`, or `refused: ...`. The
//!   delete is the binding's: asynchronous, so a held device keeps its
//!   number after it.
//! - `begin-recovery`: `begun`, `busy`, `nothing`, or `refused: ...`.
//!   One command and the driver's answer to it. With `--retry` it is
//!   the daemon's begin, which asks again while the driver is busy, and
//!   the answer ends with `; busy answers: K`.
//! - `client-holds`: `held` or `unknown`.
//! - `set-params`: `set`, `nothing`, or `refused: ...`: the driver's
//!   answer to the parameters the daemon gives a device of that size,
//!   sent by this program as one command (the daemon's own are sent by
//!   the library just before a start, which hides the driver's answer).
//!   `server_pid=` in `info` is the driver's value, unchanged.
//! - `serve` and `take-over` print two lines. When the device is live:
//!   `live after T ms; handed before the start: K; busy answers: B`,
//!   where K counts the requests the server was handed before the
//!   start's command (for a take-over: the end of the recovery) was
//!   sent. When the server ends, a line of step `served` with its
//!   counters. A step of the start that the driver refuses is the
//!   answer `refused at STEP: ...` and there is no second line.
//!
//! The exit status is 0 whatever the driver answered. It is 2 for a
//! command line this program does not understand and 1 when the
//! program itself could not work (no driver, no file).
//!
//! # The server
//!
//! `serve` serves a device this program's `add` made and that was never
//! started; with `--recovery-begun`, a device that waits for a server
//! and whose recovery the caller has begun. `take-over` begins the
//! recovery itself, with the daemon's retries. The server keeps the
//! device's content in the file, whose length is the device's size (a
//! file that is missing is made, of `--size` bytes). It ends on
//! `SIGTERM` or `SIGINT`, in order, and leaves the device waiting for a
//! server, as the daemon does.
//!
//! `--recovery-wait-ms T` waits that long between the queue being ready
//! and the start's command.
//!
//! `--hold-one-request SECONDS:read:OFFSET`, `SECONDS:write:OFFSET` or
//! `SECONDS:flush` holds the first such request (a read or a write that
//! covers the byte) for that long before it is answered; the others are
//! answered meanwhile. A server that ends while it holds the request
//! does not answer it.
//!
//! `--delete-after-ms T` deletes the server's own device, from a second
//! thread of this process, T ms after the device went live, while the
//! server goes on serving: what a daemon does whose volume failed. It
//! prints two more lines of step `self-delete`: `sent after T ms` just
//! before the command, and `returned after X ms` (or `nothing`, or
//! `refused: ...`) when the command returns, with X counted from the
//! command. The server ends when the driver ends its queue, or on a
//! signal, as without the option. If the delete has not returned 10 s
//! after the server ended, the last line is `not returned 10 s after
//! the server ended` and the program exits (if it can: a thread inside
//! the driver may hold the process).

#![forbid(unsafe_code)]

use std::time::Duration;

/// What a hold waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum What {
    Read(u64),
    Write(u64),
    Flush,
}

/// One request to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hold {
    time: Duration,
    what: What,
}

/// How a device is served.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Serve {
    file: String,
    size: Option<u64>,
    /// The device waits for a server (and is not one never started).
    take_over: bool,
    /// The recovery is begun by this program.
    begin: bool,
    wait: Duration,
    hold: Option<Hold>,
    /// Delete the device this long after it went live.
    delete_after: Option<Duration>,
}

/// One step.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Add { size: Option<u64>, depth: u16 },
    Info,
    Delete,
    BeginRecovery { retry: bool },
    ClientHolds,
    SetParams { size: u64 },
    Serve(Serve),
}

/// The depth of a device `add` makes when none is given.
const DEPTH: u16 = 32;

fn number<T: std::str::FromStr>(what: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{what}: '{text}' is not a number in range"))
}

fn hold(text: &str) -> Result<Hold, String> {
    let bad = || {
        format!("--hold-one-request: '{text}' is not SECONDS:read:OFFSET, SECONDS:write:OFFSET or SECONDS:flush")
    };
    let parts: Vec<&str> = text.split(':').collect();
    let what = match parts.as_slice() {
        [_, "flush"] => What::Flush,
        [_, "read", at] => What::Read(at.parse().map_err(|_| bad())?),
        [_, "write", at] => What::Write(at.parse().map_err(|_| bad())?),
        _ => return Err(bad()),
    };
    let seconds: u64 = parts[0].parse().map_err(|_| bad())?;
    Ok(Hold {
        time: Duration::from_secs(seconds),
        what,
    })
}

/// The step, its name and the device's number, from the command line
/// without the program's name.
fn parse(args: &[String]) -> Result<(&'static str, u32, Step), String> {
    let [step, n, rest @ ..] = args else {
        return Err("a step and a device number are needed".into());
    };
    let n: u32 = number("the device number", n)?;
    let mut rest = rest.iter();
    let (mut size, mut depth, mut retry, mut file) = (None, DEPTH, false, None);
    let (mut begun, mut wait, mut held) = (false, Duration::ZERO, None);
    let mut delete_after = None;
    while let Some(option) = rest.next() {
        let allowed: &[&str] = match step.as_str() {
            "add" => &["--size", "--depth"],
            "begin-recovery" => &["--retry"],
            "set-params" => &["--size"],
            "serve" => &[
                "--file",
                "--size",
                "--recovery-begun",
                "--recovery-wait-ms",
                "--hold-one-request",
                "--delete-after-ms",
            ],
            "take-over" => &[
                "--file",
                "--recovery-wait-ms",
                "--hold-one-request",
                "--delete-after-ms",
            ],
            _ => &[],
        };
        if !allowed.contains(&option.as_str()) {
            return Err(format!("{step}: '{option}' is not an option of this step"));
        }
        let mut value = || rest.next().ok_or_else(|| format!("{option} needs a value"));
        match option.as_str() {
            "--size" => size = Some(number("--size", value()?)?),
            "--depth" => depth = number("--depth", value()?)?,
            "--retry" => retry = true,
            "--file" => file = Some(value()?.clone()),
            "--recovery-begun" => begun = true,
            "--recovery-wait-ms" => {
                wait = Duration::from_millis(number("--recovery-wait-ms", value()?)?)
            }
            "--delete-after-ms" => {
                delete_after = Some(Duration::from_millis(number(
                    "--delete-after-ms",
                    value()?,
                )?))
            }
            _ => held = Some(hold(value()?)?),
        }
    }
    let serve = |take_over: bool, begin: bool| {
        Ok(Step::Serve(Serve {
            file: file.clone().ok_or_else(|| format!("{step} needs --file"))?,
            size,
            take_over,
            begin,
            wait,
            hold: held,
            delete_after,
        }))
    };
    let parsed: Result<(&'static str, Step), String> = match step.as_str() {
        "add" => Ok(("add", Step::Add { size, depth })),
        "info" => Ok(("info", Step::Info)),
        "delete" => Ok(("delete", Step::Delete)),
        "begin-recovery" => Ok(("begin-recovery", Step::BeginRecovery { retry })),
        "client-holds" => Ok(("client-holds", Step::ClientHolds)),
        "set-params" => size
            .map(|size| ("set-params", Step::SetParams { size }))
            .ok_or_else(|| "set-params needs --size".to_string()),
        "serve" => serve(begun, false).map(|s| ("serve", s)),
        "take-over" => serve(true, true).map(|s| ("take-over", s)),
        other => Err(format!("'{other}' is not a step")),
    };
    parsed.map(|(name, step)| (name, n, step))
}

#[cfg(target_os = "linux")]
mod run {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Write};
    use std::os::unix::fs::FileExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use skyzfs_blk::BLOCK_SIZE;
    use skyzfs_ublk::lifecycle::{AddError, Control, DeviceState, Shape};
    use skyzfs_ublk::pump::{Fetched, Queue, Wake as Woken};
    use skyzfs_ublk::translate::{op, Desc};
    use skyzfs_ublk_sys::control::Kernel;
    use skyzfs_ublk_sys::queue::{Device, Open};
    use skyzfs_ublk_sys::signals::{self, Signal};
    use skyzfs_ublk_sys::{raw, Wake, FLAGS};

    use super::{Hold, Serve, Step, What};

    /// The driver's answer, as text.
    fn errno(e: &io::Error) -> String {
        match e.raw_os_error() {
            Some(n) => format!("errno {n} ({e})"),
            None => e.to_string(),
        }
    }

    fn is(e: &io::Error, n: i32) -> bool {
        e.raw_os_error() == Some(n)
    }

    /// What the server counted.
    #[derive(Default)]
    struct Counts {
        handed: AtomicU64,
        reads: AtomicU64,
        writes: AtomicU64,
        flushes: AtomicU64,
        trims: AtomicU64,
        others: AtomicU64,
        failed: AtomicU64,
        held: AtomicU64,
    }

    fn covers(desc: &Desc, byte: u64) -> bool {
        let start = desc.start_sector.saturating_mul(512);
        (start..start.saturating_add(u64::from(desc.nr_sectors) * 512)).contains(&byte)
    }

    fn is_held(what: What, desc: &Desc) -> bool {
        match what {
            What::Read(byte) => desc.op() == op::READ && covers(desc, byte),
            What::Write(byte) => desc.op() == op::WRITE && covers(desc, byte),
            What::Flush => desc.op() == op::FLUSH,
        }
    }

    /// Answer one request from the file.
    fn answer(queue: &mut impl Queue, file: &File, f: Fetched, counts: &Counts) {
        let at = f.desc.start_sector * 512;
        let bytes = u64::from(f.desc.nr_sectors) * 512;
        let blocks = (bytes / BLOCK_SIZE) as usize;
        let (done, buf) = match f.desc.op() {
            op::READ => {
                counts.reads.fetch_add(1, Ordering::Relaxed);
                match queue.take_buf(f.tag, blocks) {
                    Some(mut buf) => (file.read_exact_at(&mut buf, at), Some(buf)),
                    None => (Err(io::Error::from_raw_os_error(libc::EIO)), None),
                }
            }
            op::WRITE => {
                counts.writes.fetch_add(1, Ordering::Relaxed);
                match queue.take_buf(f.tag, blocks) {
                    Some(buf) => (file.write_all_at(&buf, at), Some(buf)),
                    None => (Err(io::Error::from_raw_os_error(libc::EIO)), None),
                }
            }
            op::FLUSH => {
                counts.flushes.fetch_add(1, Ordering::Relaxed);
                (file.sync_data(), None)
            }
            op::DISCARD | op::WRITE_ZEROES => {
                counts.trims.fetch_add(1, Ordering::Relaxed);
                (file.write_all_at(&vec![0u8; bytes as usize], at), None)
            }
            _ => {
                counts.others.fetch_add(1, Ordering::Relaxed);
                (Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP)), None)
            }
        };
        let result = match done {
            Ok(()) => bytes as i32,
            Err(e) => {
                counts.failed.fetch_add(1, Ordering::Relaxed);
                -e.raw_os_error().unwrap_or(libc::EIO)
            }
        };
        queue.commit(f.tag, result, buf);
    }

    /// The server: every request from the file, one of them late if
    /// `hold` says so.
    fn server(
        mut queue: impl Queue,
        file: File,
        mut hold: Option<Hold>,
        wake: Wake,
        counts: Arc<Counts>,
    ) {
        let mut fetched: Vec<Fetched> = Vec::new();
        let mut held: Option<(Fetched, Instant)> = None;
        loop {
            fetched.clear();
            let Ok(woken) = queue.wait(&mut fetched) else {
                return;
            };
            for f in fetched.drain(..) {
                counts.handed.fetch_add(1, Ordering::Relaxed);
                match hold {
                    Some(h) if is_held(h.what, &f.desc) => {
                        hold = None;
                        counts.held.fetch_add(1, Ordering::Relaxed);
                        held = Some((f, Instant::now() + h.time));
                        let wake = wake.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(h.time);
                            wake.wake();
                        });
                    }
                    _ => answer(&mut queue, &file, f, &counts),
                }
            }
            if woken == Woken::Stop {
                // The answers above have no wait behind them.
                let _ = queue.flush();
                return;
            }
            if let Some((f, _)) = held.take_if(|(_, due)| Instant::now() >= *due) {
                answer(&mut queue, &file, f, &counts);
            }
        }
    }

    /// The file a device is served from, and its size.
    fn content(serve: &Serve) -> Result<(File, u64), String> {
        let path = &serve.file;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(serve.size.is_some())
            .truncate(false)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        let mut size = file.metadata().map_err(|e| format!("{path}: {e}"))?.len();
        if size == 0 {
            if let Some(wanted) = serve.size {
                file.set_len(wanted).map_err(|e| format!("{path}: {e}"))?;
                size = wanted;
            }
        }
        if size == 0 || size % BLOCK_SIZE != 0 {
            return Err(format!(
                "{path}: {size} bytes is not a device's size (a multiple of {BLOCK_SIZE}, not 0)"
            ));
        }
        Ok((file, size))
    }

    fn say(step: &str, n: u32, answer: &str) -> Result<(), String> {
        let mut out = io::stdout().lock();
        writeln!(out, "{step}\t{n}\t{answer}")
            .and_then(|()| out.flush())
            .map_err(|e| format!("standard output: {e}"))
    }

    fn serve(name: &str, n: u32, serve: &Serve, kernel: &mut Kernel) -> Result<(), String> {
        let (file, size) = content(serve)?;
        signals::block().map_err(|e| format!("blocking the signals: {e}"))?;
        let from = Instant::now();
        if serve.begin {
            if let Err(e) = kernel.begin_recovery(n) {
                let why = if is(&e, libc::EBUSY) {
                    "busy".to_string()
                } else if is(&e, libc::ENODEV) {
                    "nothing".to_string()
                } else {
                    errno(&e)
                };
                return say(
                    name,
                    n,
                    &format!(
                        "refused at begin-recovery: {why}; busy answers: {}",
                        kernel.busy_answers()
                    ),
                );
            }
        }
        let device = match Device::open(&Open {
            number: n,
            size,
            take_over: serve.take_over,
        }) {
            Ok(device) => device,
            Err(e) => return say(name, n, &format!("refused at open: {}", errno(&e))),
        };
        let wake = Wake::new().map_err(|e| format!("the wake descriptor: {e}"))?;
        let counts = Arc::new(Counts::default());
        let serving = {
            let (hold, wake, counts) = (serve.hold, wake.clone(), counts.clone());
            device.serve(&wake.clone(), false, move |queue| {
                server(queue, file, hold, wake, counts)
            })
        };
        let serving = match serving {
            Ok(serving) => serving,
            Err(e) => return say(name, n, &format!("refused at queue: {}", errno(&e))),
        };
        std::thread::sleep(serve.wait);
        let before = counts.handed.load(Ordering::Relaxed);
        if let Err(e) = device.start() {
            serving.stopper().stop();
            let _ = serving.join();
            let step = if serve.take_over {
                "end-recovery"
            } else {
                "start"
            };
            return say(name, n, &format!("refused at {step}: {}", errno(&e)));
        }
        say(
            name,
            n,
            &format!(
                "live after {} ms; handed before the start: {before}; busy answers: {}",
                from.elapsed().as_millis(),
                kernel.busy_answers()
            ),
        )?;
        let deleting = serve.delete_after.map(|after| {
            let (done, returned) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                std::thread::sleep(after);
                let _ = done.send(delete_own(n, after));
            });
            returned
        });
        let by = loop {
            if serving.is_finished() {
                break "the queue ended";
            }
            if signals::wait(Duration::from_millis(100)) == Some(Signal::Stop) {
                break "a signal";
            }
        };
        let stopper = serving.stopper();
        stopper.stop();
        serving
            .join()
            .map_err(|e| format!("the queue's thread: {e}"))?;
        let count = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let ended = Instant::now();
        let deleted = deleting.map(|returned| {
            let left = DELETE_GRACE.saturating_sub(ended.elapsed());
            returned.recv_timeout(left).unwrap_or_else(|_| {
                Ok(format!(
                    "not returned {} s after the server ended",
                    DELETE_GRACE.as_secs()
                ))
            })
        });
        say(
            "served",
            n,
            &format!(
                "ended by {by}; handed {} reads {} writes {} flushes {} discards {} others {} failed {} held {} first_write_copies {}",
                count(&counts.handed),
                count(&counts.reads),
                count(&counts.writes),
                count(&counts.flushes),
                count(&counts.trims),
                count(&counts.others),
                count(&counts.failed),
                count(&counts.held),
                stopper.first_write_copies()
            ),
        )?;
        match deleted {
            // The thread has said its own lines; an error is its own.
            Some(Ok(late)) if !late.is_empty() => say("self-delete", n, &late),
            Some(Err(e)) => Err(e),
            _ => Ok(()),
        }
    }

    /// How long the program waits, after its server ended, for its own
    /// delete to return.
    const DELETE_GRACE: Duration = Duration::from_secs(10);

    /// `--delete-after-ms`: delete device `n` with a control interface
    /// of this thread's own, and say when the command was sent and when
    /// it returned. `Ok` carries nothing more to say.
    fn delete_own(n: u32, after: Duration) -> Result<String, String> {
        let mut kernel = Kernel::new().map_err(|e| e.to_string())?;
        say(
            "self-delete",
            n,
            &format!("sent after {} ms", after.as_millis()),
        )?;
        let from = Instant::now();
        let answer = match kernel.delete(n) {
            Ok(()) => format!("returned after {} ms", from.elapsed().as_millis()),
            Err(e) if is(&e, libc::ENODEV) => "nothing".into(),
            Err(e) => format!("refused: {}", errno(&e)),
        };
        say("self-delete", n, &answer)?;
        Ok(String::new())
    }

    fn info(n: u32, kernel: &mut Kernel) -> String {
        let raw = match raw::info(n) {
            Ok(raw) => raw,
            Err(e) if is(&e, libc::ENODEV) => return "nothing".into(),
            Err(e) => return format!("refused: {}", errno(&e)),
        };
        let ours = match kernel.info(n) {
            Ok(Some(info)) => {
                let state = match info.state {
                    DeviceState::Live => "live",
                    DeviceState::Waiting => "waiting",
                    DeviceState::Dead => "dead",
                };
                let s = info.shape;
                format!(
                    "state={state} size={} queues={} depth={} flags={:#x}",
                    s.size, s.queues, s.depth, s.flags
                )
            }
            Ok(None) => return "nothing".into(),
            Err(e) => format!("state=unknown ({e})"),
        };
        format!(
            "{ours} raw_state={} raw_flags={:#x} max_io={} server_pid={}",
            raw.state, raw.flags, raw.max_io_buf_bytes, raw.ublksrv_pid
        )
    }

    /// Do the step. An error is this program's own failure.
    pub fn step(name: &str, n: u32, step: &Step) -> Result<(), String> {
        let mut kernel = Kernel::new().map_err(|e| e.to_string())?;
        let answer = match step {
            Step::Add { size, depth } => {
                let shape = Shape {
                    size: size.unwrap_or(0),
                    queues: 1,
                    depth: *depth,
                    flags: FLAGS,
                };
                let size = size.map(|s| format!(" (size {s} is given at the start)"));
                match kernel.add(n, &shape) {
                    Ok(()) => format!("added{}", size.unwrap_or_default()),
                    Err(AddError::Taken) => "taken".into(),
                    Err(AddError::Other(e)) => format!("refused: {}", errno(&e)),
                }
            }
            Step::Info => info(n, &mut kernel),
            Step::Delete => {
                let from = Instant::now();
                match kernel.delete(n) {
                    Ok(()) => format!("returned after {} ms", from.elapsed().as_millis()),
                    Err(e) if is(&e, libc::ENODEV) => "nothing".into(),
                    Err(e) => format!("refused: {}", errno(&e)),
                }
            }
            Step::BeginRecovery { retry } => {
                let result = if *retry {
                    kernel.begin_recovery(n)
                } else {
                    raw::begin_recovery(n)
                };
                let answer = match result {
                    Ok(()) => "begun".to_string(),
                    Err(e) if is(&e, libc::EBUSY) => "busy".into(),
                    Err(e) if is(&e, libc::ENODEV) => "nothing".into(),
                    Err(e) => format!("refused: {}", errno(&e)),
                };
                if *retry {
                    format!("{answer}; busy answers: {}", kernel.busy_answers())
                } else {
                    answer
                }
            }
            Step::ClientHolds => match kernel.client_holds(n) {
                Some(true) => "held".into(),
                _ => "unknown".into(),
            },
            Step::SetParams { size } => {
                match raw::set_params(n, skyzfs_ublk_sys::queue::parameters(*size)) {
                    Ok(()) => "set".into(),
                    Err(e) if is(&e, libc::ENODEV) => "nothing".into(),
                    Err(e) => format!("refused: {}", errno(&e)),
                }
            }
            Step::Serve(s) => return serve(name, n, s, &mut kernel),
        };
        say(name, n, &answer)
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (name, n, step) = match parse(&args) {
        Ok(parsed) => parsed,
        Err(e) => {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr(), "skyzfs-ublk-control: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    #[cfg(target_os = "linux")]
    let done = run::step(name, n, &step);
    #[cfg(not(target_os = "linux"))]
    let done: Result<(), String> = {
        let _ = (name, n, step);
        Err("ublk is a Linux driver".into())
    };
    match done {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr(), "skyzfs-ublk-control: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn each_step_is_read_with_its_number() {
        assert_eq!(
            parse(&args("add 7 --size 4096 --depth 8")),
            Ok((
                "add",
                7,
                Step::Add {
                    size: Some(4096),
                    depth: 8
                }
            ))
        );
        assert_eq!(
            parse(&args("add 7")),
            Ok((
                "add",
                7,
                Step::Add {
                    size: None,
                    depth: DEPTH
                }
            ))
        );
        assert_eq!(parse(&args("info 0")), Ok(("info", 0, Step::Info)));
        assert_eq!(parse(&args("delete 3")), Ok(("delete", 3, Step::Delete)));
        assert_eq!(
            parse(&args("begin-recovery 3")),
            Ok(("begin-recovery", 3, Step::BeginRecovery { retry: false }))
        );
        assert_eq!(
            parse(&args("begin-recovery 3 --retry")),
            Ok(("begin-recovery", 3, Step::BeginRecovery { retry: true }))
        );
        assert_eq!(
            parse(&args("set-params 9 --size 4096")),
            Ok(("set-params", 9, Step::SetParams { size: 4096 }))
        );
        assert_eq!(
            parse(&args("client-holds 9")),
            Ok(("client-holds", 9, Step::ClientHolds))
        );
    }

    #[test]
    fn serve_starts_a_new_device_or_one_whose_recovery_was_begun_and_take_over_begins_it() {
        let serve = |take_over, begin| Serve {
            file: "f".into(),
            size: None,
            take_over,
            begin,
            wait: Duration::ZERO,
            hold: None,
            delete_after: None,
        };
        assert_eq!(
            parse(&args("serve 1 --file f")),
            Ok(("serve", 1, Step::Serve(serve(false, false))))
        );
        assert_eq!(
            parse(&args("serve 1 --file f --recovery-begun")),
            Ok(("serve", 1, Step::Serve(serve(true, false))))
        );
        assert_eq!(
            parse(&args("take-over 1 --file f")),
            Ok(("take-over", 1, Step::Serve(serve(true, true))))
        );
        assert_eq!(
            parse(&args(
                "take-over 1 --recovery-wait-ms 1500 --hold-one-request 30:read:8192 --file f"
            )),
            Ok((
                "take-over",
                1,
                Step::Serve(Serve {
                    wait: Duration::from_millis(1500),
                    hold: Some(Hold {
                        time: Duration::from_secs(30),
                        what: What::Read(8192)
                    }),
                    ..serve(true, true)
                })
            ))
        );
        assert_eq!(
            parse(&args("serve 1 --file f --size 8192")),
            Ok((
                "serve",
                1,
                Step::Serve(Serve {
                    size: Some(8192),
                    ..serve(false, false)
                })
            ))
        );
    }

    #[test]
    fn a_server_can_be_told_to_delete_its_own_device() {
        for (line, take_over, begin) in [
            ("serve 1 --file f --delete-after-ms 250", false, false),
            ("take-over 1 --file f --delete-after-ms 250", true, true),
        ] {
            let Ok((_, 1, Step::Serve(serve))) = parse(&args(line)) else {
                panic!("{line}");
            };
            assert_eq!(serve.delete_after, Some(Duration::from_millis(250)));
            assert_eq!((serve.take_over, serve.begin), (take_over, begin));
        }
        let Ok((_, _, Step::Serve(serve))) = parse(&args(
            "serve 1 --file f --hold-one-request 60:write:0 --delete-after-ms 0",
        )) else {
            panic!("with a hold");
        };
        assert_eq!(serve.delete_after, Some(Duration::ZERO));
        assert!(serve.hold.is_some());
    }

    #[test]
    fn a_hold_names_a_read_or_a_write_at_a_byte_or_a_flush() {
        let s = Duration::from_secs(5);
        assert_eq!(
            hold("5:flush").map(|h| (h.time, h.what)),
            Ok((s, What::Flush))
        );
        assert_eq!(hold("5:write:0").map(|h| h.what), Ok(What::Write(0)));
        assert_eq!(hold("5:read:4096").map(|h| h.what), Ok(What::Read(4096)));
        for bad in [
            "",
            "5",
            "flush",
            "5:read",
            "5:flush:1",
            "x:flush",
            "5:read:x",
            "5:trim:0",
        ] {
            assert!(hold(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn what_is_not_understood_is_refused() {
        for bad in [
            "",
            "info",
            "info x",
            "info -1",
            "frobnicate 1",
            "info 1 --size 5",
            "add 1 --file f",
            "add 1 --size",
            "add 1 --depth 70000",
            "serve 1",
            "set-params 1",
            "set-params 1 --depth 4",
            "take-over 1",
            "take-over 1 --file f --recovery-begun",
            "take-over 1 --file f --size 4096",
            "serve 1 --file f --hold-one-request 5",
            "delete 1 2",
            "delete 1 --delete-after-ms 5",
            "serve 1 --file f --delete-after-ms",
            "serve 1 --file f --delete-after-ms soon",
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
    }
}
