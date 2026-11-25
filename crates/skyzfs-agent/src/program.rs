//! The program `skyzfs-agent` (M2.6c-3): its command line, what it prints,
//! and its exit codes. The binary (`bins/skyzfs-agent`) builds the
//! adapters ([`crate::zfs::cli`], [`crate::daemon`], [`crate::record`])
//! and calls [`execute`]; everything it decides is here, so that it is
//! tested on the engine's fakes.
//!
//! # Commands
//!
//! ```text
//! skyzfs-agent publish --volume VOL --run RUN --device DEV --pool POOL --dir DIR
//!                      [--limit-s S] [--expiry-s E]
//! skyzfs-agent settle  --volume VOL --run RUN --device DEV --pool POOL --dir DIR
//!                      [--limit-s S]
//! skyzfs-agent recover --volume VOL --run RUN --device DEV --pool POOL --dir DIR
//!                      --name T (--give-up | --retry) [--limit-s S]
//! skyzfs-agent record  --dir DIR
//! ```
//!
//! - `VOL` is the volume's directory, as `skyzfsd create` made it: the
//!   agent reads the volume's id from its file `remote` ([`volume_id`]).
//! - `RUN` is the daemon's directory of its run: its channel is the
//!   socket `RUN/control` ([`skyzfs_control::SOCKET`]).
//! - `DEV` is the volume's block device, as `skyzfsd serve` prints it
//!   (`ready /dev/ublkbN`).
//! - `DIR` is the agent's own directory of the volume: its record and
//!   its manifests ([`crate::record`]). One agent at a time holds it.
//!
//! `publish` begins a publish, then settles every publish under way
//! ([`crate::engine::Agent::settle`]) for at most `--limit-s` seconds
//! ([`LIMIT_DEFAULT`]). `--expiry-s` is how long the daemon may keep the
//! request pending (the protocol's default where it is not given, at
//! most [`skyzfs_control::EXPIRY_MAX`]). `settle` settles every publish
//! under way and begins none; it is how a rig, or a person, finishes
//! what a stopped agent left. `recover` records a person's choice for one
//! publish under way, then settles: `--give-up` gives it up as
//! [`crate::record::Why::Person`] (its request withdrawn first, a
//! publish already made kept), and `--retry` clears a stop for a person
//! ([`crate::record::Person::Stopped`]) so that it is planned again.
//! `record` prints the record ([`dump`]) without its lock, and changes
//! nothing. None of them runs as a daemon: each ends.
//!
//! # What it prints
//!
//! On standard output, one line for each publish that left the record
//! or stopped in this run, in the order they did ([`outcome_line`]):
//!
//! ```text
//! snap/<t> <commit, 64 hex digits>      published
//! abandoned <t> <why>                   given up (the words of [`why_word`])
//! stopped <t> <why, in words>           stopped for a person; still under way
//! unsettled <t>                         still under way at the time limit
//! ```
//!
//! On standard error, each report as it is made, before the agent acts
//! on it ([`report_line`]), and each error.
//!
//! # Exit codes
//!
//! `publish` and `recover` exit by the outcome of their own publish;
//! `settle` by the worst of all ([`worst`]): see [`exit`]. An error of
//! the engine exits by [`exit_of_error`], whatever was settled before
//! it: a `publish` whose own publish was made, and whose run then
//! failed on another, prints its `snap/` line and exits 1. 5 comes only
//! from `publish`, where the pool's premise refuses the publish it
//! begins; a publish under way whose premise is refused later is given
//! up (`abandoned <t> premise`, 3), under any command.
//!
//! `record` of a directory with no record prints `absent` and exits 0;
//! of a path that is not a directory it exits 1.
//!
//! # Test hooks
//!
//! Options for the rigs, which a build without the feature `test-hooks`
//! of the binary refuses as a usage error ([`Hooks`]):
//!
//! - `--kill-at EFFECT:before|after[:N]`: at the `N`th reach (from 1;
//!   1 where it is not given) of that point of the engine
//!   ([`crate::engine::Point`]; the effects' words are
//!   [`crate::engine::Effect::word`]), the line of [`kill_line`] on
//!   standard error, then `SIGKILL` to the agent itself: no unwinding,
//!   nothing flushed after it.
//! - `--trace-points FILE`: each point reached appended to `FILE` as a
//!   line ([`trace_line`]), written before the effect and before a kill
//!   at the same point.
//! - `--assist-after-s S`: the time before a sync helps a pending
//!   request ([`crate::plan::ASSIST_AFTER`] otherwise).
//! - `--age-max-s S`: the bound on a publish's age
//!   ([`crate::plan::AGE_MAX`] otherwise).
//!
//! # What is this module's reading
//!
//! The design names no command line, exit codes or output; these are
//! this module's, made for the rigs' needs (testing's list for M2.6c,
//! approved by the lead). `--assist-after-s` and `--age-max-s` are test
//! hooks, not options of a production build: the plan fixes both values.
//! The volume's id is read from `remote`'s line `id` and not checked
//! against the volume's name: the daemon checks that at each `serve`.

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use skyzfs_control::{hex, EXPIRY_MAX};

