//! The block device as a [`BlockTarget`], and the harness around it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use rustix::fs::{fallocate, FallocateFlags};
use skyzfs_blk::conformance::{
    cases, Capabilities, Capability, Config, Harness, HarnessError, Spec, Status,
};
use skyzfs_blk::{
    Base, BlkError, BlockTarget, Completion, Geometry, Op, Request, RequestId, SubmitError,
    SubmitErrorKind, SIZE_UNIT,
};

/// What the harness answers when a case asks for something a block
/// device cannot give. `main` turns a failure that starts with this
/// into `UNSUPPORTED`.
const UNSUPPORTED: &str = "not at a block device: ";

/// A trace of every request, for the diagnosis of a failed case: the
/// file named by `BLKCHECK_TRACE`, one line per event, tab-separated:
/// milliseconds since the first event, volume, event, id, operation,
/// first block, blocks, and a digest of each block's content.
fn trace(volume: u64, event: &str, id: Option<RequestId>, what: &str) {
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    static TRACE: OnceLock<Option<(Mutex<File>, Instant)>> = OnceLock::new();
    let Some((file, start)) = TRACE.get_or_init(|| {
        let path = std::env::var_os("BLKCHECK_TRACE")?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some((Mutex::new(file), Instant::now()))
    }) else {
        return;
    };
    let id = id.map_or_else(|| "-".to_string(), |id| id.0.to_string());
    let line = format!(
        "{}\t{volume}\t{event}\t{id}\t{what}\n",
        start.elapsed().as_millis()
    );
    if let Ok(mut file) = file.lock() {
        // A trace that cannot be written is not worth failing a run for.
        let _ = file.write_all(line.as_bytes());
    }
}

fn tracing() -> bool {
    std::env::var_os("BLKCHECK_TRACE").is_some()
}

/// One digest per 4096-byte block, so that a block's content can be
/// followed from the write that made it to the read that shows it.
fn digests(bytes: &[u8]) -> String {
    bytes
        .chunks(4096)
        .map(|block| {
            if block.iter().all(|b| *b == 0) {
                return "zero".to_string();
            }
            // FNV-1a, 64 bits: enough to tell contents apart.
            let hash = block.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
            });
            format!("{hash:016x}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The operation's name, its first block, its count of blocks and the
/// digests of a write's data, separated by tabs, for the trace.
fn describe(op: &Op) -> String {
    let (name, data): (&str, Option<&[u8]>) = match op {
        Op::Read { .. } => ("read", None),
        Op::Write { data, .. } => ("write", Some(data)),
        Op::Flush => ("flush", None),
        Op::Discard { .. } => ("discard", None),
        Op::WriteZeroes { .. } => ("write_zeroes", None),
    };
    let (first, blocks) = op.range().map_or((0, 0), |(o, l)| (o / 4096, l / 4096));
    format!(
        "{name}\t{first}\t{blocks}\t{}",
        data.map(digests).unwrap_or_default()
    )
}

/// A server process as the helper named it: its pid, and when that
/// process started, so that the pid is not signalled once it has become
/// another process's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Server {
    pid: i32,
    started: u64,
}

/// Field 22 of `/proc/PID/stat`: the start time of the process, in
/// clock ticks since boot. The fields are counted after the command
/// name, which is in brackets and may hold spaces.
fn start_time(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}

impl Server {
    /// The process `pid` as it is now, if there is one.
    fn now(pid: i32) -> Option<Self> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some(Self {
            pid,
            started: start_time(&stat)?,
        })
    }

    /// SIGKILL, if the pid is still the process it was when the helper
    /// named it. Whether the signal was sent. Between this look and the
    /// signal the process can still end and its pid be given away; the
    /// tool runs as root on a machine that is the rig's own, and the
    /// window is left at that.
    fn kill(self) -> bool {
        if Self::now(self.pid) != Some(self) {
            return false;
        }
        rustix::process::Pid::from_raw(self.pid).is_some_and(|pid| {
            rustix::process::kill_process(pid, rustix::process::Signal::KILL).is_ok()
        })
    }
}

