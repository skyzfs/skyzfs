//! The kernel contracts that cost nightly rounds 2–4, enforced rootlessly
//! (M9 `DmOps` seam; tests/README.md lessons table).
//!
//! `MockDmOps` plays the kernel and REJECTS what the kernel rejects;
//! `DmStack` — the real orchestration code, not a test double — must
//! drive it correctly. A regression that reorders freeze/suspend/
//! create_snap, skips activation before reads, or leaks a metadata-snap
//! reservation now fails `cargo test` instead of a privileged nightly.

use std::time::Duration;

use skynbd_agent::dm::{DmConfig, DmStack};
use skynbd_agent::dm_mock::{MockDmOps, MockEvent as E};
use skynbd_agent::stack::{DeltaRange, LocalStack, SnapRole};

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

fn rig() -> Rig {
    rig_with_budget(5_000)
}

fn pat(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

/// Round 2 as an ordering assertion: freeze → suspend → create_snap →
/// resume → thaw, activation strictly after the thaw.
#[test]
fn capture_orders_kernel_calls() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(2 * CHUNK as usize, 3));
    let snap = r.stack.capture("g0").expect("capture");
    assert_eq!(snap.lv_name, "g0");

    let ev = r.ops.events();
    let pos = |e: &E| {
        ev.iter()
            .position(|x| x == e)
            .unwrap_or_else(|| panic!("missing {e:?}"))
    };
    let freeze = pos(&E::Freeze);
    let suspend = pos(&E::Suspend(ORIGIN.into()));
    let create = pos(&E::CreateSnap(snap.thin_dev_id));
    let resume = pos(&E::Resume(ORIGIN.into()));
    let thaw = pos(&E::Thaw);
    let activate = pos(&E::ActivateRo("sky-vlin-c-g0".into()));
    assert!(freeze < suspend, "suspend inside the freeze window");
    assert!(suspend < create, "create_snap only after suspend (round 2)");
    assert!(create < resume && resume < thaw, "resume before thaw");
    assert!(thaw < activate, "activation outside the freeze window");
    assert!(!r.ops.frozen(), "fs left thawed");

    // The snapshot device is readable through the stack (activated).
    let img = r.stack.read("g0", 0, VOL).unwrap();
    assert_eq!(&img[..2 * CHUNK as usize], &pat(2 * CHUNK as usize, 3)[..]);
}

/// The mock is armed: issuing create_snap against an unsuspended active
/// origin is rejected exactly like the kernel does. (This is the test
/// that proves test-above's ordering assertions have teeth.)
#[test]
fn mock_rejects_create_snap_without_suspend() {
    use skynbd_agent::dm::DmOps;
    let r = rig();
    let e = r
        .ops
        .pool_message(POOL, &format!("create_snap 7 {ORIGIN_ID}"))
        .unwrap_err();
    assert!(format!("{e}").contains("EINVAL"), "got: {e}");
    r.ops.suspend_device(ORIGIN).unwrap();
    r.ops
        .pool_message(POOL, &format!("create_snap 7 {ORIGIN_ID}"))
        .expect("suspended origin snapshots fine");
    r.ops.resume_device(ORIGIN).unwrap();
}

/// A stale dev-id sidecar self-heals: capture retries past EEXIST.
#[test]
fn capture_retries_past_stale_sidecar() {
    let r = rig();
    // The pool already knows ids 2 and 3; the (absent) sidecar starts at 2.
    r.ops.seed_thin(2);
    r.ops.seed_thin(3);
    let snap = r.stack.capture("g0").expect("capture");
    assert_eq!(snap.thin_dev_id, 4, "skipped the ids the pool had");
}

/// Suspend failure must not strand the filesystem frozen (V3.2 §9.3).
#[test]
fn suspend_failure_leaves_fs_thawed() {
    let r = rig();
    r.ops.fail_next_suspend();
    let e = r.stack.capture("g0").unwrap_err();
    assert!(format!("{e}").contains("suspend"), "got: {e}");
    assert!(!r.ops.frozen(), "fs thawed on the failure path");
    assert!(
        !r.ops.events().iter().any(|e| matches!(e, E::CreateSnap(_))),
        "no snapshot without a suspend"
    );
    assert_eq!(r.ops.thin_ids(), vec![ORIGIN_ID], "pool unchanged");
}

