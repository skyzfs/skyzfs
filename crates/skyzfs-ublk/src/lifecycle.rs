//! The order of steps at every start of the daemon (design §4.2,
//! "attachment across restarts" and "what was acknowledged"), generic
//! over what it acts on, so that the order is tested with fakes.
//!
//! [`Control`] names the steps the daemon needs of the ublk control
//! device as this module needs them. It was written before the binding
//! and changed by what the kernel spike showed
//! (`docs/skyzfs-ublk-spike.md`); the binding that implements it over
//! the driver is the crate `skyzfs-ublk-sys`. The order of the steps
//! and the conditions are the design's.
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
//!    | has no server and waits for none: never started, stopped, or deleted and still held | deleted (again, if it was); attach afresh, passing its number by while it is held | left; attach afresh under another number | left; attach afresh under another number |
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
//! # The failure note
//!
//! A volume whose log failed says so in a note in its directory under
//! the run directory ([`Failure`]). The note says whether a client held
//! the volume's device at the failure; when that cannot be told, it
//! says that one did.
//!
//! - **It is read under the lock, before the acknowledged position and
//!   before the store is opened.** A note of another boot counts as
//!   none. A note that cannot be read counts as "failed while held".
//! - **Failed while held: not served again in this boot**, whatever
//!   `--acknowledge-failure` says. The store is not opened. A device of
//!   this volume that still waits (the daemon died before it deleted
//!   it) is deleted now.
//! - **Failed, and no client held it:** without the acknowledgement
//!   the start refuses and touches nothing the decision rests on: the
//!   note, the stored position and the marker stay. A waiting device
//!   of this volume is deleted all the same and the refusal says so
//!   (since M1.3d; one rule for every finder: whoever holds the lock
//!   and finds the failed volume's device waiting deletes it). With
//!   the acknowledgement, a waiting device
//!   of this volume is deleted and never taken over, the store is
//!   opened **without** a position to reach (the acknowledgement is
//!   the operator saying that what was acknowledged to nobody may be
//!   gone), then the stored position is emptied, then the note is
//!   removed. A kill anywhere in that leaves the note, and the next
//!   start asks again.
//! - **A failure found by a start** (the log is short, the recovery
//!   failed the log, a record cannot be read, the log's own marker is
//!   found): the failure note is written **first**, then the device's
//!   note is removed, then the device is deleted. If the failure note
//!   cannot be written the device is deleted all the same, and the
//!   reason says so. Whether a client held the device is decided by
//!   the attachment record: held if the record is of this boot or
//!   cannot be read, not held if there is none of this boot, since
//!   then no client of this boot can have a device of this volume.
//! - **The log's marker and no note of this boot**, with no record of
//!   this boot: no note is written (the marker is the memory); the
//!   start refuses without the acknowledgement and opens with it.
//! - **A failure while serving** ([`fail_serving`]): whether a client
//!   holds the device is the kernel's to say ([`Control::client_holds`]),
//!   and the order is the same: failure note, device's note, delete.
//!   The kernel can say that an exclusive holder exists and cannot say
//!   that none does, so the note then says **unknown**
//!   ([`Held::Unknown`]). The next start settles it by what the driver
//!   says about the recorded number: free, or another's, not held;
//!   this volume's deleted device still there, held. It writes the
//!   answer into the note and goes on as for that answer.
//! - **A daemon killed between removing the device's note and the
//!   delete** leaves a device with the recorded values and no note. No
//!   later start can show it to be this volume's (the delete may have
//!   been done and the number given out again), so none deletes it: a
//!   start that refuses for the failure leaves it and says so, and a
//!   start that would serve stops with the error of "whose device it
//!   is". It goes when someone deletes it by hand, or at a reboot.
//! - The identity in the note gates nothing: the note is honoured
//!   whatever store it names. A detach does not touch it.
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
//! The driver has one state for a device without a server that does
//! not wait for one: added and never started, stopped, or deleted and
//! still held (seen in the kernel spike, `docs/skyzfs-ublk-spike.md`).
//! The lifecycle does not tell them apart either. Such a device with
//! this volume's note is deleted, which does no harm if it was deleted
//! before, and the daemon attaches afresh, passing over numbers that
//! are taken. Without this volume's note it is left.
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
//!   The same has a second cost. A note is removed before its device
//!   is deleted, so a daemon killed between the two leaves this
//!   volume's device that waits for a server with no note. The kernel
//!   keeps no identity for a device, so nothing written before the
//!   delete would let a later start tell that device from a twin that
//!   got the freed number: the device stays until an operator deletes
//!   it or the machine reboots, and until then a start that would
//!   serve stops as above. (See also "the failure note".)
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
    /// Served by a live server.
    Live,
    /// Its server is gone and it waits for another.
    Waiting,
    /// It has no server and waits for none: added and never started,
    /// stopped, or deleted and still held by whoever has it open (it
    /// then keeps its number until the last opener closes it). The
    /// driver reports one state for all of them.
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