use crate::engine::{Agent, Choice, Clock, Daemon, Effect, Error, Outcome, Point, Points};
use crate::plan::Report;
use crate::record::{Keep, Loaded, Person, Phase, Why};
use crate::zfs::Zfs;

/// How long `publish`, `settle` and `recover` settle where `--limit-s`
/// is not given: three hours, past a publish's age bound
/// ([`crate::plan::AGE_MAX`], two hours) plus the longest expiry and its
/// margin, so that any publish whose daemon answers settles within it.
pub const LIMIT_DEFAULT: Duration = Duration::from_secs(3 * 60 * 60);

/// The program's exit codes.
pub mod exit {
    /// Published (`publish`, `recover`), or every publish published or
    /// none under way (`settle`).
    pub const PUBLISHED: i32 = 0;
    /// Any other error: ZFS, the daemon at `begin`, the file system, the
    /// record not saved, [`crate::record::UNDER_WAY_MAX`] publishes
    /// under way, a name `recover` does not find under way.
    pub const ERROR: i32 = 1;
    /// The command line is not one the program takes.
    pub const USAGE: i32 = 2;
    /// Given up.
    pub const ABANDONED: i32 = 3;
    /// Stopped for a person: still under way, its hold kept.
    pub const STOPPED: i32 = 4;
    /// The pool is refused a publish (its premise): nothing was taken.
    pub const PREMISE: i32 = 5;
    /// The record is corrupt, or another volume's, or the sweep would
    /// release holds from a directory with no agent's marker: nothing was
    /// done, and a person judges.
    pub const RECORD: i32 = 6;
    /// Still under way when the time limit passed.
    pub const UNSETTLED: i32 = 7;
    /// Another agent holds the record's directory.
    pub const HELD: i32 = 8;
}

/// What the program is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Begin a publish, then settle.
    Publish(Run),
    /// Settle.
    Settle(Run),
    /// Record a person's choice for the publish `name`, then settle.
    Recover {
        /// The volume, the daemon and the pool.
        run: Run,
        /// The publish's name.
        name: String,
        /// The choice.
        choice: Choice,
    },
    /// Print the record in `dir`.
    Record {
        /// The agent's directory of the volume.
        dir: PathBuf,
    },
    /// Print the usage.
    Help,
}

/// What `publish`, `settle` and `recover` run on ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The volume's directory.
    pub volume: PathBuf,
    /// The daemon's directory of its run.
    pub run: PathBuf,
    /// The volume's block device.
    pub device: PathBuf,
    /// The pool.
    pub pool: String,
    /// The agent's directory of the volume.
    pub dir: PathBuf,
    /// How long to settle.
    pub limit: Duration,
    /// The request's expiry (`publish`); `None` for the protocol's
    /// default.
    pub expiry: Option<Duration>,
    /// The test hooks.
    pub hooks: Hooks,
}

/// The test hooks ([module docs](self), "Test hooks").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hooks {
    /// `--kill-at`.
    pub kill_at: Option<KillAt>,
    /// `--trace-points`.
    pub trace_points: Option<PathBuf>,
    /// `--assist-after-s`.
    pub assist_after: Option<Duration>,
    /// `--age-max-s`.
    pub age_max: Option<Duration>,
}

/// `--kill-at EFFECT:before|after[:N]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillAt {
    /// The point.
    pub point: Point,
    /// Its reach, from 1, at which the agent dies.
    pub nth: u64,
}

