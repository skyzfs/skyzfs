//! The command line of `skyzfsd`, and its exit statuses.
//!
//! ```text
//! skyzfsd create --dir DIR --size BYTES [--run-dir DIR]
//! skyzfsd serve  --dir DIR [--run-dir DIR] [--acknowledge-failure] [--kernel-has-async-delete]
//!                [--queues 1] [--depth N] [--log-high SEGMENTS] [--log-low SEGMENTS]
//!                [--writeback-kib KIB] [--home-writeback-kib KIB] [--readers N]
//!                [--store log|in-place]
//! skyzfsd detach --dir DIR [--run-dir DIR] [--kernel-has-async-delete]
//! skyzfsd status --dir DIR [--run-dir DIR]
//! skyzfsd help
//! ```
//!
//! `--queues` takes only 1: this milestone serves one queue.
//!
//! An option is `--name value` or `--name=value`, and is given at most
//! once. Numbers are decimal. Anything else is a usage error, which is
//! exit status 2 and changes nothing.
//!
//! # Options that are for tests
//!
//! Two overrides of the deployment rules (design §4.1), named so that
//! nobody uses them in production by accident:
//! `--unsafe-allow-unlocked-memory` and `--unsafe-allow-any-filesystem`,
//! on `serve`.
//!
//! And the fault hooks of the kernel rig, which exist only in a build
//! with the cargo feature `test-hooks`. In a default build each of them
//! is a usage error:
//!
//! - `create --base-pattern SEED`: a base that is not zeros;
//! - `serve --fail-log-after N`: the log fails once N requests that
//!   append to it are completed: writes, write-zeroes and discards.
//!   Reads and flushes do not count;
//! - `serve --hold-one-request SECONDS:read:OFFSET`,
//!   `SECONDS:write:OFFSET` or `SECONDS:flush`: the first request of
//!   that kind that covers that byte is held for so long;
//! - `serve --boot-id-file PATH`: read the boot id from this file and
//!   not from the kernel;
//! - `serve --recover-short N`: this instance's recovery counts the log
//!   as N records shorter than it found it;
//! - `serve --step-pause-ms N` or `serve --step-pause-until PATH`:
//!   every checkpoint step waits for N ms or until a file of that
//!   path exists (a step that arrives there first makes the empty
//!   file `PATH.reached`, and nothing here ever removes it or `PATH`:
//!   a caller that uses a path again removes both files first, or the
//!   next step does not wait and `PATH.reached` says nothing new). One
//!   of the two at most. `--step-pause-at POINT`
//!   says where, and needs one of them: `saved` (the blocks the step
//!   writes over are saved), `written` (the home image is written and
//!   not synced; this unless given), `synced` (the home image is
//!   durable and the log starts where it did), `start-written` (the
//!   log's new start is written and not durable) or `start-durable`;
//! - `serve --commit-delay-ms N`: every request's result is given to
//!   the kernel N ms after the store completed it, and the acknowledged
//!   position is stored then and not before. Nothing else waits;
//! - `serve --refuse-holes`: the store behaves as on a file system
//!   that cannot deallocate: a checkpoint writes zeros for a
//!   write-zeroes record.

use std::path::PathBuf;

use skyzfs_ublk::features::ASYNC_DELETE_OPTION;
use std::process::ExitCode;

/// Where the daemon keeps its notes and records unless told otherwise.
pub const DEFAULT_RUN_DIR: &str = "/run/skyzfsd";

/// The depth of a queue unless told otherwise: the contract's bound on
/// requests in flight.
pub const DEFAULT_DEPTH: u16 = 64;

/// The most a ublk queue takes (`UBLK_MAX_QUEUE_DEPTH`).
pub const MAX_DEPTH: u16 = 4096;

/// The most queues `serve` takes: one. M1 serves one queue; the option
/// is there so that it need not be added later, and any other number
/// is a usage error until more are served.
pub const MAX_QUEUES: u16 = 1;

