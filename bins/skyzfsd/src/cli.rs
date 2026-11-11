//! The command line of `skyzfsd`, and its exit statuses.
//!
//! ```text
//! skyzfsd create --dir DIR --size BYTES [--run-dir DIR]
//! skyzfsd serve  --dir DIR [--run-dir DIR] [--acknowledge-failure]
//!                [--queues 1] [--depth N]
//! skyzfsd detach --dir DIR [--run-dir DIR]
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
//! - `serve --fail-log-after N`: the log fails after N more requests;
//! - `serve --hold-one-request SECONDS:read:OFFSET`,
//!   `SECONDS:write:OFFSET` or `SECONDS:flush`: the first request of
//!   that kind that covers that byte is held for so long;
//! - `serve --boot-id-file PATH`: read the boot id from this file and
//!   not from the kernel;
//! - `serve --recover-short N`: this instance's recovery counts the log
//!   as N records shorter than it found it.

use std::path::PathBuf;
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

/// One mebibyte: a volume's size is a multiple of it.
const MIB: u64 = 1 << 20;

/// What is printed with a usage error.
pub const USAGE: &str = "usage:
  skyzfsd create --dir DIR --size BYTES [--run-dir DIR]
  skyzfsd serve  --dir DIR [--run-dir DIR] [--acknowledge-failure] [--queues 1] [--depth N]
                 (one queue is all this version serves)
  skyzfsd detach --dir DIR [--run-dir DIR]
  skyzfsd status --dir DIR [--run-dir DIR]
  skyzfsd help";

/// What `help` prints: the usage, and what an operator has to know
/// before using a command.
pub const HELP: &str = "skyzfsd: one volume of the local store, served as a ublk device.

  skyzfsd create --dir DIR --size BYTES [--run-dir DIR]
      Make a new, empty volume in DIR, which must exist and hold no
      volume. BYTES is a multiple of 1 MiB. One line is printed: the
      size, the volume's id and the store's identity.
      If a volume in DIR failed in this boot, create warns: the note
      of that failure stays, and counts for the new volume.

  skyzfsd serve --dir DIR [--run-dir DIR] [--acknowledge-failure] [--queues 1] [--depth N]
      Serve the volume until SIGTERM. When the device serves, one line
      is printed: ready /dev/ublkbN.
      If another instance holds the volume, serve tries again for 2 s
      (20 times, 100 ms apart) and then exits with status 5.
      If the volume's own device is still shown as live by the kernel
      (the server before this one was killed a moment ago), serve asks
      again for up to 5 s before it gives up with status 5.
      A volume whose log failed in this boot is not served again
      (status 4). If no client held its device, --acknowledge-failure
      serves it: what was acknowledged to nobody may be gone. If a
      client held the device, the volume is not served again before a
      reboot, whatever is given.
      One queue is all this version serves.

  skyzfsd detach --dir DIR [--run-dir DIR]
      Delete the volume's device if no server serves it. It never stops
      a daemon: stop the daemon first (SIGTERM).
      Export the pool first. A pool whose device is deleted under it
      stays suspended, and the device keeps its number for as long as
      the pool holds it.

  skyzfsd status --dir DIR [--run-dir DIR]
      Print the volume's state and the serving daemon's counters, one
      `key value` per line. It opens no store and asks no daemon.

Exit statuses: 0 done; 1 an error; 2 the command line is wrong;
3 the volume failed while it was served; 4 refused, the volume failed
in this boot; 5 refused, another instance or a live server holds the
volume.";

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
    /// Queues of a device this instance adds: 1, the only number this
    /// milestone serves ([`MAX_QUEUES`]). A device that is taken over
    /// keeps its own.
    pub queues: u16,
    /// Depth of each queue of a device this instance adds.
    pub depth: u16,
    /// Run although the memory cannot be locked. For tests.
    pub allow_unlocked_memory: bool,
    /// Take a volume directory on any file system. For tests.
    pub allow_any_filesystem: bool,
    /// The fault hooks.
    pub hooks: Hooks,
}

/// The fault hooks of `serve`: all off in a default build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hooks {
    /// The log fails after this many more requests.
    pub fail_log_after: Option<u64>,
    /// One request to hold.
    pub hold: Option<Hold>,
    /// Where to read the boot id from.
    pub boot_id_file: Option<PathBuf>,
    /// The recovery counts the log as so many records shorter.
    pub recover_short: u32,
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

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `create`.
    Create(Create),
    /// `serve`.
    Serve(Serve),
    /// `detach`.
    Detach(Volume),
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
    opt("--queues", true, false),
    opt("--depth", true, false),
    opt("--unsafe-allow-unlocked-memory", false, false),
    opt("--unsafe-allow-any-filesystem", false, false),
    opt("--fail-log-after", true, true),
    opt("--hold-one-request", true, true),
    opt("--boot-id-file", true, true),
    opt("--recover-short", true, true),
];

const VOLUME: &[Opt] = &[opt("--dir", true, false), opt("--run-dir", true, false)];

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
            let recover_short = given.number("--recover-short")?.unwrap_or(0);
            let recover_short = u32::try_from(recover_short)
                .map_err(|_| UsageError::Value("--recover-short".into(), "out of range"))?;
            Ok(Command::Serve(Serve {
                dir,
                run_dir,
                acknowledge_failure: given.flag("--acknowledge-failure"),
                queues: small("--queues", 1, MAX_QUEUES)?,
                depth: small("--depth", DEFAULT_DEPTH, MAX_DEPTH)?,
                allow_unlocked_memory: given.flag("--unsafe-allow-unlocked-memory"),
                allow_any_filesystem: given.flag("--unsafe-allow-any-filesystem"),
                hooks: Hooks {
                    fail_log_after: given.number("--fail-log-after")?,
                    hold: given.get("--hold-one-request").map(hold).transpose()?,
                    boot_id_file: given.get("--boot-id-file").map(PathBuf::from),
                    recover_short,
                },
            }))
        }
        "detach" => Ok(Command::Detach(volume(&Given::collect(
            VOLUME, args, hooks,
        )?)?)),
        "status" => Ok(Command::Status(volume(&Given::collect(
            VOLUME, args, hooks,
        )?)?)),
        _ => Err(UsageError::Command),
    }
}

#[cfg(test)]
mod tests;
