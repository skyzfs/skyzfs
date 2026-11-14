//! The order of steps, against a fake kernel, fake records and a fake
//! store that share one list of what happened.

use super::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

const BOOT: [u8; 16] = [7; 16];
const SIZE: u64 = 1 << 30;
const FLAGS: u64 = 0x48;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Features,
    Open(bool),
    Pause,
    Info(u32),
    WriteAttachment(u32),
    Add(u32, bool),
    Disown(u32),
    WriteNote(u32),
    RemoveNote(u32),
    RemoveAttachment,
    Delete(u32),
    BeginRecovery(u32),
    WriteFailure(Held, Reason),
    RemoveFailure,
}

#[derive(Clone)]
struct World {
    events: Vec<Event>,
    /// The driver's features: `None` when it cannot say them.
    features: Option<u64>,
    /// The kernel's release.
    release: String,
    /// How often the release was read: it is not an event, since it
    /// asks the driver nothing.
    release_reads: usize,
    devices: BTreeMap<u32, DeviceInfo>,
    /// Devices that appear between the daemon's look and its add.
    late: Vec<u32>,
    /// Devices that someone has open: deleted, they keep their number.
    held: Vec<u32>,
    /// A live device whose server is seen to be gone once it has been
    /// asked for this many more times.
    settles: Option<(u32, usize)>,
    /// A delete of a device nobody holds returns before the device is
    /// gone: it is still there for this many looks.
    delete_takes: Option<u32>,
    /// The device that is going, and for how many looks it is there.
    going: Option<(u32, u32)>,
    /// What takes the number at the very look at which the going
    /// device is gone, so that the number is never seen free: another
    /// volume's device, the note its start wrote for it if it has, and
    /// whether the note cannot be read from then on.
    successor: Option<(DeviceInfo, Option<[u8; 16]>, bool)>,
    /// Who holds the volume's lock.
    lock_holder: Option<u64>,
    locks: u64,
    /// Another instance holds the lock for this many more tries.
    locked_tries: u32,
    /// For each event: whether the lock was held when it happened.
    under_lock: Vec<bool>,
    /// A second daemon starts when this many things have happened.
    intrude_at: Option<usize>,
    /// What became of it: the device it serves and whether it took it
    /// over, or why it does not serve.
    intruder: Option<Result<(u32, bool), Stop>>,
    /// The daemon is killed once this many things have happened: from
    /// then on nothing it does has an effect.
    dies_at: Option<usize>,
    disowned: Vec<u32>,
    attachment: Option<Attachment>,
    notes: BTreeMap<u32, [u8; 16]>,
    /// The attachment record is there and is not a whole record.
    torn_attachment: bool,
    /// The note of this device is there and is not a whole record.
    torn_note: Option<u32>,
    /// Errors of the next opens, before the store answers by itself.
    opens: VecDeque<OpenError>,
    nonces: u8,
    /// The failure note of this boot. `Err`: it is there and is not a
    /// whole record.
    failure: Result<Option<Failure>, ()>,
    /// The failure note cannot be written.
    failure_unwritable: bool,
    /// For each reading of the failure note: how many things had
    /// happened, and whether the lock was held.
    failure_reads: Vec<(usize, bool)>,
    /// How many things had happened at each writing of a failure note
    /// where there was none: not the settling of one that said unknown.
    fresh_notes: Vec<usize>,
    /// The store's log carries the marker of a failure.
    marked: bool,
    /// What the kernel says about a client holding a device.
    client_holds: Option<bool>,
    /// The driver cannot be asked about a device.
    info_refused: bool,
    /// The driver refuses a delete.
    delete_refused: bool,
    /// A device's note cannot be written.
    note_unwritable: bool,
}

fn killed() -> io::Error {
    io::Error::other("killed")
}

impl World {
    /// Note that `event` happens, unless the daemon is dead.
    fn happens(&mut self, event: Event) -> io::Result<()> {
        if self.dead() {
            return Err(killed());
        }
        if self.under_lock.len() != self.events.len() {
            // A test emptied the list of events.
            self.under_lock = vec![true; self.events.len()];
        }
        self.under_lock.push(self.lock_holder.is_some());
        self.events.push(event);
        Ok(())
    }

    fn dead(&self) -> bool {
        self.dies_at.is_some_and(|at| self.events.len() >= at)
    }

    fn new() -> Self {
        Self {
            events: Vec::new(),
            features: Some(u64::MAX),
            release: "7.0.0".to_string(),
            release_reads: 0,
            devices: BTreeMap::new(),
            late: Vec::new(),
            held: Vec::new(),
            settles: None,
            delete_takes: None,
            going: None,
            successor: None,
            lock_holder: None,
            locks: 0,
            locked_tries: 0,
            under_lock: Vec::new(),
            intrude_at: None,
            intruder: None,
            dies_at: None,
            disowned: Vec::new(),
            attachment: None,
            notes: BTreeMap::new(),
            torn_attachment: false,
            torn_note: None,
            opens: VecDeque::new(),
            nonces: 0,
            failure: Ok(None),
            failure_unwritable: false,
            failure_reads: Vec::new(),
            fresh_notes: Vec::new(),
            marked: false,
            client_holds: None,
            info_refused: false,
            delete_refused: false,
            note_unwritable: false,
        }
    }

    /// An earlier instance of this boot attached device `number`, and
    /// the kernel has it in `state`.
    fn attached(mut self, number: u32, state: DeviceState, shape: Shape) -> Self {
        let nonce = [0xA5; 16];
        self.attachment = Some(Attachment {
            number,
            nonce,
            boot: BOOT,
            shape,
        });
        self.notes.insert(number, nonce);
        self.devices.insert(number, DeviceInfo { state, shape });
        self
    }
}

#[derive(Clone)]
struct Fake(Rc<RefCell<World>>);

impl Fake {
    /// Another daemon starts here, if the test asked for one at this
    /// point: a whole start, between two steps of the one under test.
    fn intrude(&self) {
        let due = {
            let mut w = self.0.borrow_mut();
            let due = w.intrude_at == Some(w.events.len());
            if due {
                w.intrude_at = None;
            }
            due
        };
        if !due {
            return;
        }
        let (mut c, mut r, mut v) = (self.clone(), self.clone(), self.clone());
        let result = start_with(&mut c, &mut r, &mut v, &WANTED, Mutants(0));
        let became = result.map(|mut ready| {
            // It serves: it keeps the lock, and its device is live.
            if let Some(lock) = ready.store.lock.as_mut() {
                lock.kept = true;
            }
            if let Some(device) = self.0.borrow_mut().devices.get_mut(&ready.number) {
                device.state = DeviceState::Live;
            }
            (ready.number, ready.took_over)
        });
        self.0.borrow_mut().intruder = Some(became);
    }
}

/// The volume's lock of the fake.
struct FakeLock {
    world: Rc<RefCell<World>>,
    id: u64,
    /// Its holder goes on serving: the lock stays when this is dropped.
    kept: bool,
}

impl Drop for FakeLock {
    fn drop(&mut self) {
        let mut w = self.world.borrow_mut();
        if !self.kept && w.lock_holder == Some(self.id) {
            w.lock_holder = None;
        }
    }
}

impl Control for Fake {
    fn features(&mut self) -> io::Result<Option<u64>> {
        let mut w = self.0.borrow_mut();
        w.happens(Event::Features)?;
        Ok(w.features)
    }
    fn release(&mut self) -> io::Result<String> {
        let mut w = self.0.borrow_mut();
        w.release_reads += 1;
        Ok(w.release.clone())
    }
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::Info(number))?;
        if w.info_refused {
            return Err(io::Error::other("the driver does not answer"));
        }
        if let Some((settling, left)) = w.settles {
            if settling == number && left == 0 {
                if let Some(device) = w.devices.get_mut(&number) {
                    device.state = DeviceState::Waiting;
                }
            } else if settling == number {
                w.settles = Some((settling, left - 1));
            }
        }
        match w.going {
            Some((going, 0)) if going == number => {
                w.devices.remove(&number);
                w.going = None;
                if let Some((device, note, torn)) = w.successor.take() {
                    w.devices.insert(number, device);
                    if let Some(nonce) = note {
                        w.notes.insert(number, nonce);
                    }
                    if torn {
                        w.torn_note = Some(number);
                    }
                }
            }
            Some((going, left)) if going == number => w.going = Some((going, left - 1)),
            _ => {}
        }
        Ok(w.devices
            .get(&number)
            .filter(|_| !w.late.contains(&number))
            .copied())
    }
    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        let taken = w.devices.contains_key(&number);
        w.happens(Event::Add(number, !taken))
            .map_err(AddError::Other)?;
        w.late.retain(|late| *late != number);
        if taken {
            return Err(AddError::Taken);
        }
        w.devices.insert(
            number,
            DeviceInfo {
                state: DeviceState::Dead,
                shape: *shape,
            },
        );
        Ok(())
    }
    fn disown(&mut self, number: u32) {
        let mut w = self.0.borrow_mut();
        if w.happens(Event::Disown(number)).is_ok() {
            w.disowned.push(number);
        }
    }
    fn delete(&mut self, number: u32) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::Delete(number))?;
        if w.delete_refused {
            return Err(io::Error::other("the driver refuses"));
        }
        if w.held.contains(&number) {
            if let Some(device) = w.devices.get_mut(&number) {
                device.state = DeviceState::Dead;
            }
        } else if let Some(looks) = w.delete_takes {
            // Nobody holds it, and the delete has returned before the
            // device is gone: it is there, dead, for `looks` looks.
            if let Some(device) = w.devices.get_mut(&number) {
                device.state = DeviceState::Dead;
            }
            w.going = Some((number, looks));
        } else {
            w.devices.remove(&number);
        }
        Ok(())
    }
    fn begin_recovery(&mut self, number: u32) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::BeginRecovery(number))
    }
    fn client_holds(&mut self, _number: u32) -> Option<bool> {
        self.0.borrow().client_holds
    }
}

impl Records for Fake {
    fn boot(&self) -> [u8; 16] {
        BOOT
    }
    fn nonce(&mut self) -> io::Result<[u8; 16]> {
        let mut w = self.0.borrow_mut();
        w.nonces += 1;
        Ok([w.nonces; 16])
    }
    fn attachment(&mut self) -> io::Result<Option<Attachment>> {
        let w = self.0.borrow();
        if w.torn_attachment {
            return Err(io::Error::other("not a whole record"));
        }
        Ok(w.attachment)
    }
    fn write_attachment(&mut self, attachment: &Attachment) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::WriteAttachment(attachment.number))?;
        w.attachment = Some(*attachment);
        Ok(())
    }
    fn note(&mut self, number: u32) -> io::Result<Option<[u8; 16]>> {
        let w = self.0.borrow();
        if w.torn_note == Some(number) {
            return Err(io::Error::other("not a whole record"));
        }
        Ok(w.notes.get(&number).copied())
    }
    fn write_note(&mut self, number: u32, nonce: &[u8; 16]) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::WriteNote(number))?;
        if w.note_unwritable {
            return Err(io::Error::other("the run directory is full"));
        }
        w.notes.insert(number, *nonce);
        Ok(())
    }
    fn remove_attachment(&mut self) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::RemoveAttachment)?;
        w.attachment = None;
        Ok(())
    }
    fn remove_note(&mut self, number: u32) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::RemoveNote(number))?;
        w.notes.remove(&number);
        Ok(())
    }
    fn failure(&mut self) -> io::Result<Option<Failure>> {
        let mut w = self.0.borrow_mut();
        let read = (w.events.len(), w.lock_holder.is_some());
        w.failure_reads.push(read);
        w.failure
            .map_err(|()| io::Error::other("not a whole record"))
    }
    fn write_failure(&mut self, failure: &Failure) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        if w.failure_unwritable {
            return Err(io::Error::other("no space"));
        }
        let fresh = (w.failure == Ok(None)).then_some(w.events.len());
        w.happens(Event::WriteFailure(failure.held, failure.reason))?;
        w.fresh_notes.extend(fresh);
        w.failure = Ok(Some(*failure));
        Ok(())
    }
    fn remove_failure(&mut self) -> io::Result<()> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        w.happens(Event::RemoveFailure)?;
        w.failure = Ok(None);
        Ok(())
    }
    fn pause(&mut self) {
        let _ = self.0.borrow_mut().happens(Event::Pause);
    }
}

/// The open store of the fake: it holds the lock.
struct Opened {
    lock: Option<FakeLock>,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Opened")
    }
}

impl Volume for Fake {
    type Lock = FakeLock;
    type Open = Opened;

    fn lock(&mut self) -> Result<FakeLock, LockError> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        if w.dead() {
            return Err(LockError::Other("killed".into()));
        }
        if w.locked_tries > 0 {
            w.locked_tries -= 1;
            return Err(LockError::Locked);
        }
        if w.lock_holder.is_some() {
            return Err(LockError::Locked);
        }
        w.locks += 1;
        w.lock_holder = Some(w.locks);
        Ok(FakeLock {
            world: self.0.clone(),
            id: w.locks,
            kept: false,
        })
    }

    fn open(
        &mut self,
        lock: FakeLock,
        acknowledge_failure: bool,
    ) -> Result<(Opened, u64), (OpenError, FakeLock)> {
        self.intrude();
        let mut w = self.0.borrow_mut();
        let result = match w.opens.pop_front() {
            Some(error) => Err(error),
            None if w.marked && !acknowledge_failure => Err(OpenError::MarkedFailed),
            None => Ok(()),
        };
        let result = match w.happens(Event::Open(result.is_ok())) {
            Ok(()) => {
                // An open that acknowledges takes the marker away.
                w.marked &= result.is_err();
                result
            }
            Err(_) => Err(OpenError::Other("killed".into())),
        };
        drop(w);
        match result {
            Ok(()) => Ok((Opened { lock: Some(lock) }, SIZE)),
            Err(error) => Err((error, lock)),
        }
    }
}

