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
//! 2. **The volume's lock is taken**, as a step of its own and without
//!    a recovery ([`Volume::lock`]). Everything that changes the
//!    volume's attachment is done under it: its record, its notes,
//!    adding and deleting its device. It is held until the start has
//!    finished, also when the start fails, and by the store from then
//!    on.
//!    - Locked: tried again a few times ([`Records::pause`] between),
//!      since a lock can outlive a killed daemon for a moment; then the
//!      daemon refuses and touches nothing.
//! 3. The record of what was acknowledged is read, **under the lock**:
//!    read before it, the position could be older than what an
//!    instance that was still serving went on to acknowledge. A record
//!    that cannot be read is the worst case: the volume counts as
//!    failed while a client held it. Then **the store is opened**
//!    ([`Volume::open`]) with the position it must reach, before any
//!    step that touches a device: it recovers and makes the recovery
//!    durable. The store is never opened without the stored position.
//!    - The log is shorter than what was acknowledged, or the
//!      recovery's own sync failed, or the volume is marked failed: the
//!      volume has failed. A device of this volume that waits for a
//!      server is deleted, still under the lock, and the daemon
//!      refuses.
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
//!    | The device | Its note is this volume's | Another volume's note, or no note and other values than recorded | No note, and the recorded values |
//!    |---|---|---|---|
//!    | waits for a server | taken over; if its values are not the recorded ones, the daemon stops and leaves it | left; attach afresh under another number | the daemon stops and leaves it |
//!    | is live | waited for, for a stated time (below); if it stays live: refused, nothing added | left; attach afresh under another number | the daemon stops and leaves it |
//!    | was added and never started | deleted; attach afresh | left; attach afresh under another number | left; attach afresh under another number |
//!    | is deleted and still held | attach afresh | attach afresh | attach afresh |
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
//! A record alone does not tell: another server's device can appear
//! under the number just recorded, and the daemon can be killed before
//! it records the next. The note tells first, the kernel's values
//! second.
//!
//! - **A whole note with the record's nonce: this volume's.** The note
//!   is written only after this daemon's add succeeded, and before the
//!   device is started.
//! - **A whole note with another nonce: another volume's**, in every
//!   state. It is left, and the daemon attaches under another number.
//! - **No note, and the kernel reports other values than the record:
//!   not this volume's**, in every state. A device is added with
//!   exactly the recorded size, queues, depth and flags, they do not
//!   change afterwards, and the record is rewritten only before an add
//!   on a number that looked free.
//! - **No note, and the recorded values: it cannot be told.** A device
//!   that was added and never started is this volume's from a kill
//!   between the add and the note, or another program's twin between
//!   its add and its start; it never had a client, so it is left and
//!   the daemon attaches under another number. The cost is one leaked
//!   number for a kill in a window of milliseconds, until an operator
//!   deletes it or the machine reboots. A device that is live or waits
//!   for a server is this volume's whose note was lost with the run
//!   directory, or a stranger's twin: the daemon stops, touches
//!   nothing, and says so. Deleting the device by hand, or a reboot,
//!   resolves it.
//!
//! Whenever a device is left and another number taken, [`Ready::left`]
//! says which and why, for the daemon's log.
//!
//! # A note does not outlive its device
//!
//! The note is removed **before** the device is deleted, wherever the
//! daemon deletes one. A note that stayed would, together with a record
//! that still names the number, make this volume's device of whatever
//! program is given the number next. A kill between the two leaves
//! this volume's device without its note, which is the case that
//! cannot be told, above: a device that was never started is left, and
//! one that waits for a server stops the next start until it is deleted
//! by hand.

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
///
/// **Not settled: the flags.** The lifecycle compares the
/// [`Shape::flags`] the kernel reports with the flags in the record,
/// which is written before the add with the flags that were asked for.
/// The kernel may report other flags for a device than the add asked
/// for (it may add or mask bits). The binding has to make the two
/// comparable: either [`Control::info`] reports the flags under a mask
/// of the bits the daemon sets, or the record is completed after the
/// add. The fake here reports what was asked for. The spike decides.
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
    /// Remove the volume's attachment record, if there is one, durably.
    fn remove_attachment(&mut self) -> io::Result<()>;
    /// Remove the note for a device, if there is one.
    fn remove_note(&mut self, number: u32) -> io::Result<()>;
    /// What was acknowledged in this boot, if anything was recorded.
    /// An error if the record is there and cannot be read.
    fn acknowledged(&mut self) -> io::Result<Option<MustReach>>;
    /// Wait a moment before the store's lock is tried again.
    fn pause(&mut self);
}

