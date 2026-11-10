//! Task 15j part B: §8.2 step 3 — capturing a filesystem that has shut
//! down, and the errno classification that decides when to.
//!
//! The runbook's claim is narrow and worth stating precisely: a
//! filesystem the kernel refuses to freeze *because it is no longer
//! running* needs no freeze. Its block state already is "crashed at
//! ENOSPC", and a crash-consistent capture of that is legitimate.
//! Refusing the capture would not protect anything — it would strand the
//! delta on the node that just proved it cannot hold it.
//!
//! What must NOT happen is the same fallback swallowing a transient. An
//! `EBUSY` freeze is a race with another freezer and the capture has to
//! fail, or every contended capture silently degrades to
//! crash-consistent.

use skynbd_agent::dm::{DmConfig, DmStack};
use skynbd_agent::dm_mock::{MockDmOps, MockEvent as E};
use skynbd_agent::stack::{LocalStack, StackError};

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;
const POOL: &str = "skypool";
const ORIGIN: &str = "origin";
const ORIGIN_ID: u64 = 1;

struct Rig {
    ops: MockDmOps,
    stack: DmStack<MockDmOps>,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    rig_with_budget(50)
}

fn rig_with_budget(freeze_budget_ms: u64) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
    let cfg = DmConfig {
        pool_name: POOL.into(),
        origin_name: ORIGIN.into(),
        origin_dev_id: ORIGIN_ID,
        metadata_dev: dir.path().join("tmeta"),
        mountpoint: dir.path().to_path_buf(),
        volume: "vlin".into(),
        size_bytes: VOL,
        chunk_bytes: CHUNK,
        ids_path: dir.path().join("vlin.devids"),
        freeze_budget_ms,
        discard_name: None,
        backing_file: None,
        shim_name: None,
        mount_name: None,
        slot: None,
    };
    let stack = DmStack::new(cfg, ops.clone()).unwrap();
    Rig {
        ops,
        stack,
        _dir: dir,
    }
}

fn pat(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

/// §8.2 step 3. The kernel ordering that survives is the one the kernel
/// actually requires — suspend before `create_snap` — while the freeze
/// bracket, which exists only to make the image filesystem-consistent,
/// is gone.
#[test]
fn a_wedged_filesystem_is_captured_without_a_freeze() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(2 * CHUNK as usize, 3));
    r.ops
        .fail_next_freeze(StackError::Wedged("FIFREEZE: EIO".into()));

    let snap = r.stack.capture("g0").expect("capture proceeds");
    assert_eq!(snap.lv_name, "g0");

    let ev = r.ops.events();
    assert!(!ev.contains(&E::Freeze), "no freeze landed: {ev:?}",);
    assert!(
        !ev.contains(&E::Thaw) && !ev.contains(&E::ForceThaw),
        "nothing to thaw, so nothing thawed it: {ev:?}",
    );
    let pos = |e: &E| {
        ev.iter()
            .position(|x| x == e)
            .unwrap_or_else(|| panic!("missing {e:?}"))
    };
    let suspend = pos(&E::Suspend(ORIGIN.into()));
    let create = pos(&E::CreateSnap(snap.thin_dev_id));
    let resume = pos(&E::Resume(ORIGIN.into()));
    let activate = pos(&E::ActivateRo("sky-vlin-c-g0".into()));
    assert!(
        suspend < create && create < resume,
        "the kernel's own ordering still holds: {ev:?}",
    );
    assert!(resume < activate, "activation after the origin is back");

    // And the image is real, not an empty shell: the delta this capture
    // exists to carry off the node has to be readable.
    let img = r.stack.read("g0", 0, VOL).unwrap();
    assert_eq!(&img[..2 * CHUNK as usize], &pat(2 * CHUNK as usize, 3)[..]);
}