/// How many threads execute reads unless told otherwise.
pub const DEFAULT_READERS: u16 = 4;

/// The most reader threads `serve` takes.
pub const MAX_READERS: u16 = 64;

/// The log's live size at which the daemon begins to checkpoint, in
/// segments, unless told otherwise (design §5.1: two).
pub const DEFAULT_LOG_HIGH: u32 = 2;

/// The live size below which it stops, in segments, unless told
/// otherwise: one.
pub const DEFAULT_LOG_LOW: u32 = 1;

/// The most segments either mark takes: 64 GiB of log.
pub const MAX_LOG_SEGMENTS: u32 = 1024;

/// One mebibyte: a volume's size is a multiple of it.
const MIB: u64 = 1 << 20;

/// `--writeback-kib` unless given: the store's default unit.
pub const DEFAULT_WRITEBACK_KIB: u64 = skyzfs_local::writeback::DEFAULT_UNIT >> 10;
/// The largest `--writeback-kib`: 1 GiB.
const MAX_WRITEBACK_KIB: u64 = 1 << 20;

/// What is printed with a usage error.
pub const USAGE: &str = "usage:
  skyzfsd create --dir DIR --size BYTES [--run-dir DIR]
  skyzfsd serve  --dir DIR [--run-dir DIR] [--acknowledge-failure] [--queues 1] [--depth N]
                 [--log-high SEGMENTS] [--log-low SEGMENTS] [--writeback-kib KIB]
                 [--home-writeback-kib KIB] [--readers N] [--kernel-has-async-delete]
                 [--store log|in-place]
                 (one queue is all this version serves)
  skyzfsd detach --dir DIR [--run-dir DIR] [--kernel-has-async-delete]
  skyzfsd status --dir DIR [--run-dir DIR]
  skyzfsd help";