/// A command line that is not one the program takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage(pub String);

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The usage, printed with `--help` and after a usage error.
pub const USAGE: &str = "\
usage:
  skyzfs-agent publish --volume VOL --run RUN --device DEV --pool POOL --dir DIR
                       [--limit-s S] [--expiry-s E]
  skyzfs-agent settle  --volume VOL --run RUN --device DEV --pool POOL --dir DIR
                       [--limit-s S]
  skyzfs-agent recover --volume VOL --run RUN --device DEV --pool POOL --dir DIR
                       --name T (--give-up | --retry) [--limit-s S]
  skyzfs-agent record  --dir DIR
test hooks (a build with the feature test-hooks):
  --kill-at EFFECT:before|after[:N]  --trace-points FILE
  --assist-after-s S  --age-max-s S
exit: 0 published, 1 error, 2 usage, 3 abandoned, 4 stopped for a person,
      5 premise refused (publish), 6 record corrupt or another volume's,
      or no agent's marker where holds would be released,
      7 unsettled at the limit, 8 another agent holds the record";

/// Parse the arguments after the program's name. `hooks` says whether
/// the build takes the test hooks.
pub fn parse(args: &[String], hooks: bool) -> Result<Command, Usage> {
    let usage = |why: String| Err(Usage(why));
    let Some((verb, rest)) = args.split_first() else {
        return usage("no command".into());
    };
    if matches!(verb.as_str(), "--help" | "-h" | "help") {
        return Ok(Command::Help);
    }
    let mut options: Vec<(String, Option<String>)> = Vec::new();
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        if matches!(word.as_str(), "--help" | "-h") {
            return Ok(Command::Help);
        }
        let Some(key) = word.strip_prefix("--") else {
            return usage(format!("{word:?} is not an option"));
        };
        let (key, value) = match key.split_once('=') {
            Some((key, value)) => (key.to_string(), Some(value.to_string())),
            None if FLAGS.contains(&key) => (key.to_string(), None),
            None => match words.next() {
                // An option's value is not the next option: `--name
                // --give-up` is a name forgotten, not a name (S3 of
                // #149's review). A value that begins with `--` is given
                // as `--key=--value`. Where a value is looked for,
                // `--help` asks for help, but `-h` is taken as the value,
                // as any word with one dash is (nit 3 of the next look at
                // #149).
                Some(value) if value == "--help" => return Ok(Command::Help),
                Some(value) if value.starts_with("--") => {
                    return usage(format!(
                        "--{key} needs a value, not the option {value}; \
                         give such a value as --{key}={value}"
                    ))
                }
                Some(value) => (key.to_string(), Some(value.clone())),
                None => return usage(format!("--{key} needs a value")),
            },
        };
        if options.iter().any(|(k, _)| *k == key) {
            return usage(format!("--{key} is given twice"));
        }
        options.push((key, value));
    }
    let mut take = Options { options, hooks };
    let command = match verb.as_str() {
        "publish" => {
            let mut run = take.run()?;
            run.expiry = take.seconds("expiry-s")?;
            if let Some(expiry) = run.expiry {
                if expiry > EXPIRY_MAX {
                    return usage(format!("--expiry-s: at most {} s", EXPIRY_MAX.as_secs()));
                }
            }
            Command::Publish(run)
        }
        "settle" => Command::Settle(take.run()?),
        "recover" => {
            let run = take.run()?;
            let name = take.required("name")?;
            let choice = match (take.flag("give-up")?, take.flag("retry")?) {
                (true, false) => Choice::GiveUp,
                (false, true) => Choice::Retry,
                _ => return usage("recover takes one of --give-up and --retry".into()),
            };
            Command::Recover { run, name, choice }
        }
        "record" => Command::Record {
            dir: take.required("dir")?.into(),
        },
        other => return usage(format!("no command {other:?}")),
    };
    match take.options.first() {
        Some((key, _)) => usage(format!("{verb} takes no option --{key}")),
        None => Ok(command),
    }
}

/// The options that take no value.
const FLAGS: [&str; 2] = ["give-up", "retry"];

/// The options not yet taken.
struct Options {
    options: Vec<(String, Option<String>)>,
    hooks: bool,
}