/// Why a volume failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The log failed while the volume was served.
    LogFailed,
    /// A start found the log shorter than what was acknowledged.
    Short,
    /// The recovery of a start failed the log.
    RecoveryFailed,
    /// A start found the log marked as failed.
    Marked,
    /// A start could not read the record of what was acknowledged.
    PositionUnreadable,
    /// A start could not read the note for the volume's device.
    DeviceNoteUnreadable,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Reason::LogFailed => "the log failed while the volume was served",
            Reason::Short => "the log is shorter than what was acknowledged",
            Reason::RecoveryFailed => "the recovery's sync failed",
            Reason::Marked => "the volume is marked as failed",
            Reason::PositionUnreadable => "the record of what was acknowledged cannot be read",
            Reason::DeviceNoteUnreadable => "the note for the volume's device cannot be read",
        })
    }
}

/// Whether a client held the volume's device when the volume failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// No client can have held it.
    No,
    /// One did, or it has to be taken that one did.
    Yes,
    /// It could not be told when the failure was noted. A later start
    /// settles it by what the driver then says about the recorded
    /// number, and writes the answer into the note.
    Unknown,
}

impl From<bool> for Held {
    fn from(held: bool) -> Self {
        if held {
            Held::Yes
        } else {
            Held::No
        }
    }
}

/// What the failure note says: see "the failure note" in the
/// [module documentation](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    /// Whether a client held the volume's device at the failure.
    pub held: Held,
    /// Why the volume failed.
    pub reason: Reason,
    /// The store's identity, or zeros where it was not known.
    pub identity: [u8; 16],
}

/// The steps the daemon needs of the kernel's control interface.
///
/// **The flags.** The lifecycle compares the [`Shape::flags`] the
/// kernel reports with the flags in the record, which is written
/// before the add with the flags that were asked for. The driver
/// reports more bits for a device than the add asked for. So
/// [`Control::info`] reports the flags under the mask of the bits the
/// daemon asks for, and the record holds what was asked. The fake in
/// the tests reports what was asked for.
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
    /// Whether a client holds the device open, without doing I/O on
    /// it. `None` when that cannot be told, which is written down as
    /// unknown. The kernel spike showed what can be asked: an exclusive
    /// open is refused while another exclusive holder exists, and
    /// succeeds with no holder and with a plain one. So the real answer
    /// is `Some(true)` or `None`, never `Some(false)`.
    fn client_holds(&mut self, number: u32) -> Option<bool>;
}

/// A borrowed control interface is one: so that a caller can hold the
/// kernel's as a trait object (`&mut dyn Control`) and pass it on.
impl<C: Control + ?Sized> Control for &mut C {
    fn can_reissue(&mut self) -> io::Result<bool> {
        (**self).can_reissue()
    }
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        (**self).info(number)
    }
    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError> {
        (**self).add(number, shape)
    }
    fn disown(&mut self, number: u32) {
        (**self).disown(number);
    }
    fn delete(&mut self, number: u32) -> io::Result<()> {
        (**self).delete(number)
    }
    fn begin_recovery(&mut self, number: u32) -> io::Result<()> {
        (**self).begin_recovery(number)
    }
    fn client_holds(&mut self, number: u32) -> Option<bool> {
        (**self).client_holds(number)
    }
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
    /// Empty the record of what was acknowledged: after a failure of a
    /// volume that no client held was acknowledged.
    fn remove_acknowledged(&mut self) -> io::Result<()>;
    /// The failure note of this boot, if there is one. A note of
    /// another boot counts as none. An error if a note is there and
    /// cannot be read.
    fn failure(&mut self) -> io::Result<Option<Failure>>;
    /// Write the failure note.
    fn write_failure(&mut self, failure: &Failure) -> io::Result<()>;
    /// Remove the failure note, if there is one.
    fn remove_failure(&mut self) -> io::Result<()>;
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
    /// durable. The log must reach `must_reach`. With
    /// `acknowledge_failure` a store that is marked as failed is
    /// opened all the same. Returns the store, which takes the lock
    /// over, and its size. If the open fails the lock comes back with
    /// the error, still held.
    fn open(
        &mut self,
        lock: Self::Lock,
        must_reach: Option<MustReach>,
        acknowledge_failure: bool,
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
    /// A failure of the volume that was acknowledged by this start, for
    /// the daemon's log.
    pub acknowledged: Option<Reason>,
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
    /// How often a device that was deleted is looked at again before
    /// its number counts as held. The driver's delete does not wait:
    /// the number of a device nobody holds comes free a moment after
    /// the call, so one look straight after it says nothing.
    pub gone_retries: u32,
    /// Whether the operator acknowledges a failure of the volume. It
    /// has no effect when nothing has failed, and none on a volume that
    /// failed while a client held it.
    pub acknowledge_failure: bool,
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
    /// The failure note is written after the device is deleted.
    pub(crate) const FAILURE_NOTE_AFTER_DELETE: u32 = 1 << 19;
    /// A failure while a client held the device is acknowledged.
    pub(crate) const HELD_ACKNOWLEDGED: u32 = 1 << 20;
    /// The stored position is kept across an acknowledgement.
    pub(crate) const POSITION_KEPT: u32 = 1 << 21;
    /// The failure note is read after the store is opened.
    pub(crate) const NOTE_READ_AFTER_OPEN: u32 = 1 << 22;
    /// A failure of which it was not known whether a client held the
    /// device counts as not held while the device is still dead under
    /// its number.
    pub(crate) const UNKNOWN_WHILE_DEAD: u32 = 1 << 23;
    /// A device that is still there at the first look after its delete
    /// counts as held.
    pub(crate) const HELD_AT_FIRST_LOOK: u32 = 1 << 24;
    /// Whatever stands under the number after a delete counts as the
    /// deleted device, also another volume's device that took the
    /// number.
    pub(crate) const ANY_OCCUPANT_LINGERS: u32 = 1 << 25;
    /// A failed volume's device that is left because it is still live,
    /// because the driver or the note could not be asked, or because
    /// the delete failed, is left without a word; and a delete is not
    /// said either.
    pub(crate) const LEFT_UNSAID: u32 = 1 << 26;
    /// A start that refuses a failure nobody held, for want of the
    /// flag, changes the failure note.
    pub(crate) const REFUSAL_TOUCHES_NOTE: u32 = 1 << 27;
    /// A device with the volume's values and no note that is dead is
    /// said to need a delete by hand, like one that waits.
    pub(crate) const DEAD_BY_HAND: u32 = 1 << 28;
    /// When the driver's delete fails after the device's note was
    /// removed, the note is not put back, and the answer still says
    /// that the next start tries again.
    pub(crate) const DELETE_FAILURE_STRANDS: u32 = 1 << 29;
    /// After a failed delete the note is put back on a device that is
    /// dead, too.
    pub(crate) const PUT_BACK_ON_DEAD: u32 = 1 << 30;

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
        let (store, _, acknowledged) = open(control, records, volume, wanted, mutants)?;
        return Ok(ready(store, attached, acknowledged));
    }
    let (store, size, acknowledged) = open(control, records, volume, wanted, mutants)?;
    let found = find(control, records, wanted, mutants)?;
    let attached = attach(control, records, found, size, wanted, mutants)?;
    Ok(ready(store, attached, acknowledged))
}

