//! The order of steps at every start of the daemon (design §4.2,
//! "attachment across restarts" and "what was acknowledged"), generic
//! over what it acts on, so that the order is tested with fakes.
//!
//! **The control interface here is a guess.** [`Control`] names the
//! steps the daemon needs of the ublk control device as this module
//! needs them. It was written before the binding to `libublk`, and the
//! binding may have to change its shape. What is not a guess is the
//! order of the steps and the conditions, which are the design's.
//!
//! # The order
//!
//! 1. The kernel must be able to reissue requests after a recovery.
//!    If it cannot, the daemon refuses, naming the feature. It never
//!    falls back to the recovery that fails requests.
//! 2. The record of what was acknowledged is read. A record that cannot
//!    be read is the worst case: the volume counts as failed while a
//!    client held it.
//! 3. **The store is opened**, with the position it must reach, before
//!    any step that touches a device. Its open takes the lock, recovers
//!    and makes the recovery durable. The store is never opened without
//!    the stored position.
//!    - Locked: tried again a few times ([`Records::pause`] between),
//!      since a lock can outlive a killed daemon for a moment; then the
//!      daemon refuses and touches no device.
//!    - The log is shorter than what was acknowledged, or the
//!      recovery's own sync failed, or the volume is marked failed: the
//!      volume has failed. A device of this volume that waits for a
//!      server is deleted, and the daemon refuses.
//!    - Any other error: the daemon stops and leaves every device
//!      alone.
//! 4. The attachment record in the volume's directory says which device
//!    was this volume's. It is taken over only if the record is from
//!    this boot, the kernel has that device waiting for a server, the
//!    note for the device in the run directory carries the record's
//!    nonce, and the kernel reports the recorded size, queues, depth
//!    and flags. The new instance then follows the device's values,
//!    whatever its own options say.
//!    - The device is live under another server: refuse, add nothing.
//!      (A live device whose note is missing or another's is not this
//!      volume's device at all: see below.)
//!    - The device was added and never started, and its note is this
//!      volume's or missing: delete it, attach afresh.
//!    - The device is gone, or the record is of another boot or absent:
//!      attach afresh.
//!    - The note is there and cannot be read: the volume counts as
//!      failed while a client held it; the device is left.
//!    - Anything else under that number (a device that waits and whose
//!      note is another's, or whose values differ from the record): it
//!      cannot be told whose it is. The daemon stops and leaves it.
//! 5. Attaching afresh: a number and a fresh nonce are **recorded
//!    durably in the volume's directory before the device is added**
//!    under that number. Numbers the kernel shows as taken are passed
//!    over first; if the add still finds the number taken, the next
//!    free one is recorded and tried. Once the add has succeeded the device
//!    is **disowned at once**, so that nothing but an explicit call
//!    deletes it, and only then is the note for the device written in
//!    the run directory, before the device is started.
//!
//! A record can name a device that is not the volume's: another
//! server's device appears under the number just recorded, the add
//! fails, and the daemon is killed before it records the next number.
//! The note tells: it is written only after this daemon's add
//! succeeded, and before the device is started. So a **live** device
//! whose note is missing or another's was never this volume's, and the
//! daemon attaches afresh and leaves it.
//!
//! Readings where the design is silent, marked here and reported: a
//! device that was added and never started is deleted when its note is
//! missing as well as when it is this volume's (a crash between the add
//! and the note leaves exactly that, and nothing else would clean it
//! up; a device that was never started has no client); and a waiting
//! device with this volume's nonce whose values differ from the record
//! stops the daemon instead of being replaced by a second device for
//! the same volume.

use std::io;

/// What a device was created with, and reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// The size in bytes.
    pub size: u64,
    /// The number of queues.
    pub queues: u16,
    /// The depth of each queue.
    pub depth: u16,
    /// The driver's flags.
    pub flags: u64,
}

/// What the kernel says a device is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    /// Added, and never started.
    Added,
    /// Served by a live server.
    Live,
    /// Its server is gone and it waits for another.
    Waiting,
}

/// A device as the kernel reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInfo {
    /// What it is doing.
    pub state: DeviceState,
    /// Its values.
    pub shape: Shape,
}

/// Why a device could not be added.
#[derive(Debug)]
pub enum AddError {
    /// The number is taken.
    Taken,
    /// Anything else.
    Other(io::Error),
}