/// How often the tests' lifecycle looks again at a deleted device.
const GONE: u32 = 4;

const WANTED: Wanted = Wanted {
    queues: 1,
    depth: 64,
    flags: FLAGS,
    lock_retries: 3,
    live_retries: 5,
    gone_retries: GONE,
    acknowledge_failure: false,
    async_delete_asserted: false,
};

/// The same, with `--acknowledge-failure`.
const ACKNOWLEDGING: Wanted = Wanted {
    acknowledge_failure: true,
    ..WANTED
};

/// What an earlier instance was started with: not what `WANTED` asks.
const EARLIER: Shape = Shape {
    size: SIZE,
    queues: 2,
    depth: 32,
    flags: FLAGS,
};

struct Outcome {
    result: Result<Ready<Opened>, Stop>,
    world: World,
}

fn run(world: World, mutants: u32) -> Outcome {
    run_as(world, &WANTED, mutants)
}

fn run_as(world: World, wanted: &Wanted, mutants: u32) -> Outcome {
    let shared = Rc::new(RefCell::new(world));
    let (mut c, mut r, mut v) = (
        Fake(shared.clone()),
        Fake(shared.clone()),
        Fake(shared.clone()),
    );
    // The process ends with the start: its lock goes with it.
    let result = start_with(&mut c, &mut r, &mut v, wanted, Mutants(mutants)).map(|mut ready| {
        ready.store.lock = None;
        ready
    });
    drop((c, r, v));
    let world = match Rc::try_unwrap(shared) {
        Ok(cell) => cell.into_inner(),
        Err(_) => panic!("the fakes are dropped"),
    };
    Outcome { result, world }
}

fn check(condition: bool, what: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(what.to_string())
    }
}

/// The rules that hold after every start, whatever was found.
fn rules(outcome: &Outcome) -> Result<(), String> {
    let events = &outcome.world.events;
    let at = |wanted: &dyn Fn(&Event) -> bool| events.iter().position(wanted);
    let opened = at(&|e| matches!(e, Event::Open(true)));
    // No recovery is begun and no device is added before the store is
    // open.
    for (i, event) in events.iter().enumerate() {
        if matches!(event, Event::Add(..) | Event::BeginRecovery(_)) {
            check(
                opened.is_some_and(|o| o < i),
                "a device was added or its recovery begun before the store was open",
            )?;
        }
    }
    // Everything that changes the attachment, and the open, happen
    // under the volume's lock.
    for (i, event) in events.iter().enumerate() {
        let free = matches!(event, Event::Features | Event::Pause | Event::Info(_));
        if !free {
            check(
                outcome.world.under_lock.get(i) == Some(&true),
                &format!("{event:?} happened without the volume's lock"),
            )?;
        }
    }
    // The failure note is read under the lock and before the store is
    // opened, and a start that writes one writes it before it deletes.
    for (read_at, locked) in &outcome.world.failure_reads {
        let opened = at(&|e| matches!(e, Event::Open(..)));
        check(
            *locked && opened.is_none_or(|o| *read_at <= o),
            "the failure note was read without the lock or after the store was opened",
        )?;
    }
    let noted = outcome.world.fresh_notes.first().copied();
    let deleted = at(&|e| matches!(e, Event::RemoveNote(_) | Event::Delete(_)));
    if let (Some(noted), Some(deleted)) = (noted, deleted) {
        check(
            noted < deleted,
            "a failed volume's device was deleted before the failure was noted",
        )?;
    }
    // A device is deleted only after its note is gone.
    for (i, event) in events.iter().enumerate() {
        if let Event::Delete(n) = event {
            check(
                i > 0 && events[i - 1] == Event::RemoveNote(*n),
                "a device was deleted and its note left",
            )?;
        }
    }
    // Every add is of the number recorded last, and a successful one is
    // followed at once by the disowning, and only then by the note.
    let mut recorded = None;
    for (i, event) in events.iter().enumerate() {
        match event {
            Event::WriteAttachment(n) => recorded = Some(*n),
            Event::Add(n, succeeded) => {
                check(
                    recorded == Some(*n),
                    "a device was added before it was recorded",
                )?;
                check(
                    !events[..i].contains(&Event::WriteNote(*n)),
                    "the note was written before the device was added",
                )?;
                if *succeeded {
                    check(
                        events.get(i + 1) == Some(&Event::Disown(*n)),
                        "the device was not disowned at once",
                    )?;
                }
            }
            _ => {}
        }
    }
    // A daemon that does not serve began no recovery and left no
    // device of its own.
    if outcome.result.is_err() {
        check(
            !events
                .iter()
                .any(|e| matches!(e, Event::Add(..) | Event::BeginRecovery(_))),
            "a daemon that does not serve touched a device",
        )?;
    }
    if let Ok(ready) = &outcome.result {
        let device = outcome.world.devices.get(&ready.number);
        check(
            device.is_some_and(|d| d.shape == ready.shape),
            "the daemon serves with values that are not the device's",
        )?;
        check(
            outcome.world.attachment.is_some_and(|a| {
                a.number == ready.number
                    && a.boot == BOOT
                    && outcome.world.notes.get(&ready.number) == Some(&a.nonce)
            }),
            "the record and the note do not name the device that is served",
        )?;
    }
    Ok(())
}

/// A scenario holds without defects, and each named defect is noticed.
fn holds(scenario: fn(u32) -> Result<(), String>, noticed: &[(&str, u32)]) {
    assert_eq!(scenario(0), Ok(()));
    for (name, mutant) in noticed {
        assert!(
            scenario(*mutant).is_err(),
            "the defect `{name}` is not noticed"
        );
    }
}

fn fresh(mutants: u32) -> Result<(), String> {
    let outcome = run(World::new(), mutants);
    rules(&outcome)?;
    let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
    check(!ready.took_over && ready.number == 0, "a fresh device 0")?;
    check(
        ready.shape
            == Shape {
                size: SIZE,
                queues: 1,
                depth: 64,
                flags: FLAGS,
            },
        "the store's size and this instance's options",
    )?;
    check(outcome.world.disowned == [0], "disowned")?;
    check(
        outcome.world.events
            == [
                Event::Features,
                Event::Open(true),
                Event::Info(0),
                Event::WriteAttachment(0),
                Event::Add(0, true),
                Event::Disown(0),
                Event::WriteNote(0),
            ],
        "the order of a first start",
    )
}

#[test]
fn a_first_start_opens_the_store_records_adds_disowns_and_notes_in_that_order() {
    holds(
        fresh,
        &[
            ("device before store", Mutants::DEVICE_BEFORE_STORE),
            ("add before record", Mutants::ADD_BEFORE_RECORD),
            ("note before add", Mutants::NOTE_BEFORE_ADD),
            ("not disowned", Mutants::NOT_DISOWNED),
        ],
    );
}

fn taken_number(mutants: u32) -> Result<(), String> {
    let mut world = World::new();
    // Device 0 is seen. Device 1 appears after the daemon looked.
    world.late.push(1);
    for number in 0..2 {
        world.devices.insert(
            number,
            DeviceInfo {
                state: DeviceState::Live,
                shape: EARLIER,
            },
        );
    }
    let outcome = run(world, mutants);
    rules(&outcome)?;
    let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
    check(ready.number == 2, "the first free number")?;
    check(
        outcome.world.events[2..]
            == [
                Event::Info(0),
                Event::Info(1),
                Event::WriteAttachment(1),
                Event::Add(1, false),
                Event::Info(2),
                Event::WriteAttachment(2),
                Event::Add(2, true),
                Event::Disown(2),
                Event::WriteNote(2),
            ],
        "a taken number is not recorded, and the record is rewritten before the next add",
    )?;
    check(
        outcome.world.devices.len() == 3 && outcome.world.notes.len() == 1,
        "the devices of others are left, and have no note of ours",
    )
}

#[test]
fn a_taken_number_is_passed_over_and_the_record_rewritten_before_the_next_add() {
    holds(
        taken_number,
        &[
            ("add before record", Mutants::ADD_BEFORE_RECORD),
            ("note before add", Mutants::NOTE_BEFORE_ADD),
        ],
    );
}

fn take_over(mutants: u32) -> Result<(), String> {
    let world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    let outcome = run(world, mutants);
    rules(&outcome)?;
    let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
    check(
        ready.took_over && ready.number == 4,
        "device 4 is taken over",
    )?;
    check(ready.shape == EARLIER, "with the device's own values")?;
    check(
        outcome.world.events[1..] == [Event::Open(true), Event::Info(4), Event::BeginRecovery(4)],
        "store, then the recovery, and nothing is added",
    )
}

#[test]
fn a_waiting_device_of_this_volume_is_taken_over_with_its_own_values() {
    holds(
        take_over,
        &[
            ("device before store", Mutants::DEVICE_BEFORE_STORE),
            ("own options", Mutants::OWN_OPTIONS),
        ],
    );
}

/// A start that must change nothing and stop with `stop`.
fn untouched(world: World, mutants: u32, stop: fn(&Stop) -> bool) -> Result<(), String> {
    let devices = world.devices.clone();
    let attachment = world.attachment;
    let outcome = run(world, mutants);
    rules(&outcome)?;
    match &outcome.result {
        Err(e) if stop(e) => {}
        other => return Err(format!("stopped with {other:?}")),
    }
    check(
        outcome.world.devices == devices,
        "the devices are as they were",
    )?;
    check(
        outcome.world.attachment == attachment,
        "the record is as it was",
    )
}

/// A start that leaves the device under the recorded number alone and
/// attaches under another.
fn left_alone(world: World, number: u32, mutants: u32) -> Result<(), String> {
    let before = world.devices.get(&number).copied();
    let note = world.notes.get(&number).copied();
    let outcome = run(world, mutants);
    rules(&outcome)?;
    let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
    check(
        !ready.took_over && ready.number != number,
        "attached afresh",
    )?;
    check(
        ready.left.is_some_and(|(left, _)| left == number),
        "says which device it left",
    )?;
    check(
        outcome.world.devices.get(&number).copied() == before
            && outcome.world.notes.get(&number).copied() == note,
        "the device and its note are as they were",
    )?;
    check(
        !outcome
            .world
            .events
            .iter()
            .any(|e| matches!(e, Event::Delete(_) | Event::BeginRecovery(_))),
        "nothing is deleted or recovered",
    )
}

const STATES: [DeviceState; 3] = [DeviceState::Dead, DeviceState::Live, DeviceState::Waiting];

fn another_nonce(mutants: u32) -> Result<(), String> {
    for state in STATES {
        let mut world = World::new().attached(0, state, EARLIER);
        world.notes.insert(0, [0x5A; 16]);
        left_alone(world, 0, mutants)?;
    }
    Ok(())
}

/// No note, and other values than the record: a stranger's.
fn no_note_other_values(mutants: u32) -> Result<(), String> {
    for state in STATES {
        let mut world = World::new().attached(0, state, EARLIER);
        world.notes.clear();
        if let Some(device) = world.devices.get_mut(&0) {
            device.shape.depth = 16;
        }
        left_alone(world, 0, mutants)?;
    }
    Ok(())
}

/// No note, and the recorded values: this volume's device whose run
/// directory was wiped, or a stranger's twin.
fn no_note_same_values(mutants: u32) -> Result<(), String> {
    for state in [DeviceState::Live, DeviceState::Waiting] {
        let mut world = World::new().attached(0, state, EARLIER);
        world.notes.clear();
        untouched(
            world,
            mutants,
            |s| matches!(s, Stop::Error(e) if e.contains("no note")),
        )?;
    }
    let mut world = World::new().attached(0, DeviceState::Dead, EARLIER);
    world.notes.clear();
    left_alone(world, 0, mutants)
}

fn live(mutants: u32) -> Result<(), String> {
    let world = World::new().attached(4, DeviceState::Live, EARLIER);
    untouched(world, mutants, |s| *s == Stop::Held)
}

fn other_values(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    if let Some(device) = world.devices.get_mut(&4) {
        device.shape.depth = 16;
    }
    untouched(world, mutants, |s| matches!(s, Stop::Error(_)))
}

#[test]
fn a_device_that_is_not_surely_this_volumes_is_neither_taken_over_nor_replaced() {
    holds(another_nonce, &[("any nonce", Mutants::ANY_NONCE)]);
    holds(no_note_other_values, &[]);
    holds(
        no_note_same_values,
        &[("no note is a stranger", Mutants::NO_NOTE_IS_A_STRANGER)],
    );
    holds(live, &[("take live", Mutants::TAKE_LIVE)]);
    holds(other_values, &[("any values", Mutants::ANY_SHAPE)]);
}

#[test]
fn this_volumes_device_that_was_never_started_is_deleted_and_replaced() {
    let world = World::new().attached(4, DeviceState::Dead, EARLIER);
    let outcome = run(world, 0);
    assert_eq!(rules(&outcome), Ok(()));
    let ready = outcome.result.expect("serves");
    assert!(!ready.took_over);
    assert_eq!(ready.left, None);
    assert_eq!(
        outcome.world.events[2..5],
        [Event::Info(4), Event::RemoveNote(4), Event::Delete(4)]
    );
    assert_eq!(outcome.world.devices.len(), 1);
}

fn settles(mutants: u32) -> Result<(), String> {
    // The kill pin's shape: the server was killed, its lock is free,
    // and the kernel still shows the device live for three more looks.
    let mut world = World::new().attached(4, DeviceState::Live, EARLIER);
    world.settles = Some((4, 3));
    let outcome = run(world, mutants);
    rules(&outcome)?;
    let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
    check(ready.took_over && ready.number == 4, "taken over")?;
    let pauses = outcome
        .world
        .events
        .iter()
        .filter(|e| **e == Event::Pause)
        .count();
    check(
        pauses == 3,
        "asked again three times, with a pause before each",
    )
}