fn ready<S>(store: S, attached: Attached, acknowledged: Option<Reason>) -> Ready<S> {
    Ready {
        store,
        number: attached.number,
        shape: attached.shape,
        took_over: attached.took_over,
        left: attached.left,
        acknowledged,
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

/// Steps 2 and 3: the lock, the failure note, the record of what was
/// acknowledged, then the store. Returns the store, its size, and the
/// failure that was acknowledged, if one was.
fn open<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<(V::Open, u64, Option<Reason>), Stop> {
    let early = mutants
        .has(Mutants::POSITION_BEFORE_LOCK)
        .then(|| records.acknowledged());
    let mut held = lock(records, volume, wanted.lock_retries)?;

    // The failure note: under the lock, before the position and before
    // anything is recovered.
    let late = mutants.has(Mutants::NOTE_READ_AFTER_OPEN);
    let mut acknowledging = None;
    if !late {
        acknowledging = noted_failure(control, records, wanted, mutants)?;
    }

    let mut early = early;
    let (store, size) = loop {
        // An acknowledgement opens without a position to reach: what
        // was acknowledged to nobody may be gone.
        let forgets = acknowledging.is_some() && !mutants.has(Mutants::POSITION_KEPT);
        let must_reach = if forgets {
            None
        } else {
            match early.take().unwrap_or_else(|| records.acknowledged()) {
                Ok(must_reach) => must_reach,
                Err(_) => {
                    let held = (!mutants.has(Mutants::UNLOCK_BEFORE_FAILED)).then_some(held);
                    let stop = failed(
                        control,
                        records,
                        wanted,
                        mutants,
                        Reason::PositionUnreadable,
                    );
                    drop(held);
                    return Err(stop);
                }
            }
        };
        let must_reach = must_reach.filter(|_| !mutants.has(Mutants::NO_MUST_REACH));
        let (reason, lock) = match volume.open(held, must_reach, acknowledging.is_some()) {
            Ok(opened) => break opened,
            Err((OpenError::Other(e), _)) => {
                return Err(Stop::Error(format!("opening the store: {e}")))
            }
            Err((OpenError::SyncFailed, _)) if mutants.has(Mutants::SYNC_ERROR_MAY_PASS) => {
                return Err(Stop::Error("opening the store: a sync failed".into()))
            }
            Err((OpenError::SyncFailed, lock)) => (Reason::RecoveryFailed, lock),
            Err((OpenError::Short, lock)) => (Reason::Short, lock),
            Err((OpenError::MarkedFailed, lock)) => (Reason::Marked, lock),
        };
        // The log's own marker, and no failure note of this boot: if
        // no record of this boot names a device, no client of this
        // boot can hold one, and the failure can be acknowledged.
        if reason == Reason::Marked && acknowledging.is_none() && !held_by_record(records) {
            if !wanted.acknowledge_failure {
                return Err(Stop::Failed(format!(
                    "{reason}; no client holds its device. It is served again with \
                     --acknowledge-failure"
                )));
            }
            acknowledging = Some(reason);
            held = lock;
            continue;
        }
        // The lock is held while the failed volume's device is deleted:
        // a second start cannot take the device over in between.
        let lock = (!mutants.has(Mutants::UNLOCK_BEFORE_FAILED)).then_some(lock);
        let stop = failed(control, records, wanted, mutants, reason);
        drop(lock);
        return Err(stop);
    };
    if late {
        // The defect: the note is looked at only now.
        acknowledging = noted_failure(control, records, wanted, mutants)?;
    }
    if acknowledging.is_some() {
        // The position first, then the note: a kill in between leaves
        // the note, and the next start asks again.
        if !mutants.has(Mutants::POSITION_KEPT) {
            records
                .remove_acknowledged()
                .map_err(error("emptying the record of what was acknowledged"))?;
        }
        records
            .remove_failure()
            .map_err(error("removing the failure note"))?;
    }
    Ok((store, size, acknowledging))
}

/// What the failure note of this boot means for this start. `Ok(None)`:
/// there is none. `Ok(Some(reason))`: a failure that no client held is
/// acknowledged by this start; a waiting device of the volume has been
/// deleted. `Err`: the start refuses. Called with the lock held.
fn noted_failure<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<Option<Reason>, Stop> {
    let failure = match records.failure() {
        Ok(None) => return Ok(None),
        Ok(Some(failure)) => failure,
        Err(_) => {
            let left = give_up_device(control, records, wanted, mutants);
            return Err(Stop::Failed(format!(
                "the failure note cannot be read: the volume counts as failed while a client \
                 held its device, and is not served again in this boot{left}"
            )));
        }
    };
    let reason = failure.reason;
    let acknowledges = wanted.acknowledge_failure;
    let held = match failure.held {
        Held::Unknown => {
            let settled = settle(control, records, wanted, mutants)?;
            // The answer goes into the note before anything rests on
            // it. If it cannot be written, the volume counts as held.
            let written = records.write_failure(&Failure {
                held: settled.into(),
                ..failure
            });
            settled || written.is_err()
        }
        Held::Yes => true,
        Held::No => false,
    };
    if held && !(acknowledges && mutants.has(Mutants::HELD_ACKNOWLEDGED)) {
        // The daemon that noted the failure may have died before it
        // deleted the device.
        let left = give_up_device(control, records, wanted, mutants);
        return Err(Stop::Failed(format!(
            "{reason}, while a client held its device: it is not served again in this boot, \
             and the failure cannot be acknowledged{left}"
        )));
    }
    if !acknowledges {
        // Refused, and nothing the decision rests on is touched: the
        // note, the acknowledged position and the marker stay, so the
        // next start without the flag refuses again and one with it
        // serves afresh. The volume's waiting device goes all the
        // same, by the one rule for every finder: whoever holds the
        // volume's lock and finds the failed volume's device waiting
        // deletes it. The daemon that failed was on its way to delete
        // it and was killed first; left there, it holds whatever is
        // sent to it, without limit and without a word. (Until M1.3d
        // this start touched nothing at all.)
        //
        // The delete does not wait for a holder (it is the driver's
        // delete without waiting, of a device that is not live). It
        // can wait behind another device's delete that the driver is
        // still carrying out; nothing here bounds that, as nothing
        // does in `fail_serving`.
        let left = give_up_device(control, records, wanted, mutants);
        if mutants.has(Mutants::REFUSAL_TOUCHES_NOTE) {
            let _ = records.write_failure(&Failure {
                held: Held::Unknown,
                ..failure
            });
        }
        let device = if left.is_empty() {
            "; no device of this volume was there"
        } else {
            left.as_str()
        };
        return Err(Stop::Failed(format!(
            "{reason}; no client held its device, and the failure is not acknowledged: a start \
             with --acknowledge-failure serves the volume afresh{device}"
        )));
    }
    // Whoever opened a device of this volume after the failure was
    // acknowledged nothing: the device is deleted, never taken over.
    let _ = give_up_device(control, records, wanted, mutants);
    Ok(Some(reason))
}

/// Settle a failure of which it was not known whether a client held
/// the device, by what the driver says now about the recorded number.
/// `Ok(true)`: held. `Ok(false)`: not held. `Err`: it cannot be told,
/// the note stays as it is and the start stops.
///
/// A deleted device is freed when its last holder lets go, and a
/// suspended pool never lets go. So:
///
/// - the number is free, or holds a device with other values than the
///   recorded ones, or one with another volume's note: nobody holds
///   this volume's device any more. Not held.
/// - a device without a server, without a note, with the recorded
///   values: this volume's, deleted and still held. Held.
/// - a device with this volume's note (the failing daemon died before
///   its delete): the note is removed, the device deleted and looked
///   at again. Still there: held. Gone: not held.
/// - a device that waits for a server or is live, without a note, with
///   the recorded values: whose it is cannot be told, as everywhere.
///   The start stops and touches nothing.
/// - no record of this boot, or a record or a note that cannot be
///   read: there is no number to ask about. Held. (The design is
///   silent here; this is the conservative reading. A detach removes
///   the record, so after a detach an unknown failure stays held for
///   the boot.)
fn settle<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    wanted: &Wanted,
    mutants: Mutants,
) -> Result<bool, Stop> {
    let attachment = match records.attachment() {
        Ok(Some(attachment)) if attachment.boot == records.boot() => attachment,
        _ => return Ok(true),
    };
    let number = attachment.number;
    let asking = error("asking for the failed volume's device");
    let info = control.info(number).map_err(&asking)?;
    let info =
        settled(control, records, number, info, wanted, Mutants::default()).map_err(&asking)?;
    let Some(info) = info else {
        return Ok(false);
    };
    let Ok(note) = records.note(number) else {
        return Ok(true);
    };
    if info.shape != attachment.shape {
        return Ok(false);
    }
    match note {
        Some(nonce) if nonce != attachment.nonce => Ok(false),
        Some(_) if info.state == DeviceState::Live => Ok(true),
        Some(_) => {
            delete(control, records, number, mutants)
                .map_err(io::Error::from)
                .map_err(error("deleting the failed volume's device"))?;
            lingers(
                control,
                records,
                (number, info.shape),
                wanted.gone_retries,
                mutants,
            )
            .map_err(&asking)
        }
        None if info.state == DeviceState::Dead => Ok(!mutants.has(Mutants::UNKNOWN_WHILE_DEAD)),
        None => Err(Stop::Error(format!(
            "device {number} has this volume's size, queues, depth and flags and no note, and \
             the volume failed in this boot: whether a client holds the volume's device cannot \
             be told. The device is left as it is. Deleting it by hand, or a reboot, resolves \
             it"
        ))),
    }
}

/// Whether a client of this boot may hold a device of this volume, as
/// far as the attachment record says: it is of this boot, or it cannot
/// be read.
fn held_by_record<R: Records>(records: &mut R) -> bool {
    match records.attachment() {
        Ok(Some(attachment)) => attachment.boot == records.boot(),
        Ok(None) => false,
        Err(_) => true,
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

/// Whether the device that was just deleted under `number` is still
/// there a stated time after its delete: looked at, and while it is
/// there looked at again `retries` times with the records' pause
/// between. The driver's delete returns before the device is gone, so
/// a device nobody holds can still be there at the first look; only
/// one that outlasts the time is held.
///
/// **It asks about the deleted device, not about the number.** An
/// unheld device is gone some tens of milliseconds after its delete,
/// and another volume's start can take the freed number between two
/// looks: a device under the number is then somebody else's, the
/// deleted one is gone, and nobody held it. What stands under the
/// number counts as the deleted device unless it is known to be
/// another:
///
/// - it has other values than the deleted device had (`shape`);
/// - or the number has a note: the deleted device's note was removed
///   before the delete, and a start writes one for a device it adds;
/// - or it is live: a deleted device has no server.
///
/// Anything else stays "still there", the safe side: a dead or waiting
/// device with the deleted one's values and no note. Besides the held
/// device that can be another volume's device of the same values
/// whose start was killed between its add and its note, or has not
/// written the note yet; it is left so, and that volume is then
/// called held for the boot although nobody held it. A note that
/// cannot be read is the safe side as well, as elsewhere in this
/// file.
fn lingers<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    (number, shape): (u32, Shape),
    retries: u32,
    mutants: Mutants,
) -> io::Result<bool> {
    let look = |control: &mut C, records: &mut R| -> io::Result<bool> {
        let Some(info) = control.info(number)? else {
            return Ok(false);
        };
        if mutants.has(Mutants::ANY_OCCUPANT_LINGERS) {
            return Ok(true);
        }
        if info.shape != shape || info.state == DeviceState::Live {
            return Ok(false);
        }
        Ok(!matches!(records.note(number), Ok(Some(_))))
    };
    let mut there = look(control, records)?;
    if mutants.has(Mutants::HELD_AT_FIRST_LOOK) {
        return Ok(there);
    }
    for _ in 0..retries {
        if !there {
            break;
        }
        records.pause();
        there = look(control, records)?;
    }
    Ok(there)
}

/// Why a device was not deleted, and what became of its note. The note
/// decides what happens next: a start or a detach deletes only a
/// device that carries this volume's note.
#[derive(Debug)]
pub enum Undeleted {
    /// The note could not be removed. Nothing was changed, and the
    /// driver was not asked.
    NoteStays(io::Error),
    /// The note was removed, the driver's delete failed, and the note
    /// was put back: the device is as it was before.
    NotePutBack(io::Error),
    /// The note was removed, the driver's delete failed, and the note
    /// is not back: it could not be written, the driver could not say
    /// whether the device is still there, or the device is dead and
    /// may be on its way out. No start and no detach deletes this
    /// device.
    NoNote(io::Error),
}

impl Undeleted {
    /// What to say of device `number`: why it was not deleted, and
    /// what removes it.
    pub fn said(&self, number: u32) -> String {
        const AGAIN: &str = "the next start of this volume or `skyzfsd detach` tries again";
        match self {
            Self::NoteStays(e) => format!(
                "device {number} could not be deleted: its note could not be removed ({e}), and nothing was changed: {AGAIN}"
            ),
            Self::NotePutBack(e) => format!(
                "device {number} could not be deleted ({e}), and its note was put back: {AGAIN}"
            ),
            Self::NoNote(e) => format!(
                "device {number} could not be deleted ({e}) and is left without its note, which was not put back: deleting it by hand, or a reboot, resolves it"
            ),
        }
    }
}

impl From<Undeleted> for io::Error {
    fn from(undeleted: Undeleted) -> Self {
        match undeleted {
            Undeleted::NoteStays(e) => e,
            Undeleted::NotePutBack(e) => {
                io::Error::new(e.kind(), format!("{e}; the device's note was put back"))
            }
            Undeleted::NoNote(e) => io::Error::new(
                e.kind(),
                format!("{e}; the device is left without its note: by hand, or a reboot"),
            ),
        }
    }
}

/// Delete this volume's device: its note first, then the device. A
/// note exists only while its device may exist.
///
/// The order stays also when the delete fails: the other order can
/// leave this volume's note on a number that another's device then
/// takes. But a delete that failed has left the device, and a device
/// without its note is deleted by nobody. So the note is put back when
/// the driver still shows a device under the number: the state is then
/// the one before the call, and the next start or detach tries again.
/// If the driver shows none, the delete is done whatever it said. If
/// the driver cannot say, the note is not put back: a note on a number
/// that may be free could become a note on another's device.
///
/// Nor is it put back on a device that is dead. The driver reports
/// "dead" for a device that is deleted and still held open as well as
/// for one that was never started or was stopped. If the delete was
/// done although it answered with an error, the device goes when its
/// last opener lets go, and a note put back would outlive it and sit
/// on a free number. No path of the driver is known that fails a
/// delete and yet deletes, so this is a safeguard. Its price: a device
/// of this volume that is dead without being deleted (added and never
/// started, or stopped) and whose delete the driver refuses is left
/// without its note. The answer at that moment says so ("by hand, or a
/// reboot"), but a later start finds a dead device with the volume's
/// values and no note and describes it as already deleted and still
/// held ([`unnoted`]), which is wrong for that device. It takes a
/// refused delete of a device that never served. A kill
/// between the removal and the putting back leaves the device without
/// a note, which is the state a kill between the removal and the
/// delete always left.
fn delete<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
    mutants: Mutants,
) -> Result<(), Undeleted> {
    if mutants.has(Mutants::DELETE_BEFORE_NOTE) {
        control.delete(number).map_err(Undeleted::NoNote)?;
        return records.remove_note(number).map_err(Undeleted::NoteStays);
    }
    // A note that cannot be read cannot be put back.
    let nonce = records.note(number).ok().flatten();
    records.remove_note(number).map_err(Undeleted::NoteStays)?;
    let Err(e) = control.delete(number) else {
        return Ok(());
    };
    if mutants.has(Mutants::DELETE_FAILURE_STRANDS) {
        return Err(Undeleted::NotePutBack(e));
    }
    let on_dead = mutants.has(Mutants::PUT_BACK_ON_DEAD);
    match (control.info(number), nonce) {
        (Ok(None), _) => Ok(()),
        (Ok(Some(info)), Some(nonce))
            if (info.state != DeviceState::Dead || on_dead)
                && records.write_note(number, &nonce).is_ok() =>
        {
            Err(Undeleted::NotePutBack(e))
        }
        _ => Err(Undeleted::NoNote(e)),
    }
}

/// What to say of a device under the volume's recorded number that has
/// the volume's values and no note: for a refused start and for
/// `status`, in the same words. Whose it is cannot be told for
/// certain. A dead one is, as far as its values tell, this volume's
/// device after its delete, held open by a client: the ordinary state
/// after a volume failed under an imported pool, with nothing left to
/// delete. One that waits or is live is there, and a delete by hand
/// does something.
pub fn unnoted(number: u32, state: DeviceState) -> String {
    match state {
        DeviceState::Dead => format!(
            "device {number} has this volume's size, queues, depth and flags and no note, and is dead: as far as that tells, it is this volume's, already deleted and still held open by a client (a suspended pool does that). Nothing is left to delete: it goes when the client lets go of it, or at a reboot"
        ),
        DeviceState::Waiting | DeviceState::Live => format!(
            "device {number} has this volume's size, queues, depth and flags and no note, and was left: deleting it by hand, or a reboot, resolves it"
        ),
    }
}

/// A start found the volume failed: the failure note first, then the
/// device's note, then the device. Called with the volume's lock held.
/// If the failure note cannot be written the device goes all the same,
/// and the reason says so.
fn failed<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    wanted: &Wanted,
    mutants: Mutants,
    reason: Reason,
) -> Stop {
    let failure = Failure {
        held: held_by_record(records).into(),
        reason,
        identity: [0; 16],
    };
    let after = mutants.has(Mutants::FAILURE_NOTE_AFTER_DELETE);
    let mut written = after || records.write_failure(&failure).is_ok();
    let left = give_up_device(control, records, wanted, mutants);
    if after {
        written = records.write_failure(&failure).is_ok();
    }
    let unwritten = if written {
        ""
    } else {
        "; the failure note could not be written"
    };
    Stop::Failed(format!("{reason}{left}{unwritten}"))
}