/// The regression the fallback's disarm-FIRST ordering exists to
/// prevent, and it is worse than a stray `FITHAW`.
///
/// A leaked arm poisons the NEXT window, deterministically. The
/// watchdog is one thread with one timer parked in `recv_timeout`; an
/// `Arm` arriving while a window is still open is a protocol violation
/// it answers by firing immediately and latching `FIRED`. `FIRED` is
/// sticky until a disarm reads it — so the next capture arms, freezes,
/// snapshots, and then its own `disarm()` reports a blown window,
/// deleting the snapshot it just took. One wedged capture would cost
/// every capture after it.
#[test]
fn a_wedged_capture_leaves_no_watchdog_armed() {
    // A budget far longer than this test runs, so the only thing that
    // can fire the watchdog is the leak itself — never a timeout.
    let r = rig_with_budget(5_000);
    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 7));
    r.ops
        .fail_next_freeze(StackError::Wedged("FIFREEZE: EROFS".into()));
    r.stack.capture("g0").expect("the wedged capture proceeds");

    // The filesystem recovers, and the very next capture must be
    // ordinary. Under a leaked arm this fails with "freeze watchdog
    // fired" and takes its own snapshot down with it.
    //
    // The stall is what makes that deterministic rather than a race:
    // `delay_next_freeze` holds the main thread inside the window with
    // the lock released, so the watchdog thread is certain to have
    // processed the second `Arm` — and latched `FIRED` — before this
    // capture reaches its own `disarm()`. Without it the mock's
    // microsecond-scale window can finish first and the leak goes
    // unobserved.
    r.ops.write_origin(ORIGIN, CHUNK, &pat(CHUNK as usize, 8));
    r.ops
        .delay_next_freeze(std::time::Duration::from_millis(200));
    r.stack
        .capture("g1")
        .expect("the next capture is not poisoned by the wedged one");

    let ev = r.ops.events();
    assert!(
        !ev.contains(&E::ForceThaw),
        "a window nobody opened was force-thawed: {ev:?}",
    );
    // Both images are real: neither capture was deleted behind us.
    assert_eq!(
        &r.stack.read("g0", 0, CHUNK).unwrap()[..],
        &pat(CHUNK as usize, 7)[..]
    );
    assert_eq!(
        &r.stack.read("g1", CHUNK, CHUNK).unwrap()[..],
        &pat(CHUNK as usize, 8)[..]
    );
}

/// The other half of the classification. `EBUSY` means someone else
/// holds the freeze — transient, and a retry will get it. Taking the
/// crash-consistent path here would quietly downgrade every capture that
/// merely raced another freezer.
#[test]
fn a_transient_freeze_failure_still_fails_the_capture() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 9));
    r.ops
        .fail_next_freeze(StackError::Fault("EBUSY: filesystem already frozen"));

    let e = r.stack.capture("g0").expect_err("must not proceed");
    assert!(
        matches!(e, StackError::Fault(_)),
        "the transient propagates unchanged: {e:?}",
    );
    let ev = r.ops.events();
    assert!(
        !ev.iter().any(|x| matches!(x, E::CreateSnap(_))),
        "no snapshot was taken behind the failed freeze: {ev:?}",
    );
}

/// §8.1 rule 3's own remount does NOT arm the step 3 fallback, and that
/// is the kernel's answer, not a choice: `freeze_super` short-circuits
/// on `sb_rdonly` and returns 0, so a cleanly read-only filesystem
/// freezes *successfully*. Measured on 6.18.5:
///
/// ```text
///   ro,relatime                     FIFREEZE -> 0
///   rw,relatime,abort,emergency_ro  FIFREEZE -> EROFS
/// ```
///
/// This is the better outcome and worth pinning as intent: a volume
/// rule 3 reached in time gets an ordinary, filesystem-consistent
/// capture. The crash-consistent path is for the volume rule 3 did not
/// reach.
#[test]
fn a_cleanly_read_only_volume_still_freezes() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(2 * CHUNK as usize, 11));
    r.stack.degrade_read_only().expect("remount ro");
    assert!(r.ops.read_only());

    let snap = r.stack.capture("g0").expect("capture proceeds");
    let ev = r.ops.events();
    assert!(
        ev.contains(&E::Freeze) && ev.contains(&E::Thaw),
        "a read-only super is quiescent, not unfreezable: {ev:?}",
    );
    assert!(ev.contains(&E::Remount(true)));
    let img = r.stack.read(&snap.lv_name, 0, 2 * CHUNK).unwrap();
    assert_eq!(&img[..], &pat(2 * CHUNK as usize, 11)[..]);
}

