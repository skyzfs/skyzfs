//! The failure note and the step on the fake world (M2.7c).

use std::cell::RefCell;
use std::rc::Rc;

use skyzfs_control::failure::{Failure, Held, Reason};

use super::super::fake::{
    FakeClock, FakeDaemon, FakeZfs, MemKeep, Mode, Restart, World, DEVICE, POOL, VOLUME,
};
use super::*;
use crate::engine::Noted;
use std::time::Duration;

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
        unread,
        looked_ms,
    } = &step
    else {
        panic!("{step:?}");
    };
    assert_eq!(error, &None);
    assert_eq!(unread, &None);
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

/// A look after the reopen that fails ends the look and keeps the
/// reopen's outcome in the step, with the look's error (F3 of the review
/// of #173). Mutants: the step an error (the reopen's outcome lost); the
/// look's error dropped.
#[test]
fn a_look_that_fails_after_the_reopen_keeps_the_reopen() {
    let world = World::new(Mode::Idle);
    {
        let mut w = world.borrow_mut();
        // The look before the reopen reads, the second after it fails.
        w.pool_state_fails_after = 2;
        w.pool_state_fails = 1;
        w.reopen_refused = true;
    }
    let step = agent(&world).lose().expect("the step");
    assert_eq!(world.borrow().reopens, 1);
    let Step::Reopened {
        error,
        state,
        unread,
        looked_ms,
    } = &step
    else {
        panic!("{step:?}");
    };
    assert!(error.as_deref().is_some_and(|e| e.contains("suspended")));
    assert_eq!(state, &None);
    assert!(
        unread
            .as_deref()
            .is_some_and(|e| e.contains("cannot read the pool's state")),
        "{unread:?}"
    );
    assert_eq!(*looked_ms, 250);
    assert_eq!(Step::verdict(Some(&step)), Verdict::Retry);
}

/// A look before the reopen that fails is the step's error, and no
/// reopen is made.
#[test]
fn a_look_that_fails_before_the_reopen_is_the_error() {
    let world = World::new(Mode::Idle);
    world.borrow_mut().pool_state_fails = 1;
    let e = agent(&world).lose().expect_err("an error");
    assert!(
        e.to_string().contains("cannot read the pool's state"),
        "{e}"
    );
    assert_eq!(world.borrow().reopens, 0);
}

/// What each step shows (the lead's rulings on F2 and F4 of the review
/// of #173): only SUSPENDED is lost; a pool not imported, before the
/// reopen or after it, is failed; anything else is tried again. Mutants:
/// each arm given another verdict.
#[test]
fn the_verdict_of_each_step() {
    use crate::zfs::PoolState;
    let reopened = |state: Option<&str>, unread: bool| Step::Reopened {
        error: None,
        state: state.map(|s| PoolState(s.into())),
        unread: unread.then(|| "no".to_string()),
        looked_ms: 0,
    };
    let cases = [
        (None, Verdict::Retry),
        (Some(Step::NotImported), Verdict::Failed),
        (Some(Step::Suspended), Verdict::Lost),
        (Some(reopened(Some("SUSPENDED"), false)), Verdict::Lost),
        (Some(reopened(Some("ONLINE"), false)), Verdict::Retry),
        (Some(reopened(Some("DEGRADED"), false)), Verdict::Retry),
        (Some(reopened(None, false)), Verdict::Failed),
        (Some(reopened(None, true)), Verdict::Retry),
    ];
    for (step, want) in cases {
        assert_eq!(Step::verdict(step.as_ref()), want, "{step:?}");
    }
    let words = [Verdict::Lost, Verdict::Failed, Verdict::Retry].map(Verdict::word);
    assert_eq!(words, ["lost", "failed", "retry"]);
}

/// The watch on the note (F1 of the review of #173): nothing is read
/// with no watch set; a read at most once each [`NOTE_EVERY`]; a lost
/// note other than the one seen is news, and stays news; the note seen,
/// a note no client held, and a read that fails are not. Mutants: a
/// read at each ask; the note seen taken as news; a failed read taken
/// as news; the news forgotten at the next ask.
#[test]
fn the_watch_sees_a_new_lost_note_about_every_second() {
    let world = World::new(Mode::Idle);
    let mut a = agent(&world);
    let advance = |ms: u64| world.borrow_mut().advance(Duration::from_millis(ms));
    // No watch: no read.
    advance(5_000);
    assert!(!a.note_came());
    assert_eq!(world.borrow().note_reads, 0);
    // A watch from no note: the first read a second on.
    a.watch_note(Some(Failed::No));
    assert!(!a.note_came());
    assert_eq!(world.borrow().note_reads, 0);
    advance(999);
    assert!(!a.note_came());
    assert_eq!(world.borrow().note_reads, 0);
    advance(1);
    assert!(!a.note_came());
    assert_eq!(world.borrow().note_reads, 1);
    // A note no client held is not news.
    world.borrow_mut().noted = Some(Noted::Failed(failure(Held::No)));
    advance(1_000);
    assert!(!a.note_came());
    assert_eq!(world.borrow().note_reads, 2);
    // A read that fails is not news.
    {
        let mut w = world.borrow_mut();
        w.noted = Some(Noted::Failed(failure(Held::Yes)));
        w.note_read_fails = 1;
    }
    advance(1_000);
    assert!(!a.note_came());
    assert_eq!(world.borrow().note_reads, 3);
    // A lost note is, at the next read, and stays so with no read more.
    assert!(!a.note_came(), "no read within the second");
    advance(1_000);
    assert!(a.note_came());
    assert!(a.note_came());
    assert_eq!(world.borrow().note_reads, 4);
    // Set again on that note: it is not news.
    let seen = a.failure().expect("read");
    a.watch_note(Some(seen));
    advance(1_000);
    assert!(!a.note_came());
    // A torn note is another note.
    world.borrow_mut().noted = Some(Noted::NotWhole);
    advance(1_000);
    assert!(a.note_came());
    // Set after a read that failed: any lost note is news.
    a.watch_note(None);
    advance(1_000);
    assert!(a.note_came());
}

/// The watch stops the settle before its next pass, as an end would
/// (F1 of the review of #173; the syncs: `engine::tests::past`).
/// Mutant: the watch not asked in the settle.
#[test]
fn news_on_the_note_stops_the_settle() {
    let world = World::new(Mode::Busy);
    let mut a = agent(&world);
    a.begin().expect("begun");
    a.watch_note(Some(Failed::No));
    let from = world.borrow().now_ms;
    {
        // The daemon fails: its note, and no answer more, so the publish
        // stays under way.
        let mut w = world.borrow_mut();
        w.noted = Some(Noted::Failed(failure(Held::Unknown)));
        w.gone_from = Some(from);
    }
    let e = a
        .settle_while(Duration::from_secs(600), &mut || true)
        .expect_err("unsettled");
    assert!(matches!(e, Error::Unsettled(_)), "{e}");
    let took = world.borrow().now_ms - from;
    assert!(took < 5_000, "{took} ms");
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