/// Delete the failed volume's device if it waits for a server or was
/// never started: never taken over. The delete does not wait: a device
/// that a client still holds keeps its number, deleted.
///
/// The device is deleted only if the record is of this boot, the note
/// carries the record's nonce, and the kernel reports the recorded
/// values and a device that is not live. A noted device with other
/// values is left: a note should never outlive its device, and if one
/// did (a device deleted by hand), what stands under the number now is
/// not this volume's. A device with the recorded values and no note is
/// left too: it is this volume's if a daemon was killed between
/// removing the note and the delete, and another's if the delete was
/// done and the number given out again, and nothing tells the two
/// apart. A device that is still live after the looks is left: a live
/// device has a server, and this start is not it.
///
/// **The answer says what was done**, for every device a record of
/// this boot names: that it was deleted, or that it was left and why,
/// also when the driver, the record or the note could not be asked or
/// the delete failed. It is empty only when there is nothing to say a
/// word about: no record, a record of another boot (its device did not
/// survive the boot), a number that carries another attachment's note,
/// or a number under which nothing stands. So an answer without a
/// device in it means that this start knows of none.
///
/// **What becomes of a device that was left.** Every start that finds
/// the failure note comes through here again, whether it then refuses
/// or serves, and `skyzfsd detach` deletes a device that carries this
/// volume's note and is not live. So a device left because it was
/// still live, could not be asked about or could not be deleted goes
/// at the next start or detach that finds it otherwise; a delete that
/// the driver refused has its note put back for that ([`Undeleted`]).
/// A device left because nothing shows it to be this volume's (no
/// note, or a note that cannot be read) is not deleted by either: by
/// hand, or by a reboot. Each answer says which of the two applies. A
/// dead device with the volume's values and no note needs neither: it
/// is, as far as that tells, the device the failing daemon deleted,
/// still held open by a client ([`unnoted`]).
fn give_up_device<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    wanted: &Wanted,
    mutants: Mutants,
) -> String {
    let said = |text: String| {
        if mutants.has(Mutants::LEFT_UNSAID) {
            String::new()
        } else {
            text
        }
    };
    let attachment = match records.attachment() {
        Ok(Some(attachment)) => attachment,
        Ok(None) => return String::new(),
        Err(_) => {
            return said(
                "; the attachment record could not be read, so no device was looked for: \
                 if one of this volume is left, deleting it by hand, or a reboot, resolves it"
                    .into(),
            )
        }
    };
    let number = attachment.number;
    let this_boot = attachment.boot == records.boot() || mutants.has(Mutants::FAILED_ANY_BOOT);
    if !this_boot {
        return String::new();
    }
    let noted = match records.note(number) {
        Ok(Some(nonce)) if nonce == attachment.nonce => true,
        // Another attachment's note: not this volume's device.
        Ok(Some(_)) => return String::new(),
        Ok(None) => false,
        Err(e) => {
            return said(format!(
                "; the note of device {number} could not be read ({e}), and the device was \
                 left: deleting it by hand, or a reboot, resolves it"
            ))
        }
    };
    let info = control
        .info(number)
        .and_then(|info| settled(control, records, number, info, wanted, Mutants::default()));
    let info = match info {
        Ok(Some(info)) => info,
        Ok(None) => return String::new(),
        Err(e) => {
            return said(format!(
                "; device {number} could not be asked about ({e}), and was left: the next \
                 start of this volume or `skyzfsd detach` tries again"
            ))
        }
    };
    if info.state == DeviceState::Live {
        return said(format!(
            "; device {number} is still live after {} looks, and was left: it has a server. \
             Once it has none, the next start of this volume or `skyzfsd detach` deletes it",
            wanted.live_retries
        ));
    }
    if !noted {
        if info.shape != attachment.shape {
            return String::new();
        }
        let state = if mutants.has(Mutants::DEAD_BY_HAND) {
            DeviceState::Waiting
        } else {
            info.state
        };
        return format!("; {}", unnoted(number, state));
    }
    if info.shape != attachment.shape {
        // Neither a start nor a detach deletes it: both want the
        // recorded values under the note.
        return format!(
            "; device {number} carries this volume's note and not the recorded values, and \
             was left: deleting it by hand, or a reboot, resolves it"
        );
    }
    match delete(control, records, number, mutants) {
        Ok(()) => said(format!("; device {number} was deleted")),
        Err(undeleted) => said(format!("; {}", undeleted.said(number))),
    }
}

