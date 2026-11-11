//! `create`, `status` and `detach`. Each returns what it would print
//! and its exit status, and does no printing itself. `status` and
//! `detach` work over whatever stands for the kernel's control
//! interface ([`Control`]); `create` needs no kernel.
//!
//! `status` and `detach` do not open the store. `detach` takes the volume's lock, as the
//! lifecycle demands; `status` takes nothing and changes nothing.
//!
//! Both first ask whether the directory holds a volume at all (the
//! log's superblock is there), so that a mistyped `--dir` is an error
//! and no lock file is made in a directory that is no volume. That
//! check is this module's own reading: the design does not say what a
//! command does with a directory that is no volume.
//!
//! # What `create` fixes
//!
//! A new volume has 4096-byte record alignment, log segments of
//! [`SEGMENT_SIZE`], and the write unit of the system it is made on
//! ([`Identity::system_write_unit`]). None of them is an option: they
//! are recorded with the volume, and a later version that wants others
//! makes new volumes with others. The directory must exist and hold no
//! volume: `create` makes no directory and replaces nothing.

use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::time::Duration;

use skyzfs_blk::Base;
use skyzfs_local::format::Identity;
use skyzfs_local::fs::FsDir;
use skyzfs_local::log::{LogError, SUPER_NAME};
use skyzfs_local::store::{OpenOptions, Store, StoreConfig, StoreError};
use skyzfs_ublk::lifecycle::{self, Control, Detached, Held, Whose};
use skyzfs_ublk::records::{self, Files};
use skyzfs_ublk::status::{self, Noted};
use skyzfs_ublk::volume::StoreVolume;

use crate::cli::{Create, Exit, Volume};

/// How often a locked volume is tried again: 20 times.
pub const LOCK_RETRIES: u32 = 20;

/// How long between two tries: 100 ms. With [`LOCK_RETRIES`], 2 s.
pub const LOCK_PAUSE: Duration = Duration::from_millis(100);

/// The size of a new volume's log segments: 64 MiB. A constant, and
/// recorded with the volume.
pub const SEGMENT_SIZE: u64 = 64 << 20;

/// The record alignment of a new volume's log: 4096, so that the log
/// can be written with direct I/O.
pub const ALIGN: u32 = 4096;

/// What a command prints, and how it exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The exit status.
    pub exit: Exit,
    /// For standard output.
    pub out: String,
    /// For standard error.
    pub err: String,
}

impl Output {
    fn error(why: impl std::fmt::Display) -> Self {
        Self {
            exit: Exit::Error,
            out: String::new(),
            err: format!("skyzfsd: {why}\n"),
        }
    }

    fn said(exit: Exit, out: &str, err: &str) -> Self {
        let line = |text: &str, prefix: &str| {
            if text.is_empty() {
                String::new()
            } else {
                format!("{prefix}{text}\n")
            }
        };
        Self {
            exit,
            out: line(out, ""),
            err: line(err, "skyzfsd: "),
        }
    }
}

/// This boot and the time now, as `status` needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    /// The boot's id.
    pub boot: [u8; 16],
    /// Milliseconds since boot.
    pub uptime_ms: u64,
}

impl Now {
    /// Read both from the kernel.
    pub fn read() -> io::Result<Self> {
        Ok(Self {
            boot: records::boot_id()?,
            uptime_ms: status::uptime_ms()?,
        })
    }
}

/// The records of the volume in `dir`, if `dir` holds a volume.
fn files(volume: &Volume, boot: [u8; 16]) -> Result<Files, Output> {
    let dir: &Path = &volume.dir;
    match dir.join(SUPER_NAME).try_exists() {
        Ok(true) => {}
        Ok(false) => return Err(Output::error(format!("no volume in {}", dir.display()))),
        Err(e) => return Err(Output::error(format!("{}: {e}", dir.display()))),
    }
    Files::new(dir, &volume.run_dir, boot, LOCK_PAUSE)
        .map_err(|e| Output::error(format!("{}: {e}", dir.display())))
}