/// §7.11: the watchdog force-thaws a blown freeze window; the capture
/// deletes its (possibly thaw-spanning) snapshot and fails cleanly.
#[test]
fn watchdog_force_thaw_fails_capture_and_deletes_snap() {
    let r = rig_with_budget(25);
    r.ops.delay_create_snap(Duration::from_millis(300));
    let e = r.stack.capture("g0").unwrap_err();
    assert!(format!("{e}").contains("watchdog"), "got: {e}");
    let ev = r.ops.events();
    assert!(ev.contains(&E::ForceThaw), "watchdog thawed independently");
    assert!(
        ev.iter().any(|e| matches!(e, E::DeleteThin(_))),
        "suspect snapshot deleted"
    );
    assert_eq!(r.ops.thin_ids(), vec![ORIGIN_ID], "no snapshot survives");
    assert!(!r.ops.frozen());
}

/// COW is real: a captured snapshot is point-in-time.
#[test]
fn capture_is_point_in_time() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 11));
    let before = r.ops.origin_image(ORIGIN);
    r.stack.capture("g0").unwrap();
    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 99));
    assert_eq!(r.stack.read("g0", 0, VOL).unwrap(), before);
    assert_ne!(r.ops.origin_image(ORIGIN), before);
}

/// Deltas: reservation is taken and released around the reads, and the
/// computed ranges match what changed (including a discard → Clear).
#[test]
fn thin_delta_pairs_reservation_and_diffs_correctly() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(2 * CHUNK as usize, 5));
    r.ops
        .write_origin(ORIGIN, 9 * CHUNK, &pat(CHUNK as usize, 6));
    r.stack.capture("g0").unwrap();
    r.stack.set_role("g0", Some(SnapRole::Baseline)).unwrap();

    r.ops.write_origin(ORIGIN, CHUNK, &pat(CHUNK as usize, 77)); // modify block 1
    r.ops
        .write_origin(ORIGIN, 4 * CHUNK, &pat(CHUNK as usize, 78)); // new block 4
    r.ops.discard_origin(ORIGIN, 9 * CHUNK, CHUNK); // drop block 9
    r.stack.capture("g1").unwrap();

    let d = r.stack.thin_delta("g0", "g1").unwrap();
    assert_eq!(
        d,
        vec![
            DeltaRange::Data {
                off: CHUNK,
                len: CHUNK
            },
            DeltaRange::Data {
                off: 4 * CHUNK,
                len: CHUNK
            },
            DeltaRange::Clear {
                off: 9 * CHUNK,
                len: CHUNK
            },
        ]
    );
    assert!(!r.ops.meta_snap_reserved(), "reservation released (§7)");
    let ev = r.ops.events();
    let reserves = ev.iter().filter(|e| **e == E::ReserveMetaSnap).count();
    let releases = ev.iter().filter(|e| **e == E::ReleaseMetaSnap).count();
    assert_eq!(reserves, releases, "reserve/release strictly paired");

    // full_mapped releases too, and covers exactly the mapped blocks.
    // (Run boundaries follow data-block adjacency, as with real thinp
    // mappings — a CoW'd block splits a run — so assert coverage.)
    let full = r.stack.full_mapped("g1").unwrap();
    assert!(!r.ops.meta_snap_reserved());
    let covered: std::collections::BTreeSet<u64> = full
        .iter()
        .flat_map(|d| match d {
            DeltaRange::Data { off, len } => (off / CHUNK)..((off + len) / CHUNK),
            DeltaRange::Clear { .. } => panic!("full_mapped yields Data only"),
        })
        .collect();
    assert_eq!(covered, [0u64, 1, 4].into_iter().collect());
}