/// Why the volume's lock could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// Another instance holds it.
    Locked,
    /// Anything else.
    Other(String),
}

/// Why the store could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
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
    /// The volume's lock, held. Dropping it releases the lock.
    type Lock;
    /// The open store. It holds the lock for as long as it lives.
    type Open;
    /// Take the volume's lock, without waiting and without a recovery:
    /// nothing is read or written.
    fn lock(&mut self) -> Result<Self::Lock, LockError>;
    /// Open the store under `lock`: recover, make the recovery
    /// durable. The log must reach `must_reach`. Returns the store,
    /// which takes the lock over, and its size. If the open fails the
    /// lock comes back with the error, still held.
    fn open(
        &mut self,
        lock: Self::Lock,
        must_reach: Option<MustReach>,
    ) -> Result<(Self::Open, u64), (OpenError, Self::Lock)>;
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
    /// Another instance holds the volume's lock, or its device is live
    /// under another server. Nothing was changed. Exit status 5.
    Held,
    /// The volume has failed: its log is short, its recovery's sync
    /// failed, it is marked, or the record of what was acknowledged
    /// cannot be read. A waiting device of the volume was deleted.
    /// Exit status 4.
    Failed(String),
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
    /// A device without a note is taken for a stranger's whatever its
    /// values.
    pub(crate) const NO_NOTE_IS_A_STRANGER: u32 = 1 << 12;
    /// A device is deleted before its note is removed.
    pub(crate) const DELETE_BEFORE_NOTE: u32 = 1 << 13;
    /// A detach acts without the volume's lock.
    pub(crate) const DETACH_WITHOUT_LOCK: u32 = 1 << 14;
    /// The acknowledged position is read before the lock is taken.
    pub(crate) const POSITION_BEFORE_LOCK: u32 = 1 << 15;
    /// A start that fails lets go of the lock before it deletes.
    pub(crate) const UNLOCK_BEFORE_FAILED: u32 = 1 << 16;
    /// A failed volume deletes by a record of another boot.
    pub(crate) const FAILED_ANY_BOOT: u32 = 1 << 17;
    /// A noted device that is deleted and still held is taken over.
    pub(crate) const TAKE_DEAD: u32 = 1 << 18;

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

/// Take the volume's lock, trying again while another instance has it.
fn lock<R: Records, V: Volume>(
    records: &mut R,
    volume: &mut V,
    retries: u32,
) -> Result<V::Lock, Stop> {
    let mut tries = 0;
    loop {
        match volume.lock() {
            Ok(lock) => return Ok(lock),
            Err(LockError::Locked) if tries < retries => {
                tries += 1;
                records.pause();
            }
            Err(LockError::Locked) => return Err(Stop::Held),
            Err(LockError::Other(e)) => {
                return Err(Stop::Error(format!("locking the volume: {e}")))
            }
        }
    }
}

/// Steps 2 and 3: the lock, the record of what was acknowledged, then
/// the store.
fn open<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<(V::Open, u64), Stop> {
    let early = mutants
        .has(Mutants::POSITION_BEFORE_LOCK)
        .then(|| records.acknowledged());
    let held = lock(records, volume, wanted.lock_retries)?;
    let must_reach = match early.unwrap_or_else(|| records.acknowledged()) {
        Ok(must_reach) => must_reach,
        Err(_) => {
            let why = "the record of what was acknowledged cannot be read";
            let stop = failed(control, records, wanted, mutants, why);
            drop(held);
            return Err(stop);
        }
    };
    let must_reach = must_reach.filter(|_| !mutants.has(Mutants::NO_MUST_REACH));
    let (why, held) = match volume.open(held, must_reach) {
        Ok(opened) => return Ok(opened),
        Err((OpenError::Other(e), _)) => {
            return Err(Stop::Error(format!("opening the store: {e}")))
        }
        Err((OpenError::SyncFailed, _)) if mutants.has(Mutants::SYNC_ERROR_MAY_PASS) => {
            return Err(Stop::Error("opening the store: a sync failed".into()))
        }
        Err((OpenError::SyncFailed, held)) => ("the recovery's sync failed", held),
        Err((OpenError::Short, held)) => ("the log is shorter than what was acknowledged", held),
        Err((OpenError::MarkedFailed, held)) => ("the volume is marked as failed", held),
    };
    // The lock is held while the failed volume's device is deleted: a
    // second start cannot take the device over in between.
    let held = (!mutants.has(Mutants::UNLOCK_BEFORE_FAILED)).then_some(held);
    let stop = failed(control, records, wanted, mutants, why);
    drop(held);
    Err(stop)
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

/// Delete this volume's device: its note first, then the device. A
/// note exists only while its device may exist.
fn delete<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
    mutants: Mutants,
) -> io::Result<()> {
    if mutants.has(Mutants::DELETE_BEFORE_NOTE) {
        control.delete(number)?;
        return records.remove_note(number);
    }
    records.remove_note(number)?;
    control.delete(number)
}