impl Options {
    fn take(&mut self, key: &str) -> Option<Option<String>> {
        let at = self.options.iter().position(|(k, _)| k == key)?;
        Some(self.options.remove(at).1)
    }

    fn optional(&mut self, key: &str) -> Result<Option<String>, Usage> {
        match self.take(key) {
            None => Ok(None),
            Some(Some(value)) if !value.is_empty() => Ok(Some(value)),
            Some(_) => Err(Usage(format!("--{key} needs a value"))),
        }
    }

    fn required(&mut self, key: &str) -> Result<String, Usage> {
        self.optional(key)?
            .ok_or_else(|| Usage(format!("--{key} is needed")))
    }

    fn flag(&mut self, key: &str) -> Result<bool, Usage> {
        match self.take(key) {
            None => Ok(false),
            Some(None) => Ok(true),
            Some(Some(_)) => Err(Usage(format!("--{key} takes no value"))),
        }
    }

    /// A whole number of seconds, at least 1.
    fn seconds(&mut self, key: &str) -> Result<Option<Duration>, Usage> {
        let Some(value) = self.optional(key)? else {
            return Ok(None);
        };
        match value.parse::<u64>() {
            Ok(s) if s >= 1 => Ok(Some(Duration::from_secs(s))),
            _ => Err(Usage(format!(
                "--{key}: {value:?} is not a whole number of seconds, at least 1"
            ))),
        }
    }

    /// Refuse the test hook `key` in a build without them.
    fn hook_allowed(&self, key: &str) -> Result<(), Usage> {
        if !self.hooks && self.options.iter().any(|(k, _)| k == key) {
            return Err(Usage(format!(
                "--{key} is a test hook: this build has no feature test-hooks"
            )));
        }
        Ok(())
    }

    fn hook(&mut self, key: &str) -> Result<Option<String>, Usage> {
        self.hook_allowed(key)?;
        self.optional(key)
    }

    fn hook_seconds(&mut self, key: &str) -> Result<Option<Duration>, Usage> {
        self.hook_allowed(key)?;
        self.seconds(key)
    }

    fn run(&mut self) -> Result<Run, Usage> {
        let hooks = Hooks {
            kill_at: self.hook("kill-at")?.as_deref().map(kill_at).transpose()?,
            trace_points: self.hook("trace-points")?.map(PathBuf::from),
            assist_after: self.hook_seconds("assist-after-s")?,
            age_max: self.hook_seconds("age-max-s")?,
        };
        Ok(Run {
            volume: self.required("volume")?.into(),
            run: self.required("run")?.into(),
            device: self.required("device")?.into(),
            pool: self.required("pool")?,
            dir: self.required("dir")?.into(),
            limit: self.seconds("limit-s")?.unwrap_or(LIMIT_DEFAULT),
            expiry: None,
            hooks,
        })
    }
}

/// `EFFECT:before|after[:N]`.
pub fn kill_at(text: &str) -> Result<KillAt, Usage> {
    let bad = |why: &str| Usage(format!("--kill-at {text:?}: {why}"));
    let mut parts = text.split(':');
    let effect = parts
        .next()
        .and_then(Effect::from_word)
        .ok_or_else(|| bad("no such effect"))?;
    let after = match parts.next() {
        Some("before") => false,
        Some("after") => true,
        _ => return Err(bad("before or after")),
    };
    let nth = match parts.next() {
        None => 1,
        Some(n) => match n.parse::<u64>() {
            Ok(n) if n >= 1 => n,
            _ => return Err(bad("N is a whole number, at least 1")),
        },
    };
    if parts.next().is_some() {
        return Err(bad("too many parts"));
    }
    Ok(KillAt {
        point: Point { effect, after },
        nth,
    })
}

/// "before" or "after".
fn side(after: bool) -> &'static str {
    if after {
        "after"
    } else {
        "before"
    }
}

/// The line of standard error before a kill (testing's item 1).
pub fn kill_line(at: &KillAt) -> String {
    format!(
        "skyzfs-agent: test hook --kill-at: killed at {} {} ({})\n",
        at.point.effect.word(),
        side(at.point.after),
        at.nth
    )
}

/// The line `--trace-points` appends for a point reached at `ns` on
/// `CLOCK_MONOTONIC`, for the publish `name` (empty where it is for none;
/// [`crate::engine::Points`]): four fields between tabs.
pub fn trace_line(ns: u64, point: Point, name: &str) -> String {
    format!(
        "{ns}\t{}\t{}\t{name}\n",
        point.effect.word(),
        side(point.after)
    )
}