#[test]
fn this_volumes_device_found_live_is_waited_for_and_then_taken_over() {
    holds(
        settles,
        &[(
            "a live device is refused at once",
            Mutants::LIVE_REFUSED_AT_ONCE,
        )],
    );
    // A failed volume waits as well, and then deletes.
    let mut world = World::new().attached(4, DeviceState::Live, EARLIER);
    world.settles = Some((4, 3));
    world.opens.push_back(OpenError::MarkedFailed);
    let outcome = run(world, 0);
    assert!(matches!(outcome.result, Err(Stop::Failed(_))));
    assert!(outcome.world.devices.is_empty());
}

#[test]
fn a_device_that_stays_live_is_refused_after_the_stated_number_of_looks() {
    let world = World::new().attached(4, DeviceState::Live, EARLIER);
    let outcome = run(world, 0);
    assert_eq!(rules(&outcome), Ok(()));
    assert_eq!(outcome.result, Err(Stop::Held));
    let looks = outcome
        .world
        .events
        .iter()
        .filter(|e| **e == Event::Info(4))
        .count();
    assert_eq!(looks, 1 + WANTED.live_retries as usize);
}

#[test]
fn a_deleted_device_that_is_still_held_keeps_its_number_and_is_never_taken_over() {
    // The volume failed while a client held device 0: the delete
    // returns at once and the device stays, dead.
    let mut world = World::new().attached(0, DeviceState::Waiting, EARLIER);
    world.held.push(0);
    world.opens.push_back(OpenError::MarkedFailed);
    let outcome = run(world, 0);
    assert!(matches!(outcome.result, Err(Stop::Failed(_))));
    assert_eq!(
        outcome.world.devices.get(&0).map(|d| d.state),
        Some(DeviceState::Dead)
    );

    assert_eq!(
        outcome.world.failure.map(|f| f.map(|f| f.held)),
        Ok(Some(Held::Yes))
    );

    // The volume is not served again in this boot, whatever the
    // operator says: the store is not opened, the dead device is not
    // taken over and not deleted again.
    for wanted in [WANTED, ACKNOWLEDGING] {
        let mut next = outcome.world.clone();
        next.events.clear();
        next.failure_reads.clear();
        next.fresh_notes.clear();
        let outcome = run_as(next, &wanted, 0);
        assert_eq!(rules(&outcome), Ok(()));
        assert!(matches!(outcome.result, Err(Stop::Failed(_))));
        assert_eq!(outcome.world.events, [Event::Features, Event::Info(0)]);
        assert_eq!(
            outcome.world.devices.get(&0).map(|d| d.state),
            Some(DeviceState::Dead)
        );
    }
}

#[test]
fn a_record_of_another_boot_or_of_a_device_that_is_gone_means_a_fresh_device() {
    // Another boot: the number in the record means nothing now, and the
    // device under it is someone else's.
    let mut world = World::new().attached(0, DeviceState::Waiting, EARLIER);
    if let Some(attachment) = world.attachment.as_mut() {
        attachment.boot = [9; 16];
    }
    let outcome = run(world, 0);
    assert_eq!(rules(&outcome), Ok(()));
    let ready = outcome.result.expect("serves");
    assert_eq!((ready.number, ready.took_over), (1, false));
    assert!(!outcome
        .world
        .events
        .iter()
        .any(|e| matches!(e, Event::Delete(_) | Event::BeginRecovery(_))));

    // The device is gone.
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.devices.clear();
    let outcome = run(world, 0);
    assert_eq!(rules(&outcome), Ok(()));
    let ready = outcome.result.expect("serves");
    assert_eq!((ready.number, ready.took_over), (0, false));
    assert_ne!(
        outcome.world.notes.get(&0),
        Some(&[0xA5; 16]),
        "a fresh nonce"
    );
}

/// What the kernel is made to lack, one case each: every needed flag,
/// the features command, a release below 6.9 and one that does not
/// parse; each of the flags and the command also on a kernel below
/// 6.11. With the operator's word for the asynchronous delete, the
/// release is not read and only a lacking feature refuses.
fn unsupported_kernels() -> Vec<(Option<u64>, &'static str, bool)> {
    use crate::features::NEEDED;
    let mut features: Vec<Option<u64>> = NEEDED.iter().map(|f| Some(!f.bit)).collect();
    features.push(None);
    let mut cases = Vec::new();
    for asserted in [false, true] {
        for release in ["7.0.0", "6.11.0", "6.10.3", "6.8.0-45-generic", "linux"] {
            for &f in &features {
                cases.push((f, release, asserted));
            }
        }
    }
    for release in [
        "6.9.0",
        "6.10.3",
        "6.8.0-45-generic",
        "5.14.0-570.el9.x86_64",
        "linux",
    ] {
        cases.push((Some(u64::MAX), release, false));
    }
    cases
}

/// How often a refusal reads the release: once, unless the operator
/// vouched for the delete.
fn release_reads(asserted: bool) -> usize {
    usize::from(!asserted)
}

fn lacks_a_feature(mutants: u32) -> Result<(), String> {
    for (features, release, asserted) in unsupported_kernels() {
        let what = format!("{features:x?} on {release} (asserted: {asserted})");
        let why = crate::features::check(features, release, asserted)
            .expect_err("each case lacks something");
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.features = features;
        world.release = release.to_string();
        let wanted = Wanted {
            async_delete_asserted: asserted,
            ..WANTED
        };
        let outcome = run_as(world, &wanted, mutants);
        rules(&outcome)?;
        check(
            outcome.result == Err(Stop::Unsupported(why)),
            &format!("{what}: refused"),
        )?;
        check(
            outcome.world.events == [Event::Features],
            &format!("{what}: nothing else is done, and the store is not opened"),
        )?;
        check(
            outcome.world.release_reads == release_reads(asserted),
            &format!("{what}: the release is read unless the operator vouched"),
        )?;
    }
    Ok(())
}

/// The same for a detach: it deletes with the same command.
fn a_detach_lacks_a_feature(mutants: u32) -> Result<(), String> {
    for (features, release, asserted) in unsupported_kernels() {
        let what = format!("{features:x?} on {release} (asserted: {asserted})");
        let why = crate::features::check(features, release, asserted)
            .expect_err("each case lacks something");
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.features = features;
        world.release = release.to_string();
        let (mut control, mut records, shared) = fakes(world);
        let detached = detach_with(
            &mut control,
            &mut records,
            &mut Fake(shared.clone()),
            (3, GONE, asserted),
            Mutants(mutants),
        )
        .map_err(|e| e.to_string())?;
        check(
            detached == Detached::Unsupported(why),
            &format!("{what}: refused, not {detached:?}"),
        )?;
        let world = shared.borrow();
        check(
            world.events == [Event::Features],
            &format!("{what}: nothing else is looked at or done"),
        )?;
        check(
            world.release_reads == release_reads(asserted),
            &format!("{what}: the release is read unless the operator vouched"),
        )?;
    }
    Ok(())
}

/// A kernel with everything, below 6.9 with the operator's word: a
/// start serves and a detach deletes.
#[test]
fn the_operators_word_for_the_delete_lets_an_older_release_serve() {
    for release in ["6.8.0-45-generic", "5.14.0-570.el9.x86_64", "linux"] {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.release = release.to_string();
        let wanted = Wanted {
            async_delete_asserted: true,
            ..WANTED
        };
        let outcome = run_as(world.clone(), &wanted, 0);
        assert_eq!(rules(&outcome), Ok(()));
        assert!(
            outcome.result.is_ok(),
            "{release}: {:?}",
            outcome.result.err()
        );
        assert_eq!(outcome.world.release_reads, 0, "{release}");

        let (mut control, mut records, shared) = fakes(world);
        let detached = detach(
            &mut control,
            &mut records,
            &mut Fake(shared.clone()),
            3,
            GONE,
            true,
        );
        assert_eq!(detached.expect("detach"), Detached::Deleted(4), "{release}");
    }
}

impl PartialEq for Ready<Opened> {
    fn eq(&self, other: &Self) -> bool {
        (self.number, self.shape, self.took_over) == (other.number, other.shape, other.took_over)
    }
}

#[test]
fn a_kernel_that_lacks_a_needed_feature_is_refused_before_anything_is_touched() {
    let defect = [("no feature check", Mutants::NO_FEATURE_CHECK)];
    holds(lacks_a_feature, &defect);
    holds(a_detach_lacks_a_feature, &defect);
}

/// The volume has failed: the waiting device is deleted, never taken
/// over, and nothing is added.
fn gives_up(world: World, mutants: u32) -> Result<(), String> {
    let outcome = run(world, mutants);
    rules(&outcome)?;
    check(
        matches!(outcome.result, Err(Stop::Failed(_))),
        "the volume counts as failed",
    )?;
    check(
        outcome.world.devices.is_empty(),
        "the waiting device is deleted",
    )
}

fn marked(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.opens.push_back(OpenError::MarkedFailed);
    gives_up(world, mutants)
}

#[test]
fn a_volume_that_failed_while_held_loses_its_waiting_device_and_is_refused() {
    holds(marked, &[]);
}

#[test]
fn a_failed_volume_deletes_no_device_that_is_not_its_own() {
    // Live under another server, or with another's note: left.
    for change in [0, 1] {
        let state = if change == 0 {
            DeviceState::Live
        } else {
            DeviceState::Waiting
        };
        let mut world = World::new().attached(4, state, EARLIER);
        if change == 1 {
            world.notes.insert(4, [0x5A; 16]);
        }
        world.opens.push_back(OpenError::MarkedFailed);
        assert_eq!(
            untouched(world, 0, |s| matches!(s, Stop::Failed(_))),
            Ok(())
        );
    }
}

#[test]
fn a_locked_store_is_tried_again_a_stated_number_of_times_and_no_device_is_touched() {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.locked_tries = 4;
    let devices = world.devices.clone();
    let outcome = run(world, 0);
    assert_eq!(rules(&outcome), Ok(()));
    assert_eq!(outcome.result, Err(Stop::Held));
    assert_eq!(outcome.world.devices, devices);
    let pauses = outcome
        .world
        .events
        .iter()
        .filter(|e| **e == Event::Pause)
        .count();
    assert_eq!(pauses, 3);
    assert!(!outcome
        .world
        .events
        .iter()
        .any(|e| matches!(e, Event::Info(_))));

    // The lock of a killed daemon goes: the next try serves.
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.locked_tries = 3;
    let outcome = run(world, 0);
    assert_eq!(rules(&outcome), Ok(()));
    assert!(outcome.result.expect("serves").took_over);
}

#[test]
fn an_error_that_may_pass_stops_the_daemon_and_leaves_the_device() {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.opens.push_back(OpenError::Other("no space".into()));
    assert_eq!(untouched(world, 0, |s| matches!(s, Stop::Error(_))), Ok(()));
}

#[test]
fn a_note_that_cannot_be_read_fails_the_volume_and_leaves_the_device() {
    for state in [DeviceState::Waiting, DeviceState::Dead, DeviceState::Live] {
        let mut world = World::new().attached(4, state, EARLIER);
        world.torn_note = Some(4);
        assert_eq!(
            untouched(world, 0, |s| matches!(s, Stop::Failed(_))),
            Ok(())
        );
    }
}

/// Kill the daemon after each number of steps of a start, then start
/// again. The second start serves, by the rules, or stops and touches
/// nothing. Beside the device it serves and the devices of others, at
/// most one device is there: one that was added and never started and
/// has no note, from a kill between the add and the note. Returns how
/// many kills left such a device, and after how many the second start
/// stopped.
fn killed_anywhere(
    world: fn() -> World,
    took_over: bool,
    mutants: u32,
) -> Result<(usize, usize), String> {
    let others: Vec<u32> = world().devices.keys().copied().collect();
    let steps = run(world(), 0).world.events.len();
    let (mut leaked, mut stopped) = (0, 0);
    for at in 0..=steps {
        let mut first = world();
        first.dies_at = Some(at);
        let mut between = run(first, mutants).world;
        between.dies_at = None;
        between.events.clear();
        let (devices, attachment) = (between.devices.clone(), between.attachment);
        let outcome = run(between, mutants);
        rules(&outcome).map_err(|e| format!("killed after {at} steps: {e}"))?;
        let ready = match &outcome.result {
            Ok(ready) => ready,
            Err(Stop::Error(_)) => {
                check(
                    outcome.world.devices == devices && outcome.world.attachment == attachment,
                    &format!("killed after {at} steps: stopped, and changed something"),
                )?;
                stopped += 1;
                continue;
            }
            Err(e) => return Err(format!("killed after {at} steps: {e:?}")),
        };
        check(
            ready.took_over == took_over,
            &format!("killed after {at} steps: took over or not"),
        )?;
        let extra: Vec<u32> = outcome
            .world
            .devices
            .keys()
            .copied()
            .filter(|n| *n != ready.number && !(took_over || others.contains(n)))
            .collect();
        for number in &extra {
            let never_started =
                outcome.world.devices.get(number).map(|d| d.state) == Some(DeviceState::Dead);
            check(
                never_started && !outcome.world.notes.contains_key(number),
                &format!("killed after {at} steps: device {number} was left behind"),
            )?;
            check(
                ready.left.is_some_and(|(left, _)| left == *number),
                &format!("killed after {at} steps: device {number} was left and not said"),
            )?;
        }
        check(extra.len() <= 1, "more than one device was left")?;
        leaked += extra.len();
    }
    Ok((leaked, stopped))
}

/// Device 0 is another's and is seen. Device 1 is another's, live, and
/// appears after the daemon looked.
fn with_strangers(shape: Shape) -> World {
    let mut world = World::new();
    world.late.push(1);
    for number in 0..2 {
        world.devices.insert(
            number,
            DeviceInfo {
                state: DeviceState::Live,
                shape,
            },
        );
    }
    world
}