/// The steps the daemon needs of the kernel's control interface. A
/// guess: see the [module documentation](self).
pub trait Control {
    /// Whether the driver can reissue requests after a recovery.
    fn can_reissue(&mut self) -> io::Result<bool>;
    /// The device with this number, if there is one.
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>>;
    /// Add a device under this number.
    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError>;
    /// Give up ownership of a device this instance added, so that it is
    /// deleted only by [`Control::delete`].
    fn disown(&mut self, number: u32);
    /// Delete a device.
    fn delete(&mut self, number: u32) -> io::Result<()>;
    /// Begin the recovery of a device that waits for a server.
    fn begin_recovery(&mut self, number: u32) -> io::Result<()>;
}

/// Which device is a volume's: recorded in the volume's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attachment {
    /// The device's number.
    pub number: u32,
    /// A random value that the note for the device repeats.
    pub nonce: [u8; 16],
    /// The boot the record was made in.
    pub boot: [u8; 16],
    /// What the device was added with.
    pub shape: Shape,
}

/// The position the store's log must reach, and the store it was
/// recorded for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MustReach {
    /// The store's identity.
    pub identity: [u8; 16],
    /// The position.
    pub position: u64,
}

/// The daemon's records: in the volume's directory and in the run
/// directory.
pub trait Records {
    /// This boot.
    fn boot(&self) -> [u8; 16];
    /// A fresh random nonce.
    fn nonce(&mut self) -> io::Result<[u8; 16]>;
    /// The attachment record of the volume, if it has one.
    fn attachment(&mut self) -> io::Result<Option<Attachment>>;
    /// Write the attachment record, durably.
    fn write_attachment(&mut self, attachment: &Attachment) -> io::Result<()>;
    /// The nonce in the run directory's note for a device.
    fn note(&mut self, number: u32) -> io::Result<Option<[u8; 16]>>;
    /// Write the note for a device.
    fn write_note(&mut self, number: u32, nonce: &[u8; 16]) -> io::Result<()>;
    /// What was acknowledged in this boot, if anything was recorded.
    /// An error if the record is there and cannot be read.
    fn acknowledged(&mut self) -> io::Result<Option<MustReach>>;
    /// Wait a moment before the store's lock is tried again.
    fn pause(&mut self);
}

/// Why the store could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Another instance holds it.
    Locked,
    /// Its log ends at or before what was acknowledged.
    Short,
    /// The recovery's own sync failed.
    SyncFailed,
    /// It is marked as failed.
    MarkedFailed,
    /// Anything else: it may pass.
    Other(String),
}

/// The store, as the lifecycle needs it.
pub trait Volume {
    /// The open store.
    type Open;
    /// Open it: take its lock, recover, make the recovery durable. The
    /// log must reach `must_reach`. Returns the store and its size.
    fn open(&mut self, must_reach: Option<MustReach>) -> Result<(Self::Open, u64), OpenError>;
}

/// The daemon may serve: the store is open and a device is its.
#[derive(Debug)]
pub struct Ready<S> {
    /// The open store.
    pub store: S,
    /// The device's number.
    pub number: u32,
    /// The device's values: its own if it was taken over.
    pub shape: Shape,
    /// Whether the device was taken over from an earlier instance. If
    /// not, it was added and is to be given its parameters and started.
    pub took_over: bool,
}

/// The daemon does not serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The kernel cannot reissue requests after a recovery.
    CannotReissue,
    /// Another instance holds the volume, or its device is live under
    /// another server. Nothing was changed. Exit status 5.
    Held,
    /// The volume has failed: its log is short, its recovery's sync
    /// failed, it is marked, or the record of what was acknowledged
    /// cannot be read. A waiting device of the volume was deleted.
    /// Exit status 4.
    Failed(&'static str),
    /// Something else went wrong. No device was deleted. Exit status 1.
    Error(String),
}