/// Round 4 adjacency: mapping reads REQUIRE a reserved metadata snapshot
/// (thinp's `read_superblock_snap` fails without one).
#[test]
fn mock_rejects_mappings_without_reservation() {
    use skynbd_agent::dm::DmOps;
    let r = rig();
    let e = r
        .ops
        .thin_mappings(std::path::Path::new("/x"), ORIGIN_ID)
        .unwrap_err();
    assert!(format!("{e}").contains("metadata snap"), "got: {e}");
}

/// Round 3: pool membership is not readability — a thin dev with no
/// activated dm device has no node to read.
#[test]
fn read_requires_activation() {
    use skynbd_agent::dm::DmOps;
    let r = rig();
    // Snapshot exists in the pool but is never activated as a dm device.
    r.ops.suspend_device(ORIGIN).unwrap();
    r.ops
        .pool_message(POOL, &format!("create_snap 9 {ORIGIN_ID}"))
        .unwrap();
    r.ops.resume_device(ORIGIN).unwrap();
    let e = r.ops.read_dev("sky-vlin-c-ghost", 0, CHUNK).unwrap_err();
    assert!(format!("{e}").contains("activate"), "got: {e}");
    // Through the stack, an unknown logical name is NotFound.
    assert!(r.stack.read("ghost", 0, CHUNK).is_err());
}

/// Roles live in renameable dm names; observation reconstructs them from
/// the device list alone; deletion removes device then thin.
#[test]
fn roles_rename_observe_and_delete() {
    let r = rig();
    r.stack.capture("g0").unwrap();
    let obs = r.stack.observe();
    assert_eq!(obs.candidates, vec!["g0"]);
    assert!(obs.baselines.is_empty());

    r.stack.set_role("g0", Some(SnapRole::Baseline)).unwrap();
    assert!(r
        .ops
        .events()
        .contains(&E::Rename("sky-vlin-c-g0".into(), "sky-vlin-b-g0".into())));
    let obs = r.stack.observe();
    assert_eq!(obs.baselines, vec!["g0"]);
    assert!(obs.candidates.is_empty());

    // Same role: no-op.
    let before = r.ops.events().len();
    r.stack.set_role("g0", Some(SnapRole::Baseline)).unwrap();
    assert_eq!(r.ops.events().len(), before);

    // Untag is a REAL rename (`u-`): the device stops looking like a
    // baseline to observers, but stays findable for deletion. This is the
    // §15.4 fix the DmStack crash matrix forced — a no-op untag left two
    // `b-` devices visible mid-promotion and recovery quarantined.
    r.stack.set_role("g0", None).unwrap();
    assert!(r
        .ops
        .events()
        .contains(&E::Rename("sky-vlin-b-g0".into(), "sky-vlin-u-g0".into())));
    let obs = r.stack.observe();
    assert!(obs.baselines.is_empty() && obs.candidates.is_empty());
    r.stack.set_role("g0", None).unwrap(); // idempotent

    r.stack.snap_delete("g0").unwrap();
    assert_eq!(r.ops.thin_ids(), vec![ORIGIN_ID]);
    assert_eq!(
        r.ops.device_names(),
        vec![ORIGIN.to_string(), POOL.to_string()]
    );
}

/// The mock enforces the kernel's delete rule: an active dm device pins
/// its thin dev (DmStack must remove the device first, as snap_delete does).
#[test]
fn mock_rejects_delete_of_active_thin() {
    use skynbd_agent::dm::DmOps;
    let r = rig();
    let snap = r.stack.capture("g0").unwrap();
    let e = r
        .ops
        .pool_message(POOL, &format!("delete {}", snap.thin_dev_id))
        .unwrap_err();
    assert!(format!("{e}").contains("EBUSY"), "got: {e}");
}

// ------------------------------------------------------- D-M15 reclaim

fn fill_chunk(img: &mut [u8], c: u64, seed: u8) {
    let a = (c * CHUNK) as usize;
    img[a..a + CHUNK as usize].copy_from_slice(&pat(CHUNK as usize, seed));
}