/// What `help` prints: the usage, and what an operator has to know
/// before using a command.
pub const HELP: &str = "skyzfsd: one volume of the local store, served as a ublk device.

  It needs Linux 6.11 or later with the ublk driver: the driver's
  features UBLK_F_USER_RECOVERY, UBLK_F_USER_RECOVERY_REISSUE and
  UBLK_F_CMD_IOCTL_ENCODE, its command UBLK_U_CMD_GET_FEATURES (6.5),
  and its asynchronous delete UBLK_U_CMD_DEL_DEV_ASYNC, without which
  the daemon cannot give up a failed volume's device. The kernel's
  header names that command from 6.9, but its driver executes it from
  6.11 (the drivers of 6.9 and 6.10 answer it ENOTSUPP).
  serve, before it locks the volume, opens its store or adds or takes
  over a device, and detach, once it has the volume's lock and has
  found a record of a device of this boot and before it looks at or
  deletes that device, ask the driver for its features and read the
  kernel's release (uname -r). They refuse, naming what is missing, on
  a kernel that lacks a feature or whose release is below 6.11 or
  does not begin with a major and a minor number. A refused serve
  leaves a device of the volume that waits for a server as it is.
  The asynchronous delete has no feature bit. A kernel whose ublk
  driver executes it under an older release (a distribution kernel
  with the driver backported, or a 6.9.y or 6.10.y that carries the
  fix) is served when the operator says so with
  --kernel-has-async-delete, given to serve and to detach: the release
  is then not read, and the features are still checked. Say it only of
  a kernel whose driver is known to execute the command: nothing
  checks it before a delete needs it, and a delete it does not know
  fails and leaves the device.

  skyzfsd create --dir DIR --size BYTES [--run-dir DIR]
      Make a new, empty volume in DIR. DIR is made if it is missing
      (its parent must exist) and must hold no volume and no file but
      what an interrupted create leaves (lock, home.new, home.img,
      log.000000, log.super.new: files under exactly these names are
      removed, whoever put them there); a create that fails removes a
      DIR it made. BYTES is a multiple of 1 MiB. One
      line is printed: the size, the volume's id and the store's
      identity.
      If a volume in DIR failed in this boot, create warns: the note
      of that failure stays, and counts for the new volume.

  skyzfsd serve --dir DIR [--run-dir DIR] [--acknowledge-failure] [--queues 1] [--depth N]
                [--log-high SEGMENTS] [--log-low SEGMENTS] [--writeback-kib KIB]
                [--home-writeback-kib KIB] [--readers N] [--kernel-has-async-delete]
                [--store log|in-place]
      Serve the volume until SIGTERM. When the device serves, one line
      is printed: ready /dev/ublkbN.
      If another instance holds the volume, serve tries again for 2 s
      (20 times, 100 ms apart) and then exits with status 5.
      If the volume's own device is still shown as live by the kernel
      (the server before this one was killed a moment ago), serve asks
      again for up to 5 s before it gives up with status 5.
      A volume that failed in this boot (its log, or with --store
      in-place a write or sync of its image) is not served again
      (status 4). If no client held its device, --acknowledge-failure
      serves it: what was acknowledged to nobody may be gone. If a
      client held the device, the volume is not served again before a
      reboot, whatever is given.
      One queue is all this version serves.
      While it serves, the daemon checkpoints the volume's log into
      its home image: when the log holds --log-high segments (64 MiB
      each; 2 unless given) it works it off in small steps between
      the requests until it holds less than --log-low (1 unless
      given). The high mark bounds the log, and with it the time the
      next start takes to read it; a write load is slowed at the mark.
      Both are at least 1, and low is below high.
      The daemon writes the volume's files through the page cache and
      syncs them when it must. Once --writeback-kib KiB of the log's
      writes wait (1024 unless given), it asks the kernel to start
      writing them to the disk, so that the sync that follows has less
      left to wait for. It is a hint: nothing about what is durable, or
      when, depends on it. 0 never asks. A file system that has no such
      call is not asked again, and the status says so.
      --home-writeback-kib does the same for a step's writes into the
      home image (0, never, unless given): a thread of the daemon's
      syncs the home image while the requests are served, and the hint
      would cost the requests time that the sync does not need.
      Reads are executed by --readers threads of their own (4 unless
      given, 1 to 64), beside the thread that executes the other
      requests and beside each other.
      --store in-place serves the volume from its home image alone,
      with no log (log unless given): each write is written where it
      belongs, and a flush syncs the image. At a power cut it keeps
      what the block contract asks (a volatile-cache disk): what a
      completed flush covered is there, and any other block may hold
      its content as of the last flush, any version written since, or
      a mix of those within the block; a volume served so is not a
      prefix of its writes, as one served with the log is. A failure is
      final as with the log. The start applies and empties the log
      first, syncs the image, and drops the kernel's cache of it, so
      that it serves only what the disk holds: it refuses the volume
      where pages stay cached or the kernel cannot count them
      (cachestat, Linux 6.5), and the first reads after it are cold.
      A volume served in place must not be given back to --store log:
      a write acknowledged in place and not flushed before the daemon
      ended, however it ended, can be lost at a power cut after it,
      whatever the log store flushed. The log's options
      (--log-high, --log-low, --writeback-kib, --home-writeback-kib,
      --readers) are refused with it.

  skyzfsd detach --dir DIR [--run-dir DIR] [--kernel-has-async-delete]
      Delete the volume's device if no server serves it. It never stops
      a daemon: stop the daemon first (SIGTERM).
      Export the pool first. A pool whose device is deleted under it
      stays suspended, and the device keeps its number for as long as
      the pool holds it: detach then waits up to 5 s for the number to
      come free before it says so, with exit status 6.

  skyzfsd status --dir DIR [--run-dir DIR]
      Print the volume's state and the serving daemon's counters, one
      `key value` per line. It opens no store and asks no daemon.