#[test]
fn a_daemon_killed_at_any_step_of_a_start_is_followed_by_one_that_serves_one_device() {
    // The kills between the add and the note leave a device: before
    // the disowning, and after it.
    assert_eq!(killed_anywhere(World::new, false, 0), Ok((2, 0)));
    assert_eq!(
        killed_anywhere(
            || World::new().attached(4, DeviceState::Waiting, EARLIER),
            true,
            0
        ),
        Ok((0, 0))
    );
    // Another's device appears at the number just recorded, and the
    // daemon is killed before it records the next: the record names a
    // live device that is not this volume's. With other values than
    // ours it is known for a stranger's, and the start attaches afresh.
    assert_eq!(
        killed_anywhere(|| with_strangers(EARLIER), false, 0),
        Ok((2, 0))
    );
    // A twin, with exactly the values this daemon records: after the
    // two kills that leave the record naming it (after the add that
    // found it, and after the look at the next number), the start
    // stops and touches nothing.
    let twin = Shape {
        size: SIZE,
        queues: WANTED.queues,
        depth: WANTED.depth,
        flags: WANTED.flags,
    };
    assert_eq!(twin, run(World::new(), 0).result.expect("serves").shape);
    let twins = || {
        with_strangers(Shape {
            size: SIZE,
            queues: WANTED.queues,
            depth: WANTED.depth,
            flags: WANTED.flags,
        })
    };
    assert_eq!(killed_anywhere(twins, false, 0), Ok((2, 2)));
}

/// Whatever is given this volume's old number, with the same values,
/// is not taken for this volume's by the next start: it is neither
/// taken over nor deleted.
fn a_stranger_arrives(mut world: World, mutants: u32, at: usize) -> Result<(), String> {
    world.dies_at = None;
    world.events.clear();
    if world.devices.contains_key(&4) {
        // Not deleted yet: there is no freed number to arrive on.
        return Ok(());
    }
    world.devices.insert(
        4,
        DeviceInfo {
            state: DeviceState::Waiting,
            shape: EARLIER,
        },
    );
    let outcome = run(world, mutants);
    check(
        !outcome.result.as_ref().is_ok_and(|r| r.took_over),
        &format!("killed after {at} steps: a stranger was taken over"),
    )?;
    check(
        outcome.world.devices.get(&4).map(|d| d.state) == Some(DeviceState::Waiting),
        &format!("killed after {at} steps: a stranger was deleted"),
    )
}

/// The volume fails and its waiting device is deleted, and the daemon
/// is killed after each number of steps.
fn a_failed_volumes_number_is_given_to_another(mutants: u32) -> Result<(), String> {
    for at in 0..=8 {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.opens.push_back(OpenError::MarkedFailed);
        world.dies_at = Some(at);
        let outcome = run(world, mutants);
        a_stranger_arrives(outcome.world, mutants, at)?;
    }
    Ok(())
}

/// A detach is killed after each number of steps.
fn a_detached_volumes_number_is_given_to_another(mutants: u32) -> Result<(), String> {
    for at in 0..=7 {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.dies_at = Some(at);
        let (mut control, mut records, shared) = fakes(world);
        let _ = detach_with(
            &mut control,
            &mut records,
            &mut Fake(shared.clone()),
            (3, GONE, false),
            Mutants(mutants),
        );
        drop((control, records));
        let world = match Rc::try_unwrap(shared) {
            Ok(cell) => cell.into_inner(),
            Err(_) => panic!("the fakes are dropped"),
        };
        a_stranger_arrives(world, mutants, at)?;
    }
    Ok(())
}

#[test]
fn a_note_does_not_outlive_its_device() {
    let defect = [(
        "delete before the note is removed",
        Mutants::DELETE_BEFORE_NOTE,
    )];
    holds(a_failed_volumes_number_is_given_to_another, &defect);
    holds(a_detached_volumes_number_is_given_to_another, &defect);
}

/// A detach killed after each number of steps, then a start: it serves
/// one device by the rules, or stops and says the device has no note.
#[test]
fn a_detach_killed_at_any_step_is_followed_by_a_start_that_is_safe() {
    let mut stopped = 0;
    for at in 0..=7 {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.dies_at = Some(at);
        let (mut control, mut records, shared) = fakes(world);
        let _ = detach(
            &mut control,
            &mut records,
            &mut Fake(shared.clone()),
            3,
            GONE,
            false,
        );
        drop((control, records));
        let mut world = match Rc::try_unwrap(shared) {
            Ok(cell) => cell.into_inner(),
            Err(_) => panic!("the fakes are dropped"),
        };
        world.dies_at = None;
        world.events.clear();
        let had_device = world.devices.contains_key(&4);
        let had_note = world.notes.contains_key(&4);
        let outcome = run(world, 0);
        assert_eq!(rules(&outcome), Ok(()), "killed after {at} steps");
        match (&outcome.result, had_device, had_note) {
            // Untouched: taken over.
            (Ok(ready), true, true) => assert!(ready.took_over),
            // The note gone and the device still there: it cannot be
            // told whose the device is.
            (Err(Stop::Error(e)), true, false) => {
                assert!(e.contains("no note"), "{e}");
                stopped += 1;
            }
            // Deleted: a fresh device.
            (Ok(ready), false, _) => assert!(!ready.took_over),
            other => panic!("killed after {at} steps: {other:?}"),
        }
    }
    assert_eq!(stopped, 1, "the kill between the note and the delete");
}

fn fakes(world: World) -> (Fake, Fake, Rc<RefCell<World>>) {
    let shared = Rc::new(RefCell::new(world));
    (Fake(shared.clone()), Fake(shared.clone()), shared)
}

#[test]
fn locate_says_whose_the_recorded_device_is_and_changes_nothing() {
    let all = [
        DeviceState::Dead,
        DeviceState::Live,
        DeviceState::Waiting,
        DeviceState::Dead,
    ];
    for state in all {
        // (the note, the kernel's depth, whose)
        let table = [
            (Some([0xA5; 16]), 32, Whose::Ours),
            (Some([0xA5; 16]), 16, Whose::Ours),
            (Some([0x5A; 16]), 32, Whose::Anothers),
            (None, 16, Whose::Anothers),
            (None, 32, Whose::Unsure),
        ];
        for (note, depth, whose) in table {
            let mut world = World::new().attached(4, state, EARLIER);
            world.notes.clear();
            if let Some(note) = note {
                world.notes.insert(4, note);
            }
            if let Some(device) = world.devices.get_mut(&4) {
                device.shape.depth = depth;
            }
            let (mut control, mut records, shared) = fakes(world);
            let located = locate(&mut control, &mut records).expect("read");
            let located = located.expect("a device");
            assert_eq!(
                (located.number, located.state, located.whose),
                (4, state, whose)
            );
            assert_eq!(located.shape.depth, depth);
            // One look, and nothing else: it neither waits nor changes.
            assert_eq!(shared.borrow().events, [Event::Info(4)]);
        }
    }

    // No record, a record of another boot, a device that is gone.
    let (mut control, mut records, _) = fakes(World::new());
    assert_eq!(locate(&mut control, &mut records).expect("read"), None);
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    if let Some(attachment) = world.attachment.as_mut() {
        attachment.boot = [9; 16];
    }
    let (mut control, mut records, shared) = fakes(world);
    assert_eq!(locate(&mut control, &mut records).expect("read"), None);
    assert!(
        shared.borrow().events.is_empty(),
        "the number is not looked up"
    );
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.devices.clear();
    let (mut control, mut records, _) = fakes(world);
    assert_eq!(locate(&mut control, &mut records).expect("read"), None);

    // A note that is not a whole record is an error.
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.torn_note = Some(4);
    let (mut control, mut records, _) = fakes(world);
    assert!(locate(&mut control, &mut records).is_err());
}

/// What the driver does with a delete, for the tests of the look that
/// follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Going {
    /// Nobody holds the device and it is gone when the delete returns.
    AtOnce,
    /// Nobody holds it, and it is still there for two looks: the
    /// driver's delete does not wait.
    Late,
    /// A client holds it: it stays, dead.
    Held,
}

/// A detach deletes without waiting and then looks until the device is
/// gone or the stated looks are used: a device that goes late was held
/// by nobody, one that outlasts them is held. Mutant: the first look
/// after the delete decides.
fn detach_says_held_only_of_a_device_that_stays(mutants: u32) -> Result<(), String> {
    for state in [DeviceState::Waiting, DeviceState::Dead] {
        for going in [Going::AtOnce, Going::Late, Going::Held] {
            let mut world = World::new().attached(4, state, EARLIER);
            match going {
                Going::AtOnce => {}
                Going::Late => world.delete_takes = Some(2),
                Going::Held => world.held.push(4),
            }
            let (mut control, mut records, shared) = fakes(world);
            let detached = detach_with(
                &mut control,
                &mut records,
                &mut Fake(shared.clone()),
                (3, GONE, false),
                Mutants(mutants),
            )
            .map_err(|e| e.to_string())?;
            let world = shared.borrow();
            let what = format!("{state:?}, {going:?}");
            // The looks after the delete: one, and one more after a
            // pause for each time the device was still there.
            let looks = match going {
                Going::AtOnce => 0,
                Going::Late => 2,
                Going::Held => GONE as usize,
            };
            if going == Going::Held {
                check(
                    detached == Detached::DeletedHeld(4),
                    &format!("{what}: {detached:?}"),
                )?;
                check(
                    world.devices.get(&4).map(|d| d.state) == Some(DeviceState::Dead),
                    &format!("{what}: the held device is dead"),
                )?;
            } else {
                check(
                    detached == Detached::Deleted(4),
                    &format!("{what}: {detached:?}"),
                )?;
                check(world.devices.is_empty(), &format!("{what}: gone"))?;
            }
            check(world.notes.is_empty(), "the note is gone")?;
            check(world.attachment.is_none(), "the record is gone")?;
            // The kernel's features, then the note, one delete, the looks,
            // the record last.
            let mut want = vec![
                Event::Features,
                Event::Info(4),
                Event::RemoveNote(4),
                Event::Delete(4),
                Event::Info(4),
            ];
            for _ in 0..looks {
                want.extend([Event::Pause, Event::Info(4)]);
            }
            want.push(Event::RemoveAttachment);
            check(
                world.events == want,
                &format!("{what}: the steps were {:?}", world.events),
            )?;
        }
    }
    Ok(())
}

#[test]
fn detach_deletes_this_volumes_unserved_device_without_waiting_and_says_if_it_is_held() {
    holds(
        detach_says_held_only_of_a_device_that_stays,
        &[(
            "held decided by the first look after the delete",
            Mutants::HELD_AT_FIRST_LOOK,
        )],
    );
}

