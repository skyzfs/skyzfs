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
//! skyzfs-ublk-control serve N --file PATH [--size BYTES] [--recovery-begun] [--recovery-wait-ms T] [--hold-one-request SPEC] [--refuse-one-commit WHAT] [--refuse-one-fetch start|recovery] [--delete-after-ms T]
//! skyzfs-ublk-control take-over N --file PATH [--recovery-wait-ms T] [--hold-one-request SPEC] [--refuse-one-commit WHAT] [--refuse-one-fetch recovery] [--delete-after-ms T]
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
//!   counters: `ended by REASON; handed K reads R writes W flushes F
//!   discards D others O failed E held H first_write_copies C
//!   commands_aborted A commands_failed X`. The last two count the
//!   commands of the queue that the driver ended otherwise than OK:
//!   with its abort (`UBLK_IO_RES_ABORT`, the queue being stopped),
//!   and with any other result (a command it refused). A step of the
//!   start that the driver refuses is the answer `refused at STEP: ...`
//!   and there is no second line. With `--refuse-one-fetch` the lines
//!   are other: see that option below.
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
//! `--refuse-one-commit read:OFFSET`, `write:OFFSET` or `flush` commits
//! the first such request with no buffer, which the driver refuses as
//! malformed: it counts the request as the server's, unanswered, and no
//! command waits on its tag any more (see
//! `KernelQueue::commit_with_no_buffer_for_tests`). The request is not
//! answered otherwise. The program prints a line of step
//! `refuse-commit` when it sends the commit, `sent for a read at byte
//! B on tag T` (or a write, or `a flush`); if the library will not put
//! it on the ring, `not sent for ... on tag T: REASON` instead, and the
//! request is answered as any other and the server goes on (no test
//! step can then pass). Then one line with the driver's
//! answer: `refused: errno E (...)`, `aborted` (the queue was being
//! stopped), or `taken` (the tag was handed its next request, so the
//! driver took the commit). On a refusal the server ends at once, as
//! the daemon does on a refused commit: it answers nothing more,
//! holds what it was handed, and leaves the device waiting for a
//! server, as at any end of the program. Its `served` line then says
//! `ended by the refused commit`. A take-over (`take-over`) then shows
//! whether the kernel issues the held request again; a `delete` shows
//! what becomes of it without one. When both options match a request,
//! the refusal is checked first.
//!
//! `--refuse-one-fetch start` (`serve` of a device never started) or
//! `--refuse-one-fetch recovery` (`take-over`, or `serve
//! --recovery-begun`) sends the first fetch of tag 0 with no buffer,
//! which the driver refuses by a reading of its source, as it refuses
//! such a commit; the other tags' first fetches name their buffers as
//! always (see `Device::serve_with_fetch_refused_for_tests`). The value
//! names the command the step sends after the fetches, the start or
//! the end of a recovery, and must be the step's: it is there so that
//! the rig's command line says which of the two it looks at. The option
//! goes with neither `--refuse-one-commit` nor `--delete-after-ms`.
//! The lines of the step are then these, and no `live` line:
//!
//! - A line of step `refuse-fetch`, `sent on tag 0`, from the queue's
//!   thread before its first wait, which is when the fetches reach the
//!   driver.
//! - Another with the driver's answer, when it comes: `refused: errno E
//!   (...)`, `aborted` (the queue was being stopped), or `taken` (tag 0
//!   was handed a request, so the driver took a fetch with no buffer;
//!   having told it no address for that request's data, the program
//!   answers it with an I/O error and does not touch the file). The
//!   program goes on after each.
//! - After `--recovery-wait-ms`, the start's command is sent as
//!   without the option, from a thread of its own: step `start`, or
//!   `end-recovery` for a device that waits for a server. Its line is
//!   `returned after T ms: ok` or `returned after T ms: errno E (...)`,
//!   with T counted from the command; or, if it has not returned 10 s
//!   after it was sent, `no answer after 10000 ms; its thread waits in
//!   CALL (wchan W)`, and the line `returned after ...` follows if it
//!   returns later. CALL is the system call the command's thread is in,
//!   from `/proc`. The command goes to the driver on an io_uring, so
//!   `io_uring_enter` is a wait for the kernel's answer to it; `futex`
//!   is the wait before it, the library's or the binding's, for the
//!   queue's buffers to be registered. Another call is `syscall N`, a
//!   thread in none `running`, and `unknown` if `/proc` said nothing. W
//!   is the thread's wait channel, as the kernel names it.
//!
//! Whatever the start answered, and also when the queue has ended, the
//! program then serves what the driver hands over and ends only on
//! `SIGTERM` or `SIGINT`, with its `served` line (`ended by a signal`).
//! A start's command that has not returned is left in the driver and
//! ends with the program (if it can: a thread inside the driver may
//! hold the process).
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

/// The command a step sends after a refused fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    /// The start of a device never started.
    Start,
    /// The end of a recovery: a device that waits for a server.
    Recovery,
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
    /// The request whose commit names no buffer.
    refuse: Option<What>,
    /// Tag 0's first fetch names no buffer; the command that follows.
    refuse_fetch: Option<Then>,
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

/// `read:OFFSET`, `write:OFFSET` or `flush`, split at the colons.
fn what(parts: &[&str]) -> Option<What> {
    match parts {
        ["flush"] => Some(What::Flush),
        ["read", at] => at.parse().ok().map(What::Read),
        ["write", at] => at.parse().ok().map(What::Write),
        _ => None,
    }
}

fn hold(text: &str) -> Result<Hold, String> {
    let bad = || {
        format!("--hold-one-request: '{text}' is not SECONDS:read:OFFSET, SECONDS:write:OFFSET or SECONDS:flush")
    };
    let parts: Vec<&str> = text.split(':').collect();
    let [seconds, rest @ ..] = parts.as_slice() else {
        return Err(bad());
    };
    let what = what(rest).ok_or_else(bad)?;
    let seconds: u64 = seconds.parse().map_err(|_| bad())?;
    Ok(Hold {
        time: Duration::from_secs(seconds),
        what,
    })
}