/// `status`: the volume's state, its device, and the counters the
/// serving daemon left, one `key value` per line
/// ([`status::report`]).
///
/// A status file that cannot be read is said on standard error, and
/// the exit status is 1; the state is printed all the same. An error
/// in finding the volume's device (a record or a note that cannot be
/// read) prints nothing but the error.
pub fn status(volume: &Volume, control: &mut dyn Control, now: &Now) -> Output {
    let mut records = match files(volume, now.boot) {
        Ok(records) => records,
        Err(output) => return output,
    };
    let mut control = control;
    let located = match lifecycle::locate(&mut control, &mut records) {
        Ok(located) => located,
        Err(e) => return Output::error(format!("finding the volume's device: {e}")),
    };
    let mut err = String::new();
    let file = match status::read(&records.volume_run_dir().join(status::STATUS)) {
        Ok(file) => file,
        Err(e) => {
            err = format!("skyzfsd: the status file cannot be read: {e}\n");
            None
        }
    };
    let noted = match records::read_failure(&records.failure_path()) {
        Ok(None) => Noted::Nothing,
        Ok(Some(stored)) => Noted::Whole(stored),
        Err(_) => Noted::Unreadable,
    };
    let lines = status::report(
        located.as_ref(),
        file.as_ref(),
        noted,
        &now.boot,
        now.uptime_ms,
    );
    let mut out = String::new();
    for (key, value) in lines {
        let _ = writeln!(out, "{key} {value}");
    }
    Output {
        exit: if err.is_empty() {
            Exit::Done
        } else {
            Exit::Error
        },
        out,
        err,
    }
}

/// `detach`: delete the volume's device if it is the volume's and no
/// server serves it ([`lifecycle::detach`]). It does not touch the
/// failure note.
///
/// | What was found | Exit status | Said on |
/// |---|---|---|
/// | no device of this boot is the volume's | 0 | standard output |
/// | the device was deleted | 0 | standard output |
/// | it was deleted and something still holds it open | 0 | standard error |
/// | a device with no server and no note, with the recorded values: deleted before and still held, or another's | 0 | standard error |
/// | the device under the recorded number is not known to be the volume's | 1 | standard error |
/// | a server is live on it | 5 | standard error |
/// | a daemon holds the volume's lock, after 2 s of trying | 5 | standard error |
pub fn detach(volume: &Volume, control: &mut dyn Control, boot: [u8; 16]) -> Output {
    let mut records = match files(volume, boot) {
        Ok(records) => records,
        Err(output) => return output,
    };
    let mut store = StoreVolume::new(&volume.dir, OpenOptions::default());
    let mut control = control;
    let detached = match lifecycle::detach(&mut control, &mut records, &mut store, LOCK_RETRIES) {
        Ok(detached) => detached,
        Err(e) => return Output::error(format!("detach: {e}")),
    };
    match detached {
        Detached::Nothing => Output::said(
            Exit::Done,
            "nothing to detach: no device of this boot is this volume's",
            "",
        ),
        Detached::Deleted(n) => Output::said(Exit::Done, &format!("deleted /dev/ublkb{n}"), ""),
        Detached::DeletedHeld(n) => Output::said(
            Exit::Done,
            "",
            &format!(
                "deleted /dev/ublkb{n}; something still holds it open, and it keeps its number \
                 until that lets go (a pool that was not exported stays suspended)"
            ),
        ),

        Detached::DeadNoNote(n) => Output::said(
            Exit::Done,
            "",
            &format!(
                "device {n} under the recorded number has no server and no note: it is this \
                 volume's that was deleted before and is still held open, or another's that \
                 was never started. Nothing was done"
            ),
        ),
        Detached::NotOurs(located) => {
            let why = match located.whose {
                Whose::Unsure => "has this volume's values and no note",
                _ => "is not this volume's",
            };
            Output::said(
                Exit::Error,
                "",
                &format!(
                    "device {} under the recorded number {why}: nothing was done",
                    located.number
                ),
            )
        }
        Detached::Live(n) => Output::said(
            Exit::RefusedHeld,
            "",
            &format!("a server is live on /dev/ublkb{n}: stop it first. Nothing was done"),
        ),
        Detached::Locked => Output::said(
            Exit::RefusedHeld,
            "",
            "a daemon holds this volume: stop it first. Nothing was done",
        ),
    }
}