/// The log failed while the volume was served: the volume gives up its
/// attachment. The failure note first, with what the kernel says about
/// a client holding the device (unknown, when it cannot say), then the
/// device's note, then the device, without waiting. The caller holds
/// the volume's lock (the store does) and exits afterwards.
///
/// Returns what was noted, whether the note could be written, and
/// whether the device was deleted. The daemon takes the two steps
/// apart, with its wait for the queue's thread between them
/// ([`note_failure`], [`delete_failed`]).
pub fn fail_serving<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
    identity: [u8; 16],
) -> (Failure, bool, Result<(), Undeleted>) {
    fail_serving_with(control, records, number, identity, Mutants::default())
}

/// The first step of [`fail_serving`]: the failure note. Returns what
/// was noted, and whether the note could be written. A daemon killed
/// after it leaves the note and the device with its note, which waits
/// for a server: the next start reads the note and deletes the device.
pub fn note_failure<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
    identity: [u8; 16],
) -> (Failure, bool) {
    let failure = Failure {
        held: match control.client_holds(number) {
            Some(held) => held.into(),
            None => Held::Unknown,
        },
        reason: Reason::LogFailed,
        identity,
    };
    let written = records.write_failure(&failure).is_ok();
    (failure, written)
}

/// The second step of [`fail_serving`]: the device's note, then the
/// device, without waiting.
pub fn delete_failed<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
) -> Result<(), Undeleted> {
    delete(control, records, number, Mutants::default())
}

