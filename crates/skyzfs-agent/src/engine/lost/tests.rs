//! The failure note and the step on the fake world (M2.7c).

use std::cell::RefCell;
use std::rc::Rc;

use skyzfs_control::failure::{Failure, Held, Reason};

use super::super::fake::{
    FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, Restart, World, DEVICE, POOL, VOLUME,
};
use super::*;
use crate::engine::Noted;

type FakeAgent = Agent<FakeZfs, FakeDaemon, MemKeep, FakeClock>;

fn agent(world: &Rc<RefCell<World>>) -> FakeAgent {
    Agent::new(
        FakeZfs(world.clone()),
        FakeDaemon(world.clone()),
        MemKeep::default(),
        FakeClock(world.clone()),
        VOLUME,
        POOL,
    )
    .expect("the agent")
}

fn failure(held: Held) -> Failure {
    Failure {
        held,
        reason: Reason::ImageFailed,
        identity: [7; 16],
    }
}

/// The note read as the lead ruled on the plan's Q1 and Q2: none, a
/// failure no client held, one a client held or may have, and a torn
/// note, which counts as held. Mutants: `Unknown` taken as not held; a
/// torn note taken as none or as not held; `No` taken as lost.
#[test]
fn the_note_says_what_became_of_the_volume() {
    let world = World::new(Mode::Idle);
    let mut a = agent(&world);
    assert_eq!(a.failure().expect("read"), Failed::No);
    let cases = [
        (
            Noted::Failed(failure(Held::No)),
            Failed::Unheld(failure(Held::No)),
        ),
        (
            Noted::Failed(failure(Held::Yes)),
            Failed::Lost(Note::Whole(failure(Held::Yes))),
        ),
        (
            Noted::Failed(failure(Held::Unknown)),
            Failed::Lost(Note::Whole(failure(Held::Unknown))),
        ),
        (Noted::NotWhole, Failed::Lost(Note::Torn)),
    ];
    for (noted, want) in cases {
        world.borrow_mut().noted = Some(noted);
        assert_eq!(a.failure().expect("read"), want, "{noted:?}");
    }
    assert_eq!(Note::Torn.held_word(), "yes");
    assert_eq!(Note::Torn.reason_word(), "-");
    // Reading the note touches no pool.
    assert_eq!(world.borrow().reopens, 0);
}

/// The step: a reopen, then a look every [`LOOK_EVERY`] until the pool
/// reads SUSPENDED. Mutants: no reopen; no look after it (the command's
/// return taken for the signal); the look not repeated; the look not
/// waited for between reads.
#[test]
fn a_reopen_then_a_look_until_the_pool_suspends() {
    let world = World::new(Mode::Idle);
    world.borrow_mut().suspends_after_ms = Some(1_050);
    let mut a = agent(&world);
    let step = a.lose().expect("the step");
    assert_eq!(world.borrow().reopens, 1);
    let Step::Reopened {
        error,
        state,
        looked_ms,
    } = &step
    else {
        panic!("{step:?}");
    };
    assert_eq!(error, &None);
    assert_eq!(state.as_ref().map(|s| s.0.as_str()), Some("SUSPENDED"));
    // The first look at or after 1050 ms, at 250 ms apart.
    assert_eq!(*looked_ms, 1_250);
    assert!(step.suspended());
    assert_eq!(step.word(), "reopened");
}

/// A pool that does not suspend is looked at for [`LOOK_MAX`] and no
/// longer, and reads as it was. Mutants: no bound on the look; a bound
/// of one look.
#[test]
fn the_look_ends_at_its_limit() {
    let world = World::new(Mode::Idle);
    let mut a = agent(&world);
    let from = world.borrow().now_ms;
    let step = a.lose().expect("the step");
    let Step::Reopened {
        state, looked_ms, ..
    } = &step
    else {
        panic!("{step:?}");
    };
    assert_eq!(state.as_ref().map(|s| s.0.as_str()), Some("ONLINE"));
    let most = u64::try_from(LOOK_MAX.as_millis()).expect("ms");
    assert_eq!(*looked_ms, most);
    assert_eq!(world.borrow().now_ms - from, most);
    assert!(!step.suspended());
}