/// The volume has failed: a device of it that waits for a server is
/// deleted, never taken over. The delete does not wait: a device that
/// a client still holds keeps its number, deleted. Called with the
/// volume's lock held.
///
/// The device is deleted only if the record is of this boot, the note
/// carries the record's nonce, and the kernel reports the recorded
/// values. A noted device with other values is left, and the reason
/// says so: a note should never outlive its device, and if one did (a
/// device deleted by hand), what stands under the number now is not
/// this volume's.
fn failed<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    wanted: &Wanted,
    mutants: Mutants,
    why: &'static str,
) -> Stop {
    let Ok(Some(attachment)) = records.attachment() else {
        return Stop::Failed(why.into());
    };
    let number = attachment.number;
    let this_boot = attachment.boot == records.boot() || mutants.has(Mutants::FAILED_ANY_BOOT);
    let noted = matches!(records.note(number), Ok(Some(nonce)) if nonce == attachment.nonce);
    if !(this_boot && noted) {
        return Stop::Failed(why.into());
    }
    let info = control
        .info(number)
        .and_then(|info| settled(control, records, number, info, wanted, Mutants::default()));
    let Ok(Some(info)) = info else {
        return Stop::Failed(why.into());
    };
    if !matches!(info.state, DeviceState::Waiting | DeviceState::Added) {
        return Stop::Failed(why.into());
    }
    if info.shape != attachment.shape {
        return Stop::Failed(format!(
            "{why}; device {number} carries this volume's note and not the recorded values, \
             and was left"
        ));
    }
    let _ = delete(control, records, number, mutants);
    Stop::Failed(why.into())
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
            "the note for the volume's device cannot be read".into(),
        ));
    };
    if note != Some(attachment.nonce) {
        let left = |why| {
            Ok(Found::Afresh {
                stale: None,
                left: Some((number, why)),
            })
        };
        if info.state == DeviceState::Dead {
            return Ok(afresh);
        }
        if note.is_some() {
            if info.state == DeviceState::Waiting && mutants.has(Mutants::ANY_NONCE) {
                return Ok(Found::TakeOver(number, info.shape));
            }
            // A note is written by a daemon after its own add: this is
            // another volume's device, in whatever state.
            return left("its note is another volume's");
        }
        // No note. A device is added with exactly the recorded values,
        // and they do not change afterwards.
        if info.shape != attachment.shape || mutants.has(Mutants::NO_NOTE_IS_A_STRANGER) {
            return left("it has no note and its values are not the recorded ones");
        }
        return match info.state {
            // This volume's, from a kill between the add and the note,
            // or another's twin that is being set up: it never had a
            // client.
            DeviceState::Added => left("it was added and never started, and has no note"),
            _ => Err(Stop::Error(format!(
                "device {number} has this volume's size, queues, depth and flags and no note: \
                 it is this volume's device whose note was lost with the run directory, or \
                 another's with the same values. It is left as it is. Deleting it by hand, or \
                 a reboot, resolves it"
            ))),
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
        DeviceState::Dead if mutants.has(Mutants::TAKE_DEAD) => {
            Ok(Found::TakeOver(number, info.shape))
        }
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
            // The device's size is the store's, or it is not the
            // device of this store.
            if shape.size != size {
                return Err(Stop::Error(format!(
                    "device {number} is {} bytes and the store is {size}: it is left as it is",
                    shape.size
                )));
            }
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
        delete(control, records, number, mutants)
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

/// Whose a device under a volume's recorded number is: see "whose
/// device it is" in the [module documentation](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    /// This volume's: its note carries the record's nonce.
    Ours,
    /// Not this volume's: another volume's note, or no note and other
    /// values than the record.
    Anothers,
    /// It cannot be told: no note, and the recorded values.
    Unsure,
}

/// The device a volume's record names, as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    /// Its number.
    pub number: u32,
    /// What the kernel says it is doing.
    pub state: DeviceState,
    /// The values the kernel reports.
    pub shape: Shape,
    /// Whose it is.
    pub whose: Whose,
}