Exit statuses: 0 done; 1 an error; 2 the command line is wrong;
3 the volume failed while it was served, or at the start of the
in-place store; 4 refused, the volume failed in this boot; 5 refused,
another instance or a live server holds the volume; 6 detach deleted
the device and a client still holds it open.
After a detach only 0 says that nothing of the volume is left attached
on this node.";

/// The exit statuses. The kernel rig depends on each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// 0: `serve` ended after SIGTERM, or another command did its work.
    Done,
    /// 1: anything not named below.
    Error,
    /// 2: the command line is wrong. Nothing was done.
    Usage,
    /// 3: the volume failed while this instance served it.
    Failed,
    /// 4: refused: the volume failed in this boot.
    RefusedFailed,
    /// 5: refused: another instance holds the volume.
    RefusedHeld,
    /// 6: `detach` deleted the volume's device and a client still holds
    /// it open: the device keeps its number until the client lets go.
    DeletedHeld,
}

impl Exit {
    /// The number.
    pub fn status(self) -> u8 {
        match self {
            Exit::Done => 0,
            Exit::Error => 1,
            Exit::Usage => 2,
            Exit::Failed => 3,
            Exit::RefusedFailed => 4,
            Exit::RefusedHeld => 5,
            Exit::DeletedHeld => 6,
        }
    }

    /// As the process's exit code.
    pub fn code(self) -> ExitCode {
        ExitCode::from(self.status())
    }
}

/// A command line that cannot be understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    /// No command, or one that does not exist.
    #[error("expected a command: create, serve, detach or status")]
    Command,
    /// An argument that is not an option of the command.
    #[error("{0}: not an option of this command")]
    Unknown(String),
    /// An option that exists only in a build with the test hooks.
    #[error("{0}: only in a build with the feature test-hooks")]
    TestOnly(String),
    /// An option given twice.
    #[error("{0}: given more than once")]
    Twice(String),
    /// An option without its value, or a flag with one.
    #[error("{0}: {1}")]
    Value(String, &'static str),
    /// A required option is missing.
    #[error("{0}: required")]
    Missing(&'static str),
}

/// `create`: make a new, empty volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create {
    /// The volume's directory.
    pub dir: PathBuf,
    /// The size in bytes: a multiple of 1 MiB, not zero.
    pub size: u64,
    /// The daemon's run directory: looked at for a failure note, and
    /// not written.
    pub run_dir: PathBuf,
    /// Test hook: a base that is not zeros, from this seed.
    pub base_pattern: Option<u64>,
}

/// `serve`: serve the volume until SIGTERM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serve {
    /// The volume's directory.
    pub dir: PathBuf,
    /// The daemon's run directory.
    pub run_dir: PathBuf,
    /// Serve a volume whose failure the operator acknowledges.
    pub acknowledge_failure: bool,
    /// `--kernel-has-async-delete`: the operator says that the kernel's
    /// ublk driver has `UBLK_U_CMD_DEL_DEV_ASYNC` whatever its release
    /// ([`skyzfs_ublk::features::ASYNC_DELETE_OPTION`]).
    pub async_delete_asserted: bool,
    /// Queues of a device this instance adds: 1, the only number this
    /// milestone serves ([`MAX_QUEUES`]). A device that is taken over
    /// keeps its own.
    pub queues: u16,
    /// Depth of each queue of a device this instance adds.
    pub depth: u16,
    /// The log's live size, in segments, at which the daemon begins
    /// to checkpoint (design §5.1).
    pub log_high: u32,
    /// The live size, in segments, below which it stops: below
    /// `log_high`.
    pub log_low: u32,
    /// Ask the kernel to start writing a file's writes to the disk once
    /// this many KiB of them wait; 0 never asks
    /// ([`skyzfs_local::writeback`]).
    pub writeback_kib: u64,
    /// The same for a step's writes into the home image; 0 unless
    /// given.
    pub home_writeback_kib: u64,
    /// How many threads execute reads: 1 is the daemon before there
    /// were several.
    pub readers: u16,
    /// `--store`: which store serves the volume.
    pub store: StoreKind,
    /// Run although the memory cannot be locked. For tests.
    pub allow_unlocked_memory: bool,
    /// Take a volume directory on any file system. For tests.
    pub allow_any_filesystem: bool,
    /// The fault hooks.
    pub hooks: Hooks,
}