fn hex(bytes: &[u8; 16]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// `create`: make a new volume with the id `id` in a directory that
/// exists and holds none. `boot` is this boot, if it could be read:
/// with it, a failure note of this boot under the run directory is
/// warned of on standard error (the volume is made all the same, and
/// the note stays).
///
/// On success one line: `created DIR: N bytes, volume ID, identity
/// IDENTITY`, the two in hex. The identity is the store's
/// ([`Store::identity`]): what tells this volume from another made
/// later in the same directory.
pub fn create(create: &Create, id: [u8; 16], boot: Option<[u8; 16]>) -> Output {
    let shown = create.dir.display();
    let dir = match FsDir::new(&create.dir) {
        Ok(dir) => dir,
        Err(e) => return Output::error(format_args!("{shown}: {e}: create makes no directory")),
    };
    let config = StoreConfig {
        volume: id,
        size_mib: create.size >> 20,
        base: create.base_pattern.map_or(Base::Empty, Base::Pattern),
        align: ALIGN,
        segment_size: SEGMENT_SIZE,
        write_unit: Identity::system_write_unit(),
    };
    let store = match Store::create(dir, &config, OpenOptions::default()) {
        Ok(store) => store,
        Err(StoreError::Log(LogError::Exists)) => {
            return Output::error(format_args!(
                "{shown} holds a volume already. Nothing was done"
            ))
        }
        Err(StoreError::Locked) => {
            return Output::said(
                Exit::RefusedHeld,
                "",
                &format!("{shown} is in use by another instance. Nothing was done"),
            )
        }
        Err(e) => return Output::error(format_args!("creating a volume in {shown}: {e}")),
    };
    let identity = store.identity();
    drop(store);
    let warning = boot
        .and_then(|boot| create_warning(&create.dir, &create.run_dir, &boot))
        .map_or_else(String::new, |warning| format!("skyzfsd: {warning}\n"));
    Output {
        exit: Exit::Done,
        out: format!(
            "created {shown}: {} bytes, volume {}, identity {}\n",
            create.size,
            hex(&id),
            hex(&identity)
        ),
        err: warning,
    }
}

/// What `create` says when the directory it is given carries a failure
/// note of this boot: the note belongs to the directory, not to the
/// volume that was in it, and `create` does not remove it. `None` if
/// there is no such note.
pub fn create_warning(dir: &Path, run_dir: &Path, boot: &[u8; 16]) -> Option<String> {
    let path = records::volume_run_dir(run_dir, dir)
        .ok()?
        .join(records::FAILED);
    let held = match records::read_failure(&path) {
        Ok(None) => return None,
        Ok(Some(stored)) if stored.boot != *boot => return None,
        Ok(Some(stored)) => stored.failure.held,
        Err(_) => Held::Yes,
    };
    let what = match held {
        Held::Yes => "while a client held its device: the new volume is not served before a reboot",
        Held::No => "the new volume is served only with --acknowledge-failure",
        Held::Unknown => {
            "and it is not known whether a client holds its device: the next serve finds out, \
             and the new volume is served only with --acknowledge-failure, if at all"
        }
    };
    Some(format!(
        "warning: a volume in {} failed in this boot, {what} ({} is left as it is)",
        dir.display(),
        path.display()
    ))
}

#[cfg(test)]
mod tests;
