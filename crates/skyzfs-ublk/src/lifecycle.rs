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
//!    whatever its own options say. Otherwise, for a record of this
//!    boot whose device the kernel has:
//!
//!    | The device | Its note is this volume's | Its note is missing or another's |
//!    |---|---|---|
//!    | waits for a server | taken over; if its values are not the recorded ones, the daemon stops and leaves it | the daemon stops and leaves it: it may have this volume's client on it |
//!    | is live | waited for, for a stated time (below); if it stays live: refused, nothing added | left; attach afresh |
//!    | was added and never started | deleted; attach afresh | left; attach afresh under another number |
//!    | is deleted and still held | attach afresh | attach afresh |
//!
//!    - The device is gone, or the record is of another boot or absent:
//!      attach afresh.
//!    - The note is there and cannot be read: the volume counts as
//!      failed while a client held it; the device is left.
//! 5. Attaching afresh: a number and a fresh nonce are **recorded
//!    durably in the volume's directory before the device is added**
//!    under that number. Numbers the kernel shows as taken are passed
//!    over first; if the add still finds the number taken, the next
//!    free one is recorded and tried. Once the add has succeeded the
//!    device is **disowned at once**, so that nothing but an explicit
//!    call deletes it, and only then is the note for the device written
//!    in the run directory, before the device is started.
//!
//! # A live device is waited for
//!
//! A server that was killed lets go of the store's lock when its
//! descriptors are closed, and the kernel may see the device's server
//! as gone only a moment later. So an instance that holds the lock and
//! finds the volume's own device live asks again, up to
//! [`Wanted::live_retries`] times with [`Records::pause`] between,
//! before it concludes anything. (That the device can still be live
//! then is read from how io_uring tears down, not yet measured.)
//!
//! # A delete does not wait
//!
//! [`Control::delete`] is the kernel's asynchronous delete, and nothing
//! here waits for the number to become free. The waiting delete returns
//! only when the last opener has closed the device, and a suspended
//! pool never closes it: a daemon that waited would hang on its way
//! out, holding the store's lock. A device that was deleted and is
//! still held keeps its number ([`DeviceState::Dead`]): it is never
//! taken over, and an add passes its number by.
//!
//! # Whose device it is
//!
//! The note tells: it is written only after this daemon's add
//! succeeded, and before the device is started. A record alone does
//! not: another server's device can appear under the number just
//! recorded, and the daemon can be killed before it records the next.
//!
//! - A **live** device without the record's note was never this
//!   volume's. It is left.
//! - A device that was **added and never started**, without the
//!   record's note, is this volume's from a kill between the add and
//!   the note, or another program's that is between its own add and
//!   start. It cannot be told, so it is left, and the daemon attaches
//!   under another number and says which number it left and why
//!   ([`Ready::left`]). A device that was never started has never had a
//!   client, so this cannot give the volume two devices. The cost is
//!   one leaked number for a kill in a window of milliseconds, until an
//!   operator deletes it or the machine reboots.
//! - A device that **waits for a server** without the record's note may
//!   still have this volume's client on it (if the run directory was
//!   emptied). The daemon stops and leaves it.

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
    /// Deleted, and still held by whoever has it open: it has no disk
    /// and no server, and keeps its number until the last opener closes
    /// it.
    Dead,
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
    /// Delete a device, without waiting: the kernel's asynchronous
    /// delete. A device that is still held keeps its number, as
    /// [`DeviceState::Dead`].
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
    /// A device under the number the volume's record named that was
    /// left alone because it could not be shown to be this volume's,
    /// and why: for the daemon's log.
    pub left: Option<(u32, &'static str)>,
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
    /// How often the volume's own device is asked for again while it is
    /// live.
    pub live_retries: u32,
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
    /// The volume's own device, found live, is refused at once.
    pub(crate) const LIVE_REFUSED_AT_ONCE: u32 = 1 << 11;

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
    /// Attach afresh.
    Afresh {
        /// This volume's device that was added and never started: it is
        /// deleted first.
        stale: Option<u32>,
        /// A device that is left alone, and why.
        left: Option<(u32, &'static str)>,
    },
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
        let attached = attach(control, records, found, 0, wanted, mutants)?;
        let (store, _) = open(control, records, volume, wanted, mutants)?;
        return Ok(ready(store, attached));
    }
    let (store, size) = open(control, records, volume, wanted, mutants)?;
    let found = find(control, records, wanted, mutants)?;
    let attached = attach(control, records, found, size, wanted, mutants)?;
    Ok(ready(store, attached))
}

fn ready<S>(store: S, attached: Attached) -> Ready<S> {
    Ready {
        store,
        number: attached.number,
        shape: attached.shape,
        took_over: attached.took_over,
        left: attached.left,
    }
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
                wanted,
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
        return Err(failed(control, records, wanted, why));
    }
}

