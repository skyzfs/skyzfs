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
//! [`SEGMENT_SIZE`], and a write unit that is the larger of the
//! system's page size and the block size of the file system the
//! directory is on ([`Identity::write_unit_for`]): the files are
//! written through the page cache, and a file system writes back
//! whole blocks, so that is how far a write can reach. A directory
//! whose file system has blocks above 65536 bytes (a network file
//! system that reports its transfer size, for one), blocks that are no
//! power of two, or a block size that cannot be read is refused: the
//! format carries no such unit, and the store's protection against a
//! power cut rests on it. None of them is an option: they
//! are recorded with the volume, and a later version that wants others
//! makes new volumes with others. If the directory is missing, `create`
//! makes it (its last component only, mode 0700); a missing parent, a
//! path that is not a directory and a directory that holds a volume
//! are errors. `create` replaces nothing, and a `create` that fails
//! after it made the directory removes the directory again if it is
//! empty, so that it leaves nothing behind.

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

/// How often a device that was deleted is looked at again before its
/// number is said to be held: with [`LOCK_PAUSE`] between, 5 s, the
/// time a start gives a device that is still live. The driver's delete
/// does not wait, and the binding's kernel tests measure how long the
/// number of a device nobody holds takes to come free
/// (`control_delete_frees_the_number_within`, with its detail line).
pub const GONE_RETRIES: u32 = 50;

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

/// A control interface that is opened when it is first asked
/// something, and not before. `status` and `detach` ask the driver
/// only when the volume has a record of this boot to ask about; after
/// a reboot, before the driver is loaded, there is none, the answer is
/// "detached", and the driver's absence must not turn it into an
/// error. If the opening fails, every question fails with its error
/// (it is tried again at the next question), and whether a client
/// holds a device cannot be told.
pub struct Lazy<F> {
    open: F,
    control: Option<Box<dyn Control>>,
}

impl<F: FnMut() -> io::Result<Box<dyn Control>>> Lazy<F> {
    /// A control interface that `open` makes at the first question.
    pub fn new(open: F) -> Self {
        Self {
            open,
            control: None,
        }
    }

    fn control(&mut self) -> io::Result<&mut Box<dyn Control>> {
        if self.control.is_none() {
            self.control = Some((self.open)()?);
        }
        self.control
            .as_mut()
            .ok_or_else(|| io::Error::other("no control interface"))
    }
}