/// `--store`: which store serves the volume. Both keep the same files:
/// the in-place store is entered from the log store's open with a
/// checkpoint of everything ([`skyzfs_local::inplace`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StoreKind {
    /// `log`, the default: the write log and its checkpoints (rules R1
    /// to R12).
    #[default]
    Log,
    /// `in-place`: the home image alone, with the volatile-cache disk's
    /// promise at a power cut.
    InPlace,
}

/// The fault hooks of `serve`: all off in a default build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hooks {
    /// The log fails once this many requests that append to it
    /// (writes, write-zeroes, discards) are completed.
    pub fail_log_after: Option<u64>,
    /// One request to hold.
    pub hold: Option<Hold>,
    /// Where to read the boot id from.
    pub boot_id_file: Option<PathBuf>,
    /// The recovery counts the log as so many records shorter.
    pub recover_short: u32,
    /// Every checkpoint step waits.
    pub step_pause: Option<StepPause>,
    /// Where it waits.
    pub step_pause_at: StepPoint,
    /// Every commit is delayed by so many milliseconds.
    pub commit_delay_ms: Option<u64>,
    /// The store writes zeros for a write-zeroes record, as on a file
    /// system that cannot deallocate.
    pub refuse_holes: bool,
}

/// How long a checkpoint step waits in its middle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepPause {
    /// For so many milliseconds.
    Ms(u64),
    /// Until a file of this path exists.
    Until(PathBuf),
}

/// Where a checkpoint step waits: the cuts between its parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepPoint {
    /// The blocks it writes over are saved.
    Saved,
    /// The home image is written and not synced.
    #[default]
    Written,
    /// The home image is durable; the log starts where it did.
    Synced,
    /// The log's new start is written and not durable.
    StartWritten,
    /// The log's new start is durable.
    StartDurable,
}

impl StepPoint {
    fn parse(text: &str) -> Result<Self, UsageError> {
        match text {
            "saved" => Ok(Self::Saved),
            "written" => Ok(Self::Written),
            "synced" => Ok(Self::Synced),
            "start-written" => Ok(Self::StartWritten),
            "start-durable" => Ok(Self::StartDurable),
            _ => Err(UsageError::Value(
                "--step-pause-at".into(),
                "one of saved, written, synced, start-written, start-durable",
            )),
        }
    }
}

/// One request to hold for a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hold {
    /// For how long.
    pub seconds: u64,
    /// Which request.
    pub what: Held,
}

/// The request a [`Hold`] waits for: the first of its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// A read that covers this byte.
    Read(u64),
    /// A write that covers this byte.
    Write(u64),
    /// A flush.
    Flush,
}

/// `detach` and `status`: which volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// The volume's directory.
    pub dir: PathBuf,
    /// The daemon's run directory.
    pub run_dir: PathBuf,
}

/// `detach`: which volume, and what the operator says of the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detach {
    /// The volume.
    pub volume: Volume,
    /// `--kernel-has-async-delete`, as for [`Serve`].
    pub async_delete_asserted: bool,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `create`.
    Create(Create),
    /// `serve`.
    Serve(Serve),
    /// `detach`.
    Detach(Detach),
    /// `status`.
    Status(Volume),
    /// `help`, `--help` or `-h`: print [`HELP`].
    Help,
}

impl Command {
    /// The command's name.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Create(_) => "create",
            Command::Serve(_) => "serve",
            Command::Detach(_) => "detach",
            Command::Status(_) => "status",
            Command::Help => "help",
        }
    }
}

/// Parse the arguments behind the program's name. The test hooks are
/// taken only in a build with the feature `test-hooks`.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, UsageError> {
    parse_with(args, cfg!(feature = "test-hooks"))
}