/// Find the device the volume's record names: for `status` and
/// `detach`. Opens no store, takes no lock, changes nothing and does
/// not wait.
/// `Ok(None)` if there is no record of this boot or the kernel has no
/// device under the recorded number. An error if a record or the note
/// cannot be read.
pub fn locate<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
) -> io::Result<Option<Located>> {
    let Some(attachment) = records.attachment()?.filter(|a| a.boot == records.boot()) else {
        return Ok(None);
    };
    let number = attachment.number;
    let Some(info) = control.info(number)? else {
        return Ok(None);
    };
    let whose = match records.note(number)? {
        Some(nonce) if nonce == attachment.nonce => Whose::Ours,
        Some(_) => Whose::Anothers,
        None if info.shape != attachment.shape => Whose::Anothers,
        None => Whose::Unsure,
    };
    Ok(Some(Located {
        number,
        state: info.state,
        shape: info.shape,
        whose,
    }))
}

/// What a detach did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detached {
    /// The volume has no device.
    Nothing,
    /// The device was deleted and its number is free.
    Deleted(u32),
    /// The device was deleted and something still holds it open: it
    /// keeps its number until that closes it.
    DeletedHeld(u32),
    /// The device had been deleted before and is still held. Nothing
    /// was done.
    Held(u32),
    /// The device is live under a server. Nothing was done.
    Live(u32),
    /// The device under the recorded number is not known to be this
    /// volume's. Nothing was done.
    NotOurs(Located),
    /// A daemon holds this volume: its lock could not be taken.
    /// Nothing was done.
    Locked,
}

/// Give up the volume's device: delete it if it is this volume's and
/// no server serves it. It never stops a daemon. It opens no store,
/// recovers nothing and does not touch what was acknowledged.
///
/// **It holds the volume's lock from its first look to its last step**
/// ([`Volume::lock`], tried again `lock_retries` times), and answers
/// [`Detached::Locked`] if a daemon has it. Without the lock a daemon
/// that starts in between has its device deleted under it, or its
/// fresh record removed.
///
/// The steps, for this volume's device that waits for a server or was
/// never started: the note is removed, the device is deleted without
/// waiting, the device is looked at once more, and the attachment
/// record is removed. If the device is still there at that look, the
/// answer says that its number is still held. A kill between any two
/// steps leaves a state the next start or the next detach treats
/// safely.
///
/// A device that is live is refused. One that was deleted before and is
/// still held is reported. One that cannot be shown to be this
/// volume's is left, and the answer says which number.
///
/// Not decided here, and on the list for the kernel spike: whether a
/// detach can know beforehand that a client holds a waiting device.
pub fn detach<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    lock_retries: u32,
) -> io::Result<Detached> {
    detach_with(control, records, volume, lock_retries, Mutants::default())
}

pub(crate) fn detach_with<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    lock_retries: u32,
    mutants: Mutants,
) -> io::Result<Detached> {
    let _held = if mutants.has(Mutants::DETACH_WITHOUT_LOCK) {
        None
    } else {
        match lock(records, volume, lock_retries) {
            Ok(held) => Some(held),
            Err(Stop::Held) => return Ok(Detached::Locked),
            Err(other) => return Err(io::Error::other(format!("{other:?}"))),
        }
    };
    let Some(located) = locate(control, records)? else {
        return Ok(Detached::Nothing);
    };
    let number = located.number;
    if located.whose != Whose::Ours {
        return Ok(Detached::NotOurs(located));
    }
    match located.state {
        DeviceState::Live => Ok(Detached::Live(number)),
        DeviceState::Dead => Ok(Detached::Held(number)),
        DeviceState::Waiting | DeviceState::Added => {
            delete(control, records, number, mutants)?;
            let after = control.info(number)?;
            records.remove_attachment()?;
            Ok(match after {
                None => Detached::Deleted(number),
                Some(_) => Detached::DeletedHeld(number),
            })
        }
    }
}

#[cfg(test)]
mod tests;