impl<F: FnMut() -> io::Result<Box<dyn Control>>> Control for Lazy<F> {
    fn features(&mut self) -> io::Result<Option<u64>> {
        self.control()?.features()
    }
    fn release(&mut self) -> io::Result<String> {
        self.control()?.release()
    }
    fn info(&mut self, number: u32) -> io::Result<Option<lifecycle::DeviceInfo>> {
        self.control()?.info(number)
    }
    fn add(&mut self, number: u32, shape: &lifecycle::Shape) -> Result<(), lifecycle::AddError> {
        self.control()
            .map_err(lifecycle::AddError::Other)?
            .add(number, shape)
    }
    fn disown(&mut self, number: u32) {
        if let Ok(control) = self.control() {
            control.disown(number);
        }
    }
    fn delete(&mut self, number: u32) -> io::Result<()> {
        self.control()?.delete(number)
    }
    fn begin_recovery(&mut self, number: u32) -> io::Result<()> {
        self.control()?.begin_recovery(number)
    }
    fn client_holds(&mut self, number: u32) -> Option<bool> {
        self.control().ok()?.client_holds(number)
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
    // The run directory is named after the volume's directory, so a
    // volume that was in this directory before and was made again
    // leaves its status file where this one's would be. Counters are
    // shown only from a file that names this store. The store is not
    // opened for that: its identity is read from the superblock.
    let file = match file {
        None => None,
        Some(file) => match FsDir::new(&volume.dir).and_then(|dir| Store::identity_of(&dir)) {
            Ok(Some(identity)) if identity == file.identity => Some(file),
            Ok(_) => None,
            Err(e) => {
                err = format!(
                    "skyzfsd: the volume's identity cannot be read ({e}): the status file \
                     is not shown\n"
                );
                None
            }
        },
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
/// | the device was deleted, and its number is free | 0 | standard output |
/// | it was deleted and something still holds it open | 6 | standard error |
/// | a device with no server and no note, with the recorded values: deleted before and still held, or another's | 1 | standard error |
/// | the device under the recorded number is not known to be the volume's | 1 | standard error |
/// | a server is live on it | 5 | standard error |
/// | a daemon holds the volume's lock, after 2 s of trying | 5 | standard error |
/// | the kernel lacks what the daemon needs (`skyzfs_ublk::features`), with a record of this boot: nothing was done | 1 | standard error |
///
/// **Only 0 says that nothing of this volume is left attached on this
/// node**, so that the volume may be served elsewhere on it. With 6
/// the device is deleted and a client (a pool that was not exported)
/// still holds it under its number; with 1 the device was left as it
/// is because whose it is cannot be told; with 5 nothing was done.
///
/// The driver's delete does not wait, so "still held" is said only of
/// a device that is still there 5 s after the delete
/// ([`GONE_RETRIES`]): a detach of a held device takes that much
/// longer.
pub fn detach(
    volume: &Volume,
    async_delete_asserted: bool,
    control: &mut dyn Control,
    boot: [u8; 16],
) -> Output {
    let mut records = match files(volume, boot) {
        Ok(records) => records,
        Err(output) => return output,
    };
    let mut store = StoreVolume::new(&volume.dir, OpenOptions::default());
    let mut control = control;
    let detached = match lifecycle::detach(
        &mut control,
        &mut records,
        &mut store,
        LOCK_RETRIES,
        GONE_RETRIES,
        async_delete_asserted,
    ) {
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
            Exit::DeletedHeld,
            "",
            &format!(
                "deleted /dev/ublkb{n}; something still holds it open, and it keeps its number \
                 until that lets go (a pool that was not exported stays suspended)"
            ),
        ),

        Detached::DeadNoNote(n) => Output::said(
            Exit::Error,
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
        Detached::Unsupported(why) => Output::said(
            Exit::Error,
            "",
            &format!("not detached: {why}. Nothing was done"),
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
/// exists and holds none. The directory may hold nothing but what an
/// interrupted create leaves
/// ([`CREATE_LEFTOVERS`](skyzfs_local::store::CREATE_LEFTOVERS)); any
/// other file makes create refuse it and touch nothing. A file under
/// exactly one of those names is removed as a leftover, also if it was
/// an operator's: create cannot tell them apart. `boot` is this boot, if it could be read:
/// with it, a failure note of this boot under the run directory is
/// warned of on standard error (the volume is made all the same, and
/// the note stays).
///
/// On success one line: `created DIR: N bytes, volume ID, identity
/// IDENTITY`, the two in hex. The identity is the store's
/// ([`Store::identity`]): what tells this volume from another made
/// later in the same directory.
pub fn create(create: &Create, id: [u8; 16], boot: Option<[u8; 16]>) -> Output {
    let (made, write_unit) = match in_directory(&create.dir, |dir| make(create, dir, id)) {
        Ok(made) => made,
        Err(output) => return output,
    };
    let shown = create.dir.display();
    let mut warning = boot
        .and_then(|boot| create_warning(&create.dir, &create.run_dir, &boot))
        .map_or_else(String::new, |warning| format!("skyzfsd: {warning}\n"));
    if let Some(note) = write_unit_note(write_unit, Identity::system_write_unit()) {
        let _ = writeln!(warning, "skyzfsd: {shown}: {note}");
    }
    Output {
        exit: Exit::Done,
        out: format!(
            "created {shown}: {} bytes, volume {}, identity {}\n",
            create.size,
            hex(&id),
            hex(&made)
        ),
        err: warning,
    }
}

/// Run `work` in the directory `path`, made first (its last component
/// only, mode 0700) if it is missing. If `work` fails and the directory
/// was made here, what a create leaves in it is removed, by its exact
/// names ([`CREATE_LEFTOVERS`](skyzfs_local::store::CREATE_LEFTOVERS)),
/// and then the directory: a create that fails leaves nothing of a
/// directory it made. A directory that was there before is left with
/// whatever the failed create left in it; the next create removes
/// that.
pub fn in_directory<T>(
    path: &Path,
    work: impl FnOnce(FsDir) -> Result<T, Output>,
) -> Result<T, Output> {
    use std::os::unix::fs::DirBuilderExt;
    let shown = path.display();
    let made = match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Output::error(format_args!(
                "{shown}: its parent directory does not exist: create makes only the last part"
            )))
        }
        Err(e) => return Err(Output::error(format_args!("{shown}: {e}"))),
    };
    let result = match FsDir::new(path) {
        Ok(dir) => work(dir),
        Err(e) => Err(Output::error(format_args!("{shown}: {e}"))),
    };
    if result.is_err() && made {
        unmake(path);
    }
    result
}

/// Remove a directory this create made, and what a create leaves in
/// it. Under the volume's lock, and only if the directory holds no
/// volume: if another instance has the lock or made a volume here in
/// the meantime, everything is left. The directory itself goes only if
/// it is empty then, so a file of anybody else keeps it.
///
/// A second create that opened the lock file just before it is
/// unlinked here does not get its lock on the removed file: the lock
/// is on the file the name names, or it is not taken
/// (`FsDir::try_lock`). It opens the name again, which makes a new
/// lock file if the directory is still there, and then this
/// `remove_dir` fails and leaves the directory to it; or it finds the
/// directory gone and fails with "no such file".
fn unmake(path: &Path) {
    let Ok(dir) = FsDir::new(path) else {
        return;
    };
    if let Ok(lock) = Store::lock_volume(&dir) {
        if !path.join(SUPER_NAME).exists() {
            let lock_name = skyzfs_local::fs::LOCK_NAME;
            for name in skyzfs_local::store::CREATE_LEFTOVERS {
                if name != lock_name {
                    let _ = std::fs::remove_file(path.join(name));
                }
            }
            // Last, and still held: whoever opens the lock file from
            // here on finds the directory gone or makes its own.
            let _ = std::fs::remove_file(path.join(lock_name));
        }
        drop(lock);
    }
    let _ = std::fs::remove_dir(path);
}

/// The write unit of a new volume in a directory whose file system
/// says `block` for its block size, or why no volume is made there
/// (words that follow the directory's path).
fn write_unit_on(block: io::Result<u64>) -> Result<u32, String> {
    match block {
        Ok(block) => Identity::write_unit_for(block).ok_or_else(|| {
            format!(
                "is on a file system with blocks of {block} bytes: a volume's write unit must \
                 be at least the file system's block, and it is a power of two of at most \
                 65536 bytes"
            )
        }),
        Err(e) => Err(format!(
            "is on a file system whose block size could not be read ({e}): whether a write \
             there stays within a volume's write unit cannot be checked"
        )),
    }
}

/// What `create` says, on standard error, about a write unit that is
/// not the system's page size: the volume then pads its log and saves
/// neighbours by a larger unit than a volume on this system otherwise
/// would, and whoever made it should know why. Nothing for the usual
/// case, so that the line `created ...` stays all there is to read.
fn write_unit_note(unit: u32, page: u32) -> Option<String> {
    (unit != page).then(|| {
        format!(
            "the volume's write unit is {unit} bytes, the block size of the directory's file \
             system; this system's pages are {page} bytes"
        )
    })
}

/// Make the store of `create` in `dir`; its identity and the write
/// unit it was given.
fn make(create: &Create, dir: FsDir, id: [u8; 16]) -> Result<([u8; 16], u32), Output> {
    use skyzfs_local::io::Dir as _;
    let shown = create.dir.display();
    let write_unit = write_unit_on(dir.block_size())
        .map_err(|why| Output::error(format_args!("{shown} {why}. Nothing was done")))?;
    let config = StoreConfig {
        volume: id,
        size_mib: create.size >> 20,
        base: create.base_pattern.map_or(Base::Empty, Base::Pattern),
        align: ALIGN,
        segment_size: SEGMENT_SIZE,
        write_unit,
    };
    match Store::create(dir, &config, OpenOptions::default()) {
        Ok(store) => Ok((store.identity(), write_unit)),
        Err(StoreError::Log(LogError::Exists)) => Err(Output::error(format_args!(
            "{shown} holds a volume already. Nothing was done"
        ))),
        Err(StoreError::NotEmpty | StoreError::Log(LogError::NotEmpty)) => {
            Err(Output::error(format_args!(
                "{shown} holds {}, which is not left by a create: create makes a volume only \
                 in a directory that holds nothing else. Nothing was done",
                stranger(&create.dir)
            )))
        }
        Err(StoreError::Locked) => Err(Output::said(
            Exit::RefusedHeld,
            "",
            &format!("{shown} is in use by another instance. Nothing was done"),
        )),
        Err(e) => Err(Output::error(format_args!(
            "creating a volume in {shown}: {e}"
        ))),
    }
}

/// The file in `dir` that made the store refuse it, for the message.
fn stranger(dir: &Path) -> String {
    let names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let mut strangers: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| !skyzfs_local::store::CREATE_LEFTOVERS.contains(name))
        .collect();
    strangers.sort_unstable();
    match strangers.as_slice() {
        [] => "a file".to_owned(),
        [one] => format!("`{one}`"),
        [first, rest @ ..] => format!("`{first}` and {} more", rest.len()),
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