/// A reopen that is refused, as where zed suspended the pool first, is
/// not the signal either way: the look is. Mutant: the step ended at the
/// reopen's error.
#[test]
fn a_refused_reopen_is_still_looked_after() {
    let world = World::new(Mode::Idle);
    {
        let mut w = world.borrow_mut();
        w.reopen_refused = true;
        // zed suspends the pool a moment after the reopen was refused.
        let at = w.now_ms + 300;
        w.suspended_at = Some(at);
    }
    let mut a = agent(&world);
    let step = a.lose().expect("the step");
    let Step::Reopened { error, state, .. } = &step else {
        panic!("{step:?}");
    };
    assert!(error
        .as_deref()
        .is_some_and(|e| e.contains("pool I/O is currently suspended")));
    assert_eq!(state.as_ref().map(|s| s.0.as_str()), Some("SUSPENDED"));
}

/// No reopen where the pool is not imported, or reads SUSPENDED
/// already: a process started again on the same note does not reopen
/// again (the lead's answer to Q4). Mutants: either reopened.
#[test]
fn no_reopen_of_a_pool_not_imported_or_suspended() {
    let world = World::new(Mode::Idle);
    world.borrow_mut().suspended_at = Some(0);
    assert_eq!(agent(&world).lose().expect("step"), Step::Suspended);
    world.borrow_mut().pool = None;
    assert_eq!(agent(&world).lose().expect("step"), Step::NotImported);
    assert_eq!(world.borrow().reopens, 0);
    assert!(!Step::NotImported.suspended());
}

/// The same note met twice: the first step reopens and the pool
/// suspends, the second finds it SUSPENDED and does nothing. Mutant: the
/// second reopens.
#[test]
fn the_step_taken_again_does_nothing_more() {
    let world = World::new(Mode::Idle);
    world.borrow_mut().suspends_after_ms = Some(1_000);
    assert!(agent(&world).lose().expect("first").suspended());
    assert_eq!(agent(&world).lose().expect("step"), Step::Suspended);
    assert_eq!(world.borrow().reopens, 1);
}

/// The attachment watched at each `cuts`: a take-over keeps it, a new
/// attach (here a power cut) does not, and the first change stays.
/// Kept across a restart of the engine. Mutants: the attachment not
/// compared; the change forgotten at a restart or at a later read.
#[test]
fn a_new_attachment_is_a_move_and_a_take_over_is_not() {
    let world = World::new(Mode::Idle);
    let mut a = agent(&world);
    a.sync_past(&mut || true).expect("cuts");
    assert_eq!(a.moved(), None);
    world.borrow_mut().restart(Restart::TakeOver, None);
    a.sync_past(&mut || true).expect("cuts");
    assert_eq!(a.moved(), None, "a take-over keeps the attachment");
    world.borrow_mut().restart(Restart::Reboot, None);
    let _ = a.sync_past(&mut || true);
    let why = a.moved().expect("moved").to_string();
    assert!(
        why.starts_with("the daemon's attachment changed from "),
        "{why}"
    );
    a.restart().expect("restart");
    let _ = a.sync_past(&mut || true);
    assert_eq!(a.moved(), Some(why.as_str()));
}

/// The device's number watched at each read of it: a new number is a
/// move, named by major and minor. Mutant: the device not compared.
#[test]
fn a_new_device_number_is_a_move() {
    let world = World::new(Mode::Idle);
    let mut a = agent(&world);
    a.begin().expect("begun");
    assert_eq!(a.moved(), None);
    world.borrow_mut().device_now = Some(DEVICE + 1);
    let _ = a.begin();
    assert_eq!(
        a.moved(),
        Some("the volume's device changed from 0:259 to 0:260")
    );
}

/// A device number is split as glibc's `major` and `minor` split it:
/// `makedev(259, 3)` is 0x10303, and the high bits of each are above
/// bit 32. Mutant: the old 8-bit split.
#[test]
fn a_device_number_splits_as_glibc_splits_it() {
    use crate::engine::{major, minor};
    assert_eq!((major(0x10303), minor(0x10303)), (259, 3));
    let big: u64 = (0x0123_4000_u64 << 32) | (0x5678_9000_u64 << 12) | (0xabc << 8) | 0xde;
    assert_eq!((major(big), minor(big)), (0x0123_4abc, 0x5678_90de));
}