/// Invariant B end to end at Tier A: reclaim discards exactly
/// `mapped(S) − dirtied-since(S)`, and with the external origin serving
/// the generation published from S (the repoint that must precede it),
/// no observable byte changes — reads of the reclaimed ranges now come
/// through the backing. Also asserts the repoint itself was invisible
/// (Invariant A) and the freeze/reservation hygiene.
#[test]
fn reclaim_preserves_reads_and_unmaps_only_undirtied_shared_blocks() {
    let r = rig();
    let backing0 = pat(VOL as usize, 0x10);
    r.ops.set_external_origin(ORIGIN, backing0.clone());

    // Pre-capture writes: chunks 1, 2, 4.
    r.ops
        .write_origin(ORIGIN, CHUNK, &pat(CHUNK as usize, 0x21));
    r.ops
        .write_origin(ORIGIN, 2 * CHUNK, &pat(CHUNK as usize, 0x22));
    r.ops
        .write_origin(ORIGIN, 4 * CHUNK, &pat(CHUNK as usize, 0x24));
    r.stack.capture("s").expect("capture");

    // Dirtied-since: chunk 2 rewritten; chunk 6 newly provisioned.
    r.ops
        .write_origin(ORIGIN, 2 * CHUNK, &pat(CHUNK as usize, 0x42));
    r.ops
        .write_origin(ORIGIN, 6 * CHUNK, &pat(CHUNK as usize, 0x56));

    // The generation published from S: base read-through ⊕ S's content.
    let mut published = backing0;
    fill_chunk(&mut published, 1, 0x21);
    fill_chunk(&mut published, 2, 0x22);
    fill_chunk(&mut published, 4, 0x24);

    // Repoint (backing swap) is invisible to origin reads (Invariant A).
    let before_repoint = r.ops.origin_image(ORIGIN);
    r.ops.set_external_origin(ORIGIN, published);
    let before = r.ops.origin_image(ORIGIN);
    assert_eq!(before, before_repoint, "repoint must not change a byte");

    // Reclaim: chunks 1 and 4 are shared with S; chunk 2 was rewritten
    // (fresh data block) and chunk 6 is not in S — both stay mapped.
    let bytes = r.stack.reclaim("s").expect("reclaim");
    assert_eq!(bytes, 2 * CHUNK);
    assert_eq!(r.ops.mapped_blocks(ORIGIN), vec![2, 6]);

    // The content-equivalence claim: not one observable byte moved.
    assert_eq!(r.ops.origin_image(ORIGIN), before);

    // Discards happened inside the freeze window; hygiene holds after.
    let ev = r.ops.events();
    let last_freeze = ev.iter().rposition(|e| *e == E::Freeze).unwrap();
    let last_thaw = ev.iter().rposition(|e| *e == E::Thaw).unwrap();
    let discards: Vec<usize> = ev
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, E::Discard(..)))
        .map(|(i, _)| i)
        .collect();
    assert!(!discards.is_empty());
    for d in discards {
        assert!(
            last_freeze < d && d < last_thaw,
            "discard outside the freeze window"
        );
    }
    assert!(!r.ops.frozen(), "fs left frozen after reclaim");
    assert!(
        !r.ops.meta_snap_reserved(),
        "metadata reservation leaked by reclaim"
    );
}

/// Everything S mapped was rewritten since → the reclaim set is empty:
/// zero bytes, no discard issued, snapshot content untouched.
#[test]
fn reclaim_of_fully_dirtied_origin_discards_nothing() {
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 1));
    r.ops
        .write_origin(ORIGIN, 3 * CHUNK, &pat(CHUNK as usize, 2));
    r.stack.capture("s").expect("capture");
    let snap_before = r.stack.read("s", 0, VOL).unwrap();

    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 9));
    r.ops
        .write_origin(ORIGIN, 3 * CHUNK, &pat(CHUNK as usize, 8));

    assert_eq!(r.stack.reclaim("s").unwrap(), 0);
    assert!(!r.ops.events().iter().any(|e| matches!(e, E::Discard(..))));
    // The snapshot still owns its (now unshared) blocks.
    assert_eq!(r.stack.read("s", 0, VOL).unwrap(), snap_before);
    assert!(!r.ops.frozen());
    assert!(!r.ops.meta_snap_reserved());
}