/// The device `number`, once it is no longer live, or still live after
/// the stated time. A server that was killed has let go of the store's
/// lock before the kernel has seen all of it go, so the next instance
/// can hold the lock and still find the device live for a moment.
fn settled<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
    mut info: Option<DeviceInfo>,
    wanted: &Wanted,
    mutants: Mutants,
) -> io::Result<Option<DeviceInfo>> {
    if mutants.has(Mutants::LIVE_REFUSED_AT_ONCE) {
        return Ok(info);
    }
    for _ in 0..wanted.live_retries {
        if !info.is_some_and(|i| i.state == DeviceState::Live) {
            break;
        }
        records.pause();
        info = control.info(number)?;
    }
    Ok(info)
}

/// The volume has failed: a device of it that waits for a server is
/// deleted, never taken over. The delete does not wait: a device that
/// a client still holds keeps its number, deleted.
fn failed<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    wanted: &Wanted,
    why: &'static str,
) -> Stop {
    if let Ok(Some(attachment)) = records.attachment() {
        let ours = attachment.boot == records.boot()
            && matches!(records.note(attachment.number), Ok(Some(nonce)) if nonce == attachment.nonce);
        if ours {
            let number = attachment.number;
            let info = control.info(number).and_then(|info| {
                settled(control, records, number, info, wanted, Mutants::default())
            });
            if let Ok(Some(DeviceInfo {
                state: DeviceState::Waiting | DeviceState::Added,
                ..
            })) = info
            {
                let _ = control.delete(attachment.number);
            }
        }
    }
    Stop::Failed(why)
}

/// Step 4: whose is the device the record names.
fn find<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<Found, Stop> {
    let afresh = Found::Afresh {
        stale: None,
        left: None,
    };
    let attachment = records
        .attachment()
        .map_err(error("reading the attachment record"))?;
    let Some(attachment) = attachment.filter(|a| a.boot == records.boot()) else {
        return Ok(afresh);
    };
    let number = attachment.number;
    let info = control
        .info(number)
        .map_err(error("asking for the device"))?;
    let Some(info) = info else {
        return Ok(afresh);
    };
    // A note that is there and cannot be read cannot say whose the
    // device is: the volume counts as failed while held, and the device
    // is left, since it cannot be shown to be this volume's.
    let Ok(note) = records.note(number) else {
        return Err(Stop::Failed(
            "the note for the volume's device cannot be read",
        ));
    };
    if note != Some(attachment.nonce) {
        let left = |why| {
            Ok(Found::Afresh {
                stale: None,
                left: Some((number, why)),
            })
        };
        return match info.state {
            DeviceState::Waiting if mutants.has(Mutants::ANY_NONCE) => {
                Ok(Found::TakeOver(number, info.shape))
            }
            // It may have this volume's client on it.
            DeviceState::Waiting => Err(Stop::Error(format!(
                "device {number} is recorded for this volume and waits for a server, and its \
                 note is not this volume's: it is left as it is"
            ))),
            // The note is written before a device is started: this one
            // was never this volume's.
            DeviceState::Live => left("it is live and its note is not this volume's"),
            // This volume's, from a kill between the add and the note,
            // or another's that is being set up: it cannot be told, and
            // a device that was never started has no client.
            DeviceState::Added => {
                left("it was added and never started, and its note is not this volume's")
            }
            DeviceState::Dead => Ok(afresh),
        };
    }
    let info = if mutants.has(Mutants::TAKE_LIVE) {
        Some(info)
    } else {
        settled(control, records, number, Some(info), wanted, mutants)
            .map_err(error("asking for the device"))?
    };
    let Some(info) = info else {
        return Ok(afresh);
    };
    let recorded = info.shape == attachment.shape || mutants.has(Mutants::ANY_SHAPE);
    match info.state {
        DeviceState::Live if !mutants.has(Mutants::TAKE_LIVE) => Err(Stop::Held),
        DeviceState::Live | DeviceState::Waiting if recorded => {
            Ok(Found::TakeOver(number, info.shape))
        }
        DeviceState::Live | DeviceState::Waiting => Err(Stop::Error(format!(
            "device {number} is this volume's, and its values are not the recorded ones: it is \
             left as it is"
        ))),
        DeviceState::Added => Ok(Found::Afresh {
            stale: Some(number),
            left: None,
        }),
        // Deleted, and still held by whoever had it open: gone, as far
        // as a server is concerned.
        DeviceState::Dead => Ok(afresh),
    }
}

/// What step 5 gives.
struct Attached {
    number: u32,
    shape: Shape,
    took_over: bool,
    left: Option<(u32, &'static str)>,
}

/// Step 4's end and step 5: take over, or add.
fn attach<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    found: Found,
    size: u64,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<Attached, Stop> {
    let (stale, left) = match found {
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
            return Ok(Attached {
                number,
                shape,
                took_over: true,
                left: None,
            });
        }
        Found::Afresh { stale, left } => (stale, left),
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
        // recorded the next number. A deleted device that is still held
        // keeps its number, and is passed over here like any other.
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
        return Ok(Attached {
            number,
            shape,
            took_over: false,
            left,
        });
    }
    Err(Stop::Error("every device number is taken".into()))
}

#[cfg(test)]
mod tests;