fn refuse(text: &str) -> Result<What, String> {
    let parts: Vec<&str> = text.split(':').collect();
    what(&parts).ok_or_else(|| {
        format!("--refuse-one-commit: '{text}' is not read:OFFSET, write:OFFSET or flush")
    })
}

fn then(text: &str) -> Result<Then, String> {
    match text {
        "start" => Ok(Then::Start),
        "recovery" => Ok(Then::Recovery),
        _ => Err(format!(
            "--refuse-one-fetch: '{text}' is not start or recovery"
        )),
    }
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
    let (mut delete_after, mut refused, mut refused_fetch) = (None, None, None);
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
                "--refuse-one-commit",
                "--refuse-one-fetch",
                "--delete-after-ms",
            ],
            "take-over" => &[
                "--file",
                "--recovery-wait-ms",
                "--hold-one-request",
                "--refuse-one-commit",
                "--refuse-one-fetch",
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
            "--refuse-one-commit" => refused = Some(refuse(value()?)?),
            "--refuse-one-fetch" => refused_fetch = Some(then(value()?)?),
            _ => held = Some(hold(value()?)?),
        }
    }
    let serve = |take_over: bool, begin: bool| {
        if let Some(then) = refused_fetch {
            if (then == Then::Recovery) != take_over {
                return Err(format!(
                    "{step}: --refuse-one-fetch {} is not what this step sends after its fetches",
                    if take_over { "start" } else { "recovery" }
                ));
            }
            if refused.is_some() || delete_after.is_some() {
                return Err(format!(
                    "{step}: --refuse-one-fetch goes with neither --refuse-one-commit nor --delete-after-ms"
                ));
            }
        }
        Ok(Step::Serve(Serve {
            file: file.clone().ok_or_else(|| format!("{step} needs --file"))?,
            size,
            take_over,
            begin,
            wait,
            hold: held,
            refuse: refused,
            refuse_fetch: refused_fetch,
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use skyzfs_blk::BLOCK_SIZE;
    use skyzfs_ublk::lifecycle::{AddError, Control, DeviceState, Shape};
    use skyzfs_ublk::pump::{Command, Fetched, Queue, Unfetched, Wake as Woken, ABORTED};
    use skyzfs_ublk::translate::{op, Desc};
    use skyzfs_ublk_sys::control::Kernel;
    use skyzfs_ublk_sys::queue::{Device, KernelQueue, Open};
    use skyzfs_ublk_sys::signals::{self, Signal};
    use skyzfs_ublk_sys::{raw, Wake, FLAGS};

    use super::{Hold, Serve, Step, Then, What};

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
        /// Commands of the queue that the driver ended with its abort.
        commands_aborted: AtomicU64,
        /// Commands it ended with any other result than OK.
        commands_failed: AtomicU64,
        /// Whether the server ended because the driver refused its
        /// commit with no buffer.
        refused: AtomicBool,
    }

    fn covers(desc: &Desc, byte: u64) -> bool {
        let start = desc.start_sector.saturating_mul(512);
        (start..start.saturating_add(u64::from(desc.nr_sectors) * 512)).contains(&byte)
    }

    fn matches(what: What, desc: &Desc) -> bool {
        match what {
            What::Read(byte) => desc.op() == op::READ && covers(desc, byte),
            What::Write(byte) => desc.op() == op::WRITE && covers(desc, byte),
            What::Flush => desc.op() == op::FLUSH,
        }
    }

    /// A request, for the program's lines.
    fn named(desc: &Desc) -> String {
        let at = desc.start_sector.saturating_mul(512);
        match desc.op() {
            op::READ => format!("a read at byte {at}"),
            op::WRITE => format!("a write at byte {at}"),
            op::FLUSH => "a flush".into(),
            other => format!("a request of kind {other} at byte {at}"),
        }
    }

    /// Answer one request from the file.
    ///
    /// Only a request with a place (a read, a write, a discard, a write
    /// of zeros) has its byte computed. A flush has none, whatever its
    /// sector field holds. The driver hands a flush over with
    /// `start_sector` u64::MAX and no sectors: the block layer's own
    /// flush request is made with sector -1 (`blk_rq_init`). The
    /// daemon's request log shows it on every one of the 330 flushes of
    /// the direct-write split probe's run 43455086383 (Ubuntu 26.04,
    /// kernel 7.0, amd64), and the control rig's panic on all three of
    /// its images shows a field whose byte does not fit in 64 bits. A
    /// place whose byte does not fit in 64 bits is answered with an I/O
    /// error, as the daemon answers a request outside its device.
    fn answer(queue: &mut impl Queue, file: &File, f: Fetched, counts: &Counts) {
        let at = || {
            f.desc
                .start_sector
                .checked_mul(512)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))
        };
        let bytes = u64::from(f.desc.nr_sectors) * 512;
        let blocks = (bytes / BLOCK_SIZE) as usize;
        let (done, buf) = match f.desc.op() {
            op::READ => {
                counts.reads.fetch_add(1, Ordering::Relaxed);
                match queue.take_buf(f.tag, blocks) {
                    Some(mut buf) => (
                        at().and_then(|at| file.read_exact_at(&mut buf, at)),
                        Some(buf),
                    ),
                    None => (Err(io::Error::from_raw_os_error(libc::EIO)), None),
                }
            }
            op::WRITE => {
                counts.writes.fetch_add(1, Ordering::Relaxed);
                match queue.take_buf(f.tag, blocks) {
                    Some(buf) => (at().and_then(|at| file.write_all_at(&buf, at)), Some(buf)),
                    None => (Err(io::Error::from_raw_os_error(libc::EIO)), None),
                }
            }
            op::FLUSH => {
                counts.flushes.fetch_add(1, Ordering::Relaxed);
                (file.sync_data(), None)
            }
            op::DISCARD | op::WRITE_ZEROES => {
                counts.trims.fetch_add(1, Ordering::Relaxed);
                let zeroed = at().and_then(|at| file.write_all_at(&vec![0u8; bytes as usize], at));
                (zeroed, None)
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

    /// What the server is told to do otherwise than answer at once.
    #[derive(Debug, Clone, Copy, Default)]
    struct Plan {
        hold: Option<Hold>,
        refuse: Option<What>,
        /// The tag whose first fetch was sent with no buffer.
        refuse_fetch: Option<u16>,
    }

    /// The tag whose first fetch `--refuse-one-fetch` sends with no
    /// buffer.
    const REFUSED_FETCH_TAG: u16 = 0;

    /// How long the start's command is waited for, with
    /// `--refuse-one-fetch`, before the program says that it has not
    /// returned.
    const START_LIMIT: Duration = Duration::from_secs(10);

    /// A result of the driver's other than OK, as text.
    fn refusal(result: i32) -> String {
        if result < 0 {
            errno(&io::Error::from_raw_os_error(-result))
        } else {
            format!("result {result}")
        }
    }

    /// A queue that can commit a request with no buffer: the malformed
    /// commit of `--refuse-one-commit`.
    /// An error says that nothing was sent.
    trait NoBuffer: Queue {
        fn commit_with_no_buffer(&mut self, tag: u16, result: i32) -> io::Result<()>;
    }

    impl NoBuffer for KernelQueue<'_> {
        fn commit_with_no_buffer(&mut self, tag: u16, result: i32) -> io::Result<()> {
            self.commit_with_no_buffer_for_tests(tag, result)
        }
    }

    /// The server: every request from the file, one of them late if
    /// `plan.hold` says so, and one committed with no buffer if
    /// `plan.refuse` says so. `say` prints a line of a step.
    ///
    /// It counts the commands the driver ended otherwise than OK. When
    /// the driver refuses the commit with no buffer, the server ends at
    /// once, as the daemon does on a refused commit: it answers nothing
    /// more (not what the same wait handed over, not a held request)
    /// and sends nothing more, and what it holds is the kernel's to
    /// issue again to the next server.
    ///
    /// With `plan.refuse_fetch` it says, before its first wait, that
    /// the tag's first fetch was sent with no buffer, and then what the
    /// driver answered to that fetch; it goes on whatever the answer.
    fn server(
        queue: &mut impl NoBuffer,
        file: File,
        plan: Plan,
        wake: Wake,
        counts: Arc<Counts>,
        say: impl Fn(&str, &str),
    ) {
        let (mut hold, mut refuse) = (plan.hold, plan.refuse);
        let mut fetched: Vec<Fetched> = Vec::new();
        let mut unfetched: Vec<Unfetched> = Vec::new();
        let mut held: Option<(Fetched, Instant)> = None;
        // The tag whose commit named no buffer, until the driver
        // answers that commit.
        let mut refusing: Option<u16> = None;
        // The tag whose first fetch named no buffer, until the driver
        // answers that fetch.
        let mut refusing_fetch = plan.refuse_fetch;
        if let Some(tag) = refusing_fetch {
            say("refuse-fetch", &format!("sent on tag {tag}"));
        }
        loop {
            fetched.clear();
            unfetched.clear();
            let waited = queue.wait(&mut fetched, &mut unfetched);
            let mut refused = false;
            for command in &unfetched {
                if command.result == ABORTED {
                    counts.commands_aborted.fetch_add(1, Ordering::Relaxed);
                } else {
                    counts.commands_failed.fetch_add(1, Ordering::Relaxed);
                }
                if refusing == Some(command.tag) && command.command == Command::Commit {
                    refusing = None;
                    if command.result == ABORTED {
                        say("refuse-commit", "aborted");
                    } else {
                        refused = true;
                        let why = refusal(command.result);
                        say("refuse-commit", &format!("refused: {why}"));
                    }
                }
                if refusing_fetch == Some(command.tag) && command.command == Command::Fetch {
                    refusing_fetch = None;
                    if command.result == ABORTED {
                        say("refuse-fetch", "aborted");
                    } else {
                        let why = refusal(command.result);
                        say("refuse-fetch", &format!("refused: {why}"));
                    }
                }
            }
            if refused {
                counts.refused.store(true, Ordering::SeqCst);
                return;
            }
            let Ok(woken) = waited else {
                return;
            };
            for f in fetched.drain(..) {
                counts.handed.fetch_add(1, Ordering::Relaxed);
                if refusing == Some(f.tag) {
                    // The tag was handed its next request: the driver
                    // took the commit with no buffer.
                    refusing = None;
                    say("refuse-commit", "taken");
                }
                if refusing_fetch == Some(f.tag) {
                    // The tag was handed a request: the driver took the
                    // fetch with no buffer. It was told no address for
                    // this request's data, so the request is answered
                    // with an I/O error and the file is not touched.
                    refusing_fetch = None;
                    say("refuse-fetch", "taken");
                    counts.failed.fetch_add(1, Ordering::Relaxed);
                    queue.commit(f.tag, -libc::EIO, None);
                    continue;
                }
                if refuse.is_some_and(|what| matches(what, &f.desc)) {
                    refuse = None;
                    match queue.commit_with_no_buffer(f.tag, -libc::EIO) {
                        Ok(()) => {
                            say(
                                "refuse-commit",
                                &format!("sent for {} on tag {}", named(&f.desc), f.tag),
                            );
                            refusing = Some(f.tag);
                            continue;
                        }
                        // Nothing was sent: the request is answered as
                        // any other, so that nothing waits on it.
                        Err(e) => say(
                            "refuse-commit",
                            &format!("not sent for {} on tag {}: {e}", named(&f.desc), f.tag),
                        ),
                    }
                }
                match hold {
                    Some(h) if matches(h.what, &f.desc) => {
                        hold = None;
                        counts.held.fetch_add(1, Ordering::Relaxed);
                        held = Some((f, Instant::now() + h.time));
                        let wake = wake.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(h.time);
                            wake.wake();
                        });
                    }
                    _ => answer(queue, &file, f, &counts),
                }
            }
            if woken == Woken::Stop {
                // The answers above have no wait behind them.
                let _ = queue.flush();
                return;
            }
            if let Some((f, _)) = held.take_if(|(_, due)| Instant::now() >= *due) {
                answer(queue, &file, f, &counts);
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
        let plan = Plan {
            hold: serve.hold,
            refuse: serve.refuse,
            refuse_fetch: serve.refuse_fetch.map(|_| REFUSED_FETCH_TAG),
        };
        let serving = {
            let (theirs, counts) = (wake.clone(), counts.clone());
            let body = move |mut queue: KernelQueue<'_>| {
                server(&mut queue, file, plan, theirs, counts, |step, answer| {
                    let _ = say(step, n, answer);
                })
            };
            match plan.refuse_fetch {
                Some(tag) => device.serve_with_fetch_refused_for_tests(&wake, false, tag, body),
                None => device.serve(&wake, false, body),
            }
        };
        let serving = match serving {
            Ok(serving) => serving,
            Err(e) => return say(name, n, &format!("refused at queue: {}", errno(&e))),
        };
        std::thread::sleep(serve.wait);
        let (by, deleting) = match serve.refuse_fetch {
            Some(then) => (start_aside(n, then, device)?, None),
            None => {
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
                        break if counts.refused.load(Ordering::SeqCst) {
                            "the refused commit"
                        } else {
                            "the queue ended"
                        };
                    }
                    if signals::wait(Duration::from_millis(100)) == Some(Signal::Stop) {
                        break "a signal";
                    }
                };
                (by, deleting)
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
                "ended by {by}; handed {} reads {} writes {} flushes {} discards {} others {} failed {} held {} first_write_copies {} commands_aborted {} commands_failed {}",
                count(&counts.handed),
                count(&counts.reads),
                count(&counts.writes),
                count(&counts.flushes),
                count(&counts.trims),
                count(&counts.others),
                count(&counts.failed),
                count(&counts.held),
                stopper.first_write_copies(),
                count(&counts.commands_aborted),
                count(&counts.commands_failed)
            ),
        )?;
        match deleted {
            // The thread has said its own lines; an error is its own.
            Some(Ok(late)) if !late.is_empty() => say("self-delete", n, &late),
            Some(Err(e)) => Err(e),
            _ => Ok(()),
        }
    }

    /// `--refuse-one-fetch`: send the start's command (`then`) from a
    /// thread of its own, say what it returned and how long it took,
    /// or that it has not returned within [`START_LIMIT`] (and then
    /// what it returns, if it does later), and wait for a signal,
    /// whatever it answered. The thread is not waited for: a command
    /// that has not returned ends with the program.
    fn start_aside(n: u32, then: Then, device: Device) -> Result<&'static str, String> {
        use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
        let step = match then {
            Then::Start => "start",
            Then::Recovery => "end-recovery",
        };
        let (done, answer) = std::sync::mpsc::channel();
        let (named, its_tid) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = named.send(own_tid());
            let from = Instant::now();
            let started = device.start_from_another_thread_for_tests();
            let _ = done.send((started, from.elapsed()));
        });
        let said = |(started, took): (io::Result<()>, Duration)| {
            let ms = took.as_millis();
            match started {
                Ok(()) => format!("returned after {ms} ms: ok"),
                Err(e) => format!("returned after {ms} ms: {}", errno(&e)),
            }
        };
        let gone = || format!("{step}: the command's thread ended with no answer");
        let mut waiting = match answer.recv_timeout(START_LIMIT) {
            Ok(started) => {
                say(step, n, &said(started))?;
                None
            }
            Err(RecvTimeoutError::Timeout) => {
                let tid = its_tid.try_recv().ok().flatten();
                say(
                    step,
                    n,
                    &format!(
                        "no answer after {} ms; its thread waits in {}",
                        START_LIMIT.as_millis(),
                        tid.map_or_else(|| "unknown".into(), waits_in)
                    ),
                )?;
                Some(answer)
            }
            Err(RecvTimeoutError::Disconnected) => return Err(gone()),
        };
        loop {
            if let Some(late) = &waiting {
                match late.try_recv() {
                    Ok(started) => {
                        say(step, n, &said(started))?;
                        waiting = None;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => return Err(gone()),
                }
            }
            if signals::wait(Duration::from_millis(100)) == Some(Signal::Stop) {
                return Ok("a signal");
            }
        }
    }

    /// The calling thread's id, from `/proc/thread-self`.
    fn own_tid() -> Option<u32> {
        let link = std::fs::read_link("/proc/thread-self").ok()?;
        link.file_name()?.to_str()?.parse().ok()
    }

    /// Where thread `tid` of this process waits, as the kernel says:
    /// the system call it is in and its wait channel. A wait for the
    /// kernel's answer to the command, which goes on an io_uring, is in
    /// `io_uring_enter`; the wait before the command for the queue's
    /// buffers is in `futex`.
    fn waits_in(tid: u32) -> String {
        let read = |what: &str| {
            std::fs::read_to_string(format!("/proc/self/task/{tid}/{what}")).unwrap_or_default()
        };
        let wchan = read("wchan");
        let wchan = match wchan.trim() {
            "" => "unknown",
            known => known,
        };
        format!("{} (wchan {wchan})", syscall_name(&read("syscall")))
    }

    /// The system call of a thread's `syscall` file: its first field,
    /// named if it is `io_uring_enter` or `futex`, `running` if the thread is
    /// not in one, `unknown` if the file said nothing.
    fn syscall_name(text: &str) -> String {
        match text.split_whitespace().next() {
            None => "unknown".into(),
            Some(first) => match first.parse::<libc::c_long>() {
                Ok(libc::SYS_io_uring_enter) => "io_uring_enter".into(),
                Ok(libc::SYS_futex) => "futex".into(),
                Ok(n) => format!("syscall {n}"),
                Err(_) => first.to_string(),
            },
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

    #[cfg(test)]
    mod tests {
        use std::cell::RefCell;
        use std::collections::VecDeque;

        use skyzfs_blk::Buf;
        use skyzfs_ublk::pump::QueueDown;

        use super::*;

        /// One wait of a [`Scripted`] queue: what it hands over and how
        /// it returns.
        type Turn = (Vec<Fetched>, Vec<Unfetched>, Result<Woken, QueueDown>);

        /// A queue that hands over what its script says, one wait at a
        /// time (then the queue is down), and records what the server
        /// committed: by tag, with the result and whether a buffer came
        /// back, or with no buffer.
        #[derive(Default)]
        struct Scripted {
            turns: VecDeque<Turn>,
            commits: Vec<(u16, i32, bool)>,
            no_buffer: Vec<(u16, i32)>,
            /// The library refuses a commit with no buffer.
            no_buffer_not_sent: bool,
        }

        impl Queue for Scripted {
            fn depth(&self) -> u16 {
                8
            }
            fn wait(
                &mut self,
                fetched: &mut Vec<Fetched>,
                unfetched: &mut Vec<Unfetched>,
            ) -> Result<Woken, QueueDown> {
                let (f, u, woken) =
                    self.turns
                        .pop_front()
                        .unwrap_or((Vec::new(), Vec::new(), Err(QueueDown)));
                fetched.extend(f);
                unfetched.extend(u);
                woken
            }
            fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<Buf> {
                Some(Buf::zeroed(blocks))
            }
            fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
                self.commits.push((tag, result, buf.is_some()));
            }
            fn flush(&mut self) -> Result<(), QueueDown> {
                Ok(())
            }
        }

        impl NoBuffer for Scripted {
            fn commit_with_no_buffer(&mut self, tag: u16, result: i32) -> io::Result<()> {
                if self.no_buffer_not_sent {
                    return Err(io::Error::other("refused by the library"));
                }
                self.no_buffer.push((tag, result));
                Ok(())
            }
        }

        fn read(tag: u16, at: u64) -> Fetched {
            Fetched {
                tag,
                desc: Desc {
                    op_flags: u32::from(op::READ),
                    nr_sectors: 8,
                    start_sector: at / 512,
                },
            }
        }

        fn ended(tag: u16, command: Command, result: i32) -> Unfetched {
            Unfetched {
                tag,
                command,
                result,
            }
        }

        /// Serve `turns` with `plan`; what was committed, the lines
        /// said, and the counts.
        fn run(turns: Vec<Turn>, plan: Plan) -> (Scripted, Vec<String>, Arc<Counts>) {
            run_on(
                Scripted {
                    turns: turns.into(),
                    ..Scripted::default()
                },
                plan,
            )
        }

        /// [`run`] on a queue made by the caller.
        fn run_on(mut queue: Scripted, plan: Plan) -> (Scripted, Vec<String>, Arc<Counts>) {
            let path = std::env::temp_dir().join(format!(
                "skyzfs-ublk-control-server-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .expect("a file");
            file.set_len(1 << 20).expect("its size");
            let _ = std::fs::remove_file(&path);
            let lines = RefCell::new(Vec::new());
            let counts = Arc::new(Counts::default());
            server(
                &mut queue,
                file,
                plan,
                Wake::new().expect("a wake"),
                counts.clone(),
                |step, answer| lines.borrow_mut().push(format!("{step}\t{answer}")),
            );
            (queue, lines.into_inner(), counts)
        }

        /// The request named is committed with no buffer and nothing
        /// else; when the driver refuses that commit the server ends at
        /// once: what the same wait handed over is held, not answered,
        /// and no wait follows. The refusal and the abort of another
        /// tag are counted apart.
        #[test]
        fn a_refused_commit_with_no_buffer_ends_the_server_at_once() {
            let plan = Plan {
                refuse: Some(What::Read(8192)),
                ..Plan::default()
            };
            let turns: Vec<Turn> = vec![
                (vec![read(1, 0), read(3, 8192)], Vec::new(), Ok(Woken::Work)),
                (
                    vec![read(4, 4096)],
                    vec![
                        ended(5, Command::Fetch, ABORTED),
                        ended(3, Command::Commit, -libc::EINVAL),
                    ],
                    Ok(Woken::Work),
                ),
                // Never reached.
                (vec![read(6, 0)], Vec::new(), Ok(Woken::Work)),
            ];
            let (queue, lines, counts) = run(turns, plan);
            assert_eq!(queue.no_buffer, vec![(3, -libc::EIO)]);
            assert_eq!(
                queue.commits,
                vec![(1, 4096, true)],
                "only tag 1 is answered"
            );
            assert_eq!(queue.turns.len(), 1, "no wait after the refusal");
            assert_eq!(
                lines,
                vec![
                    "refuse-commit\tsent for a read at byte 8192 on tag 3".to_string(),
                    format!(
                        "refuse-commit\trefused: {}",
                        errno(&io::Error::from_raw_os_error(libc::EINVAL))
                    ),
                ]
            );
            assert!(counts.refused.load(Ordering::SeqCst));
            assert_eq!(counts.commands_failed.load(Ordering::SeqCst), 1);
            assert_eq!(counts.commands_aborted.load(Ordering::SeqCst), 1);
            assert_eq!(counts.handed.load(Ordering::SeqCst), 2);
        }

        /// A commit with no buffer that the driver takes (its tag is
        /// handed the next request) or aborts does not end the server,
        /// and only the first request named is so committed.
        #[test]
        fn a_commit_with_no_buffer_that_is_taken_or_aborted_ends_nothing() {
            let plan = Plan {
                refuse: Some(What::Read(0)),
                ..Plan::default()
            };
            let taken: Vec<Turn> = vec![
                (vec![read(2, 0)], Vec::new(), Ok(Woken::Work)),
                (vec![read(2, 0)], Vec::new(), Ok(Woken::Work)),
                (Vec::new(), Vec::new(), Ok(Woken::Stop)),
            ];
            let (queue, lines, counts) = run(taken, plan);
            assert_eq!(queue.no_buffer, vec![(2, -libc::EIO)]);
            assert_eq!(queue.commits, vec![(2, 4096, true)]);
            assert_eq!(
                lines.last().map(String::as_str),
                Some("refuse-commit\ttaken")
            );
            assert!(!counts.refused.load(Ordering::SeqCst));
            let aborted: Vec<Turn> = vec![
                (vec![read(2, 0)], Vec::new(), Ok(Woken::Work)),
                (
                    Vec::new(),
                    vec![ended(2, Command::Commit, ABORTED)],
                    Ok(Woken::Work),
                ),
            ];
            let (_, lines, counts) = run(aborted, plan);
            assert_eq!(
                lines.last().map(String::as_str),
                Some("refuse-commit\taborted")
            );
            assert!(!counts.refused.load(Ordering::SeqCst));
            assert_eq!(counts.commands_aborted.load(Ordering::SeqCst), 1);
            assert_eq!(counts.commands_failed.load(Ordering::SeqCst), 0);
        }

        /// `--refuse-one-fetch`: the server says that the tag's first
        /// fetch was sent with no buffer before its first wait. The
        /// driver's refusal of that fetch is said with its error and
        /// counted as a refused command, and the server goes on: it
        /// answers the other tags and ends only when it is stopped.
        #[test]
        fn a_refused_first_fetch_is_said_and_the_server_goes_on() {
            let plan = Plan {
                refuse_fetch: Some(0),
                ..Plan::default()
            };
            let turns: Vec<Turn> = vec![
                (
                    Vec::new(),
                    vec![ended(0, Command::Fetch, -libc::EINVAL)],
                    Ok(Woken::Work),
                ),
                (vec![read(1, 0)], Vec::new(), Ok(Woken::Work)),
                (vec![read(2, 4096)], Vec::new(), Ok(Woken::Stop)),
                // Never reached: the server stopped.
                (vec![read(3, 0)], Vec::new(), Ok(Woken::Work)),
            ];
            let (queue, lines, counts) = run(turns, plan);
            assert_eq!(
                lines,
                vec![
                    "refuse-fetch\tsent on tag 0".to_string(),
                    format!(
                        "refuse-fetch\trefused: {}",
                        errno(&io::Error::from_raw_os_error(libc::EINVAL))
                    ),
                ]
            );
            assert_eq!(queue.commits, vec![(1, 4096, true), (2, 4096, true)]);
            assert_eq!(queue.turns.len(), 1, "the server ended at the stop");
            assert!(queue.no_buffer.is_empty());
            assert_eq!(counts.commands_failed.load(Ordering::SeqCst), 1);
            assert_eq!(counts.commands_aborted.load(Ordering::SeqCst), 0);
            assert!(!counts.refused.load(Ordering::SeqCst));
        }

        /// A first fetch with no buffer that the driver takes (its tag
        /// is handed a request) is said `taken`, and that request is
        /// answered with an I/O error, the file not touched: the driver
        /// was told no address for its data. One the driver aborts is said
        /// `aborted`. Only the tag named is followed: another tag's
        /// refused fetch is counted and not said, and a later command
        /// of the tag named is not said either.
        #[test]
        fn a_first_fetch_with_no_buffer_that_is_taken_or_aborted_is_said() {
            let plan = Plan {
                refuse_fetch: Some(0),
                ..Plan::default()
            };
            let taken: Vec<Turn> = vec![
                (
                    vec![read(0, 8192)],
                    vec![ended(5, Command::Fetch, -libc::EINVAL)],
                    Ok(Woken::Work),
                ),
                // Tag 0's next request, after the one its fetch with no
                // buffer brought: answered as any other, and not said.
                (vec![read(0, 8192)], Vec::new(), Ok(Woken::Work)),
                (
                    Vec::new(),
                    vec![ended(0, Command::Commit, ABORTED)],
                    Ok(Woken::Stop),
                ),
            ];
            let (queue, lines, counts) = run(taken, plan);
            assert_eq!(
                lines,
                vec![
                    "refuse-fetch\tsent on tag 0".to_string(),
                    "refuse-fetch\ttaken".to_string(),
                ]
            );
            assert_eq!(
                queue.commits,
                vec![(0, -libc::EIO, false), (0, 4096, true)],
                "the first answered with an error, the file not touched; the next as any other"
            );
            assert_eq!(counts.failed.load(Ordering::SeqCst), 1);
            assert_eq!(counts.reads.load(Ordering::SeqCst), 1);
            assert_eq!(counts.commands_failed.load(Ordering::SeqCst), 1);
            assert_eq!(counts.commands_aborted.load(Ordering::SeqCst), 1);
            let aborted: Vec<Turn> = vec![(
                Vec::new(),
                vec![ended(0, Command::Fetch, ABORTED)],
                Ok(Woken::Stop),
            )];
            let (_, lines, counts) = run(aborted, plan);
            assert_eq!(
                lines.last().map(String::as_str),
                Some("refuse-fetch\taborted")
            );
            assert_eq!(counts.commands_aborted.load(Ordering::SeqCst), 1);
            assert_eq!(counts.commands_failed.load(Ordering::SeqCst), 0);
        }

        /// Where the start's thread waits is read from its `syscall`
        /// and `wchan` files: `io_uring_enter` (the command is with the
        /// kernel) and `futex` (the wait for the buffers) by name, other
        /// calls by number.
        #[test]
        fn a_threads_wait_is_named_by_its_system_call() {
            let enter = format!(
                "{} 0x3 0x1 0x1 0x1 0x0 0x0 0x7ffc 0x7f00",
                libc::SYS_io_uring_enter
            );
            assert_eq!(syscall_name(&enter), "io_uring_enter");
            let futex = format!(
                "{} 0x55 0x80 0x0 0x0 0x0 0x0 0x7ffc 0x7f00",
                libc::SYS_futex
            );
            assert_eq!(syscall_name(&futex), "futex");
            assert_eq!(syscall_name("999999 0x0"), "syscall 999999");
            assert_eq!(syscall_name("running\n"), "running");
            assert_eq!(syscall_name(""), "unknown");
            // This thread reads its own files while it is in `read`, so
            // its call is that one, by number.
            let tid = own_tid().expect("this thread's id");
            let here = waits_in(tid);
            assert!(
                here.starts_with(&format!("syscall {} (wchan ", libc::SYS_read)),
                "{here}"
            );
        }

        /// A commit with no buffer that the library will not send is
        /// said so, and the request is answered as any other: nothing
        /// waits on it, nothing is counted as refused, and the server
        /// goes on.
        #[test]
        fn a_commit_with_no_buffer_that_is_not_sent_is_said_and_the_request_answered() {
            let plan = Plan {
                refuse: Some(What::Read(0)),
                ..Plan::default()
            };
            let turns: Vec<Turn> = vec![
                (vec![read(2, 0)], Vec::new(), Ok(Woken::Work)),
                (vec![read(3, 4096)], Vec::new(), Ok(Woken::Work)),
                (Vec::new(), Vec::new(), Ok(Woken::Stop)),
            ];
            let (queue, lines, counts) = run_on(
                Scripted {
                    turns: turns.into(),
                    no_buffer_not_sent: true,
                    ..Scripted::default()
                },
                plan,
            );
            assert!(queue.no_buffer.is_empty());
            assert_eq!(queue.commits, vec![(2, 4096, true), (3, 4096, true)]);
            assert_eq!(
                lines,
                vec![
                    "refuse-commit\tnot sent for a read at byte 0 on tag 2: refused by the library"
                        .to_string()
                ]
            );
            assert!(!counts.refused.load(Ordering::SeqCst));
            assert_eq!(counts.handed.load(Ordering::SeqCst), 2);
        }

        /// Every command the driver ends otherwise than OK is counted,
        /// also in the wait that finds the queue down: aborts apart from
        /// the rest. Without a plan nothing is committed with no buffer.
        #[test]
        fn the_commands_the_driver_ends_otherwise_than_ok_are_counted() {
            let aborts = (0..8)
                .map(|tag| ended(tag, Command::Commit, ABORTED))
                .collect();
            let turns: Vec<Turn> = vec![
                (
                    vec![read(0, 0)],
                    vec![ended(7, Command::Fetch, -libc::EBUSY)],
                    Ok(Woken::Work),
                ),
                (Vec::new(), aborts, Err(QueueDown)),
            ];
            let (queue, lines, counts) = run(turns, Plan::default());
            assert!(queue.no_buffer.is_empty());
            assert!(lines.is_empty());
            assert_eq!(counts.commands_aborted.load(Ordering::SeqCst), 8);
            assert_eq!(counts.commands_failed.load(Ordering::SeqCst), 1);
            assert!(!counts.refused.load(Ordering::SeqCst));
        }

        fn flush(tag: u16, start_sector: u64) -> Fetched {
            Fetched {
                tag,
                desc: Desc {
                    op_flags: u32::from(op::FLUSH),
                    nr_sectors: 0,
                    start_sector,
                },
            }
        }

        /// A flush has no place, whatever its sector field holds (the
        /// driver hands a flush over with u64::MAX there: see
        /// `answer`). It is answered as any flush, with a sync of the
        /// file and the result 0, and the server goes on. The control
        /// rig's take-over of a refused flush is handed such a flush
        /// again.
        #[test]
        fn a_flush_is_answered_whatever_its_sector_reads() {
            let turns: Vec<Turn> = vec![
                (
                    vec![flush(0, u64::MAX), read(1, 0)],
                    Vec::new(),
                    Ok(Woken::Work),
                ),
                (
                    vec![flush(2, 0), flush(3, 1 << 60)],
                    Vec::new(),
                    Ok(Woken::Work),
                ),
                (Vec::new(), Vec::new(), Ok(Woken::Stop)),
            ];
            let (queue, lines, counts) = run(turns, Plan::default());
            assert_eq!(
                queue.commits,
                vec![(0, 0, false), (1, 4096, true), (2, 0, false), (3, 0, false)]
            );
            assert!(lines.is_empty());
            assert_eq!(counts.flushes.load(Ordering::SeqCst), 3);
            assert_eq!(counts.failed.load(Ordering::SeqCst), 0);
        }

        /// The flush of the control rig's course: refused with no
        /// buffer by the first server, handed over again to the next
        /// one with `start_sector` u64::MAX, and answered by it.
        #[test]
        fn a_refused_flush_handed_over_again_is_answered() {
            let plan = Plan {
                refuse: Some(What::Flush),
                ..Plan::default()
            };
            let first: Vec<Turn> = vec![
                (vec![flush(4, u64::MAX)], Vec::new(), Ok(Woken::Work)),
                (
                    Vec::new(),
                    vec![ended(4, Command::Commit, -libc::EINVAL)],
                    Ok(Woken::Work),
                ),
            ];
            let (queue, _, counts) = run(first, plan);
            assert_eq!(queue.no_buffer, vec![(4, -libc::EIO)]);
            assert!(counts.refused.load(Ordering::SeqCst));
            let next: Vec<Turn> = vec![
                (vec![flush(4, u64::MAX)], Vec::new(), Ok(Woken::Work)),
                (Vec::new(), Vec::new(), Ok(Woken::Stop)),
            ];
            let (queue, lines, counts) = run(next, Plan::default());
            assert_eq!(queue.commits, vec![(4, 0, false)]);
            assert!(lines.is_empty());
            assert_eq!(counts.flushes.load(Ordering::SeqCst), 1);
        }

        /// A request with a place whose byte does not fit in 64 bits is
        /// answered with an I/O error, as the daemon answers a request
        /// outside the device, and the server goes on. The driver never
        /// hands one over for a device of a real size; the program must
        /// not end on it if it did.
        #[test]
        fn a_place_beyond_64_bits_is_answered_with_an_error() {
            let far = |tag, op_code: u8| Fetched {
                tag,
                desc: Desc {
                    op_flags: u32::from(op_code),
                    nr_sectors: 8,
                    start_sector: u64::MAX - 7,
                },
            };
            let turns: Vec<Turn> = vec![
                (
                    vec![
                        far(0, op::READ),
                        far(1, op::WRITE),
                        far(2, op::DISCARD),
                        far(3, op::WRITE_ZEROES),
                        read(4, 0),
                    ],
                    Vec::new(),
                    Ok(Woken::Work),
                ),
                (Vec::new(), Vec::new(), Ok(Woken::Stop)),
            ];
            let (queue, _, counts) = run(turns, Plan::default());
            assert_eq!(
                queue.commits,
                vec![
                    (0, -libc::EIO, true),
                    (1, -libc::EIO, true),
                    (2, -libc::EIO, false),
                    (3, -libc::EIO, false),
                    (4, 4096, true),
                ]
            );
            assert_eq!(counts.failed.load(Ordering::SeqCst), 4);
        }
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
            refuse: None,
            refuse_fetch: None,
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
    fn a_server_can_be_told_to_refuse_one_commit() {
        for (line, take_over, want) in [
            (
                "serve 1 --file f --refuse-one-commit read:8192",
                false,
                What::Read(8192),
            ),
            (
                "take-over 1 --file f --refuse-one-commit write:0",
                true,
                What::Write(0),
            ),
            (
                "serve 1 --refuse-one-commit flush --file f",
                false,
                What::Flush,
            ),
        ] {
            let Ok((_, 1, Step::Serve(serve))) = parse(&args(line)) else {
                panic!("{line}");
            };
            assert_eq!(serve.refuse, Some(want), "{line}");
            assert_eq!(serve.take_over, take_over, "{line}");
            assert_eq!(serve.hold, None, "{line}");
        }
        let Ok((_, _, Step::Serve(serve))) = parse(&args(
            "serve 1 --file f --hold-one-request 60:read:0 --refuse-one-commit read:4096",
        )) else {
            panic!("with a hold");
        };
        assert_eq!(serve.refuse, Some(What::Read(4096)));
        assert!(serve.hold.is_some());
        assert_eq!(refuse("read:4096"), Ok(What::Read(4096)));
        for bad in [
            "", "flush:1", "5:read:0", "read", "read:x", "trim:0", "5:flush",
        ] {
            assert!(refuse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_server_can_be_told_to_refuse_one_fetch() {
        for (line, want, take_over, begin) in [
            (
                "serve 1 --file f --refuse-one-fetch start",
                Then::Start,
                false,
                false,
            ),
            (
                "serve 1 --file f --recovery-begun --refuse-one-fetch recovery",
                Then::Recovery,
                true,
                false,
            ),
            (
                "take-over 1 --refuse-one-fetch recovery --file f",
                Then::Recovery,
                true,
                true,
            ),
        ] {
            let Ok((_, 1, Step::Serve(serve))) = parse(&args(line)) else {
                panic!("{line}");
            };
            assert_eq!(serve.refuse_fetch, Some(want), "{line}");
            assert_eq!((serve.take_over, serve.begin), (take_over, begin), "{line}");
            assert_eq!(serve.refuse, None, "{line}");
        }
        let Ok((_, _, Step::Serve(serve))) = parse(&args(
            "take-over 1 --file f --recovery-wait-ms 500 --hold-one-request 30:read:0 --refuse-one-fetch recovery",
        )) else {
            panic!("with a hold and a wait");
        };
        assert_eq!(serve.refuse_fetch, Some(Then::Recovery));
        assert_eq!(serve.wait, Duration::from_millis(500));
        assert!(serve.hold.is_some());
        assert_eq!(then("start"), Ok(Then::Start));
        assert_eq!(then("recovery"), Ok(Then::Recovery));
        for bad in [
            "",
            "Start",
            "start:0",
            "0",
            "recover",
            "end-recovery",
            "flush",
        ] {
            assert!(then(bad).is_err(), "{bad}");
        }
        // The value names the command the step sends after the fetch,
        // and the option goes with neither a refused commit nor a
        // delete of the server's own device.
        for bad in [
            "serve 1 --file f --refuse-one-fetch recovery",
            "serve 1 --file f --recovery-begun --refuse-one-fetch start",
            "take-over 1 --file f --refuse-one-fetch start",
            "serve 1 --file f --refuse-one-fetch start --refuse-one-commit flush",
            "take-over 1 --file f --refuse-one-commit read:0 --refuse-one-fetch recovery",
            "serve 1 --file f --refuse-one-fetch start --delete-after-ms 5",
            "serve 1 --file f --refuse-one-fetch",
            "serve 1 --file f --refuse-one-fetch now",
            "add 1 --refuse-one-fetch start",
            "info 1 --refuse-one-fetch start",
            "begin-recovery 1 --refuse-one-fetch recovery",
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
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
            "serve 1 --file f --refuse-one-commit",
            "serve 1 --file f --refuse-one-commit 5:read:0",
            "add 1 --refuse-one-commit flush",
            "delete 1 --refuse-one-commit flush",
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
    }
}