/// The engine's points for `hooks` ([module docs](self)): each point
/// written to `trace`, then `die` called with [`kill_line`] at the
/// chosen reach. `now_ns` reads `CLOCK_MONOTONIC`. A trace that cannot
/// be written stops the engine there: a rig would read a trace with a
/// point missing as a point not reached. `None` where there is no hook.
pub fn points(
    kill_at: Option<KillAt>,
    mut trace: Option<Box<dyn Write>>,
    mut now_ns: impl FnMut() -> u64 + 'static,
    mut die: impl FnMut(&str) + 'static,
    mut err: Box<dyn Write>,
) -> Option<Points> {
    if kill_at.is_none() && trace.is_none() {
        return None;
    }
    let mut reached = 0u64;
    Some(Box::new(move |point: Point, name: &str| {
        if let Some(out) = &mut trace {
            let line = trace_line(now_ns(), point, name);
            if let Err(e) = out.write_all(line.as_bytes()).and_then(|()| out.flush()) {
                let _ = writeln!(err, "skyzfs-agent: --trace-points: {e}; stopped");
                return true;
            }
        }
        if let Some(at) = kill_at {
            if at.point == point {
                reached += 1;
                if reached == at.nth {
                    die(&kill_line(&at));
                }
            }
        }
        false
    }))
}

/// The word of `why`, as `abandoned` lines and the record's dump give it.
pub fn why_word(why: Why) -> &'static str {
    match why {
        Why::ManifestLost => "manifest-lost",
        Why::ManifestTooLarge => "manifest-too-large",
        Why::Destroyed => "destroyed",
        Why::Replaced => "replaced",
        Why::Premise => "premise",
        Why::NameTaken => "name-taken",
        Why::Rewound => "rewound",
        Why::NoProgress => "no-progress",
        Why::TooOld => "too-old",
        Why::Stopped => "stopped",
        Why::Person => "person",
    }
}

/// The line of standard output for a publish that left the record or
/// stopped ([module docs](self)).
pub fn outcome_line(name: &str, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Published(commit) => format!("snap/{name} {}\n", hex(commit)),
        Outcome::Abandoned(why) => format!("abandoned {name} {}\n", why_word(*why)),
        Outcome::Stopped(why) => format!("stopped {name} {why}\n"),
    }
}

/// The line of standard error for a report: its first word after the
/// name says which, for a rig to match.
pub fn report_line(name: &str, report: &Report) -> String {
    let what = match report {
        Report::SnapshotGone => "snapshot-gone: snap/t is this publish's, and pool@t or a \
             member's @t is gone; only opening its cut tells whether it holds pool@t"
            .to_string(),
        Report::NoProgress(n) => {
            format!("no-progress {n}: asked {n} times in a row with no cut counted")
        }
        Report::Refused(code) => {
            format!("refused {}: given up on the daemon's refusal", code.word())
        }
        Report::HeldAway => "held-away: older than its bound, its hold kept while the daemon \
             does not answer"
            .to_string(),
    };
    format!("skyzfs-agent: report {name} {what}\n")
}

/// The exit code of one outcome; `None` is a publish still under way.
pub fn exit_of(outcome: Option<&Outcome>) -> i32 {
    match outcome {
        Some(Outcome::Published(_)) => exit::PUBLISHED,
        Some(Outcome::Abandoned(_)) => exit::ABANDONED,
        Some(Outcome::Stopped(_)) => exit::STOPPED,
        None => exit::UNSETTLED,
    }
}

/// The exit code of `settle`: [`exit::UNSETTLED`] where a publish is
/// still under way at the limit, else [`exit::STOPPED`] where one
/// stopped, else [`exit::ABANDONED`] where one was given up, else
/// [`exit::PUBLISHED`].
pub fn worst(outcomes: &[(String, Outcome)], unsettled: bool) -> i32 {
    let any = |code: i32| outcomes.iter().any(|(_, o)| exit_of(Some(o)) == code);
    if unsettled {
        exit::UNSETTLED
    } else if any(exit::STOPPED) {
        exit::STOPPED
    } else if any(exit::ABANDONED) {
        exit::ABANDONED
    } else {
        exit::PUBLISHED
    }
}

