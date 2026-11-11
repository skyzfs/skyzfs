//! The block device as a [`BlockTarget`], and the harness around it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::process::{Command, ExitCode};
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

/// A block device opened with `O_DIRECT`. Each request in flight is one
/// thread in one system call.
struct DeviceTarget {
    number: u64,
    file: Arc<File>,
    /// The path, if it is a block device and not a plain file.
    device: Option<Arc<str>>,
    geometry: Geometry,
    done_tx: Sender<Completion>,
    done_rx: Receiver<Completion>,
    /// Ids handed to a thread whose completion has not been returned.
    in_flight: BTreeSet<RequestId>,
    /// Results of requests that were in flight at a kill. The kernel
    /// issued them again by itself; the suite's own resubmission of the
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
        let (done_tx, done_rx) = channel();
        Ok(Self {
            number,
            file: Arc::new(file),
            device: is_device.then(|| Arc::from(path)),
            geometry,
            done_tx,
            done_rx,
            in_flight: BTreeSet::new(),
            carried: BTreeMap::new(),
            ready: Vec::new(),
        })
    }

    fn take(&mut self, completion: Completion, out: &mut Vec<Completion>) {
        self.in_flight.remove(&completion.id);
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

    /// Wait for every outstanding call to return, and keep the results
    /// for the ids the suite will resubmit.
    fn settle(&mut self, timeout: Duration) -> Result<(), HarnessError> {
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
        for c in got {
            self.carried.insert(c.id, c);
        }
        Ok(())
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
        if let Some(c) = self.carried.remove(&request.id) {
            self.ready.push(c);
            return Ok(());
        }
        let file = Arc::clone(&self.file);
        let device = self.device.clone();
        let tx = self.done_tx.clone();
        std::thread::spawn(move || {
            let Request { id, op } = request;
            let (result, buf) = perform(&file, device.as_deref(), op);
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
    /// Run the helper; the last line of its stdout.
    fn helper(&self, verb: &str, number: u64) -> Result<String, HarnessError> {
        let out = Command::new(&self.command)
            .arg(verb)
            .arg(number.to_string())
            .output()
            .map_err(|e| HarnessError(format!("{} {verb} {number}: {e}", self.command)))?;
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
        DeviceTarget::open(number, &path, self.depth)
    }

    fn kill(&mut self, mut target: DeviceTarget, _seed: u64) -> Result<DeviceTarget, HarnessError> {
        self.helper("kill", target.number)?;
        target.settle(self.timeout)?;
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
        self.helper("fail", target.number).map(|_| ())
    }

    fn reopen_failed(&mut self, target: DeviceTarget) -> Result<DeviceTarget, HarnessError> {
        let number = target.number;
        // The client lets go of the failed device first.
        drop(target);
        let path = self.helper("reopen", number)?;
        DeviceTarget::open(number, &path, self.depth)
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
    let mut depth = 16u32;
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
