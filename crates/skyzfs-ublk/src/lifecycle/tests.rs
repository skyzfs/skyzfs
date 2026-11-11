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
    Acknowledged,
    Open(Option<MustReach>, bool),
    Pause,
    Info(u32),
    WriteAttachment(u32),
    Add(u32, bool),
    Disown(u32),
    WriteNote(u32),
    Delete(u32),
    BeginRecovery(u32),
}

struct World {
    events: Vec<Event>,
    can_reissue: bool,
    devices: BTreeMap<u32, DeviceInfo>,
    /// Devices that appear between the daemon's look and its add.
    late: Vec<u32>,
    /// Devices that someone has open: deleted, they keep their number.
    held: Vec<u32>,
    /// A live device whose server is seen to be gone once it has been
    /// asked for this many more times.
    settles: Option<(u32, usize)>,
    /// The daemon is killed once this many things have happened: from
    /// then on nothing it does has an effect.
    dies_at: Option<usize>,
    disowned: Vec<u32>,
    attachment: Option<Attachment>,
    notes: BTreeMap<u32, [u8; 16]>,
    /// The note of this device is there and is not a whole record.
    torn_note: Option<u32>,
    acknowledged: Result<Option<MustReach>, ()>,
    /// Where the store's valid log ends.
    log_end: u64,
    identity: [u8; 16],
    /// Errors of the next opens, before the store answers by itself.
    opens: VecDeque<OpenError>,
    nonces: u8,
}

fn killed() -> io::Error {
    io::Error::other("killed")
}

impl World {
    /// Note that `event` happens, unless the daemon is dead.
    fn happens(&mut self, event: Event) -> io::Result<()> {
        if self.dies_at.is_some_and(|at| self.events.len() >= at) {
            return Err(killed());
        }
        self.events.push(event);
        Ok(())
    }