/// What an instance asks for when it adds a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    /// Queues.
    pub queues: u16,
    /// Depth of each.
    pub depth: u16,
    /// The driver's flags: recovery with reissue.
    pub flags: u64,
    /// How often a locked store is tried again.
    pub lock_retries: u32,
}

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// A device is taken over or added before the store is opened.
    pub(crate) const DEVICE_BEFORE_STORE: u32 = 1 << 0;
    /// A waiting device is taken over whatever its note's nonce.
    pub(crate) const ANY_NONCE: u32 = 1 << 1;
    /// A device that is live under another server is taken over.
    pub(crate) const TAKE_LIVE: u32 = 1 << 2;
    /// A device whose values differ from the record is taken over.
    pub(crate) const ANY_SHAPE: u32 = 1 << 3;
    /// The device is added before the attachment record is written.
    pub(crate) const ADD_BEFORE_RECORD: u32 = 1 << 4;
    /// The note for the device is written before the add.
    pub(crate) const NOTE_BEFORE_ADD: u32 = 1 << 5;
    /// The device is not disowned.
    pub(crate) const NOT_DISOWNED: u32 = 1 << 6;
    /// A device that is taken over is served with this instance's
    /// options in place of its own values.
    pub(crate) const OWN_OPTIONS: u32 = 1 << 7;
    /// The store is opened without the position it must reach.
    pub(crate) const NO_MUST_REACH: u32 = 1 << 8;
    /// An error of the recovery's sync is treated as one that may pass.
    pub(crate) const SYNC_ERROR_MAY_PASS: u32 = 1 << 9;
    /// A kernel that cannot reissue is served all the same.
    pub(crate) const NO_FEATURE_CHECK: u32 = 1 << 10;

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

fn error<E: std::fmt::Display>(during: &'static str) -> impl Fn(E) -> Stop {
    move |e| Stop::Error(format!("{during}: {e}"))
}

/// What the attachment record and the kernel say about the volume's
/// device.
enum Found {
    /// Take this device over.
    TakeOver(u32, Shape),
    /// Attach afresh. A device that was added and never started is
    /// deleted first.
    Afresh(Option<u32>),
}

/// Run the start: see the [module documentation](self).
pub fn start<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    wanted: &Wanted,
) -> Result<Ready<V::Open>, Stop> {
    start_with(control, records, volume, wanted, Mutants::default())
}

pub(crate) fn start_with<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<Ready<V::Open>, Stop> {
    let can = control
        .can_reissue()
        .map_err(error("reading the driver's features"))?;
    if !can && !mutants.has(Mutants::NO_FEATURE_CHECK) {
        return Err(Stop::CannotReissue);
    }
    if mutants.has(Mutants::DEVICE_BEFORE_STORE) {
        // The defect: the device first.
        let found = find(control, records, wanted, mutants)?;
        let (number, shape, took_over) = attach(control, records, found, 0, wanted, mutants)?;
        let (store, _) = open(control, records, volume, wanted, mutants)?;
        return Ok(Ready {
            store,
            number,
            shape,
            took_over,
        });
    }
    let (store, size) = open(control, records, volume, wanted, mutants)?;
    let found = find(control, records, wanted, mutants)?;
    let (number, shape, took_over) = attach(control, records, found, size, wanted, mutants)?;
    Ok(Ready {
        store,
        number,
        shape,
        took_over,
    })
}

/// Step 2 and 3: the record of what was acknowledged, then the store.
fn open<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<(V::Open, u64), Stop> {
    let must_reach = match records.acknowledged() {
        Ok(must_reach) => must_reach,
        Err(_) => {
            return Err(failed(
                control,
                records,
                "the record of what was acknowledged cannot be read",
            ))
        }
    };
    let must_reach = must_reach.filter(|_| !mutants.has(Mutants::NO_MUST_REACH));
    let mut tries = 0;
    loop {
        let why = match volume.open(must_reach) {
            Ok(opened) => return Ok(opened),
            Err(OpenError::Locked) if tries < wanted.lock_retries => {
                tries += 1;
                records.pause();
                continue;
            }
            Err(OpenError::Locked) => return Err(Stop::Held),
            Err(OpenError::Other(e)) => return Err(Stop::Error(format!("opening the store: {e}"))),
            Err(OpenError::SyncFailed) if mutants.has(Mutants::SYNC_ERROR_MAY_PASS) => {
                return Err(Stop::Error("opening the store: a sync failed".into()))
            }
            Err(OpenError::SyncFailed) => "the recovery's sync failed",
            Err(OpenError::Short) => "the log is shorter than what was acknowledged",
            Err(OpenError::MarkedFailed) => "the volume is marked as failed",
        };
        return Err(failed(control, records, why));
    }
}

/// The volume has failed: a device of it that waits for a server is
/// deleted, never taken over.
fn failed<C: Control, R: Records>(control: &mut C, records: &mut R, why: &'static str) -> Stop {
    if let Ok(Some(attachment)) = records.attachment() {
        let ours = attachment.boot == records.boot()
            && matches!(records.note(attachment.number), Ok(Some(nonce)) if nonce == attachment.nonce);
        let waiting = matches!(
            control.info(attachment.number),
            Ok(Some(DeviceInfo {
                state: DeviceState::Waiting | DeviceState::Added,
                ..
            }))
        );
        if ours && waiting {
            let _ = control.delete(attachment.number);
        }
    }
    Stop::Failed(why)
}

