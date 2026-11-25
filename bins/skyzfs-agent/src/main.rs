//! `skyzfs-agent`: the SkyZFS node agent's program (M2.6c-3). It builds
//! the agent's adapters on the machine and hands the rest to
//! [`skyzfs_agent::program`], which holds its command line, its output
//! and its exit codes (see its docs).

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::path::Path;

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
            println!("{USAGE}");
            exit::PUBLISHED
        }
        Command::Record { dir } => dump(&dir),
        Command::Publish(run) => settle(&run, &Task::Publish),
        Command::Settle(run) => settle(&run, &Task::Settle),
        Command::Recover { run, name, choice } => settle(&run, &Task::Recover(name, choice)),
    }
}

/// `skyzfs-agent record`.
fn dump(dir: &Path) -> i32 {
    match record::peek(dir) {
        Ok(loaded) => match program::dump(&loaded) {
            Ok(text) => {
                print!("{text}");
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

/// `publish`, `settle` and `recover`.
fn settle(run: &Run, task: &Task) -> i32 {
    let remote = run.volume.join("remote");
    let volume = match std::fs::read(&remote) {
        Ok(bytes) => match program::volume_id(&bytes) {
            Ok(volume) => volume,
            Err(why) => {
                eprintln!("skyzfs-agent: {}: {why}", remote.display());
                return exit::ERROR;
            }
        },
        Err(e) => {
            eprintln!(
                "skyzfs-agent: {}: {e} (a volume with no place in object storage publishes nothing)",
                remote.display()
            );
            return exit::ERROR;
        }
    };
    let store = match Store::open(&run.dir) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("skyzfs-agent: {e}");
            return match e {
                OpenError::Held(_) => exit::HELD,
                OpenError::Io(_) => exit::ERROR,
            };
        }
    };
    let zfs = ZfsCli::new(System, Limits::default());
    let daemon = DaemonSocket::new(
        run.run.join(skyzfs_control::SOCKET),
        run.device.clone(),
        skyzfs_control::ASK_LIMIT,
        System,
    );
    let agent = match Agent::new(
        zfs,
        daemon,
        store,
        SystemClock::default(),
        volume,
        &run.pool,
    ) {
        Ok(agent) => agent,
        Err(e) => {
            eprintln!("skyzfs-agent: {e}");
            return program::exit_of_error(&e);
        }
    };
    let mut agent = program::configured(agent, run).with_sink(Box::new(|name, report| {
        let mut err = io::stderr().lock();
        let _ = err.write_all(program::report_line(name, report).as_bytes());
        let _ = err.flush();
    }));
    match points(&run.hooks) {
        Ok(Some(points)) => agent = agent.with_points(points),
        Ok(None) => {}
        Err(e) => {
            eprintln!("skyzfs-agent: --trace-points: {e}");
            return exit::ERROR;
        }
    }
    let (mut out, mut err) = (io::stdout().lock(), io::stderr());
    program::execute(&mut agent, task, run.limit, &mut out, &mut err)
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