/// The dry-run range list matches what a quiesced reclaim discards.
#[test]
fn reclaim_ranges_previews_the_discard_set() {
    let r = rig();
    r.ops.set_external_origin(ORIGIN, pat(VOL as usize, 0x10));
    r.ops
        .write_origin(ORIGIN, CHUNK, &pat(2 * CHUNK as usize, 0x21));
    r.stack.capture("s").expect("capture");

    let ranges = r.stack.reclaim_ranges("s").unwrap();
    assert_eq!(ranges, vec![(CHUNK, 2 * CHUNK)], "coalesced shared run");
    // Repoint: backing := base ⊕ S's content over the shared run.
    let before = r.ops.origin_image(ORIGIN);
    let mut repointed = pat(VOL as usize, 0x10);
    repointed[CHUNK as usize..3 * CHUNK as usize]
        .copy_from_slice(&before[CHUNK as usize..3 * CHUNK as usize]);
    r.ops.set_external_origin(ORIGIN, repointed);
    assert_eq!(
        r.stack.reclaim("s").unwrap(),
        ranges.iter().map(|(_, l)| l).sum::<u64>()
    );
    assert_eq!(r.ops.origin_image(ORIGIN), before);
}

/// The kernel unmaps only whole pool blocks; the mock rejects a
/// misaligned discard so the agent can never half-reclaim silently.
#[test]
fn mock_rejects_misaligned_discard() {
    use skynbd_agent::dm::DmOps;
    let r = rig();
    r.ops.write_origin(ORIGIN, 0, &pat(CHUNK as usize, 1));
    assert!(r.ops.discard_dev(ORIGIN, 100, CHUNK).is_err());
    assert!(r.ops.discard_dev(ORIGIN, 0, CHUNK / 2).is_err());
    assert!(r.ops.discard_dev(ORIGIN, 0, 0).is_err());
    assert!(r.ops.discard_dev(ORIGIN, 0, CHUNK).is_ok());
}

/// D-M15 discard port: with `discard_name` set to a dm-linear sibling of
/// the origin, the reclaim's discards route through the port and unmap
/// the origin thin's blocks — the shape production must use, because the
/// kernel refuses BLKDISCARD on a claimed (mounted) device.
#[test]
fn reclaim_discards_through_a_linear_port() {
    use skynbd_agent::dm::DmOps;
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
    // The port: a linear over the origin (dm's shared claim holder makes
    // this legal on a real kernel even while a shim/fs holds the origin).
    ops.create_device(
        "rec",
        "SKYNBD-vlin-rec",
        &[(
            0,
            VOL / 512,
            "linear".into(),
            format!("/dev/mapper/{ORIGIN} 0"),
        )],
    )
    .unwrap();
    let stack = DmStack::new(
        DmConfig {
            pool_name: POOL.into(),
            origin_name: ORIGIN.into(),
            origin_dev_id: ORIGIN_ID,
            metadata_dev: dir.path().join("tmeta"),
            mountpoint: dir.path().to_path_buf(),
            volume: "vlin".into(),
            size_bytes: VOL,
            chunk_bytes: CHUNK,
            ids_path: dir.path().join("vlin.devids"),
            freeze_budget_ms: 5_000,
            discard_name: Some("rec".into()),
            backing_file: None,
            shim_name: None,
            mount_name: None,
            slot: None,
        },
        ops.clone(),
    )
    .unwrap();

    ops.set_external_origin(ORIGIN, pat(VOL as usize, 0x10));
    ops.write_origin(ORIGIN, CHUNK, &pat(CHUNK as usize, 0x21));
    stack.capture("s").expect("capture");
    let before = ops.origin_image(ORIGIN);
    let mut repointed = pat(VOL as usize, 0x10);
    repointed[CHUNK as usize..2 * CHUNK as usize]
        .copy_from_slice(&before[CHUNK as usize..2 * CHUNK as usize]);
    ops.set_external_origin(ORIGIN, repointed);

    assert_eq!(stack.reclaim("s").unwrap(), CHUNK);
    assert_eq!(ops.origin_image(ORIGIN), before, "port discard moved bytes");
    assert!(
        ops.mapped_blocks(ORIGIN).is_empty(),
        "origin block not unmapped through the port"
    );
    // The discard event names the PORT, not the origin: the ioctl went
    // where the kernel's claim rules allow.
    assert!(ops
        .events()
        .iter()
        .any(|e| matches!(e, E::Discard(n, _, _) if n == "rec")));
}