/// A block device opened with `O_DIRECT`. Each request in flight is one
/// thread in one system call.
struct DeviceTarget {
    number: u64,
    /// The server's process, if the helper can say: the tool then
    /// sends the SIGKILL itself, at once, so that it can land while
    /// requests are in flight. Through the helper the signal comes
    /// tens of milliseconds later, when a fast server has long
    /// answered everything.
    server: Option<Server>,
    file: Arc<File>,
    /// The path, if it is a block device and not a plain file.
    device: Option<Arc<str>>,
    /// Set when the harness has made the volume's log fail. The server
    /// then deletes its device and exits, and what answers a request is
    /// the kernel, for a deleted device this target still holds open:
    /// an I/O error of one kind or another. From here on any such error
    /// is reported as the failure of the volume, which is what it is to
    /// a client; a request that succeeds is still reported as a success.
    failed: Arc<AtomicBool>,
    geometry: Geometry,
    done_tx: Sender<Completion>,
    done_rx: Receiver<Completion>,
    /// When each call returned, by the clock of the thread that made it.
    returned: Arc<std::sync::Mutex<BTreeMap<RequestId, std::time::Instant>>>,
    /// Flushes among the ids in flight: the kernel's count of requests
    /// in flight at a device is of reads and writes.
    flushes: BTreeSet<RequestId>,
    /// When the calls outstanding at the last kill were sorted into
    /// "completed before" and "in flight at the death".
    sorted_at: Option<std::time::Instant>,
    /// Ids handed to a thread whose completion has not been returned.
    in_flight: BTreeSet<RequestId>,
    /// Results of requests that were in flight at a kill. The kernel
    /// issued again by itself those the old server had not completed;
    /// one it had completed returns late, its call held by something
    /// else the old server held. The suite's own resubmission of the
    /// same id gets this result and is not issued.
    carried: BTreeMap<RequestId, Completion>,
    /// Carried results whose id has been resubmitted, ready for `poll`.
    ready: Vec<Completion>,
}

fn io_error(e: &std::io::Error) -> BlkError {
    BlkError::Io(e.kind())
}

/// Carry out one operation with plain system calls.
fn perform(
    file: &File,
    device: Option<&str>,
    op: Op,
) -> (Result<(), BlkError>, Option<skyzfs_blk::Buf>) {
    match op {
        Op::Read { offset, mut buf } => {
            let r = file
                .read_exact_at(&mut buf, offset)
                .map_err(|e| io_error(&e));
            (r, Some(buf))
        }
        Op::Write { offset, data } => {
            let r = file.write_all_at(&data, offset).map_err(|e| io_error(&e));
            (r, Some(data))
        }
        Op::Flush => (file.sync_all().map_err(|e| io_error(&e)), None),
        // A discard of a block device is the BLKDISCARD ioctl. There
        // is no call for it without `unsafe`, and the fallocate that
        // means the same (punch-hole with no-hide-stale) came back
        // "unsupported" from the rig's ublk devices (check a of the rig
        // observes which calls a device takes), so util-linux issues it. On a plain file, punch a hole.
        Op::Discard { offset, len } => {
            let r = match device {
                Some(path) => Command::new("blkdiscard")
                    .arg("--force")
                    .args(["--offset", &offset.to_string()])
                    .args(["--length", &len.to_string()])
                    .arg(path)
                    .output()
                    .map_err(|e| io_error(&e))
                    .and_then(|out| {
                        if out.status.success() {
                            Ok(())
                        } else {
                            Err(BlkError::Io(std::io::ErrorKind::Other))
                        }
                    }),
                None => {
                    let flags = FallocateFlags::PUNCH_HOLE | FallocateFlags::KEEP_SIZE;
                    fallocate(file, flags, offset, len)
                        .map_err(|e| BlkError::Io(std::io::Error::from(e).kind()))
                }
            };
            (r, None)
        }
        // Zero-range on a block device is a write of zeros that keeps
        // the blocks.
        Op::WriteZeroes { offset, len } => {
            let flags = FallocateFlags::ZERO_RANGE | FallocateFlags::KEEP_SIZE;
            let r = fallocate(file, flags, offset, len)
                .map_err(|e| BlkError::Io(std::io::Error::from(e).kind()));
            (r, None)
        }
    }
}