pub(crate) fn fail_serving_with<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
    identity: [u8; 16],
    mutants: Mutants,
) -> (Failure, bool, Result<(), Undeleted>) {
    if !mutants.has(Mutants::FAILURE_NOTE_AFTER_DELETE) {
        let (failure, written) = note_failure(control, records, number, identity);
        let deleted = delete(control, records, number, mutants);
        return (failure, written, deleted);
    }
    let deleted = delete(control, records, number, mutants);
    let (failure, written) = note_failure(control, records, number, identity);
    (failure, written, deleted)
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
        // The record is of this boot: noted as held. The device stays.
        let failure = Failure {
            held: Held::Yes,
            reason: Reason::DeviceNoteUnreadable,
            identity: [0; 16],
        };
        let unwritten = match records.write_failure(&failure) {
            Ok(()) => "",
            Err(_) => "; the failure note could not be written",
        };
        return Err(Stop::Failed(format!(
            "{}{unwritten}",
            Reason::DeviceNoteUnreadable
        )));
    };
    if note != Some(attachment.nonce) {
        let left = |why| {
            Ok(Found::Afresh {
                stale: None,
                left: Some((number, why)),
            })
        };
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
            // This volume's, from a kill between the add and the note
            // or between the note's removal and the delete of a device
            // that is still held, or another's twin: no server can be
            // given to it.
            DeviceState::Dead => left("it has no server and no note"),
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
        DeviceState::Dead if mutants.has(Mutants::TAKE_DEAD) => {
            Ok(Found::TakeOver(number, info.shape))
        }
        // Never started, or deleted and still held: it is deleted
        // (again), and never taken over.
        DeviceState::Dead => Ok(Found::Afresh {
            stale: Some(number),
            left: None,
        }),
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
            .map_err(io::Error::from)
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

    /// The device under the recorded number has no server, waits for
    /// none, has no note and has the recorded values: it may be this
    /// volume's that was deleted before and is still held, or another's
    /// that was never started. Nothing was done, and the record stays.
    DeadNoNote(u32),
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
/// waiting, the device is looked at again until it is gone or
/// `gone_retries` looks have found it still there, and the attachment
/// record is removed. If the device is still there after the last
/// look, the answer says that its number is still held: one look
/// straight after the delete would say that of a device nobody holds,
/// since the driver's delete returns before the device is gone. A kill between any two
/// steps leaves a state the next start or the next detach treats
/// safely.
///
/// A device that is live is refused. One that cannot be shown to be
/// this volume's is left, and the answer says which number. Of those,
/// one without a server and without a note that has the recorded
/// values is reported apart ([`Detached::DeadNoNote`]): that is how
/// this volume's device looks after it was deleted while something
/// held it, since a note is removed before its delete.
///
/// A detach does not ask beforehand whether a client holds the device:
/// the kernel can say only that an exclusive holder exists
/// ([`Control::client_holds`]). It deletes, looks again, and says
/// whether the number is still held.
pub fn detach<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    lock_retries: u32,
    gone_retries: u32,
) -> io::Result<Detached> {
    detach_with(
        control,
        records,
        volume,
        (lock_retries, gone_retries),
        Mutants::default(),
    )
}

pub(crate) fn detach_with<C: Control, R: Records, V: Volume>(
    control: &mut C,
    records: &mut R,
    volume: &mut V,
    (lock_retries, gone_retries): (u32, u32),
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
    if located.whose == Whose::Unsure && located.state == DeviceState::Dead {
        return Ok(Detached::DeadNoNote(number));
    }
    if located.whose != Whose::Ours {
        return Ok(Detached::NotOurs(located));
    }
    match located.state {
        DeviceState::Live => Ok(Detached::Live(number)),
        DeviceState::Waiting | DeviceState::Dead => {
            delete(control, records, number, mutants).map_err(io::Error::from)?;
            let held = lingers(
                control,
                records,
                (number, located.shape),
                gone_retries,
                mutants,
            )?;
            records.remove_attachment()?;
            Ok(if held {
                Detached::DeletedHeld(number)
            } else {
                Detached::Deleted(number)
            })
        }
    }
}

#[cfg(test)]
mod tests;