/// An option of a command: its name, whether it takes a value, and
/// whether it is a test hook.
struct Opt {
    name: &'static str,
    value: bool,
    hook: bool,
}

const fn opt(name: &'static str, value: bool, hook: bool) -> Opt {
    Opt { name, value, hook }
}

const CREATE: &[Opt] = &[
    opt("--dir", true, false),
    opt("--size", true, false),
    opt("--run-dir", true, false),
    opt("--base-pattern", true, true),
];

const SERVE: &[Opt] = &[
    opt("--dir", true, false),
    opt("--run-dir", true, false),
    opt("--acknowledge-failure", false, false),
    opt(ASYNC_DELETE_OPTION, false, false),
    opt("--queues", true, false),
    opt("--depth", true, false),
    opt("--log-high", true, false),
    opt("--log-low", true, false),
    opt("--writeback-kib", true, false),
    opt("--home-writeback-kib", true, false),
    opt("--readers", true, false),
    opt("--store", true, false),
    opt("--unsafe-allow-unlocked-memory", false, false),
    opt("--unsafe-allow-any-filesystem", false, false),
    opt("--fail-log-after", true, true),
    opt("--hold-one-request", true, true),
    opt("--boot-id-file", true, true),
    opt("--recover-short", true, true),
    opt("--commit-delay-ms", true, true),
    opt("--step-pause-ms", true, true),
    opt("--step-pause-until", true, true),
    opt("--step-pause-at", true, true),
    opt("--refuse-holes", false, true),
];

const VOLUME: &[Opt] = &[opt("--dir", true, false), opt("--run-dir", true, false)];

const DETACH: &[Opt] = &[
    opt("--dir", true, false),
    opt("--run-dir", true, false),
    opt(ASYNC_DELETE_OPTION, false, false),
];