impl DeviceTarget {
    fn open(number: u64, path: &str, depth: u32) -> Result<Self, HarnessError> {
        let err =
            |what: &str, e: &dyn std::fmt::Display| HarnessError(format!("{what} {path}: {e}"));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_DIRECT)
            .open(path)
            .map_err(|e| err("open", &e))?;
        let size = file
            .seek(SeekFrom::End(0))
            .map_err(|e| err("size of", &e))?;
        if size == 0 || size % SIZE_UNIT != 0 {
            return Err(err(
                "size of",
                &format!("{size} bytes is not a multiple of 1 MiB"),
            ));
        }
        let geometry =
            Geometry::new(size / SIZE_UNIT, depth, false).map_err(|e| err("geometry of", &e))?;
        let is_device = file
            .metadata()
            .map(|m| std::os::unix::fs::FileTypeExt::is_block_device(&m.file_type()))
            .map_err(|e| err("type of", &e))?;
        if tracing() {
            // For the replayer: the model is built for this geometry.
            trace(
                number,
                "open",
                None,
                &format!("{}\t{depth}", size / SIZE_UNIT),
            );
        }
        let (done_tx, done_rx) = channel();
        Ok(Self {
            number,
            server: None,
            file: Arc::new(file),
            device: is_device.then(|| Arc::from(path)),
            failed: Arc::new(AtomicBool::new(false)),
            geometry,
            done_tx,
            done_rx,
            returned: Arc::default(),
            flushes: BTreeSet::new(),
            sorted_at: None,
            in_flight: BTreeSet::new(),
            carried: BTreeMap::new(),
            ready: Vec::new(),
        })
    }

    fn take(&mut self, completion: Completion, out: &mut Vec<Completion>) {
        // The moment the suite learns of the result: with the submits,
        // these lines are the history the suite's model is given.
        trace(self.number, "delivered", Some(completion.id), "");
        self.in_flight.remove(&completion.id);
        self.flushes.remove(&completion.id);
        out.push(completion);
    }

    fn drain(&mut self, out: &mut Vec<Completion>) -> usize {
        let before = out.len();
        for c in std::mem::take(&mut self.ready) {
            self.take(c, out);
        }
        while let Ok(c) = self.done_rx.try_recv() {
            self.take(c, out);
        }
        out.len() - before
    }

    /// The server has been stopped and is about to be killed. Every call
    /// that has returned by now was completed before the kill, and the
    /// kernel will have nothing of it to issue again. The suite does not
    /// know these results yet and will resubmit the ids, and to the
    /// suite's model they were in flight at the kill. These results are
    /// dropped and the resubmissions are issued: each is applied a
    /// second time, after the kill, which the model allows of a request
    /// in flight at a kill (rule R9).
    /// Returns how many there were.
    fn returned_before_kill(&mut self) -> usize {
        let mut got = Vec::new();
        self.drain(&mut got);
        for c in &got {
            trace(
                self.number,
                "before_kill",
                Some(c.id),
                &format!("{:?}", c.result),
            );
        }
        self.sorted_at = Some(std::time::Instant::now());
        got.len()
    }

    /// Reads, writes, discards and writes of zeros still outstanding.
    fn outstanding_data_calls(&self) -> usize {
        self.in_flight.len() - self.flushes.len()
    }

    /// Wait for every outstanding call to return, and keep the results
    /// for the ids the suite will resubmit. These calls were outstanding
    /// at a server that died. The kernel issued again by itself each
    /// request the old server had not completed, which is its
    /// application after the kill. A request the old server had
    /// completed is not issued again, though its call may return only
    /// now: an `O_DIRECT` write returns once the device's cached pages in
    /// its range are dropped, and a read holding one of them may have
    /// waited on the old server. The model allows both (rule R9).
    fn settle(&mut self, timeout: Duration) -> Result<(), HarnessError> {
        if let Some(what) = self.settle_checked(timeout)? {
            rig_error(&what);
        }
        Ok(())
    }

    /// `settle`, returning instead of exiting: `Some(why)` when a call
    /// carried across the kill had returned before the sorting.
    fn settle_checked(&mut self, timeout: Duration) -> Result<Option<String>, HarnessError> {
        let mut wrong = None;
        let mut got = Vec::new();
        self.drain(&mut got);
        while !self.in_flight.is_empty() {
            match self.done_rx.recv_timeout(timeout) {
                Ok(c) => self.take(c, &mut got),
                Err(_) => {
                    return Err(HarnessError(format!(
                        "{} requests in flight at the kill did not return within {timeout:?} of the new server",
                        self.in_flight.len()
                    )))
                }
            }
        }
        let returned = self.returned.lock().map(|r| r.clone()).unwrap_or_default();
        for c in got {
            trace(
                self.number,
                "carried",
                Some(c.id),
                &format!("{:?}", c.result),
            );
            if let Some(what) = sorted_wrongly(returned.get(&c.id).copied(), self.sorted_at) {
                wrong.get_or_insert(format!("volume {}, request {}: {what}", self.number, c.id));
            }
            self.carried.insert(c.id, c);
        }
        self.sorted_at = None;
        Ok(wrong)
    }
}

