//! `skyzfs-agent`: the SkyZFS node agent's program (M2.6c-3). It builds
//! the agent's adapters on the machine and hands the rest to
//! [`skyzfs_agent::program`], which holds its command line, its output
//! and its exit codes (see its docs).

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use skyzfs_agent::daemon::DaemonSocket;
use skyzfs_agent::engine::{Agent, Points, SystemClock};
use skyzfs_agent::host::System;
use skyzfs_agent::program::{self, exit, Command, Hooks, Run, Task, USAGE};
use skyzfs_agent::record::{self, OpenError, Store};
use skyzfs_agent::zfs::cli::{Limits, ZfsCli};

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => {
                eprintln!("skyzfs-agent: {arg:?} is not UTF-8\n{USAGE}");
                return exit::USAGE;
            }
        }
    }
    let command = match program::parse(&args, cfg!(feature = "test-hooks")) {
        Ok(command) => command,
        Err(why) => {
            eprintln!("skyzfs-agent: {why}\n{USAGE}");
            return exit::USAGE;
        }
    };
    match command {
        Command::Help => {
            print_out(&format!("{USAGE}\n"));
            exit::PUBLISHED
        }
        Command::Record { dir } => dump(&dir),
        Command::Publish(run) => settle(&run, &Task::Publish),
        Command::Settle(run) => settle(&run, &Task::Settle),
        Command::Recover { run, name, choice } => settle(&run, &Task::Recover(name, choice)),
        Command::Run { run, every } => scheduled(&run, every),
    }
}

/// Write `text` to standard output. A pipe closed by its reader (`|
/// head -1`) is not an error of the agent's: what it printed is lost to
/// none who reads (S4 of #149's review).
fn print_out(text: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// `skyzfs-agent record`: `absent` for a directory with no record, an
/// error for a path that is not a directory (S2 of #149's review: a
/// mistyped `--dir` does not read as "no record").
fn dump(dir: &Path) -> i32 {
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            eprintln!("skyzfs-agent: {}: not a directory", dir.display());
            return exit::ERROR;
        }
        Err(e) => {
            eprintln!("skyzfs-agent: {}: {e}", dir.display());
            return exit::ERROR;
        }
    }
    match record::peek(dir) {
        Ok(loaded) => match program::dump(&loaded) {
            Ok(text) => {
                print_out(&text);
                exit::PUBLISHED
            }
            Err(why) => {
                eprintln!("skyzfs-agent: {}: {why}", dir.display());
                exit::RECORD
            }
        },
        Err(e) => {
            eprintln!("skyzfs-agent: {}: {e}", dir.display());
            exit::ERROR
        }
    }
}

/// The agent on the machine's adapters ([`program::wired`]), with the
/// test hooks' points; the exit code where it cannot be made.
type MachineAgent = Agent<ZfsCli<System>, DaemonSocket<System>, Store, SystemClock>;

fn agent(run: &Run) -> Result<MachineAgent, i32> {
    let volume = program::volume_of(run).map_err(|why| {
        eprintln!("skyzfs-agent: {why}");
        exit::ERROR
    })?;
    // The daemon's channel, in the volume's own directory under the run
    // directory, named as the daemon names it.
    let socket = skyzfs_control::socket_path(&run.run, &run.volume).map_err(|e| {
        eprintln!("skyzfs-agent: {}: {e}", run.volume.display());
        exit::ERROR
    })?;
    let store = Store::open(&run.dir).map_err(|e| {
        eprintln!("skyzfs-agent: {e}");
        match e {
            OpenError::Held(_) => exit::HELD,
            OpenError::Io(_) => exit::ERROR,
        }
    })?;
    let zfs = ZfsCli::new(System, Limits::default());
    let daemon = DaemonSocket::new(
        socket,
        run.device.clone(),
        skyzfs_control::ASK_LIMIT,
        System,
    );
    let parts = (zfs, daemon, store, SystemClock::default());
    let mut agent = program::wired(
        run,
        volume,
        parts,
        Box::new(io::stdout()),
        Box::new(io::stderr()),
    )
    .map_err(|e| {
        eprintln!("skyzfs-agent: {e}");
        program::exit_of_error(&e)
    })?;
    match points(&run.hooks) {
        Ok(Some(points)) => agent = agent.with_points(points),
        Ok(None) => {}
        Err(e) => {
            eprintln!("skyzfs-agent: --trace-points: {e}");
            return Err(exit::ERROR);
        }
    }
    Ok(agent)
}

/// `publish`, `settle` and `recover`.
fn settle(run: &Run, task: &Task) -> i32 {
    match agent(run) {
        Ok(mut agent) => {
            let (mut out, mut err) = (io::stdout(), io::stderr());
            program::execute(&mut agent, task, run.limit, &mut out, &mut err)
        }
        Err(code) => code,
    }
}

/// `run`: until `SIGTERM` or `SIGINT`, which it takes between two rounds
/// of the engine ([`program::schedule`]). A second one ends it at once,
/// with exit 1, as a kill would: the agent is made to be killed anywhere.
fn scheduled(run: &Run, every: std::time::Duration) -> i32 {
    let told = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        // The order is signal-hook's own for "a second one ends it": the
        // conditional exit first, so that the first signal finds the
        // flag unset and only sets it.
        let registered = signal_hook::flag::register_conditional_shutdown(
            signal,
            exit::ERROR,
            Arc::clone(&told),
        )
        .and_then(|_| signal_hook::flag::register(signal, Arc::clone(&told)));
        if let Err(e) = registered {
            eprintln!("skyzfs-agent: the signal {signal}: {e}");
            return exit::ERROR;
        }
    }
    match agent(run) {
        Ok(mut agent) => {
            let (mut out, mut err) = (io::stdout(), io::stderr());
            let mut go = || !told.load(Ordering::SeqCst);
            program::schedule(&mut agent, every, &mut go, &mut out, &mut err)
        }
        Err(code) => code,
    }
}

/// The test hooks' points ([`program::points`]).
#[cfg(feature = "test-hooks")]
fn points(hooks: &Hooks) -> io::Result<Option<Points>> {
    let trace: Option<Box<dyn Write>> = match &hooks.trace_points {
        Some(path) => Some(Box::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?,
        )),
        None => None,
    };
    let now_ns = || {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        u64::try_from(t.tv_sec)
            .unwrap_or(0)
            .saturating_mul(1_000_000_000)
            .saturating_add(u64::try_from(t.tv_nsec).unwrap_or(0))
    };
    let die = |line: &str| {
        let mut err = io::stderr().lock();
        let _ = err.write_all(line.as_bytes());
        let _ = err.flush();
        let me = rustix::process::getpid();
        let _ = rustix::process::kill_process(me, rustix::process::Signal::KILL);
        // The signal is delivered before the call returns; should it not
        // be, the agent still goes no further.
        std::process::abort();
    };
    Ok(program::points(
        hooks.kill_at,
        trace,
        now_ns,
        die,
        Box::new(io::stderr()),
    ))
}

/// A build without the test hooks has none: [`program::parse`] refuses
/// them.
#[cfg(not(feature = "test-hooks"))]
fn points(_: &Hooks) -> io::Result<Option<Points>> {
    Ok(None)
}