/// One case of [`successors`]: its name, the device, its note, whether
/// the note is unreadable, and whether the deleted device then counts
/// as still there.
type Successor = (&'static str, DeviceInfo, Option<[u8; 16]>, bool, bool);

/// What takes a number right after its device is gone, for the tests
/// of the looks: the device, its note, whether the note is unreadable,
/// and whether the deleted device then counts as still there.
fn successors() -> Vec<Successor> {
    let other = Shape {
        size: EARLIER.size * 2,
        ..EARLIER
    };
    let device = |state, shape| DeviceInfo { state, shape };
    vec![
        // Another volume's device: gone, and nobody held it.
        (
            "waiting, the same values, with its note",
            device(DeviceState::Waiting, EARLIER),
            Some([0x5A; 16]),
            false,
            false,
        ),
        (
            "dead, the same values, with its note",
            device(DeviceState::Dead, EARLIER),
            Some([0x5A; 16]),
            false,
            false,
        ),
        (
            "live, the same values, no note yet",
            device(DeviceState::Live, EARLIER),
            None,
            false,
            false,
        ),
        (
            "dead, other values, no note yet",
            device(DeviceState::Dead, other),
            None,
            false,
            false,
        ),
        // Not told apart from the deleted device: still there.
        (
            "dead, the same values, no note",
            device(DeviceState::Dead, EARLIER),
            None,
            false,
            true,
        ),
        (
            "waiting, the same values, no note",
            device(DeviceState::Waiting, EARLIER),
            None,
            false,
            true,
        ),
        (
            "dead, the same values, a note that cannot be read",
            device(DeviceState::Dead, EARLIER),
            Some([0x5A; 16]),
            true,
            true,
        ),
    ]
}

/// The looks after a delete ask about the deleted device, not about
/// the number: nobody holds the device, it is there at the first look
/// and gone at the second, and at that very look another volume's
/// device stands under the number. A detach then says deleted, not
/// held, unless what stands there cannot be told from the deleted
/// device; and it leaves the other device and its note alone. Mutant:
/// whatever stands under the number counts.
fn detach_is_not_held_by_another_volumes_device(mutants: u32) -> Result<(), String> {
    for (what, device, note, torn, stays) in successors() {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.delete_takes = Some(0);
        world.successor = Some((device, note, torn));
        let (mut control, mut records, shared) = fakes(world);
        let detached = detach_with(
            &mut control,
            &mut records,
            &mut Fake(shared.clone()),
            (3, GONE, false),
            Mutants(mutants),
        )
        .map_err(|e| e.to_string())?;
        let world = shared.borrow();
        let wanted = if stays {
            Detached::DeletedHeld(4)
        } else {
            Detached::Deleted(4)
        };
        check(detached == wanted, &format!("{what}: {detached:?}"))?;
        check(
            world.devices.get(&4) == Some(&device),
            &format!("{what}: the other device is as it was"),
        )?;
        check(
            world.notes.get(&4).copied() == note,
            &format!("{what}: the other device's note is as it was"),
        )?;
        check(
            world
                .events
                .iter()
                .filter(|e| **e == Event::Delete(4))
                .count()
                == 1,
            &format!("{what}: one delete"),
        )?;
        check(world.attachment.is_none(), "the record is gone")?;
    }
    Ok(())
}

#[test]
fn a_number_taken_by_another_volume_after_a_delete_does_not_make_the_deleted_device_held() {
    holds(
        detach_is_not_held_by_another_volumes_device,
        &[(
            "whatever stands under the number counts as the deleted device",
            Mutants::ANY_OCCUPANT_LINGERS,
        )],
    );
    holds(
        settling_is_not_held_by_another_volumes_device,
        &[(
            "whatever stands under the number counts as the deleted device",
            Mutants::ANY_OCCUPANT_LINGERS,
        )],
    );
}

/// The same for a start that settles a failure of unknown holding: the
/// failing daemon died before it deleted the device, this start
/// deletes it, and another volume's device takes the number as it
/// goes. The failure is settled as not held and acknowledged, and this
/// volume gets a device under another number (or the start stops on
/// its own rule for a device with this volume's values and no note,
/// which refuses nothing for the boot); what cannot be told from the
/// deleted device keeps the volume refused for the boot.
fn settling_is_not_held_by_another_volumes_device(mutants: u32) -> Result<(), String> {
    for (what, device, note, torn, stays) in successors() {
        let mut world = unknown_world();
        world.notes.insert(4, [0xA5; 16]);
        if let Some(own) = world.devices.get_mut(&4) {
            own.state = DeviceState::Waiting;
        }
        world.delete_takes = Some(0);
        world.successor = Some((device, note, torn));
        let outcome = run_as(world, &ACKNOWLEDGING, mutants);
        if stays {
            check(
                refusal(&outcome)?.contains("not served again in this boot"),
                &format!("{what}: refused for the boot"),
            )?;
            check(
                settled_as(&outcome)? == Held::Yes,
                &format!("{what}: written down as held"),
            )?;
        } else {
            // Not written down as held, whatever the start then does.
            check(
                !matches!(outcome.world.failure, Ok(Some(failure)) if failure.held == Held::Yes),
                &format!("{what}: written down as held"),
            )?;
            match &outcome.result {
                Ok(ready) => check(
                    !ready.took_over && ready.number != 4,
                    &format!("{what}: a fresh device under another number"),
                )?,
                // The start's own rule for a device under the recorded
                // number with this volume's values and no note: whose
                // it is cannot be told, and the start stops. That is
                // not the looks' verdict, and nothing is refused for
                // the boot.
                Err(Stop::Error(why))
                    if device.shape == EARLIER && note.is_none() && why.contains("no note") => {}
                other => return Err(format!("{what}: {other:?}")),
            }
        }
        check(
            outcome.world.devices.get(&4) == Some(&device),
            &format!("{what}: the other device is as it was"),
        )?;
    }
    Ok(())
}

#[test]
fn detach_touches_nothing_it_may_not() {
    // Live under a server.
    let world = World::new().attached(4, DeviceState::Live, EARLIER);
    let (mut control, mut records, shared) = fakes(world);
    assert_eq!(
        detach(
            &mut control,
            &mut records,
            &mut Fake(shared.clone()),
            3,
            GONE,
            false
        )
        .expect("detach"),
        Detached::Live(4)
    );
    assert_eq!(shared.borrow().events, [Event::Features, Event::Info(4)]);
    // Not known to be this volume's, in any state.
    for state in STATES {
        for note in [None, Some([0x5A; 16])] {
            let mut world = World::new().attached(4, state, EARLIER);
            world.notes.clear();
            if let Some(note) = note {
                world.notes.insert(4, note);
            }
            let (mut control, mut records, shared) = fakes(world);
            let detached = detach(
                &mut control,
                &mut records,
                &mut Fake(shared.clone()),
                3,
                GONE,
                false,
            )
            .expect("detach");
            if state == DeviceState::Dead && note.is_none() {
                // How this volume's device looks once it was deleted
                // while something held it: said apart, and left.
                assert_eq!(detached, Detached::DeadNoNote(4));
                assert!(shared.borrow().attachment.is_some(), "the record stays");
            } else {
                assert!(matches!(detached, Detached::NotOurs(l) if l.number == 4));
            }
            assert_eq!(shared.borrow().events, [Event::Features, Event::Info(4)]);
            assert_eq!(shared.borrow().notes.get(&4).copied(), note);
        }
    }
    // No device.
    let (mut control, mut records, shared) = fakes(World::new());
    assert_eq!(
        detach(
            &mut control,
            &mut records,
            &mut Fake(shared.clone()),
            3,
            GONE,
            false
        )
        .expect("detach"),
        Detached::Nothing
    );
    assert!(shared.borrow().events.is_empty());
}

/// What a second daemon that came to serve still has at the end: its
/// device, live, and the record and the note that name it.
fn served_intact(world: &World) -> Result<(), String> {
    let Some(Ok((number, _))) = world.intruder else {
        return Ok(());
    };
    check(
        world.failure == Ok(None),
        "a daemon serves a volume that is noted as failed in this boot",
    )?;
    check(
        world.devices.get(&number).map(|d| d.state) == Some(DeviceState::Live),
        &format!("the serving daemon's device {number} was taken from it"),
    )?;
    check(
        world
            .attachment
            .is_some_and(|a| a.number == number && world.notes.get(&number) == Some(&a.nonce)),
        &format!("the serving daemon's record or note for device {number} was taken from it"),
    )
}

fn unwrap(shared: Rc<RefCell<World>>) -> World {
    match Rc::try_unwrap(shared) {
        Ok(cell) => cell.into_inner(),
        Err(_) => panic!("the fakes are dropped"),
    }
}

/// A daemon starts between any two steps of a detach. Whichever of the
/// two has the lock, the other finds the volume locked: a daemon that
/// came to serve keeps its device, its record and its note.
fn a_daemon_starts_during_a_detach(mutants: u32) -> Result<(), String> {
    for state in [DeviceState::Waiting, DeviceState::Dead] {
        for at in 0..=7 {
            let mut world = World::new().attached(4, state, EARLIER);
            world.intrude_at = Some(at);
            let (mut control, mut records, shared) = fakes(world);
            let detached = detach_with(
                &mut control,
                &mut records,
                &mut Fake(shared.clone()),
                (3, GONE, false),
                Mutants(mutants),
            );
            drop((control, records));
            let world = unwrap(shared);
            served_intact(&world).map_err(|e| format!("{state:?}, after {at} steps: {e}"))?;
            match (&world.intruder, &detached) {
                // The daemon was first: the detach finds the volume
                // locked.
                (Some(Ok(_)), Ok(Detached::Locked)) => {}
                (Some(Ok(_)), other) if mutants == 0 => {
                    return Err(format!(
                        "{state:?}, after {at} steps: the daemon serves and the detach \
                         answered {other:?}"
                    ))
                }
                // The detach was first: the daemon finds it locked.
                (Some(Err(stop)), _) if mutants == 0 => {
                    check(
                        *stop == Stop::Held,
                        &format!("the daemon stopped with {stop:?}"),
                    )?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[test]
fn a_detach_and_a_start_never_act_on_one_volume_at_once() {
    holds(
        a_daemon_starts_during_a_detach,
        &[("detach without the lock", Mutants::DETACH_WITHOUT_LOCK)],
    );
}

/// A second start between any two steps of a start that fails on the
/// store's marker while a record of this boot names a device. The fake
/// gives the marker to one open only, so that a second start whose
/// open comes later serves: a delete of the first that is not under
/// the lock is then seen to take its device from it. (Until M1.3t-b
/// this was the log store's failed recovery sync, which did not
/// repeat.)
fn a_daemon_starts_during_a_failing_start(mutants: u32) -> Result<(), String> {
    for at in 0..=8 {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.opens.push_back(OpenError::MarkedFailed);
        world.intrude_at = Some(at);
        let outcome = run(world, mutants);
        served_intact(&outcome.world).map_err(|e| format!("after {at} steps: {e}"))?;
    }
    Ok(())
}

#[test]
fn a_start_that_fails_deletes_under_the_lock() {
    let defect = [("unlocked before the delete", Mutants::UNLOCK_BEFORE_FAILED)];
    holds(a_daemon_starts_during_a_failing_start, &defect);
}

#[test]
fn a_detach_of_a_volume_a_daemon_holds_is_refused_and_touches_nothing() {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.locked_tries = 10;
    let (mut control, mut records, shared) = fakes(world);
    let detached = detach(
        &mut control,
        &mut records,
        &mut Fake(shared.clone()),
        3,
        GONE,
        false,
    );
    assert_eq!(detached.expect("detach"), Detached::Locked);
    assert_eq!(shared.borrow().events, vec![Event::Pause; 3]);

    // The lock of a daemon that was just killed goes: the detach acts.
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.locked_tries = 3;
    let (mut control, mut records, shared) = fakes(world);
    let detached = detach(
        &mut control,
        &mut records,
        &mut Fake(shared.clone()),
        3,
        GONE,
        false,
    );
    assert_eq!(detached.expect("detach"), Detached::Deleted(4));
    // Every step of it under the lock, which is free again afterwards.
    let world = shared.borrow();
    for (i, event) in world.events.iter().enumerate() {
        if *event != Event::Pause {
            assert_eq!(world.under_lock.get(i), Some(&true), "{event:?}");
        }
    }
    assert_eq!(world.lock_holder, None);
}

/// A failed volume deletes by a record of this boot only: with a run
/// directory that survives a reboot, the note of the last boot is
/// there, and the device under the number is a stranger's. The volume
/// failed in this boot with no device of its own (its image failed at a
/// start), and the next start, which does not acknowledge it, gives up
/// whatever device of the volume it finds. (Until M1.3t-b the failure
/// here was the log store's failed recovery sync.)
fn a_record_of_another_boot(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    if let Some(attachment) = world.attachment.as_mut() {
        attachment.boot = [9; 16];
    }
    world.failure = Ok(Some(Failure {
        held: Held::No,
        reason: Reason::ImageFailed,
        identity: [3; 16],
    }));
    untouched(world, mutants, |s| matches!(s, Stop::Failed(_)))
}

/// This volume's device, with its note, that has no server and waits
/// for none (never started, or deleted and still held) is not taken
/// over: it is deleted, again if need be, and the daemon attaches
/// afresh, passing its number by while it is held.
fn our_dead_device(mutants: u32) -> Result<(), String> {
    for held in [false, true] {
        let mut world = World::new().attached(4, DeviceState::Dead, EARLIER);
        // Number 0 is another's, so that "afresh" is seen to pass over
        // taken numbers.
        world.devices.insert(
            0,
            DeviceInfo {
                state: DeviceState::Live,
                shape: EARLIER,
            },
        );
        if held {
            world.held.push(4);
        }
        let outcome = run(world, mutants);
        rules(&outcome)?;
        let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
        check(
            !ready.took_over && ready.number != 0 && (ready.number != 4 || !held),
            "a fresh device under a free number",
        )?;
        check(
            !any(&outcome.world, |e| matches!(e, Event::BeginRecovery(_))),
            "no recovery",
        )?;
        let events = &outcome.world.events;
        let removed = events.iter().position(|e| *e == Event::RemoveNote(4));
        let deleted = events.iter().position(|e| *e == Event::Delete(4));
        check(
            removed.is_some() && removed < deleted,
            "the note is removed, then the device deleted",
        )?;
        check(
            outcome.world.devices.get(&4).map(|d| d.state)
                == held
                    .then_some(DeviceState::Dead)
                    .filter(|_| ready.number != 4),
            "the dead device is gone, or stays while it is held",
        )?;
    }
    Ok(())
}

#[test]
fn rules_the_review_found_no_test_for() {
    holds(
        a_record_of_another_boot,
        &[(
            "failed deletes by any boot's record",
            Mutants::FAILED_ANY_BOOT,
        )],
    );
    holds(
        our_dead_device,
        &[("a dead device taken over", Mutants::TAKE_DEAD)],
    );
}

#[test]
fn a_failed_volume_leaves_a_noted_device_whose_values_are_not_the_recorded_ones() {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    if let Some(device) = world.devices.get_mut(&4) {
        device.shape.depth = 16;
    }
    world.opens.push_back(OpenError::MarkedFailed);
    assert_eq!(
        untouched(
            world,
            0,
            |s| matches!(s, Stop::Failed(why) if why.contains("was left"))
        ),
        Ok(())
    );
}

#[test]
fn a_device_that_is_not_of_the_stores_size_is_not_taken_over() {
    // The record and the kernel agree, and the store that was opened
    // is of another size: this is not the device of this store.
    let other = Shape {
        size: SIZE * 2,
        ..EARLIER
    };
    let world = World::new().attached(4, DeviceState::Waiting, other);
    assert_eq!(
        untouched(
            world,
            0,
            |s| matches!(s, Stop::Error(why) if why.contains("bytes"))
        ),
        Ok(())
    );
}

// The failure note.

fn note(held: bool) -> Failure {
    Failure {
        held: held.into(),
        reason: Reason::LogFailed,
        identity: [3; 16],
    }
}

/// The volume failed in this boot while it was served, and the daemon
/// died before it deleted the device: the note, the store's marker, and
/// the device, waiting.
fn failed_world(held: bool) -> World {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.failure = Ok(Some(note(held)));
    world.marked = true;
    world
}

fn any(world: &World, wanted: fn(&Event) -> bool) -> bool {
    world.events.iter().any(wanted)
}

fn refusal(outcome: &Outcome) -> Result<&str, String> {
    match &outcome.result {
        Err(Stop::Failed(why)) => Ok(why),
        other => Err(format!("not refused as failed: {other:?}")),
    }
}

/// Point 3: a note of this boot that says held. Status 4 whatever the
/// flag says, the store is not opened, and the delete is finished.
fn a_held_failure(mutants: u32) -> Result<(), String> {
    for wanted in [WANTED, ACKNOWLEDGING] {
        let outcome = run_as(failed_world(true), &wanted, mutants);
        rules(&outcome)?;
        check(
            refusal(&outcome)?.contains("not served again in this boot"),
            "says that it is not served again in this boot",
        )?;
        check(
            !any(&outcome.world, |e| matches!(e, Event::Open(..))),
            "the store is not opened",
        )?;
        check(
            outcome.world.events
                == [
                    Event::Features,
                    Event::Info(4),
                    Event::RemoveNote(4),
                    Event::Delete(4),
                ],
            "the delete is finished and nothing else is done",
        )?;
        check(
            outcome.world.failure == Ok(Some(note(true))) && outcome.world.marked,
            "the note and the marker stay",
        )?;
    }
    Ok(())
}

/// A note that is not a whole record counts as held.
fn a_torn_failure_note(mutants: u32) -> Result<(), String> {
    for wanted in [WANTED, ACKNOWLEDGING] {
        let mut world = failed_world(false);
        world.failure = Err(());
        let outcome = run_as(world, &wanted, mutants);
        rules(&outcome)?;
        check(
            refusal(&outcome)?.contains("not served again in this boot"),
            "says that it is not served again in this boot",
        )?;
        check(
            !any(&outcome.world, |e| matches!(e, Event::Open(..))),
            "the store is not opened",
        )?;
        check(outcome.world.devices.is_empty(), "the device is deleted")?;
        check(outcome.world.failure == Err(()), "the note stays")?;
    }
    Ok(())
}

/// Point 4, without the flag: status 4, and nothing the decision rests
/// on is touched: the note and the marker are as they were. The volume's waiting device is deleted and said
/// (the rule since M1.3d: whoever holds the lock and finds the failed
/// volume's device waiting deletes it; before, this start touched
/// nothing at all and left that device to hold what was sent to it).
/// A second such start finds no device, says so and refuses as the
/// first did.
fn an_unheld_failure_not_acknowledged(mutants: u32) -> Result<(), String> {
    let world = failed_world(false);
    let outcome = run(world, mutants);
    rules(&outcome)?;
    let said = refusal(&outcome)?;
    check(
        said.contains("not acknowledged") && said.contains("--acknowledge-failure"),
        "says that it is not acknowledged, and names the flag",
    )?;
    check(
        said.contains("device 4 was deleted"),
        &format!("says what became of the device: {said}"),
    )?;
    check(
        outcome.world.events
            == [
                Event::Features,
                Event::Info(4),
                Event::RemoveNote(4),
                Event::Delete(4),
            ],
        &format!(
            "the lookup, the note and the delete, and nothing else: {:?}",
            outcome.world.events
        ),
    )?;
    let untouched = |world: &World| world.failure == Ok(Some(note(false))) && world.marked;
    check(
        untouched(&outcome.world),
        "the note and the marker are as they were",
    )?;
    check(outcome.world.devices.is_empty(), "the device is deleted")?;

    let mut again = outcome.world;
    again.events.clear();
    again.failure_reads.clear();
    let outcome = run(again, mutants);
    rules(&outcome)?;
    let said = refusal(&outcome)?;
    check(
        said.contains("--acknowledge-failure")
            && said.contains("no device of this volume was there"),
        &format!("the second refusal says that none was there: {said}"),
    )?;
    check(
        !any(&outcome.world, |e| {
            matches!(e, Event::Delete(_) | Event::RemoveNote(_))
        }),
        "the second refusal deletes nothing",
    )?;
    check(untouched(&outcome.world), "after the second refusal too")
}

/// Point 4, with the flag: the waiting device goes before the open,
/// then the note is removed.
fn an_unheld_failure_acknowledged(mutants: u32) -> Result<(), String> {
    let outcome = run_as(failed_world(false), &ACKNOWLEDGING, mutants);
    rules(&outcome)?;
    let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
    check(
        ready.acknowledged == Some(Reason::LogFailed),
        "says which failure was acknowledged",
    )?;
    check(!ready.took_over, "the device is a fresh one")?;
    check(
        !any(&outcome.world, |e| matches!(e, Event::BeginRecovery(_))),
        "no device is taken over",
    )?;
    check(
        outcome.world.events[..6]
            == [
                Event::Features,
                Event::Info(4),
                Event::RemoveNote(4),
                Event::Delete(4),
                Event::Open(true),
                Event::RemoveFailure,
            ],
        "the device, the open, the note: in that order",
    )?;
    check(
        outcome.world.failure == Ok(None) && !outcome.world.marked,
        "the note and the marker are gone",
    )
}

/// A start after a failure of the in-place store's image: the note's
/// reason is what the start says, held or not, and what it
/// acknowledges. Nothing else differs from a failure of the log.
#[test]
fn a_start_says_the_reason_the_note_gives() {
    let image = |held: bool| {
        let mut world = failed_world(held);
        world.failure = Ok(Some(Failure {
            reason: Reason::ImageFailed,
            ..note(held)
        }));
        world
    };
    let outcome = run_as(image(true), &ACKNOWLEDGING, 0);
    let why = refusal(&outcome).unwrap();
    assert!(
        why.starts_with("a write or sync of the volume's image failed, while a client held"),
        "{why}"
    );
    let outcome = run_as(image(false), &WANTED, 0);
    let why = refusal(&outcome).unwrap();
    assert!(
        why.starts_with("a write or sync of the volume's image failed"),
        "{why}"
    );
    let outcome = run_as(image(false), &ACKNOWLEDGING, 0);
    let ready = outcome.result.as_ref().unwrap();
    assert_eq!(ready.acknowledged, Some(Reason::ImageFailed));
}

#[test]
fn a_noted_failure_is_refused_or_acknowledged_by_what_the_note_says() {
    let late = (
        "the note read after the open",
        Mutants::NOTE_READ_AFTER_OPEN,
    );
    holds(
        a_held_failure,
        &[
            ("a held note acknowledged", Mutants::HELD_ACKNOWLEDGED),
            late,
        ],
    );
    holds(a_torn_failure_note, &[late]);
    holds(
        an_unheld_failure_not_acknowledged,
        &[
            late,
            ("the device deleted and not said", Mutants::LEFT_UNSAID),
            (
                "the refusal changes the note",
                Mutants::REFUSAL_TOUCHES_NOTE,
            ),
        ],
    );
    holds(an_unheld_failure_acknowledged, &[late]);
}

#[test]
fn an_acknowledged_volume_whose_device_a_client_holds_gets_another_number() {
    // No client held the device at the failure; one opened it since.
    // It was acknowledged nothing: the device is not taken over.
    let mut world = failed_world(false);
    world.held.push(4);
    let outcome = run_as(world, &ACKNOWLEDGING, 0);
    assert_eq!(rules(&outcome), Ok(()));
    let ready = outcome.result.expect("serves");
    assert!(!ready.took_over && ready.number != 4);
    assert_eq!(
        outcome.world.devices.get(&4).map(|d| d.state),
        Some(DeviceState::Dead)
    );
    assert!(!any(&outcome.world, |e| matches!(
        e,
        Event::BeginRecovery(_)
    )));
}

/// Something that makes a start fail.
type Breaks = fn(&mut World);

/// Point 5: a failure found at the start, with no note. The note is
/// written first, and "held" is what the attachment record says. The
/// failure is the store's marker, which a start notes only where a
/// record of this boot names a device or cannot be read: where none
/// does, the marker alone is refused or acknowledged and nothing is
/// noted (`a_marker_and_no_record_of_this_boot`). (Until M1.3t-b the
/// failure here was a log shorter than what was acknowledged, noted
/// whatever the record said; the cells with no record of this boot, no
/// record and one of another boot, went with it.)
fn a_start_notes_its_failure(mutants: u32) -> Result<(), String> {
    // (the record, the device's state if there is one, held)
    let cells: [(u8, Option<DeviceState>, bool); 4] = [
        (1, Some(DeviceState::Waiting), true),
        (1, Some(DeviceState::Live), true),
        (1, None, true),
        (3, Some(DeviceState::Waiting), true),
    ];
    for (record, state, held) in cells {
        let mut world = World::new();
        if record != 0 {
            world = world.attached(4, state.unwrap_or(DeviceState::Waiting), EARLIER);
        }
        if state.is_none() {
            world.devices.clear();
        }
        if let (2, Some(attachment)) = (record, world.attachment.as_mut()) {
            attachment.boot = [9; 16];
        }
        world.torn_attachment = record == 3;
        world.marked = true;
        let ours_and_waiting = record == 1 && state == Some(DeviceState::Waiting);
        let devices = world.devices.clone();
        let outcome = run(world, mutants);
        let cell = format!("record {record}, device {state:?}");
        rules(&outcome).map_err(|e| format!("{cell}: {e}"))?;
        refusal(&outcome)?;
        let noted = Failure {
            held: held.into(),
            reason: Reason::Marked,
            identity: [0; 16],
        };
        check(
            outcome.world.failure == Ok(Some(noted)),
            &format!("{cell}: the note says held = {held}"),
        )?;
        check(
            if ours_and_waiting {
                outcome.world.devices.is_empty()
            } else {
                outcome.world.devices == devices
            },
            &format!("{cell}: only this volume's waiting device is deleted"),
        )?;
    }
    Ok(())
}

#[test]
fn a_failure_found_at_the_start_is_noted_before_the_device_is_deleted() {
    holds(
        a_start_notes_its_failure,
        &[(
            "the failure note written after the delete",
            Mutants::FAILURE_NOTE_AFTER_DELETE,
        )],
    );

    // Each reason is noted as itself.
    let reasons: [(Breaks, Reason); 2] = [
        (|w| w.marked = true, Reason::Marked),
        (|w| w.torn_note = Some(4), Reason::DeviceNoteUnreadable),
    ];
    for (fails, reason) in reasons {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        fails(&mut world);
        let outcome = run(world, 0);
        assert_eq!(rules(&outcome), Ok(()));
        assert!(matches!(outcome.result, Err(Stop::Failed(_))));
        assert_eq!(
            outcome.world.failure.map(|f| f.map(|f| (f.held, f.reason))),
            Ok(Some((Held::Yes, reason)))
        );
    }

    // The note cannot be written: the device goes all the same, and the
    // daemon says so.
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.marked = true;
    world.failure_unwritable = true;
    let outcome = run(world, 0);
    assert!(refusal(&outcome)
        .expect("failed")
        .contains("the failure note could not be written"));
    assert!(outcome.world.devices.is_empty());
}

/// Point 6: the log's marker and no note of this boot.
fn a_marker_and_a_record_of_this_boot(mutants: u32) -> Result<(), String> {
    for wanted in [WANTED, ACKNOWLEDGING] {
        for gone in [false, true] {
            let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
            world.marked = true;
            if gone {
                world.devices.clear();
            }
            let outcome = run_as(world, &wanted, mutants);
            rules(&outcome)?;
            refusal(&outcome)?;
            check(
                outcome.world.failure.map(|f| f.map(|f| (f.held, f.reason)))
                    == Ok(Some((Held::Yes, Reason::Marked))),
                "noted as held",
            )?;
            check(
                outcome.world.devices.is_empty() && outcome.world.marked,
                "the device is deleted and the marker stays",
            )?;
        }
    }
    Ok(())
}

fn a_marker_and_no_record_of_this_boot(mutants: u32) -> Result<(), String> {
    for another_boot in [false, true] {
        let world = || {
            let mut world = World::new();
            if another_boot {
                world = world.attached(4, DeviceState::Waiting, EARLIER);
                if let Some(attachment) = world.attachment.as_mut() {
                    attachment.boot = [9; 16];
                }
            }
            world.marked = true;
            world
        };
        // Without the flag: status 4, no note, nothing touched.
        let devices = world().devices;
        let outcome = run(world(), mutants);
        rules(&outcome)?;
        check(
            refusal(&outcome)?.contains("--acknowledge-failure"),
            "names the flag",
        )?;
        check(
            outcome.world.failure == Ok(None)
                && outcome.world.marked
                && outcome.world.devices == devices,
            "no note is written and nothing is touched",
        )?;
        check(
            outcome.world.events == [Event::Features, Event::Open(false)],
            "only the open is tried",
        )?;

        // With it: the store opens.
        let outcome = run_as(world(), &ACKNOWLEDGING, mutants);
        rules(&outcome)?;
        let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
        check(
            ready.acknowledged == Some(Reason::Marked) && !ready.took_over,
            "says which failure was acknowledged, and serves a fresh device",
        )?;
        check(
            !outcome.world.marked && outcome.world.failure == Ok(None),
            "the marker is gone and there is no note",
        )?;
        check(
            !any(&outcome.world, |e| {
                matches!(e, Event::Delete(_) | Event::BeginRecovery(_))
            }),
            "no device of another boot is touched",
        )?;
    }
    Ok(())
}

#[test]
fn a_marked_log_without_a_note_is_held_if_a_record_of_this_boot_names_a_device() {
    holds(a_marker_and_a_record_of_this_boot, &[]);
    holds(a_marker_and_no_record_of_this_boot, &[]);
}

#[test]
fn the_flag_changes_nothing_when_nothing_has_failed() {
    for world in [World::new, || {
        World::new().attached(4, DeviceState::Waiting, EARLIER)
    }] {
        let plain = run(world(), 0);
        let flagged = run_as(world(), &ACKNOWLEDGING, 0);
        assert_eq!(rules(&flagged), Ok(()));
        assert_eq!(flagged.world.events, plain.world.events);
        let (plain, flagged) = (
            plain.result.expect("serves"),
            flagged.result.expect("serves"),
        );
        assert_eq!(flagged.acknowledged, None);
        assert_eq!(
            (flagged.number, flagged.took_over),
            (plain.number, plain.took_over)
        );
    }
}

/// The volume fails while it is served, for `reason`.
fn fails_while_serving(
    client_holds: Option<bool>,
    dies_at: Option<usize>,
    reason: Reason,
    mutants: u32,
) -> (World, (Failure, bool)) {
    let mut world = World::new().attached(4, DeviceState::Live, EARLIER);
    world.client_holds = client_holds;
    world.dies_at = dies_at;
    let (mut control, mut records, shared) = fakes(world);
    // The serving daemon holds the lock.
    let lock = Volume::lock(&mut control.clone()).ok();
    let (failure, written, _) = fail_serving_with(
        &mut control,
        &mut records,
        4,
        [3; 16],
        reason,
        Mutants(mutants),
    );
    let said = (failure, written);
    drop(lock);
    drop((control, records));
    (unwrap(shared), said)
}

/// The note carries the reason the caller gives: the store's (the log
/// store's log, or the in-place store's image).
#[test]
fn a_failure_while_serving_is_noted_with_what_the_kernel_says_and_then_the_device_goes() {
    // The kernel says that a client holds the device, or cannot tell;
    // a control interface that could say "nobody" would be believed.
    for reason in [Reason::LogFailed, Reason::ImageFailed] {
        for (says, held) in [
            (None, Held::Unknown),
            (Some(true), Held::Yes),
            (Some(false), Held::No),
        ] {
            let (world, (failure, written)) = fails_while_serving(says, None, reason, 0);
            let noted = Failure {
                held,
                reason,
                identity: [3; 16],
            };
            assert_eq!((failure, written), (noted, true));
            assert_eq!(world.failure, Ok(Some(noted)));
            assert_eq!(
                world.events,
                [
                    Event::WriteFailure(held, reason),
                    Event::RemoveNote(4),
                    Event::Delete(4),
                ]
            );
            assert_eq!(world.under_lock, [true; 3]);
            assert!(world.devices.is_empty());
        }
    }

    // The note cannot be written: the device goes, and the caller is
    // told.
    let mut world = World::new().attached(4, DeviceState::Live, EARLIER);
    world.failure_unwritable = true;
    let (mut control, mut records, shared) = fakes(world);
    let (_, written, deleted) =
        fail_serving(&mut control, &mut records, 4, [3; 16], Reason::ImageFailed);
    assert!(deleted.is_ok());
    drop((control, records));
    let world = unwrap(shared);
    assert!(!written);
    assert!(world.devices.is_empty() && world.failure == Ok(None));
}

/// What must hold after a daemon that was failing its volume is
/// killed: if the volume's device is gone, the volume is not served in
/// this boot without an acknowledgement, and never if a client held it.
/// A start that refuses finishes the delete.
fn refused_once_given_up(mut between: World, mutants: u32, at: usize) -> Result<(), String> {
    between.dies_at = None;
    between.events.clear();
    between.failure_reads.clear();
    between.fresh_notes.clear();
    // The server is gone: a device that was live waits.
    for device in between.devices.values_mut() {
        if device.state == DeviceState::Live {
            device.state = DeviceState::Waiting;
        }
    }
    let given_up = !between.devices.contains_key(&4) || !between.notes.contains_key(&4);
    // Killed between the note's removal and the delete: the device can
    // no longer be shown to be this volume's, and stays.
    let orphan = between.devices.contains_key(&4) && !between.notes.contains_key(&4);
    // Nothing holds a device open in these worlds: a failure of which
    // it was not known settles to "not held" once the device is gone.
    let held =
        between.failure.is_err() || matches!(between.failure, Ok(Some(f)) if f.held == Held::Yes);
    let noted = between.failure != Ok(None);
    for wanted in [WANTED, ACKNOWLEDGING] {
        let outcome = run_as(between.clone(), &wanted, mutants);
        let after = format!(
            "killed after {at} steps, acknowledging = {}",
            wanted.acknowledge_failure
        );
        rules(&outcome).map_err(|e| format!("{after}: {e}"))?;
        match &outcome.result {
            Ok(ready) if !noted => check(
                !given_up && ready.took_over && ready.number == 4,
                &format!("{after}: the volume's device is gone and the volume is served"),
            )?,
            Ok(ready) => check(
                !held && wanted.acknowledge_failure && !ready.took_over,
                &format!("{after}: a noted failure was passed over"),
            )?,
            Err(Stop::Failed(why)) if orphan => check(
                outcome.world.devices == between.devices
                    && (!held || why.contains("device 4") && why.contains("was left")),
                &format!("{after}: a device without a note was touched, or left and not said"),
            )?,
            Err(Stop::Failed(_)) => check(
                !(noted && held) || outcome.world.devices.is_empty(),
                &format!("{after}: refused, and the delete is not finished"),
            )?,
            Err(Stop::Error(_)) if orphan => check(
                outcome.world.devices == between.devices,
                &format!("{after}: a device without a note was touched"),
            )?,
            Err(other) => return Err(format!("{after}: {other:?}")),
        }
    }
    Ok(())
}

/// A start that finds the store's marker while a record of this boot
/// names a device, killed after each number of steps: a marker that
/// stays, and one that the fake gives to the first open only. (Until
/// M1.3t-b these were a log shorter than what was acknowledged and a
/// failed recovery sync.)
fn a_failing_start_killed_anywhere(mutants: u32) -> Result<(), String> {
    for lasting in [false, true] {
        let world = || {
            let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
            if lasting {
                world.marked = true;
            } else {
                world.opens.push_back(OpenError::MarkedFailed);
            }
            world
        };
        let steps = run(world(), 0).world.events.len();
        for at in 0..=steps {
            let mut first = world();
            first.dies_at = Some(at);
            let between = run(first, mutants).world;
            if lasting {
                // The marker is still there: every next start refuses
                // and leaves no device.
                let mut next = between.clone();
                next.dies_at = None;
                next.events.clear();
                next.failure_reads.clear();
                next.fresh_notes.clear();
                let (next_notes, next_devices) = (next.notes.clone(), next.devices.clone());
                let outcome = run(next, mutants);
                refusal(&outcome).map_err(|e| format!("killed after {at} steps: {e}"))?;
                let orphan = !next_notes.contains_key(&4) && next_devices.contains_key(&4);
                check(
                    (outcome.world.devices.is_empty() || orphan)
                        && matches!(outcome.world.failure, Ok(Some(f)) if f.held == Held::Yes),
                    &format!("killed after {at} steps: the delete is not finished"),
                )?;
            }
            refused_once_given_up(between, mutants, at)?;
        }
    }
    Ok(())
}

/// The failure of a serving daemon, killed after each number of steps.
fn a_failing_server_killed_anywhere(mutants: u32) -> Result<(), String> {
    for says in [None, Some(false)] {
        for at in 0..=3 {
            let (between, _) = fails_while_serving(says, Some(at), Reason::LogFailed, mutants);
            refused_once_given_up(between, mutants, at)?;
        }
    }
    Ok(())
}

#[test]
fn a_daemon_killed_while_it_fails_its_volume_is_followed_by_one_that_refuses() {
    let defect = [(
        "the failure note written after the delete",
        Mutants::FAILURE_NOTE_AFTER_DELETE,
    )];
    holds(a_failing_start_killed_anywhere, &defect);
    holds(a_failing_server_killed_anywhere, &defect);
}

/// An acknowledgement killed after each number of steps: while the note
/// is there the next start asks again, and one that is told serves a
/// fresh device with nothing left of the failure.
fn an_acknowledgement_killed_anywhere(mutants: u32) -> Result<(), String> {
    let steps = run_as(failed_world(false), &ACKNOWLEDGING, 0)
        .world
        .events
        .len();
    for at in 0..=steps {
        let mut first = failed_world(false);
        first.dies_at = Some(at);
        let mut between = run_as(first, &ACKNOWLEDGING, mutants).world;
        between.dies_at = None;
        between.events.clear();
        between.failure_reads.clear();
        between.fresh_notes.clear();
        let after = format!("killed after {at} steps");

        if between.failure != Ok(None) {
            // A start that asks: it touches nothing but the volume's
            // waiting device, which it deletes and says; a device it
            // cannot show to be the volume's it leaves and says.
            let ours = between.devices.contains_key(&4) && between.notes.contains_key(&4);
            let mut devices = between.devices.clone();
            if ours {
                devices.remove(&4);
            }
            let outcome = run(between.clone(), mutants);
            rules(&outcome).map_err(|e| format!("{after}: {e}"))?;
            let said = refusal(&outcome)?;
            check(
                said.contains("--acknowledge-failure"),
                &format!("{after}: the next start does not ask again"),
            )?;
            check(
                outcome.world.devices == devices
                    && outcome.world.failure == between.failure
                    && outcome.world.marked == between.marked
                    && outcome.world.attachment == between.attachment
                    && !any(&outcome.world, |e| {
                        matches!(e, Event::Open(..) | Event::Add(..))
                    }),
                &format!("{after}: a start that asks touched more than the waiting device"),
            )?;
            let there = between.devices.contains_key(&4);
            check(
                if ours {
                    said.contains("device 4 was deleted")
                } else if there {
                    said.contains("device 4") && said.contains("was left")
                } else {
                    said.contains("no device of this volume was there")
                },
                &format!("{after}: what became of the device is not said: {said}"),
            )?;
        } else {
            // The open that acknowledges takes the marker away, and the
            // note goes after it.
            check(
                !between.marked,
                &format!("{after}: the note is gone and the marker is not"),
            )?;
        }

        // Killed between the note's removal and the delete: the device
        // can no longer be shown to be this volume's. It is left, and
        // the daemon stops and says so until someone deletes it.
        let orphan = between.devices.contains_key(&4) && !between.notes.contains_key(&4);
        let devices = between.devices.clone();
        let outcome = run_as(between, &ACKNOWLEDGING, mutants);
        rules(&outcome).map_err(|e| format!("{after}: {e}"))?;
        if orphan {
            check(
                matches!(&outcome.result, Err(Stop::Error(why)) if why.contains("no note"))
                    && outcome.world.devices == devices,
                &format!("{after}: a device without a note was served or touched"),
            )?;
            continue;
        }
        let ready = outcome
            .result
            .as_ref()
            .map_err(|e| format!("{after}: {e:?}"))?;
        check(
            !ready.took_over && !outcome.world.devices.contains_key(&4),
            &format!("{after}: the failed volume's device is served or left"),
        )?;
        check(
            outcome.world.failure == Ok(None) && !outcome.world.marked,
            &format!("{after}: something of the failure is left"),
        )?;
    }
    Ok(())
}

#[test]
fn an_acknowledgement_killed_at_any_step_is_asked_for_again() {
    holds(an_acknowledgement_killed_anywhere, &[]);
}

// The failure note's third value: not known whether a client held the
// device. A later start settles it by the driver.

/// The log failed while the volume was served and the kernel could not
/// say whether a client held device 4: the note says unknown, and the
/// daemon has removed the device's note and deleted the device.
fn unknown_world() -> World {
    let mut world = World::new().attached(4, DeviceState::Dead, EARLIER);
    world.notes.clear();
    world.failure = Ok(Some(Failure {
        held: Held::Unknown,
        ..note(false)
    }));
    world.marked = true;
    world
}

/// What became of an unknown failure: how the start ended, and what
/// the note says now.
fn settled_as(outcome: &Outcome) -> Result<Held, String> {
    match outcome.world.failure {
        Ok(Some(failure)) => Ok(failure.held),
        other => Err(format!("the note is {other:?}")),
    }
}

/// Still dead under the recorded number with the recorded values:
/// held, whatever the flag says, and written down as held.
fn unknown_and_still_dead(mutants: u32) -> Result<(), String> {
    for wanted in [WANTED, ACKNOWLEDGING] {
        let outcome = run_as(unknown_world(), &wanted, mutants);
        rules(&outcome)?;
        check(
            refusal(&outcome)?.contains("not served again in this boot"),
            "refused for the boot",
        )?;
        check(settled_as(&outcome)? == Held::Yes, "written down as held")?;
        check(
            !any(&outcome.world, |e| {
                matches!(e, Event::Open(..) | Event::Delete(_) | Event::Add(..))
            }),
            "the store is not opened and no device is touched",
        )?;
        check(
            outcome.world.devices.get(&4).map(|d| d.state) == Some(DeviceState::Dead),
            "the dead device is as it was",
        )?;
    }
    Ok(())
}

/// The number is free, or holds a device that is not this volume's:
/// nobody holds this volume's device any more. Not held: refused
/// without the flag, served with it.
fn unknown_and_gone(mutants: u32) -> Result<(), String> {
    for case in 0..3 {
        let world = || {
            let mut world = unknown_world();
            match case {
                0 => world.devices.clear(),
                1 => {
                    if let Some(device) = world.devices.get_mut(&4) {
                        device.shape.depth = 16;
                        device.state = DeviceState::Live;
                    }
                }
                _ => {
                    world.notes.insert(4, [0x5A; 16]);
                    if let Some(device) = world.devices.get_mut(&4) {
                        device.state = DeviceState::Waiting;
                    }
                }
            }
            world
        };
        let devices = world().devices;
        let outcome = run(world(), mutants);
        rules(&outcome)?;
        check(
            refusal(&outcome)?.contains("--acknowledge-failure"),
            &format!("case {case}: names the flag"),
        )?;
        check(
            settled_as(&outcome)? == Held::No && outcome.world.devices == devices,
            &format!("case {case}: written down as not held, and nothing touched"),
        )?;
        check(
            !any(&outcome.world, |e| matches!(e, Event::Open(..))),
            &format!("case {case}: the store is not opened"),
        )?;

        let outcome = run_as(world(), &ACKNOWLEDGING, mutants);
        rules(&outcome)?;
        let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
        check(
            ready.acknowledged == Some(Reason::LogFailed) && !ready.took_over,
            &format!("case {case}: acknowledged, and a fresh device"),
        )?;
        check(
            outcome.world.failure == Ok(None) && !outcome.world.marked,
            &format!("case {case}: the note and the marker are gone"),
        )?;
    }
    Ok(())
}

/// The failing daemon died before it deleted the device: this start
/// removes the note, deletes, and looks again until the device is gone
/// or the stated looks are used. A device that goes late was held by
/// nobody. Mutant: the first look after the delete decides, and the
/// answer is written down for the boot.
fn unknown_and_not_yet_deleted(mutants: u32) -> Result<(), String> {
    for going in [Going::AtOnce, Going::Late, Going::Held] {
        let held = going == Going::Held;
        for state in [DeviceState::Waiting, DeviceState::Dead] {
            let mut world = unknown_world();
            world.notes.insert(4, [0xA5; 16]);
            if let Some(device) = world.devices.get_mut(&4) {
                device.state = state;
            }
            match going {
                Going::AtOnce => {}
                // The driver's delete does not wait: nobody holds the
                // device, and it is still there for two looks.
                Going::Late => world.delete_takes = Some(2),
                Going::Held => world.held.push(4),
            }
            let outcome = run_as(world, &ACKNOWLEDGING, mutants);
            rules(&outcome)?;
            let events = &outcome.world.events;
            check(
                events.get(1..4) == Some(&[Event::Info(4), Event::RemoveNote(4), Event::Delete(4)]),
                "the note is removed and the device deleted",
            )?;
            if held {
                check(
                    refusal(&outcome)?.contains("not served again in this boot"),
                    "still there: refused for the boot",
                )?;
                check(settled_as(&outcome)? == Held::Yes, "written down as held")?;
            } else {
                let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
                check(!ready.took_over, "gone: acknowledged, a fresh device")?;
            }
        }
    }
    Ok(())
}

#[test]
fn a_failure_of_unknown_holding_is_settled_by_what_the_driver_says_now() {
    holds(
        unknown_and_still_dead,
        &[(
            "unknown acknowledged while the device is still dead",
            Mutants::UNKNOWN_WHILE_DEAD,
        )],
    );
    holds(unknown_and_gone, &[]);
    holds(
        unknown_and_not_yet_deleted,
        &[(
            "held decided by the first look after the delete",
            Mutants::HELD_AT_FIRST_LOOK,
        )],
    );
}

#[test]
fn an_unknown_failure_that_cannot_be_settled_stays_unknown() {
    // A device that waits or is live, with the recorded values and no
    // note: whose it is cannot be told. The start stops with an error,
    // touches nothing, and the note still says unknown.
    for state in [DeviceState::Waiting, DeviceState::Live] {
        for wanted in [WANTED, ACKNOWLEDGING] {
            let mut world = unknown_world();
            if let Some(device) = world.devices.get_mut(&4) {
                device.state = state;
            }
            let devices = world.devices.clone();
            let outcome = run_as(world, &wanted, 0);
            assert_eq!(rules(&outcome), Ok(()));
            assert!(
                matches!(&outcome.result, Err(Stop::Error(why)) if why.contains("cannot be told")),
                "{:?}",
                outcome.result.as_ref().err()
            );
            assert_eq!(settled_as(&outcome), Ok(Held::Unknown));
            assert_eq!(outcome.world.devices, devices);
            assert!(!any(&outcome.world, |e| matches!(
                e,
                Event::Open(..) | Event::Delete(_) | Event::WriteFailure(..)
            )));
        }
    }

    // No record of this boot, or one that cannot be read: there is no
    // number to ask about, and the failure counts as held.
    for case in 0..3 {
        let mut world = unknown_world();
        match case {
            0 => world.attachment = None,
            1 => {
                if let Some(attachment) = world.attachment.as_mut() {
                    attachment.boot = [9; 16];
                }
            }
            _ => world.torn_attachment = true,
        }
        let outcome = run_as(world, &ACKNOWLEDGING, 0);
        assert!(refusal(&outcome)
            .expect("refused")
            .contains("not served again in this boot"));
        assert_eq!(settled_as(&outcome), Ok(Held::Yes), "case {case}");
    }

    // The settled value cannot be written: held.
    let mut world = unknown_world();
    world.devices.clear();
    world.failure_unwritable = true;
    let outcome = run_as(world, &ACKNOWLEDGING, 0);
    assert!(refusal(&outcome)
        .expect("refused")
        .contains("not served again in this boot"));
    assert_eq!(settled_as(&outcome), Ok(Held::Unknown));
}

/// A start that refuses a failed volume says what became of the
/// volume's device, in every case: deleted, or left and why. An answer
/// that names no device means that no record of this boot names one.
/// Before M1.3d a device that was still live after the looks, a driver
/// that could not be asked, a note that could not be read and a delete
/// that failed were all left unsaid, so that "refused" read the same
/// whether the device was gone or not. Mutant: those cases say nothing.
#[test]
fn a_refused_start_says_what_became_of_the_device() {
    fn refused(
        change: fn(&mut World),
        says: &[&str],
        there: bool,
        mutants: u32,
    ) -> Result<(), String> {
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.opens.push_back(OpenError::MarkedFailed);
        change(&mut world);
        let outcome = run(world, mutants);
        let why = match &outcome.result {
            Err(Stop::Failed(why)) => why.clone(),
            other => return Err(format!("stopped with {other:?}")),
        };
        for part in says {
            check(why.contains(part), &format!("`{part}` is not in: {why}"))?;
        }
        check(
            outcome.world.devices.contains_key(&4) == there,
            &format!("device 4 there: {}", !there),
        )
    }
    fn scenario(mutants: u32) -> Result<(), String> {
        refused(|_| {}, &["device 4 was deleted"], false, mutants)?;
        refused(
            |w| {
                if let Some(device) = w.devices.get_mut(&4) {
                    device.state = DeviceState::Live;
                }
            },
            &["device 4 is still live", "was left"],
            true,
            mutants,
        )?;
        refused(
            |w| w.delete_refused = true,
            &["device 4 could not be deleted", "the driver refuses"],
            true,
            mutants,
        )?;
        refused(
            |w| w.info_refused = true,
            &["device 4 could not be asked about", "was left"],
            true,
            mutants,
        )
    }
    holds(scenario, &[("left and not said", Mutants::LEFT_UNSAID)]);
}

/// A start that refuses the failed volume, and what it says.
fn refused_start(world: World, mutants: u32) -> Result<(String, World), String> {
    let mut world = world;
    world.opens.push_back(OpenError::MarkedFailed);
    let outcome = run(world, mutants);
    match outcome.result {
        Err(Stop::Failed(why)) => Ok((why, outcome.world)),
        other => Err(format!("stopped with {other:?}")),
    }
}

/// A delete that the driver refuses after the device's note was
/// removed does not strand the device (Codex on #18, and the review):
/// the note is put back, the answer says so, and the next start, which
/// is told that it "tries again", does delete the device. Before, the
/// note stayed removed, the answer still promised the next start, and
/// that start found a device with no note and left it for good. Where
/// the note cannot be put back the answer says what is true: by hand,
/// or a reboot. The same holds for the daemon that fails while it
/// serves. Mutant: the note is not put back and the promise is made.
#[test]
fn a_delete_that_fails_leaves_a_device_the_next_start_deletes() {
    fn scenario(mutants: u32) -> Result<(), String> {
        // A refusing start whose delete the driver refuses.
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.delete_refused = true;
        let (why, mut world) = refused_start(world, mutants)?;
        for part in [
            "device 4 could not be deleted (the driver refuses)",
            "its note was put back",
            "tries again",
        ] {
            check(why.contains(part), &format!("`{part}` is not in: {why}"))?;
        }
        check(
            world.devices.contains_key(&4) && world.notes.get(&4) == Some(&[0xA5; 16]),
            "the device is there with its note, as before the start",
        )?;
        // The start that was promised: the driver deletes now.
        world.delete_refused = false;
        world.events.clear();
        let (why, world) = refused_start(world, mutants)?;
        check(
            why.contains("device 4 was deleted"),
            &format!("the next start: {why}"),
        )?;
        check(
            !world.devices.contains_key(&4) && world.notes.is_empty(),
            "the next start deleted the device and its note",
        )?;

        // The note cannot be put back: no promise.
        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.delete_refused = true;
        world.note_unwritable = true;
        let (why, world) = refused_start(world, mutants)?;
        check(
            why.contains("is left without its note")
                && why.contains("by hand")
                && !why.contains("tries again"),
            &format!("the note could not be put back: {why}"),
        )?;
        check(
            world.devices.contains_key(&4) && world.notes.is_empty(),
            "the device is there without a note",
        )?;

        // The daemon that fails while it serves: the same delete.
        let mut world = World::new().attached(4, DeviceState::Live, EARLIER);
        world.delete_refused = true;
        let (mut control, mut records, shared) = fakes(world);
        let lock = Volume::lock(&mut control.clone()).ok();
        let (_, written, deleted) = fail_serving_with(
            &mut control,
            &mut records,
            4,
            [3; 16],
            Reason::LogFailed,
            Mutants(mutants),
        );
        drop(lock);
        drop((control, records));
        let mut world = unwrap(shared);
        check(written, "the failure note is written")?;
        match &deleted {
            Err(undeleted @ Undeleted::NotePutBack(_)) => check(
                undeleted.said(4).contains("its note was put back"),
                "what the failing daemon says",
            )?,
            other => return Err(format!("the failing daemon's delete: {other:?}")),
        }
        check(
            world.notes.get(&4) == Some(&[0xA5; 16]),
            "the failing daemon put the note back",
        )?;
        // Its process ends: the device waits. The next start finds
        // the failure note and deletes the device.
        if let Some(device) = world.devices.get_mut(&4) {
            device.state = DeviceState::Waiting;
        }
        world.delete_refused = false;
        world.events.clear();
        let outcome = run(world, mutants);
        let why = match &outcome.result {
            Err(Stop::Failed(why)) => why.clone(),
            other => return Err(format!("the start after the failure: {other:?}")),
        };
        // (This failure's holding was unknown, so the start settles it
        // by deleting the device and looking whether it goes.)
        check(
            outcome.world.events.contains(&Event::Delete(4))
                && !outcome.world.devices.contains_key(&4)
                && outcome.world.notes.is_empty(),
            &format!("the start after the failure: {why}"),
        )
    }
    holds(
        scenario,
        &[("the note is not put back", Mutants::DELETE_FAILURE_STRANDS)],
    );
}

/// A failed delete with nothing under the number afterwards is a
/// delete that is done: no note is put back on a free number.
#[test]
fn a_failed_delete_of_a_device_that_is_gone_puts_no_note_back() {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.delete_refused = true;
    world.devices.remove(&4);
    let (mut control, mut records, shared) = fakes(world);
    let deleted = delete(&mut control, &mut records, 4, Mutants::default());
    drop((control, records));
    let world = unwrap(shared);
    assert!(deleted.is_ok(), "{deleted:?}");
    assert!(world.notes.is_empty());
    assert_eq!(
        world.events,
        [Event::RemoveNote(4), Event::Delete(4), Event::Info(4)]
    );
}

/// After a volume failed under an imported pool, the device the
/// failing daemon deleted is still listed by the driver, dead, with no
/// note: the pool holds it open. A later start says that, and does not
/// send the operator to delete what is deleted already (seen on a
/// kernel in the rig's check d, four starts of four). A device with
/// the volume's values and no note that waits for a server is another
/// matter: a delete by hand does something there, and is still named.
/// Mutant: the dead one is said to need a delete by hand.
#[test]
fn a_dead_device_without_a_note_is_said_to_be_deleted_already() {
    fn scenario(mutants: u32) -> Result<(), String> {
        let mut world = World::new().attached(4, DeviceState::Dead, EARLIER);
        world.notes.clear();
        world.held.push(4);
        let (why, world) = refused_start(world, mutants)?;
        check(
            why.contains("device 4 has this volume's size, queues, depth and flags and no note")
                && why.contains("already deleted and still held open by a client")
                && why.contains("Nothing is left to delete")
                && !why.contains("by hand"),
            &format!("a dead device with no note: {why}"),
        )?;
        check(
            world.devices.contains_key(&4)
                && !world.events.iter().any(|e| matches!(e, Event::Delete(_))),
            "it is left and no delete is sent",
        )?;
        check(
            why.contains(&unnoted(4, DeviceState::Dead)),
            "the words are the ones `status` uses",
        )?;

        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
        world.notes.clear();
        let (why, _) = refused_start(world, mutants)?;
        check(
            why.contains("no note, and was left: deleting it by hand, or a reboot"),
            &format!("a waiting device with no note: {why}"),
        )
    }
    holds(
        scenario,
        &[("a dead device by hand", Mutants::DEAD_BY_HAND)],
    );
}

/// After a failed delete the note is put back only on a device that is
/// not dead (the reviewer of #18). "Dead" is also what the driver
/// reports for a device that is deleted and still held open: if the
/// delete was done although it answered with an error, a note put back
/// would outlive the device and sit on a free number. So a dead device
/// keeps no note, and the answer names the delete by hand. A device
/// that waits gets its note back, as before. Mutant: the note is put
/// back on a dead device too.
#[test]
fn a_failed_delete_puts_no_note_back_on_a_dead_device() {
    fn scenario(mutants: u32) -> Result<(), String> {
        for (state, back) in [(DeviceState::Dead, false), (DeviceState::Waiting, true)] {
            let mut world = World::new().attached(4, state, EARLIER);
            world.delete_refused = true;
            let (mut control, mut records, shared) = fakes(world);
            let deleted = delete(&mut control, &mut records, 4, Mutants(mutants));
            drop((control, records));
            let world = unwrap(shared);
            let said = match &deleted {
                Err(undeleted) => undeleted.said(4),
                Ok(()) => return Err(format!("{state:?}: the delete is said to be done")),
            };
            check(
                world.notes.contains_key(&4) == back,
                &format!("{state:?}: the note is back: {}", !back),
            )?;
            check(
                matches!(deleted, Err(Undeleted::NotePutBack(_))) == back
                    && said.contains("tries again") == back
                    && said.contains("by hand") != back,
                &format!("{state:?}: {said}"),
            )?;
        }
        Ok(())
    }
    holds(
        scenario,
        &[("a note back on a dead device", Mutants::PUT_BACK_ON_DEAD)],
    );
}