impl BlockTarget for DeviceTarget {
    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
        if self.in_flight.contains(&request.id) {
            return Err(SubmitError {
                kind: SubmitErrorKind::DuplicateId,
                request,
            });
        }
        if self.in_flight.len() >= self.geometry.queue_depth() as usize {
            return Err(SubmitError {
                kind: SubmitErrorKind::Busy,
                request,
            });
        }
        self.in_flight.insert(request.id);
        if request.op == Op::Flush {
            self.flushes.insert(request.id);
        }
        if let Some(c) = self.carried.remove(&request.id) {
            trace(self.number, "held_back", Some(c.id), "");
            self.ready.push(c);
            return Ok(());
        }
        let file = Arc::clone(&self.file);
        let device = self.device.clone();
        let failed = Arc::clone(&self.failed);
        let tx = self.done_tx.clone();
        let returned = Arc::clone(&self.returned);
        let number = self.number;
        if tracing() {
            trace(number, "submit", Some(request.id), &describe(&request.op));
        }
        std::thread::spawn(move || {
            let Request { id, op } = request;
            let is_read = matches!(op, Op::Read { .. });
            let (result, buf) = perform(&file, device.as_deref(), op);
            if let Ok(mut returned) = returned.lock() {
                returned.insert(id, std::time::Instant::now());
            }
            // Looked at when the call has returned, not when it began:
            // a request in flight while the log fails is answered by
            // the failure.
            let raw = format!("{result:?}");
            let result = after_failure(result, failed.load(Ordering::SeqCst));
            if tracing() {
                let seen = match &buf {
                    Some(buf) if is_read => digests(buf),
                    _ => String::new(),
                };
                // What the suite is given, which the replayer reads,
                // and then what the system call returned.
                trace(
                    number,
                    "done",
                    Some(id),
                    &format!("{result:?}\t{seen}\t{raw}"),
                );
            }
            // The receiver is gone only when the target was dropped.
            let _ = tx.send(Completion { id, result, buf });
        });
        Ok(())
    }

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        self.drain(out)
    }

    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize {
        let n = self.drain(out);
        if n > 0 || self.in_flight.is_empty() {
            return n;
        }
        match self.done_rx.recv_timeout(timeout) {
            Ok(c) => {
                self.take(c, out);
                1 + self.drain(out)
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => 0,
        }
    }
}

/// The depth the tool gives the suite when none is asked for, and the
/// one a trace without an "open" line is replayed at.
pub const DEPTH: u32 = 16;

/// The sorting of the calls outstanding at a kill cannot be trusted:
/// say so and stop, with a status the rig reports as its own error. The
/// suite must not judge a server on a history the tool got wrong.
fn rig_error(what: &str) -> ! {
    eprintln!("RIG ERROR: {what}");
    std::process::exit(3)
}

/// A call carried across a kill as "in flight at the server's death"
/// must have returned after the sorting, which is done while no server
/// runs. One that returned before it was completed by the old server
/// and should have been issued again: `Some(why)`.
fn sorted_wrongly(
    returned: Option<std::time::Instant>,
    sorted_at: Option<std::time::Instant>,
) -> Option<String> {
    let (returned, sorted_at) = (returned?, sorted_at?);
    (returned < sorted_at).then(|| {
        format!(
            "the call returned {:?} before the calls outstanding at the kill were sorted, and was taken as in flight at the server's death",
            sorted_at - returned
        )
    })
}

/// What a result means once the log has been made to fail. The server
/// then deletes its device and exits, so whatever I/O error the kernel
/// answers with for a deleted device is the volume having failed. A
/// request that succeeds stays a success, for the suite to judge.
fn after_failure(result: Result<(), BlkError>, failed: bool) -> Result<(), BlkError> {
    match result {
        Err(BlkError::Io(_)) if failed => Err(BlkError::Failed),
        other => other,
    }
}

/// Makes, kills and fails volumes through the helper command.
struct DeviceHarness {
    command: String,
    depth: u32,
    timeout: Duration,
    capabilities: Capabilities,
    next: u64,
    /// The volume before the newest, destroyed when the next is made.
    last: Option<u64>,
}

impl DeviceHarness {
    /// The server's pid from the helper's `pid`, if it has one to give.
    fn server_of(&self, number: u64) -> Option<Server> {
        let pid: i32 = self.helper("pid", number).ok()?.parse().ok()?;
        Server::now(pid)
    }

    /// Run the helper; the last line of its stdout.
    fn helper(&self, verb: &str, number: u64) -> Result<String, HarnessError> {
        self.helper_watched(verb, number, &mut |_| None)
    }