/// The exit code of an error of the engine.
pub fn exit_of_error(e: &Error) -> i32 {
    match e {
        Error::Corrupt(_) | Error::Volume(_) | Error::Unmarked(_) => exit::RECORD,
        Error::Premise(_) => exit::PREMISE,
        Error::Unsettled(_) => exit::UNSETTLED,
        Error::Stopped
        | Error::Full
        | Error::Daemon(_)
        | Error::Zfs(_)
        | Error::Save(_)
        | Error::Io(_)
        | Error::NotUnderWay(_) => exit::ERROR,
    }
}

/// What [`execute`] does before it settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    /// Begin a publish.
    Publish,
    /// Nothing.
    Settle,
    /// Record a person's choice for a publish.
    Recover(String, Choice),
}

/// Run `task` on `agent`, then settle for at most `limit`: the outcome
/// lines on `out`, errors on `err`; the exit code ([module docs](self)).
pub fn execute<Z: Zfs, D: Daemon, K: Keep, C: Clock>(
    agent: &mut Agent<Z, D, K, C>,
    task: &Task,
    limit: Duration,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let mine = match task {
        Task::Publish => match agent.begin() {
            Ok(name) => Some(name),
            Err(e) => return failed(&e, err),
        },
        Task::Settle => None,
        Task::Recover(name, choice) => match agent.recover(name, *choice) {
            Ok(()) => Some(name.clone()),
            Err(e) => return failed(&e, err),
        },
    };
    let settled = agent.settle(limit);
    for (name, outcome) in agent.outcomes() {
        let _ = out.write_all(outcome_line(name, outcome).as_bytes());
    }
    let unsettled = match &settled {
        Ok(()) => Vec::new(),
        Err(Error::Unsettled(names)) => names.clone(),
        Err(e) => return failed(e, err),
    };
    for name in &unsettled {
        let _ = writeln!(out, "unsettled {name}");
    }
    let _ = out.flush();
    match mine {
        // By the name the publish goes by now: it may have taken the
        // next one (D2 of #149's review).
        Some(mine) => {
            let mine = agent.name_now(&mine);
            exit_of(
                agent
                    .outcomes()
                    .iter()
                    .rev()
                    .find(|(name, _)| *name == mine)
                    .map(|(_, outcome)| outcome),
            )
        }
        None => worst(agent.outcomes(), !unsettled.is_empty()),
    }
}

/// Say `e` on `err`; its exit code.
fn failed(e: &Error, err: &mut dyn Write) -> i32 {
    let _ = writeln!(err, "skyzfs-agent: {e}");
    exit_of_error(e)
}

/// The volume's id from the bytes of its file `remote` ([module
/// docs](self)): the ULID of its line `id`, as 16 bytes, most
/// significant first.
pub fn volume_id(remote: &[u8]) -> Result<[u8; 16], String> {
    let text = std::str::from_utf8(remote).map_err(|_| "not UTF-8".to_string())?;
    let body = text
        .strip_suffix('\n')
        .ok_or("its last line has no line feed")?;
    let lines: Vec<&str> = body.split('\n').collect();
    let keys = ["endpoint", "bucket", "volume", "id"];
    match lines.split_first() {
        Some((&"skyzfs-remote 1", rest)) if rest.len() == keys.len() => {
            let mut id = "";
            for (line, key) in rest.iter().zip(keys) {
                match line.strip_prefix(key).and_then(|v| v.strip_prefix(' ')) {
                    Some(value) if !value.is_empty() => id = value,
                    _ => return Err(format!("{line:?} where the line {key:?} belongs")),
                }
            }
            ulid(id).map(u128::to_be_bytes)
        }
        Some((first, _)) if *first != "skyzfs-remote 1" => Err(format!(
            "its first line is {first:?}, not \"skyzfs-remote 1\""
        )),
        _ => Err("not the five lines of \"skyzfs-remote 1\"".into()),
    }
}