/// Step 4: whose is the device the record names.
fn find<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    _wanted: &Wanted,
    mutants: Mutants,
) -> Result<Found, Stop> {
    let attachment = records
        .attachment()
        .map_err(error("reading the attachment record"))?;
    let Some(attachment) = attachment.filter(|a| a.boot == records.boot()) else {
        return Ok(Found::Afresh(None));
    };
    let number = attachment.number;
    let info = control
        .info(number)
        .map_err(error("asking for the device"))?;
    let Some(info) = info else {
        return Ok(Found::Afresh(None));
    };
    // A note that is there and cannot be read cannot say whose the
    // device is: the volume counts as failed while held, and the device
    // is left, since it cannot be shown to be this volume's.
    let Ok(note) = records.note(number) else {
        return Err(Stop::Failed(
            "the note for the volume's device cannot be read",
        ));
    };
    let noted = note == Some(attachment.nonce);
    let recorded = info.shape == attachment.shape || mutants.has(Mutants::ANY_SHAPE);
    match info.state {
        // A live device without this record's note was never this
        // volume's: the note is written before a device is started.
        DeviceState::Live if !noted => Ok(Found::Afresh(None)),
        DeviceState::Live if !mutants.has(Mutants::TAKE_LIVE) => Err(Stop::Held),
        DeviceState::Live | DeviceState::Waiting
            if (noted || mutants.has(Mutants::ANY_NONCE)) && recorded =>
        {
            Ok(Found::TakeOver(number, info.shape))
        }
        DeviceState::Added if noted || note.is_none() => Ok(Found::Afresh(Some(number))),
        _ => Err(Stop::Error(format!(
            "device {number} is recorded for this volume, and its note or its values are not: \
             it is left as it is"
        ))),
    }
}

/// Step 4's end and step 5: take over, or add.
fn attach<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    found: Found,
    size: u64,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<(u32, Shape, bool), Stop> {
    let stale = match found {
        Found::TakeOver(number, shape) => {
            control
                .begin_recovery(number)
                .map_err(error("beginning the recovery"))?;
            let shape = if mutants.has(Mutants::OWN_OPTIONS) {
                Shape {
                    queues: wanted.queues,
                    depth: wanted.depth,
                    ..shape
                }
            } else {
                shape
            };
            return Ok((number, shape, true));
        }
        Found::Afresh(stale) => stale,
    };
    if let Some(number) = stale {
        control
            .delete(number)
            .map_err(error("deleting a device that was never started"))?;
    }
    let shape = Shape {
        size,
        queues: wanted.queues,
        depth: wanted.depth,
        flags: wanted.flags,
    };
    let nonce = records.nonce().map_err(error("making a nonce"))?;
    let boot = records.boot();
    for number in 0..=u32::from(u16::MAX) {
        let attachment = Attachment {
            number,
            nonce,
            boot,
            shape,
        };
        // A number that is seen to be taken is not recorded: a record
        // names another's device only if that device appeared between
        // this look and the add, and the daemon was killed before it
        // recorded the next number.
        let seen = control.info(number).map_err(error("asking for a device"))?;
        if seen.is_some() {
            continue;
        }
        let early = mutants.has(Mutants::ADD_BEFORE_RECORD);
        if !early {
            records
                .write_attachment(&attachment)
                .map_err(error("writing the attachment record"))?;
        }
        if mutants.has(Mutants::NOTE_BEFORE_ADD) {
            let _ = records.write_note(number, &nonce);
        }
        match control.add(number, &shape) {
            Ok(()) => {}
            Err(AddError::Taken) => continue,
            Err(AddError::Other(e)) => return Err(Stop::Error(format!("adding a device: {e}"))),
        }
        if !mutants.has(Mutants::NOT_DISOWNED) {
            control.disown(number);
        }
        if early {
            records
                .write_attachment(&attachment)
                .map_err(error("writing the attachment record"))?;
        }
        records
            .write_note(number, &nonce)
            .map_err(error("writing the device's note"))?;
        return Ok((number, shape, false));
    }
    Err(Stop::Error("every device number is taken".into()))
}

#[cfg(test)]
mod tests;