    /// Run the helper and call `on_line` with each line of its stdout as
    /// it is printed; what `on_line` returns is written to the helper's
    /// stdin as a line. The last line of its stdout.
    fn helper_watched(
        &self,
        verb: &str,
        number: u64,
        on_line: &mut dyn FnMut(&str) -> Option<&'static str>,
    ) -> Result<String, HarnessError> {
        use std::io::{BufRead, BufReader, Write};
        use std::process::Stdio;
        let failed = |e: &dyn std::fmt::Display| {
            HarnessError(format!("{} {verb} {number}: {e}", self.command))
        };
        let mut child = Command::new(&self.command)
            .arg(verb)
            .arg(number.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| failed(&e))?;
        let mut printed = Vec::new();
        let mut stdin = child.stdin.take();
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines() {
                let line = line.map_err(|e| failed(&e))?;
                if let (Some(reply), Some(stdin)) = (on_line(&line), stdin.as_mut()) {
                    // A helper that does not read has gone on by itself.
                    let _ = writeln!(stdin, "{reply}");
                    let _ = stdin.flush();
                }
                printed.extend_from_slice(line.as_bytes());
                printed.push(b'\n');
            }
        }
        let mut out = child.wait_with_output().map_err(|e| failed(&e))?;
        out.stdout = printed;
        if !out.status.success() {
            return Err(HarnessError(format!(
                "{} {verb} {number}: {}: {}",
                self.command,
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .unwrap_or("")
            .trim()
            .to_string())
    }
}

impl Harness for DeviceHarness {
    type Target = DeviceTarget;

    /// Never a power cut, a base, a read-only volume or an invalid
    /// request: the kernel is between the suite and the server. A log
    /// that fails and a failed volume opened again only when the helper
    /// command can do them (`--can`).
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn create(&mut self, spec: &Spec) -> Result<DeviceTarget, HarnessError> {
        if spec.read_only {
            return Err(HarnessError(format!("{UNSUPPORTED}a read-only volume")));
        }
        if spec.base != Base::Empty {
            return Err(HarnessError(format!("{UNSUPPORTED}a volume on a base")));
        }
        if let Some(old) = self.last.take() {
            self.helper("destroy", old)?;
        }
        self.next += 1;
        let number = self.next;
        let path = self.helper("create", number)?;
        self.last = Some(number);
        let mut target = DeviceTarget::open(number, &path, self.depth)?;
        target.server = self.server_of(number);
        Ok(target)
    }

    fn kill(&mut self, mut target: DeviceTarget, _seed: u64) -> Result<DeviceTarget, HarnessError> {
        let number = target.number;
        // The signal first, from here: nothing is faster.
        let sent = target.server.take().map(Server::kill);
        trace(
            number,
            "kill",
            None,
            &format!("signal sent by the tool: {sent:?}"),
        );
        // The helper says "dead R W" when the old server is gone and
        // the new one is not started: no request completes in between.
        // It serves again only when it has read "sorted" from here.
        // R and W are the kernel's counts of reads and writes in flight
        // at the device at that moment. They are written down and not
        // used: the kernel counts 0 there whatever waits to be issued
        // again (seen at 18 of 18 kills with calls outstanding, on
        // each of two kernels), so they cannot check the sorting.
        self.helper_watched("kill", number, &mut |line| {
            let mut words = line.split_whitespace();
            if words.next() != Some("dead") {
                return None;
            }
            target.returned_before_kill();
            let counts: Vec<u64> = words.filter_map(|w| w.parse().ok()).collect();
            let outstanding = target.outstanding_data_calls();
            trace(
                number,
                "sorted",
                None,
                &format!("outstanding {outstanding}\tkernel in flight {counts:?}"),
            );
            Some("sorted")
        })?;
        trace(target.number, "served_again", None, "");
        target.settle(self.timeout)?;
        trace(target.number, "settled", None, "");
        // A volume whose log has failed has no server to ask for, and
        // a pid remembered from before may be anybody's by now.
        if !target.failed.load(Ordering::SeqCst) {
            target.server = self.server_of(number);
        }
        Ok(target)
    }

    fn cut_points(&mut self, _target: &DeviceTarget) -> u64 {
        0
    }

    fn power_cut(
        &mut self,
        _target: DeviceTarget,
        _keep: u64,
    ) -> Result<DeviceTarget, HarnessError> {
        Err(HarnessError(format!("{UNSUPPORTED}a power cut")))
    }

    fn fail_log(&mut self, target: &mut DeviceTarget) -> Result<(), HarnessError> {
        // Before the helper runs, not after it returns: the helper
        // waits for the server to exit, and a request in flight in that
        // time comes back with the kernel's I/O error while the helper
        // is still running. If the helper could not make the log fail,
        // the volume has not failed.
        trace(target.number, "fail", None, "");
        // The server exits when its log fails: its pid is nobody's, or
        // somebody else's, from here on.
        target.server = None;
        target.failed.store(true, Ordering::SeqCst);
        if let Err(e) = self.helper("fail", target.number) {
            target.failed.store(false, Ordering::SeqCst);
            return Err(e);
        }
        Ok(())
    }

    fn reopen_failed(&mut self, target: DeviceTarget) -> Result<DeviceTarget, HarnessError> {
        let number = target.number;
        // The client lets go of the failed device first.
        drop(target);
        trace(number, "reopen", None, "");
        let path = self.helper("reopen", number)?;
        let mut target = DeviceTarget::open(number, &path, self.depth)?;
        target.server = self.server_of(number);
        Ok(target)
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: skyzfs-blkcheck --harness CMD [--list] [--can fail-log] [--can reopen-failed] [--depth N] \
         [--cases N] [--rounds N] [--seed N] [--timeout SECONDS] [--case NAME]..."
    );
    ExitCode::from(2)
}

/// One line, tabs and newlines flattened.
fn flat(s: &str) -> String {
    s.replace(['\t', '\n'], " ")
}

pub fn main() -> ExitCode {
    let mut command = None;
    let mut depth = DEPTH;
    let mut config = Config {
        cases: 16,
        rounds: 4,
        ..Config::default()
    };
    let mut only: Vec<String> = Vec::new();
    let (mut can_fail, mut can_reopen) = (false, false);
    let mut list = false;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--list" {
            list = true;
            continue;
        }
        let Some(value) = args.next() else {
            return usage();
        };
        let number = value.parse::<u64>();
        match (flag.as_str(), number) {
            ("--harness", _) => command = Some(value),
            ("--case", _) => only.push(value),
            ("--can", _) if value == "fail-log" => can_fail = true,
            ("--can", _) if value == "reopen-failed" => can_reopen = true,
            ("--depth", Ok(n)) if n > 0 && n <= 1024 => depth = n as u32,
            ("--cases", Ok(n)) if n > 0 && n <= u64::from(u32::MAX) => config.cases = n as u32,
            ("--rounds", Ok(n)) if n > 0 && n <= u64::from(u32::MAX) => config.rounds = n as u32,
            ("--seed", Ok(n)) => config.seed = n,
            ("--timeout", Ok(n)) if n > 0 => config.timeout = Duration::from_secs(n),
            _ => return usage(),
        }
    }
    let Some(command) = command else {
        return usage();
    };
    let mut capabilities = Capabilities::ALL
        .without(Capability::PowerCut)
        .without(Capability::Base)
        .without(Capability::ReadOnly)
        .without(Capability::InvalidRequests);
    if !can_fail {
        capabilities = capabilities.without(Capability::FailLog);
    }
    if !can_reopen {
        capabilities = capabilities.without(Capability::ReopenFailed);
    }
    let mut harness = DeviceHarness {
        command,
        depth,
        timeout: config.timeout,
        capabilities,
        next: 0,
        last: None,
    };
    for case in cases::<DeviceHarness>() {
        if !only.is_empty() && !only.iter().any(|n| n == case.name) {
            continue;
        }
        // What the suite says of this case for a harness with these
        // capabilities, before anything runs.
        let declared = if case.requires.iter().any(|c| !capabilities.has(*c)) {
            "NOT_EXERCISED"
        } else if case.weaker.iter().any(|(c, _)| !capabilities.has(*c)) {
            "WEAKER"
        } else {
            "PASS"
        };
        println!("declared\t{}\t{declared}", case.name);
        if list {
            continue;
        }
        let (result, detail) = match case.run_on(&mut harness, &config) {
            Ok(Status::Passed) => ("PASS", String::new()),
            Ok(Status::Weaker(what)) => ("WEAKER", what),
            Ok(Status::NotExercised(why)) => ("NOT_EXERCISED", why.to_string()),
            Err(e) => {
                let text = e.to_string();
                match text.find(UNSUPPORTED) {
                    Some(at) => ("UNSUPPORTED", text[at..].to_string()),
                    None => ("FAIL", text),
                }
            }
        };
        println!("case\t{}\t{result}\t{}", case.name, flat(&detail));
    }
    if let Some(old) = harness.last.take() {
        if let Err(e) = harness.helper("destroy", old) {
            eprintln!("{e}");
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use skyzfs_blk::{Buf, BLOCK_SIZE, BLOCK_USIZE};

    fn filled(byte: u8) -> Buf {
        let mut buf = Buf::zeroed(1);
        buf.fill(byte);
        buf
    }

    /// A plain file stands in for the device: what is tested is this
    /// adapter's bookkeeping, which is the same on either.
    fn target(name: &str) -> (DeviceTarget, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("blkcheck-{}-{name}", std::process::id()));
        let file = File::create(&path).unwrap();
        file.set_len(4 * SIZE_UNIT).unwrap();
        (
            DeviceTarget::open(1, path.to_str().unwrap(), 16).unwrap(),
            path,
        )
    }

    /// After a kill the kernel issues the requests in flight again by
    /// itself, and the suite resubmits the same ids. Each id must then
    /// complete exactly once, with the result of the call that was
    /// outstanding, and the resubmission must not be issued: a second
    /// write under the same id would otherwise land after later work.
    #[test]
    fn a_request_in_flight_at_a_kill_completes_exactly_once() {
        let (mut t, path) = target("once");
        let ids: Vec<RequestId> = (1..=8).map(RequestId).collect();
        for id in &ids {
            let op = Op::Write {
                offset: (id.0 - 1) * BLOCK_SIZE,
                data: filled(0xaa),
            };
            t.submit(Request { id: *id, op }).unwrap();
        }
        // The kill: the outstanding calls return under the new server.
        t.settle(Duration::from_secs(30)).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            t.poll(&mut out),
            0,
            "nothing is delivered before the resubmission"
        );
        // The suite's reissue, under the same ids. The data differs, so
        // that a resubmission that was issued shows on the device.
        for id in &ids {
            let op = Op::Write {
                offset: (id.0 - 1) * BLOCK_SIZE,
                data: filled(0x55),
            };
            t.submit(Request { id: *id, op }).unwrap();
        }
        assert_eq!(t.poll(&mut out), 8);
        let mut seen: Vec<u64> = out.iter().map(|c| c.id.0).collect();
        seen.sort_unstable();
        assert_eq!(seen, (1..=8).collect::<Vec<u64>>(), "each id exactly once");
        assert!(out.iter().all(|c| c.result == Ok(())));
        // Nothing more arrives, however long one waits.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(t.poll(&mut out), 0, "no second completion");
        let on_device = std::fs::read(&path).unwrap();
        assert!(
            on_device[..8 * BLOCK_USIZE].iter().all(|b| *b == 0xaa),
            "the resubmission was issued"
        );
        std::fs::remove_file(&path).unwrap();
    }