/// §7.1: on a cached stack the guest mounts the rcache, whose queue
/// advertises its own (nonzero) virtual discard limits with passdown
/// disabled — the shim's mask does not re-export through it. The
/// per-cycle re-assert must therefore land on BOTH queues; verified on
/// the shim alone, it would report a mask the guest cannot see while
/// the device the guest CAN see advertises discard support.
#[test]
fn the_per_cycle_reassert_masks_the_mount_device_too() {
    use skynbd_agent::dm::DmOps as _;
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
    for name in ["shim", "rcache"] {
        ops.create_device(
            name,
            &format!("uuid-{name}"),
            &[(
                0,
                VOL / 512,
                "linear".into(),
                format!("/dev/mapper/{ORIGIN} 0"),
            )],
        )
        .unwrap();
    }
    let stack = DmStack::new(
        DmConfig {
            pool_name: POOL.into(),
            origin_name: ORIGIN.into(),
            origin_dev_id: ORIGIN_ID,
            metadata_dev: dir.path().join("tmeta"),
            mountpoint: dir.path().to_path_buf(),
            volume: "vlin".into(),
            size_bytes: VOL,
            chunk_bytes: CHUNK,
            ids_path: dir.path().join("vlin.devids"),
            freeze_budget_ms: 5_000,
            discard_name: None,
            backing_file: None,
            shim_name: Some("shim".into()),
            mount_name: Some("rcache".into()),
            slot: None,
        },
        ops.clone(),
    )
    .unwrap();
    stack.assert_guest_discard_mask().unwrap();
    let ev = ops.events();
    for dev in ["shim", "rcache"] {
        assert!(
            ev.iter()
                .any(|e| matches!(e, E::SetDiscardMax(n, 0) if n == dev)),
            "{dev} not masked: {ev:?}"
        );
    }
}

/// Invariant B's silent opt-out, closed. `DmConfig.shim_name` is
/// `#[serde(default)]` and `slot` is optional, so every config-file
/// deployment written before either field existed carries neither — and
/// the per-cycle re-assert used to answer `Ok(())` for exactly those
/// stacks. `Ok` is the mask's one claim of protection: a deployment
/// with no shim at all was reporting it, which made "a lapse cannot
/// outlive one cadence interval unnoticed" a statement about the
/// deployments that had opted in and about nothing else.
#[test]
fn a_stack_with_no_shim_configured_is_unsupported_not_masked() {
    let r = rig();
    let err = r
        .stack
        .assert_guest_discard_mask()
        .expect_err("an unconfigured mask must not read as an asserted one");
    assert!(
        matches!(err, skynbd_agent::StackError::Unsupported(_)),
        "the caller has to tell a config gap apart from a shim that \
         refuses the mask — those need different responses: {err}"
    );
    // And the message names the knob an operator would set.
    let text = err.to_string();
    assert!(text.contains("shim_name"), "{text}");
    assert!(text.contains("vlin"), "{text}");
    // Nothing was written to any queue, because there was no queue to
    // write to: the answer is about configuration, not about a failure.
    assert!(
        !r.ops
            .events()
            .iter()
            .any(|e| matches!(e, E::SetDiscardMax(_, _))),
        "{:?}",
        r.ops.events()
    );
}