/// A ULID's 26 characters of Crockford's base 32 (upper or lower case),
/// as its 128 bits.
fn ulid(text: &str) -> Result<u128, String> {
    const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bad = || format!("the id {text:?} is not a ULID");
    if text.len() != 26 {
        return Err(bad());
    }
    let mut value: u128 = 0;
    for (i, c) in text.bytes().enumerate() {
        let digit = DIGITS
            .iter()
            .position(|&d| d == c.to_ascii_uppercase())
            .ok_or_else(bad)?;
        // The first character holds 3 bits: 26 × 5 = 130.
        if i == 0 && digit > 7 {
            return Err(bad());
        }
        value = (value << 5) | u128::try_from(digit).map_err(|_| bad())?;
    }
    Ok(value)
}

/// The record as `skyzfs-agent record` prints it (testing's item 4), or
/// why it is not read. One line a fact, its words between single spaces,
/// `key=value` after the first two; `-` where there is no value:
///
/// ```text
/// volume <32 hex digits>
/// enrolled <pool> pool_guid=<n> leaf_guid=<n>      or: enrolled -
/// publish <t> phase=<intent|taken|held|asked|done> count=<C> group=<G>
///   stalls=<n> attempts=<n> held=<yes|no> asked_ms=<ms> held_until_ms=<ms>
///   manifest=<64 hex digits> commit=<64 hex digits> person=<-|stopped|give-up>
///   begun_ms=<ms>                                   (one line)
/// abandoned <t> why=<word> at_ms=<ms>
/// ```
///
/// With no record, the one line `absent`.
pub fn dump(loaded: &Loaded) -> Result<String, String> {
    let record = match loaded {
        Loaded::Absent => return Ok("absent\n".into()),
        Loaded::Corrupt(why) => return Err(format!("the record is corrupt: {why}")),
        Loaded::Present(record) => record,
    };
    let mut out = format!("volume {}\n", hex(&record.volume));
    match &record.enrolled {
        None => out.push_str("enrolled -\n"),
        Some(e) => out.push_str(&format!(
            "enrolled {} pool_guid={} leaf_guid={}\n",
            e.pool, e.pool_guid, e.leaf_guid
        )),
    }
    let dash = || "-".to_string();
    for p in &record.under_way {
        let (phase, taken, asked, until, commit) = match &p.phase {
            Phase::Intent => ("intent", None, None, None, None),
            Phase::Taken(t) => ("taken", Some(t), None, None, None),
            Phase::Held(t) => ("held", Some(t), None, None, None),
            Phase::Asked {
                taken,
                asked_ms,
                held_until_ms,
            } => (
                "asked",
                Some(taken),
                Some(*asked_ms),
                Some(*held_until_ms),
                None,
            ),
            Phase::Done { taken, commit } => ("done", Some(taken), None, None, Some(commit)),
        };
        let person = match p.person {
            None => "-",
            Some(Person::Stopped) => "stopped",
            Some(Person::GiveUp) => "give-up",
        };
        out.push_str(&format!(
            "publish {} phase={phase} count={} group={} stalls={} attempts={} held={} \
             asked_ms={} held_until_ms={} manifest={} commit={} person={person} begun_ms={}\n",
            p.name,
            p.count.after,
            taken.map_or_else(dash, |t| t.group.to_string()),
            p.stalls,
            p.attempts,
            if p.held { "yes" } else { "no" },
            asked.map_or_else(dash, |ms| ms.to_string()),
            until.map_or_else(dash, |ms| ms.to_string()),
            taken.map_or_else(dash, |t| hex(&t.manifest_hash)),
            commit.map_or_else(dash, |c| hex(c)),
            p.begun_ms,
        ));
    }
    for a in &record.abandoned {
        out.push_str(&format!(
            "abandoned {} why={} at_ms={}\n",
            a.name,
            why_word(a.why),
            a.at_ms
        ));
    }
    Ok(out)
}

/// The agent's settings from `run`'s test hooks and expiry, on `agent`.
pub fn configured<Z: Zfs, D: Daemon, K: Keep, C: Clock>(
    mut agent: Agent<Z, D, K, C>,
    run: &Run,
) -> Agent<Z, D, K, C> {
    if let Some(expiry) = run.expiry {
        agent = agent.with_expiry(expiry);
    }
    if let Some(after) = run.hooks.assist_after {
        agent = agent.with_assist_after(after);
    }
    if let Some(age_max) = run.hooks.age_max {
        agent = agent.with_age_max(age_max);
    }
    agent
}

#[cfg(test)]
mod tests;