    /// A call that is carried across a kill as "in flight at the death"
    /// and had returned before the sorting is a rig error from `settle`.
    /// The sorting is the real one, and the call is one that returned
    /// after it; its time of return is then put before the sorting, as
    /// for a thread that returned under the old server and was not seen
    /// by the sorting. Without the look in `settle`, or with a sorting
    /// that does not keep its time, this fails.
    #[test]
    fn settle_reports_a_call_that_returned_before_the_sorting() {
        let (mut t, path) = target("settle");
        let before = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(t.returned_before_kill(), 0, "nothing was outstanding");
        let id = RequestId(9);
        let op = Op::Write {
            offset: 0,
            data: filled(0x11),
        };
        t.submit(Request { id, op }).unwrap();
        for _ in 0..3000 {
            if t.returned.lock().unwrap().contains_key(&id) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // Returned after the sorting: carried, and nothing to report.
        // Its stamp moved before the sorting: reported.
        t.returned.lock().unwrap().insert(id, before);
        let said = t.settle_checked(Duration::from_secs(30)).unwrap();
        assert!(
            said.as_deref().is_some_and(|s| s.contains("request #9")),
            "{said:?}"
        );
        // And the same call with its own stamp is not reported.
        assert_eq!(t.returned_before_kill(), 0);
        let id = RequestId(10);
        let op = Op::Write {
            offset: 0,
            data: filled(0x22),
        };
        t.submit(Request { id, op }).unwrap();
        assert_eq!(t.settle_checked(Duration::from_secs(30)).unwrap(), None);
        std::fs::remove_file(&path).unwrap();
    }

    /// An I/O error counts as "the volume has failed" only after the
    /// log was made to fail; nothing else is touched.
    #[test]
    fn an_io_error_after_the_failure_is_the_volume_having_failed() {
        let io = Err(BlkError::Io(std::io::ErrorKind::Other));
        assert_eq!(after_failure(io, true), Err(BlkError::Failed));
        assert_eq!(after_failure(io, false), io, "before the failure");
        assert_eq!(after_failure(Ok(()), true), Ok(()), "a success stays one");
        assert_eq!(
            after_failure(Err(BlkError::ZeroLength), true),
            Err(BlkError::ZeroLength),
            "another error stays what it is"
        );
        assert_eq!(
            after_failure(Err(BlkError::Failed), false),
            Err(BlkError::Failed)
        );
    }

    fn harness(name: &str, script: &str) -> (DeviceHarness, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("blkcheck-{}-{name}.sh", std::process::id()));
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let harness = DeviceHarness {
            command: path.to_str().unwrap().to_string(),
            depth: 16,
            timeout: Duration::from_secs(30),
            capabilities: Capabilities::ALL,
            next: 0,
            last: None,
        };
        (harness, path)
    }