/// The options given, by name. A flag's value is empty.
struct Given(Vec<(&'static str, String)>);

impl Given {
    fn collect(
        known: &[Opt],
        args: impl Iterator<Item = String>,
        hooks: bool,
    ) -> Result<Self, UsageError> {
        let mut given: Vec<(&'static str, String)> = Vec::new();
        let mut args = args;
        while let Some(arg) = args.next() {
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (arg.as_str(), None),
            };
            let Some(opt) = known.iter().find(|o| o.name == name) else {
                return Err(UsageError::Unknown(arg));
            };
            if opt.hook && !hooks {
                return Err(UsageError::TestOnly(opt.name.into()));
            }
            if given.iter().any(|(n, _)| *n == opt.name) {
                return Err(UsageError::Twice(opt.name.into()));
            }
            let value = match (opt.value, inline) {
                (true, Some(value)) => value,
                (true, None) => args
                    .next()
                    .ok_or(UsageError::Value(opt.name.into(), "needs a value"))?,
                (false, None) => String::new(),
                (false, Some(_)) => {
                    return Err(UsageError::Value(opt.name.into(), "takes no value"))
                }
            };
            if opt.value && value.is_empty() {
                return Err(UsageError::Value(opt.name.into(), "needs a value"));
            }
            given.push((opt.name, value));
        }
        Ok(Self(given))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, value)| value.as_str())
    }

    fn flag(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    fn path(&self, name: &'static str) -> Result<PathBuf, UsageError> {
        self.get(name)
            .map(PathBuf::from)
            .ok_or(UsageError::Missing(name))
    }

    fn number(&self, name: &'static str) -> Result<Option<u64>, UsageError> {
        self.get(name).map(|text| number(name, text)).transpose()
    }
}

/// A decimal number: digits only, no sign, no unit.
fn number(name: &'static str, text: &str) -> Result<u64, UsageError> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    digits
        .then(|| text.parse().ok())
        .flatten()
        .ok_or(UsageError::Value(name.into(), "not a decimal number"))
}

fn hold(text: &str) -> Result<Hold, UsageError> {
    let name = "--hold-one-request";
    let bad = UsageError::Value(
        name.into(),
        "expected SECONDS:read:OFFSET, SECONDS:write:OFFSET or SECONDS:flush",
    );
    let mut parts = text.split(':');
    let seconds = parts.next().ok_or(bad.clone())?;
    let seconds = number(name, seconds).map_err(|_| bad.clone())?;
    let kind = parts.next().ok_or(bad.clone())?;
    let offset = parts.next().map(|o| number(name, o)).transpose();
    let offset = offset.map_err(|_| bad.clone())?;
    if parts.next().is_some() {
        return Err(bad);
    }
    let what = match (kind, offset) {
        ("read", Some(offset)) => Held::Read(offset),
        ("write", Some(offset)) => Held::Write(offset),
        ("flush", None) => Held::Flush,
        _ => return Err(bad),
    };
    Ok(Hold { seconds, what })
}

fn run_dir(given: &Given) -> PathBuf {
    given
        .get("--run-dir")
        .map_or_else(|| PathBuf::from(DEFAULT_RUN_DIR), PathBuf::from)
}

fn volume(given: &Given) -> Result<Volume, UsageError> {
    Ok(Volume {
        dir: given.path("--dir")?,
        run_dir: run_dir(given),
    })
}

fn parse_with(args: impl IntoIterator<Item = String>, hooks: bool) -> Result<Command, UsageError> {
    let mut args = args.into_iter();
    let command = args.next().ok_or(UsageError::Command)?;
    match command.as_str() {
        "help" | "--help" | "-h" => match args.next() {
            None => Ok(Command::Help),
            Some(more) => Err(UsageError::Unknown(more)),
        },
        "create" => {
            let given = Given::collect(CREATE, args, hooks)?;
            let dir = given.path("--dir")?;
            let size = given
                .number("--size")?
                .ok_or(UsageError::Missing("--size"))?;
            if size == 0 || size % MIB != 0 {
                return Err(UsageError::Value(
                    "--size".into(),
                    "not a multiple of 1 MiB above zero",
                ));
            }
            Ok(Command::Create(Create {
                dir,
                size,
                run_dir: run_dir(&given),
                base_pattern: given.number("--base-pattern")?,
            }))
        }
        "serve" => {
            let given = Given::collect(SERVE, args, hooks)?;
            let Volume { dir, run_dir } = volume(&given)?;
            let small = |name: &'static str, default: u16, most: u16| {
                let Some(n) = given.number(name)? else {
                    return Ok(default);
                };
                u16::try_from(n)
                    .ok()
                    .filter(|n| (1..=most).contains(n))
                    .ok_or(UsageError::Value(name.into(), "out of range"))
            };
            // A writeback unit in KiB: at most 1 GiB, 0 for never.
            let unit = |name: &'static str, default: u64| match given.number(name)? {
                None => Ok(default),
                Some(kib) if kib <= MAX_WRITEBACK_KIB => Ok(kib),
                Some(_) => Err(UsageError::Value(
                    name.into(),
                    "out of range: at most 1048576 (1 GiB)",
                )),
            };
            let recover_short = given.number("--recover-short")?.unwrap_or(0);
            let recover_short = u32::try_from(recover_short)
                .map_err(|_| UsageError::Value("--recover-short".into(), "out of range"))?;
            let segments = |name: &'static str, default: u32| {
                let Some(n) = given.number(name)? else {
                    return Ok(default);
                };
                u32::try_from(n)
                    .ok()
                    .filter(|n| (1..=MAX_LOG_SEGMENTS).contains(n))
                    .ok_or(UsageError::Value(name.into(), "out of range"))
            };
            let log_high = segments("--log-high", DEFAULT_LOG_HIGH)?;
            let log_low = segments("--log-low", DEFAULT_LOG_LOW.min(log_high.max(2) - 1))?;
            // The refusal names an option that was given, and the
            // default it collided with if the other was not.
            if log_high < 2 {
                return Err(UsageError::Value(
                    "--log-high".into(),
                    "at least 2: the low mark is at least 1 and below it",
                ));
            }
            if log_low >= log_high {
                let why = if given.get("--log-high").is_some() {
                    "not below --log-high"
                } else {
                    "not below --log-high, which is 2 unless given"
                };
                return Err(UsageError::Value("--log-low".into(), why));
            }
            let step_pause = match (
                given.number("--step-pause-ms")?,
                given.get("--step-pause-until"),
            ) {
                (Some(_), Some(_)) => {
                    return Err(UsageError::Value(
                        "--step-pause-until".into(),
                        "not together with --step-pause-ms",
                    ))
                }
                (Some(ms), None) => Some(StepPause::Ms(ms)),
                (None, Some(path)) => Some(StepPause::Until(PathBuf::from(path))),
                (None, None) => None,
            };
            let step_pause_at = match given.get("--step-pause-at") {
                Some(_) if step_pause.is_none() => {
                    return Err(UsageError::Value(
                        "--step-pause-at".into(),
                        "only with --step-pause-ms or --step-pause-until",
                    ))
                }
                Some(text) => StepPoint::parse(text)?,
                None => StepPoint::default(),
            };
            let store = match given.get("--store") {
                None | Some("log") => StoreKind::Log,
                Some("in-place") => StoreKind::InPlace,
                Some(_) => {
                    return Err(UsageError::Value(
                        "--store".into(),
                        "expected log or in-place",
                    ))
                }
            };
            // The options of the log and its checkpoints, which the
            // in-place store does not have: refused rather than ignored.
            if store == StoreKind::InPlace {
                let refused = [
                    "--log-high",
                    "--log-low",
                    "--writeback-kib",
                    "--home-writeback-kib",
                    "--readers",
                    "--step-pause-ms",
                    "--step-pause-until",
                    "--step-pause-at",
                    "--refuse-holes",
                ];
                if let Some(name) = refused.into_iter().find(|name| given.flag(name)) {
                    return Err(UsageError::Value(name.into(), "not with --store in-place"));
                }
            }
            Ok(Command::Serve(Serve {
                dir,
                run_dir,
                acknowledge_failure: given.flag("--acknowledge-failure"),
                async_delete_asserted: given.flag(ASYNC_DELETE_OPTION),
                queues: small("--queues", 1, MAX_QUEUES)?,
                depth: small("--depth", DEFAULT_DEPTH, MAX_DEPTH)?,
                log_high,
                log_low,
                writeback_kib: unit("--writeback-kib", DEFAULT_WRITEBACK_KIB)?,
                home_writeback_kib: unit("--home-writeback-kib", 0)?,
                readers: small("--readers", DEFAULT_READERS, MAX_READERS)?,
                store,
                allow_unlocked_memory: given.flag("--unsafe-allow-unlocked-memory"),
                allow_any_filesystem: given.flag("--unsafe-allow-any-filesystem"),
                hooks: Hooks {
                    fail_log_after: given.number("--fail-log-after")?,
                    hold: given.get("--hold-one-request").map(hold).transpose()?,
                    boot_id_file: given.get("--boot-id-file").map(PathBuf::from),
                    recover_short,
                    step_pause,
                    step_pause_at,
                    commit_delay_ms: given.number("--commit-delay-ms")?,
                    refuse_holes: given.flag("--refuse-holes"),
                },
            }))
        }
        "detach" => {
            let given = Given::collect(DETACH, args, hooks)?;
            Ok(Command::Detach(Detach {
                volume: volume(&given)?,
                async_delete_asserted: given.flag(ASYNC_DELETE_OPTION),
            }))
        }
        "status" => Ok(Command::Status(volume(&Given::collect(
            VOLUME, args, hooks,
        )?)?)),
        _ => Err(UsageError::Command),
    }
}

#[cfg(test)]
mod tests;