/// What actually arms §8.2 step 3: the ABORTED super. The kernel still
/// reports it `rw` in `/proc/mounts` while refusing every write, which
/// is exactly why "read-only" and "aborted" cannot be one flag.
#[test]
fn an_aborted_filesystem_arms_the_crash_consistent_capture() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(2 * CHUNK as usize, 13));
    r.ops.abort_fs();
    // Still not read-only, as the kernel reports it.
    assert!(!r.ops.read_only(), "an aborted super reads rw");

    let snap = r.stack.capture("g0").expect("capture proceeds");
    let ev = r.ops.events();
    assert!(!ev.contains(&E::Freeze), "nothing to freeze: {ev:?}");
    assert!(!ev.contains(&E::Thaw), "and nothing to thaw: {ev:?}");
    let img = r.stack.read(&snap.lv_name, 0, 2 * CHUNK).unwrap();
    assert_eq!(&img[..], &pat(2 * CHUNK as usize, 13)[..]);
}

/// A healthy filesystem is untouched by any of this: the freeze bracket
/// is still there, and `capture` is still the freeze-consistent
/// operation §9.3 specifies. The fallback is a fallback.
#[test]
fn a_healthy_filesystem_still_freezes() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 5));
    r.stack.capture("g0").expect("capture");
    let ev = r.ops.events();
    assert!(ev.contains(&E::Freeze) && ev.contains(&E::Thaw), "{ev:?}");
    assert!(!r.ops.frozen(), "fs left thawed");
}

/// The `DmStack` → `DmOps` forwarding edge, for BOTH mount probes.
///
/// This is the seam that had no test and lost one: `mount_is_writable`
/// was implemented on `DmOps`, on `LinuxDmOps`, on the mock, on
/// `LocalStack` and on `FakeStack` — and never forwarded by `DmStack`,
/// so the production path silently took the trait default (`None`) and
/// §8.2 step 6's guard, written as `== Some(false)`, could never fire.
/// Every engine-level test passed, because they all drive `FakeStack`,
/// which does implement it.
///
/// Asserting through `LocalStack` rather than `DmOps` is the entire
/// point: a forwarder that does not exist is invisible from below.
#[test]
fn the_mount_probes_reach_the_kernel_seam() {
    let r = rig();

    // Healthy: writable, not read-only.
    assert_eq!(
        LocalStack::mount_is_writable(&r.stack),
        Some(true),
        "a healthy stack must answer, not fall through to the default"
    );
    assert_eq!(LocalStack::mount_is_read_only(&r.stack), Some(false));

    // Cleanly read-only: visible to both probes.
    r.stack.degrade_read_only().expect("remount ro");
    assert_eq!(LocalStack::mount_is_writable(&r.stack), Some(false));
    assert_eq!(LocalStack::mount_is_read_only(&r.stack), Some(true));
    r.stack.restore_read_write().expect("remount rw");

    // Aborted: the case the guard exists for. Only the write probe sees
    // it — the read-only probe reports `rw`, as the kernel does.
    r.ops.abort_fs();
    assert_eq!(
        LocalStack::mount_is_writable(&r.stack),
        Some(false),
        "the aborted super must reach the engine as a definite negative"
    );
    assert_eq!(
        LocalStack::mount_is_read_only(&r.stack),
        Some(false),
        "and it still reports rw, which is why the write probe exists"
    );
}