    /// The volume counts as failed from the moment the helper is asked
    /// to make the log fail: a request in flight returns with the
    /// kernel's I/O error while the helper is still waiting for the
    /// server to exit, and must be reported as failed, not as an error.
    #[test]
    fn the_volume_counts_as_failed_while_the_helper_runs() {
        let (mut t, path) = target("flag");
        let (mut h, script) = harness("slow", "sleep 1");
        let flag = Arc::clone(&t.failed);
        let watcher = std::thread::spawn(move || {
            // Well inside the helper's second.
            std::thread::sleep(Duration::from_millis(400));
            flag.load(Ordering::SeqCst)
        });
        h.fail_log(&mut t).unwrap();
        assert!(
            watcher.join().unwrap(),
            "the flag was set only after the helper returned"
        );
        assert!(t.failed.load(Ordering::SeqCst));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&script).unwrap();
    }

    /// A helper that could not make the log fail leaves the volume as
    /// it was: later I/O errors are errors.
    #[test]
    fn a_failure_the_helper_could_not_cause_is_not_a_failure() {
        let (mut t, path) = target("nofail");
        let (mut h, script) = harness("refuses", "exit 1");
        assert!(h.fail_log(&mut t).is_err());
        assert!(!t.failed.load(Ordering::SeqCst));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&script).unwrap();
    }

    /// A call that had returned before the server was killed was not in
    /// flight at the kill, and the kernel issues nothing of it again.
    /// The suite does not know that: the id is unacknowledged, it
    /// resubmits it, and its model requires the request to be applied
    /// from the kill on. So that resubmission is issued, and the id
    /// still completes exactly once.
    #[test]
    fn a_request_completed_before_the_kill_is_issued_again() {
        let (mut t, path) = target("before");
        let write = |byte: u8| Request {
            id: RequestId(1),
            op: Op::Write {
                offset: 0,
                data: filled(byte),
            },
        };
        t.submit(write(0xaa)).unwrap();
        // The call returns; the suite has not polled.
        std::thread::sleep(Duration::from_millis(200));
        // The helper says the old server is dead: what has returned by
        // now was completed before the kill.
        assert_eq!(t.returned_before_kill(), 1);
        t.settle(Duration::from_secs(30)).unwrap();
        let mut out = Vec::new();
        assert_eq!(t.poll(&mut out), 0, "the old result is not handed out");
        // The suite's reissue, with other data so that it shows.
        t.submit(write(0x55)).unwrap();
        assert_eq!(t.wait(&mut out, Duration::from_secs(30)), 1);
        assert_eq!(out[0].id, RequestId(1));
        assert_eq!(out[0].result, Ok(()));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(t.poll(&mut out), 0, "no second completion");
        let on_device = std::fs::read(&path).unwrap();
        assert!(
            on_device[..BLOCK_USIZE].iter().all(|b| *b == 0x55),
            "the resubmission was not issued"
        );
        std::fs::remove_file(&path).unwrap();
    }

    /// The start time is field 22 whatever the command is called, and a
    /// pid is signalled only while it is the process the helper named.
    #[test]
    fn a_pid_is_signalled_only_while_it_is_the_same_process() {
        let stat = "1234 (a b) c) S 1 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10 18446744073709551615";
        assert_eq!(start_time(stat), Some(987_654));
        assert_eq!(start_time("garbage"), None);
        let me = i32::try_from(std::process::id()).unwrap();
        let this = Server::now(me).unwrap();
        // The same pid with another start time is another process: no signal.
        let other = Server {
            started: this.started + 1,
            ..this
        };
        assert!(!other.kill());
        // A pid that is nobody's.
        assert!(Server::now(i32::MAX - 7).is_none());
        // A child of this test is killed when it is the process named.
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        let named = Server::now(i32::try_from(child.id()).unwrap()).unwrap();
        assert!(named.kill());
        assert!(!child.wait().unwrap().success());
        assert!(!named.kill(), "a process that is gone is not signalled");
    }

    /// The check on the sorting of calls at a kill.
    #[test]
    fn a_wrong_sorting_is_noticed() {
        let now = std::time::Instant::now();
        let later = now + Duration::from_millis(5);
        assert!(
            sorted_wrongly(Some(now), Some(later)).is_some(),
            "returned before the sorting"
        );
        assert!(
            sorted_wrongly(Some(later), Some(now)).is_none(),
            "returned after it"
        );
        assert!(
            sorted_wrongly(Some(now), None).is_none(),
            "no sorting: a helper without the line"
        );
        assert!(sorted_wrongly(None, Some(now)).is_none());
    }

    /// The helper's line is acted on when it is printed, not when the
    /// helper has finished: by then a new server is up.
    #[test]
    fn the_stop_is_noticed_while_the_helper_runs() {
        let (h, script) = harness("stops", "echo dead 0 0; sleep 1; echo /dev/null");
        let started = std::time::Instant::now();
        let mut at = None;
        let last = h
            .helper_watched("kill", 1, &mut |line| {
                if line == "dead 0 0" {
                    at = Some(started.elapsed());
                }
                None
            })
            .unwrap();
        assert_eq!(last, "/dev/null");
        assert!(
            at.unwrap() < Duration::from_millis(700),
            "noticed only at the end"
        );
        std::fs::remove_file(&script).unwrap();
    }

    /// A request submitted after the kill under a new id is issued as
    /// usual: only the ids that were in flight are held back.
    #[test]
    fn a_new_request_after_a_kill_is_issued() {
        let (mut t, path) = target("new");
        let write = |id: u64, byte: u8| Request {
            id: RequestId(id),
            op: Op::Write {
                offset: 0,
                data: filled(byte),
            },
        };
        t.submit(write(1, 0xaa)).unwrap();
        t.settle(Duration::from_secs(30)).unwrap();
        t.submit(write(1, 0xaa)).unwrap();
        t.submit(write(2, 0x55)).unwrap();
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while out.len() < 2 && std::time::Instant::now() < deadline {
            t.poll(&mut out);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(out.len(), 2);
        let on_device = std::fs::read(&path).unwrap();
        assert!(on_device[..BLOCK_USIZE].iter().all(|b| *b == 0x55));
        std::fs::remove_file(&path).unwrap();
    }
}