    fn new() -> Self {
        Self {
            events: Vec::new(),
            can_reissue: true,
            devices: BTreeMap::new(),
            late: Vec::new(),
            held: Vec::new(),
            settles: None,
            dies_at: None,
            disowned: Vec::new(),
            attachment: None,
            notes: BTreeMap::new(),
            torn_note: None,
            acknowledged: Ok(None),
            log_end: 1000,
            identity: [3; 16],
            opens: VecDeque::new(),
            nonces: 0,
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

impl Control for Fake {
    fn can_reissue(&mut self) -> io::Result<bool> {
        let mut w = self.0.borrow_mut();
        w.happens(Event::Features)?;
        Ok(w.can_reissue)
    }
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        let mut w = self.0.borrow_mut();
        w.happens(Event::Info(number))?;
        if let Some((settling, left)) = w.settles {
            if settling == number && left == 0 {
                if let Some(device) = w.devices.get_mut(&number) {
                    device.state = DeviceState::Waiting;
                }
            } else if settling == number {
                w.settles = Some((settling, left - 1));
            }
        }
        Ok(w.devices
            .get(&number)
            .filter(|_| !w.late.contains(&number))
            .copied())
    }
    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError> {
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
                state: DeviceState::Added,
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
        let mut w = self.0.borrow_mut();
        w.happens(Event::Delete(number))?;
        if w.held.contains(&number) {
            if let Some(device) = w.devices.get_mut(&number) {
                device.state = DeviceState::Dead;
            }
        } else {
            w.devices.remove(&number);
        }
        Ok(())
    }
    fn begin_recovery(&mut self, number: u32) -> io::Result<()> {
        let mut w = self.0.borrow_mut();
        w.happens(Event::BeginRecovery(number))
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
        Ok(self.0.borrow().attachment)
    }
    fn write_attachment(&mut self, attachment: &Attachment) -> io::Result<()> {
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
        let mut w = self.0.borrow_mut();
        w.happens(Event::WriteNote(number))?;
        w.notes.insert(number, *nonce);
        Ok(())
    }
    fn acknowledged(&mut self) -> io::Result<Option<MustReach>> {
        let mut w = self.0.borrow_mut();
        w.happens(Event::Acknowledged)?;
        w.acknowledged
            .map_err(|()| io::Error::other("not a whole record"))
    }
    fn pause(&mut self) {
        let _ = self.0.borrow_mut().happens(Event::Pause);
    }
}

/// The open store of the fake: a token.
#[derive(Debug, PartialEq, Eq)]
struct Opened;

impl Volume for Fake {
    type Open = Opened;
    fn open(&mut self, must_reach: Option<MustReach>) -> Result<(Opened, u64), OpenError> {
        let mut w = self.0.borrow_mut();
        let result = match w.opens.pop_front() {
            Some(error) => Err(error),
            // As the store does: the log must end beyond the position,
            // if the position was recorded for this store.
            None => match must_reach {
                Some(m) if m.identity == w.identity && w.log_end <= m.position => {
                    Err(OpenError::Short)
                }
                _ => Ok((Opened, SIZE)),
            },
        };
        match w.happens(Event::Open(must_reach, result.is_ok())) {
            Ok(()) => result,
            Err(_) => Err(OpenError::Other("killed".into())),
        }
    }
}

const WANTED: Wanted = Wanted {
    queues: 1,
    depth: 64,
    flags: FLAGS,
    lock_retries: 3,
    live_retries: 5,
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
    let shared = Rc::new(RefCell::new(world));
    let (mut c, mut r, mut v) = (
        Fake(shared.clone()),
        Fake(shared.clone()),
        Fake(shared.clone()),
    );
    let result = start_with(&mut c, &mut r, &mut v, &WANTED, Mutants(mutants));
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
    let opened = at(&|e| matches!(e, Event::Open(_, true)));
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
    // The record of what was acknowledged is read before the store is
    // opened, and the store is opened with it.
    let wanted = outcome.world.acknowledged.ok().flatten();
    for event in events {
        if let Event::Open(must_reach, _) = event {
            check(
                *must_reach == wanted,
                "the store was opened without the position it must reach",
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
                Event::Acknowledged,
                Event::Open(None, true),
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
        outcome.world.events[3..]
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
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.acknowledged = Ok(Some(MustReach {
        identity: [3; 16],
        position: 999,
    }));
    let outcome = run(world, mutants);
    rules(&outcome)?;
    let ready = outcome.result.as_ref().map_err(|e| format!("{e:?}"))?;
    check(
        ready.took_over && ready.number == 4,
        "device 4 is taken over",
    )?;
    check(ready.shape == EARLIER, "with the device's own values")?;
    check(
        outcome.world.events[2..]
            == [
                Event::Open(
                    Some(MustReach {
                        identity: [3; 16],
                        position: 999,
                    }),
                    true,
                ),
                Event::Info(4),
                Event::BeginRecovery(4),
            ],
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
            ("no position", Mutants::NO_MUST_REACH),
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

fn another_nonce(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.notes.insert(4, [0x5A; 16]);
    untouched(world, mutants, |s| matches!(s, Stop::Error(_)))
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
    holds(live, &[("take live", Mutants::TAKE_LIVE)]);
    holds(other_values, &[("any values", Mutants::ANY_SHAPE)]);
}

#[test]
fn this_volumes_device_that_was_never_started_is_deleted_and_replaced() {
    let world = World::new().attached(4, DeviceState::Added, EARLIER);
    let outcome = run(world, 0);
    assert_eq!(rules(&outcome), Ok(()));
    let ready = outcome.result.expect("serves");
    assert!(!ready.took_over);
    assert_eq!(ready.left, None);
    assert_eq!(
        outcome.world.events[3..5],
        [Event::Info(4), Event::Delete(4)]
    );
    assert_eq!(outcome.world.devices.len(), 1);
}

#[test]
fn a_device_that_was_never_started_and_has_not_this_volumes_note_is_left() {
    // With no note it is this volume's from a kill between the add and
    // the note, or another program's between its add and its start.
    for note in [None, Some([0x5A; 16])] {
        let mut world = World::new().attached(0, DeviceState::Added, EARLIER);
        world.notes.clear();
        if let Some(note) = note {
            world.notes.insert(0, note);
        }
        let outcome = run(world, 0);
        assert_eq!(rules(&outcome), Ok(()));
        let ready = outcome.result.expect("serves");
        assert_eq!((ready.number, ready.took_over), (1, false));
        assert!(matches!(ready.left, Some((0, _))), "{:?}", ready.left);
        assert!(!outcome
            .world
            .events
            .iter()
            .any(|e| matches!(e, Event::Delete(_))));
        assert_eq!(
            outcome.world.devices.get(&0).map(|d| d.state),
            Some(DeviceState::Added)
        );
        assert_eq!(outcome.world.notes.get(&0).copied(), note);
    }
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

    // The failure is acknowledged and the volume served again: the dead
    // device is not taken over, not deleted again, and its number is
    // passed by.
    let mut next = outcome.world;
    next.events.clear();
    let outcome = run(next, 0);
    assert_eq!(rules(&outcome), Ok(()));
    let ready = outcome.result.expect("serves");
    assert_eq!((ready.number, ready.took_over), (1, false));
    assert!(!outcome.world.events.iter().any(|e| matches!(
        e,
        Event::BeginRecovery(_) | Event::Delete(_) | Event::Add(0, _)
    )));
    assert_eq!(
        outcome.world.devices.get(&0).map(|d| d.state),
        Some(DeviceState::Dead)
    );
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

fn cannot_reissue(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.can_reissue = false;
    let outcome = run(world, mutants);
    rules(&outcome)?;
    check(outcome.result == Err(Stop::CannotReissue), "refused")?;
    check(
        outcome.world.events == [Event::Features],
        "nothing else is done, and the store is not opened",
    )
}

impl PartialEq for Ready<Opened> {
    fn eq(&self, other: &Self) -> bool {
        (self.number, self.shape, self.took_over) == (other.number, other.shape, other.took_over)
    }
}

#[test]
fn a_kernel_that_cannot_reissue_is_refused_before_anything_is_touched() {
    holds(
        cannot_reissue,
        &[("no feature check", Mutants::NO_FEATURE_CHECK)],
    );
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

fn short_log(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.acknowledged = Ok(Some(MustReach {
        identity: [3; 16],
        position: 1000,
    }));
    gives_up(world, mutants)
}

fn sync_failed(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.opens.push_back(OpenError::SyncFailed);
    gives_up(world, mutants)
}

fn marked(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.opens.push_back(OpenError::MarkedFailed);
    gives_up(world, mutants)
}

fn unreadable(mutants: u32) -> Result<(), String> {
    let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);
    world.acknowledged = Err(());
    let outcome = run(world, mutants);
    check(
        !outcome
            .world
            .events
            .iter()
            .any(|e| matches!(e, Event::Open(..))),
        "the store is not opened without the position",
    )?;
    check(matches!(outcome.result, Err(Stop::Failed(_))), "failed")?;
    check(outcome.world.devices.is_empty(), "the device is deleted")
}

#[test]
fn a_volume_that_failed_while_held_loses_its_waiting_device_and_is_refused() {
    holds(short_log, &[("no position", Mutants::NO_MUST_REACH)]);
    holds(
        sync_failed,
        &[("sync error may pass", Mutants::SYNC_ERROR_MAY_PASS)],
    );
    holds(marked, &[]);
    holds(unreadable, &[]);
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
    world.opens.extend(vec![OpenError::Locked; 4]);
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
    world.opens.extend(vec![OpenError::Locked; 3]);
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
    for state in [DeviceState::Waiting, DeviceState::Added, DeviceState::Live] {
        let mut world = World::new().attached(4, state, EARLIER);
        world.torn_note = Some(4);
        assert_eq!(
            untouched(world, 0, |s| matches!(s, Stop::Failed(_))),
            Ok(())
        );
    }
}

/// Kill the daemon after each number of steps of a start, then start
/// again: the second start serves, by the rules. Beside the device it
/// serves and the devices of others, at most one device is there: one
/// that was added and never started and has no note, from a kill
/// between the add and the note. Returns how many kills left one.
fn killed_anywhere(world: fn() -> World, took_over: bool, mutants: u32) -> Result<usize, String> {
    let others: Vec<u32> = world().devices.keys().copied().collect();
    let steps = run(world(), 0).world.events.len();
    let mut leaked = 0;
    for at in 0..=steps {
        let mut first = world();
        first.dies_at = Some(at);
        let mut between = run(first, mutants).world;
        between.dies_at = None;
        between.events.clear();
        let outcome = run(between, mutants);
        rules(&outcome).map_err(|e| format!("killed after {at} steps: {e}"))?;
        let ready = outcome
            .result
            .as_ref()
            .map_err(|e| format!("killed after {at} steps: {e:?}"))?;
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
                outcome.world.devices.get(number).map(|d| d.state) == Some(DeviceState::Added);
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
    Ok(leaked)
}

#[test]
fn a_daemon_killed_at_any_step_of_a_start_is_followed_by_one_that_serves_one_device() {
    let with_others = || {
        let mut world = World::new();
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
        world
    };
    // The kills between the add and the note leave a device: before
    // the disowning, and after it.
    assert_eq!(killed_anywhere(World::new, false, 0), Ok(2));
    assert_eq!(
        killed_anywhere(
            || World::new().attached(4, DeviceState::Waiting, EARLIER),
            true,
            0
        ),
        Ok(0)
    );
    // Another's device appears at the number just recorded, and the
    // daemon is killed before it records the next: the record names a
    // live device that is not this volume's.
    assert_eq!(killed_anywhere(with_others, false, 0), Ok(2));
}

#[test]
fn a_live_device_without_this_volumes_note_is_anothers_and_is_left() {
    for note in [None, Some([0x5A; 16])] {
        let mut world = World::new().attached(0, DeviceState::Live, EARLIER);
        world.notes.clear();
        if let Some(note) = note {
            world.notes.insert(0, note);
        }
        let outcome = run(world, 0);
        assert_eq!(rules(&outcome), Ok(()));
        let ready = outcome.result.expect("serves");
        assert_eq!((ready.number, ready.took_over), (1, false));
        assert_eq!(
            outcome.world.devices.get(&0).map(|d| d.state),
            Some(DeviceState::Live)
        );
        assert!(!outcome.world.events.contains(&Event::Delete(0)));
        assert_eq!(outcome.world.notes.get(&0).copied(), note);
    }
}
