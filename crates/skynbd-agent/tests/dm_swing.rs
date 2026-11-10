//! The two-pool swing against the contract-enforcing mock kernel
//! (design doc §7): slot-B build, barrier-diffed live copy, the frozen
//! top-up → retarget → flip → thaw window, wholesale A teardown with
//! canonicalization, both sweeps, and the §7.1 dual-discard eviction.

use std::path::PathBuf;
use std::time::Duration;

use skynbd_agent::dm::{mask_guest_discard, slot_b_path, DmConfig, DmStack, SlotConfig};
use skynbd_agent::dm_mock::{MockDmOps, MockEvent as E};
use skynbd_agent::stack::{DeltaRange, LocalStack, StackError, SwingHooks};
use skynbd_agent::DmOps;

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;
const META: u64 = 4 * CHUNK;
const POOL: &str = "skypool";
const ORIGIN: &str = "origin";
const ORIGIN_ID: u64 = 1;
const SHIM: &str = "shim";
const PORT: &str = "rec";
const EXT: &str = "/dev/nbd9";

struct Rig {
    ops: MockDmOps,
    stack: DmStack<MockDmOps>,
    live: PathBuf,
    _dir: tempfile::TempDir,
}

fn mm(ops: &MockDmOps, name: &str) -> String {
    let i = ops.device_info(name).unwrap();
    format!("{}:{}", i.major, i.minor)
}

fn pat(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

fn rig_with_budget(freeze_budget_ms: u64) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
    // The slot shape's upper linears: the shim the fs claims and the
    // unclaimed discard port, both over the origin.
    let origin_mm = mm(&ops, ORIGIN);
    for (name, part) in [(SHIM, "shim"), (PORT, "rec")] {
        ops.create_device(
            name,
            &format!("SKYNBD-vlin-stack-{part}"),
            &[(0, VOL / 512, "linear".into(), format!("{origin_mm} 0"))],
        )
        .unwrap();
    }
    // Slot A's backing file, attached and preallocated — the canonical
    // steady state a slot-managed stage leaves behind.
    let live = dir.path().join("pool.img");
    ops.prepare_backing(&live, META + VOL, true).unwrap();
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
            freeze_budget_ms,
            discard_name: Some(PORT.into()),
            backing_file: None,
            shim_name: None,
            mount_name: None,
            slot: Some(SlotConfig {
                live_file: live.clone(),
                data_bytes: VOL,
                meta_bytes: META,
                shim_name: SHIM.into(),
                external_origin: Some(EXT.into()),
                tmeta_name: None,
                data_name: None,
            }),
        },
        ops.clone(),
    )
    .unwrap();
    Rig {
        ops,
        stack,
        live,
        _dir: dir,
    }
}

fn rig() -> Rig {
    rig_with_budget(5_000)
}

/// Seed the standard swing scenario: base content through the origin
/// attach, pre-capture writes (chunks 1, 2), capture S, dirtied-since
/// (chunk 2 rewritten, chunk 6 new), then model the repoint — the
/// published-from-S generation served on the origin attach path (both
/// slots share it). Returns the origin's logical image at that point.
fn seed(r: &Rig) -> Vec<u8> {
    let base = pat(VOL as usize, 0x10);
    r.ops.set_external_origin(ORIGIN, base.clone());
    r.ops
        .write_origin(ORIGIN, CHUNK, &pat(CHUNK as usize, 0x21));
    r.ops
        .write_origin(ORIGIN, 2 * CHUNK, &pat(CHUNK as usize, 0x22));
    r.stack.capture("s").expect("capture");
    r.ops
        .write_origin(ORIGIN, 2 * CHUNK, &pat(CHUNK as usize, 0x42));
    r.ops
        .write_origin(ORIGIN, 6 * CHUNK, &pat(CHUNK as usize, 0x56));
    let mut published = base;
    for (c, seed) in [(1u64, 0x21u8), (2, 0x22)] {
        published[(c * CHUNK) as usize..((c + 1) * CHUNK) as usize]
            .copy_from_slice(&pat(CHUNK as usize, seed));
    }
    r.ops.set_external_origin(ORIGIN, published.clone());
    r.ops.set_path_image(EXT, published);
    r.ops.origin_image(ORIGIN)
}

/// The full swing arc: B built sparse, dirty set copied behind a
/// barrier, retarget + flip inside the freeze window (in §7's normative
/// order), A torn down wholesale, B canonicalized and preallocated —
/// with the pool ending as exactly the unpublished delta.
#[test]
fn swing_builds_copies_flips_and_canonicalizes() {
    let r = rig();
    let before = seed(&r);
    let usage_before = r.stack.pool_usage().expect("pool occupancy");

    let flip_at = std::cell::Cell::new(usize::MAX);
    let mut hook = || {
        flip_at.set(r.ops.events().len());
        Ok(())
    };
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    let stats = r.stack.swing("s", &mut hooks).expect("swing");
    assert!(stats.rounds >= 1);
    assert_eq!(stats.copied_bytes, 2 * CHUNK, "D = chunks 2 and 6");

    // §7 ordering: build B → copy → freeze → retarget → flip → thaw →
    // teardown/renames.
    let ev = r.ops.events();
    let pos = |e: &E| {
        ev.iter()
            .position(|x| x == e)
            .unwrap_or_else(|| panic!("missing {e:?}"))
    };
    let create_b = pos(&E::Create(format!("{ORIGIN}-b")));
    let first_copy = ev
        .iter()
        .position(|e| matches!(e, E::WriteDev(n, _, _) if n == &format!("{ORIGIN}-b")))
        .expect("copy writes");
    let last_freeze = ev.iter().rposition(|e| *e == E::Freeze).unwrap();
    let retarget = pos(&E::ReloadInFrozenWindow(SHIM.into()));
    let port_retarget = pos(&E::ReloadInFrozenWindow(PORT.into()));
    let last_thaw = ev.iter().rposition(|e| *e == E::Thaw).unwrap();
    let remove_a = pos(&E::Remove(ORIGIN.into()));
    let rename_b = pos(&E::Rename(format!("{ORIGIN}-b"), ORIGIN.into()));
    assert!(create_b < first_copy, "B exists before the copy");
    assert!(first_copy < last_freeze, "live rounds run unfrozen");
    assert!(last_freeze < retarget, "retarget inside the freeze window");
    assert!(retarget < flip_at.get(), "flip strictly after the retarget");
    assert!(port_retarget < flip_at.get(), "port follows the live thin");
    assert!(flip_at.get() <= last_thaw, "flip inside the window (§7)");
    assert!(last_thaw < remove_a, "teardown after the thaw");
    assert!(
        remove_a < rename_b,
        "canonical name freed before the rename"
    );

    // Post-state: canonical names resolve to the B pool; no slot names,
    // no barriers survive.
    let names = r.ops.device_names();
    for n in &names {
        assert!(!n.contains("-b") && !n.contains("swb"), "leftover {n}");
    }
    let pool_table = r.ops.table(POOL).expect("canonical pool is the B pool");
    assert_eq!(pool_table[0].2, "thin-pool");
    assert!(
        pool_table[0].3.contains("no_discard_passdown"),
        "{pool_table:?}"
    );
    let origin_table = r.ops.table(ORIGIN).expect("canonical origin is the B thin");
    assert_eq!(origin_table[0].2, "thin");
    assert!(
        origin_table[0].3.ends_with(&format!(" {ORIGIN_ID} {EXT}")),
        "the B thin carries A's dev id, so the config still resolves \
         it after the flip: {origin_table:?}"
    );
    assert_eq!(
        r.ops.thin_ids_of(POOL),
        vec![ORIGIN_ID],
        "the canonical pool holds exactly the origin, under its own id"
    );

    // Invariant C shape at the pool level: only the dirty set is
    // provisioned; everything else reads through the repointed origin.
    assert_eq!(
        r.ops.mapped_blocks(ORIGIN),
        vec![2, 6],
        "pool = unpublished delta"
    );
    // The swing IS the pool reset — and it is visible where the §4
    // watermark reads it, over the same capacity as before (a promotion
    // that changed the denominator would make the reset unreadable).
    let usage_after = r.stack.pool_usage().expect("pool occupancy");
    assert_eq!(
        usage_after,
        (2, usage_before.1),
        "occupancy after the swing is exactly D, over an unchanged pool: \
         {usage_before:?} → {usage_after:?}"
    );
    assert!(usage_after.0 < usage_before.0, "the pool shrank");
    assert_eq!(
        r.ops.read_dev(SHIM, 0, VOL).unwrap(),
        before,
        "not one byte moved"
    );
    assert_eq!(r.ops.origin_image(ORIGIN), before);

    // File side: the B file wears the canonical name, preallocated; the
    // slot name is gone.
    let (size, prealloc, loop_dev) = r.ops.backing_info(&r.live).expect("live file");
    assert_eq!(size, META + VOL);
    assert!(prealloc, "never-sparse steady state restored");
    assert!(loop_dev.is_some(), "loop still backs the live pool");
    assert!(r.ops.backing_info(&slot_b_path(&r.live)).is_none());

    // §7: which executor ran is not observable from outside the node —
    // so the cycle AFTER a swing must run on the canonicalized stack
    // exactly as it did before. A full snapshot cycle over it (capture,
    // enumerate under a metadata reservation, delete) proves the config
    // still resolves the origin and the machinery is executor-agnostic.
    let s2 = r.stack.capture("s2").expect("post-swing capture");
    assert_ne!(s2.thin_dev_id, ORIGIN_ID, "the capture minted its own id");
    let sets = r.stack.enumerate("s2").expect("post-swing enumerate");
    assert_eq!(
        sets.shared,
        vec![(2 * CHUNK, CHUNK), (6 * CHUNK, CHUNK)],
        "nothing written since the capture: the delta is wholly shared"
    );
    assert!(sets.dirty.is_empty(), "{:?}", sets.dirty);
    r.stack
        .snap_delete("s2")
        .expect("post-swing snapshot delete");
    assert_eq!(
        r.ops.thin_ids_of(POOL),
        vec![ORIGIN_ID],
        "the cycle leaves the canonical pool as it found it"
    );
    assert_eq!(
        r.ops.read_dev(SHIM, 0, VOL).unwrap(),
        before,
        "a post-swing cycle moves no byte either"
    );

    // Hygiene: thawed, no reservation leaked.
    assert!(!r.ops.frozen());
    assert!(!r.ops.meta_snap_reserved());
}

/// Invariant B's guest-discard mask survives the swing. A table reload
/// recomputes the queue's limits, so the retarget — which is a reload
/// of the shim — drops the mask unless the executor re-asserts it
/// inside the same window. Post-swing, an unmasked shim is a live
/// divergence vector on a default `fstrim.timer`: the unmap reads back
/// as the origin's published content while `thin_delta` calls the range
/// removed. The port must stay unmasked either side — reclaim is a
/// discard.
#[test]
fn the_retarget_re_asserts_the_guest_discard_mask() {
    let r = rig();
    seed(&r);
    // The steady-state guard a slot-managed stage leaves behind.
    mask_guest_discard(&r.ops, SHIM).expect("mask");
    assert_eq!(r.ops.discard_max_set(SHIM), Some(0));

    let mut hook = || Ok(());
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    r.stack.swing("s", &mut hooks).expect("swing");

    assert_eq!(
        r.ops.discard_max_set(SHIM),
        Some(0),
        "the shim's mask outlives the retarget"
    );
    assert_eq!(
        r.ops.discard_max_set(PORT),
        None,
        "the port is never masked — the agent's reclaim discards go there"
    );
    // Ordering: the re-assertion belongs to the retarget, not to some
    // earlier build step, so it must land after the LAST shim reload.
    let ev = r.ops.events();
    let retarget = ev
        .iter()
        .rposition(|e| *e == E::ReloadInFrozenWindow(SHIM.into()))
        .expect("the retarget reloaded the shim");
    assert!(
        ev.iter()
            .skip(retarget)
            .any(|e| *e == E::SetDiscardMax(SHIM.into(), 0)),
        "the mask is re-applied after the retarget: {ev:?}"
    );
    // And it means what it says on the canonicalized stack.
    let err = r.ops.discard_dev(SHIM, 0, CHUNK).unwrap_err();
    assert!(err.to_string().contains("EOPNOTSUPP"), "{err}");
    r.ops
        .discard_dev(PORT, 0, CHUNK)
        .expect("the port still reclaims after the swing");
}

/// The other side of task 15j's split verdict: at the retarget the
/// mask is verified too, but a failure must NOT unwind the swap. The
/// shim already points at slot B, the flip is about to be (or has been)
/// persisted, and rolling a completed swing back over a queue knob
/// would trade a discard hazard for the loss of the delta. So the swing
/// completes and the failure is logged — the loud, non-fatal arm.
#[test]
fn a_mask_that_cannot_be_re_asserted_does_not_unwind_the_swing() {
    let r = rig();
    let before = seed(&r);
    // The queue takes the write and drops it: after the retarget the
    // shim reads back unmasked, which `mask_guest_discard` reports.
    r.ops.set_discard_knob_ignored(SHIM);

    let mut flipped = false;
    let mut hook = || {
        flipped = true;
        Ok(())
    };
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    r.stack.swing("s", &mut hooks).expect("the swap stands");

    assert_eq!(
        r.ops.read_dev(SHIM, 0, VOL).unwrap(),
        before,
        "the completed swing still moves no byte"
    );
    assert_eq!(
        r.ops.discard_max_set(SHIM),
        None,
        "the mask genuinely did not take — that is the point of the readback"
    );
    assert!(!r.ops.frozen());
}

/// The readback's `Unsupported` is NOT the tolerated no-knob arm once
/// the write has succeeded. A kernel without the knob refuses the
/// write; a write that landed proves the knob existed one syscall ago,
/// so a readback that cannot find it means the name now resolves to a
/// different device — the reload race the readback exists to catch.
/// Reporting that combination as "masked OK" would hand the caller a
/// verdict about a shim it never examined.
#[test]
fn a_knob_that_vanishes_after_the_write_is_never_reported_masked() {
    let r = rig();
    seed(&r);
    r.ops.set_discard_knob_vanishing(SHIM);
    let err = mask_guest_discard(&r.ops, SHIM).expect_err("a swapped device is not a masked one");
    assert!(err.to_string().contains("replaced mid-mask"), "{err}");

    // The genuinely knob-less kernel is unchanged: it refuses the WRITE,
    // and that arm stays tolerated — a capability the host never had
    // cannot make a volume unstageable.
    let r = rig();
    seed(&r);
    r.ops.set_discard_knob_absent(SHIM);
    mask_guest_discard(&r.ops, SHIM).expect("no knob at all is tolerated");
}

/// `sweep_flipped_inner` is the recovery half of the retarget: it is
/// reached only because a swing's flip is durable, so the shim's queue
/// limits were recomputed by a reload this process did not perform.
/// Whether the pre-crash re-assert ran is exactly what the crash lost,
/// so the sweep re-derives it.
#[test]
fn the_flipped_sweep_re_asserts_the_guest_discard_mask() {
    let r = rig();
    seed(&r);
    // A flipped state the sweep can finish: run a swing, then drop the
    // mask behind its back the way a reload would.
    let mut hook = || Ok(());
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    r.stack.swing("s", &mut hooks).expect("swing");
    r.ops
        .set_discard_max_bytes(SHIM, 1 << 30)
        .expect("model the reload's recomputed limits");
    assert_eq!(r.ops.discard_max_set(SHIM), Some(1 << 30));

    r.stack.sweep_slot(true).expect("idempotent replay");
    assert_eq!(
        r.ops.discard_max_set(SHIM),
        Some(0),
        "the post-flip sweep re-asserts the mask instead of trusting the crashed run"
    );
}

/// A failing flip persist is a PRE-flip failure: the retarget is rolled
/// back inside the window (no write can ever have reached B), the swing
/// errors, and `sweep_slot(false)` removes every leftover.
#[test]
fn failed_flip_persist_rolls_back_and_sweeps_clean() {
    let r = rig();
    let before = seed(&r);
    let origin_a_mm = mm(&r.ops, ORIGIN);

    let mut hook = || Err(StackError::Fault("flip persist refused"));
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    let err = r.stack.swing("s", &mut hooks).unwrap_err();
    assert!(err.to_string().contains("flip persist"), "{err}");
    assert!(!r.ops.frozen(), "thawed on the failure path");

    // The shim points back at A — the abort is invisible to the mount.
    let shim_table = r.ops.table(SHIM).unwrap();
    assert_eq!(shim_table[0].3, format!("{origin_a_mm} 0"));
    assert_eq!(r.ops.read_dev(SHIM, 0, VOL).unwrap(), before);

    // The engine's fallback path: sweep B, then the port reclaim still
    // works on the untouched A stack.
    assert!(r.stack.sweep_slot(false).unwrap(), "leftovers existed");
    for n in r.ops.device_names() {
        assert!(!n.contains("-b") && !n.contains("swb"), "leftover {n}");
    }
    assert!(r.ops.backing_info(&slot_b_path(&r.live)).is_none());
    assert_eq!(r.stack.reclaim("s").unwrap(), CHUNK, "port fallback");
    assert_eq!(r.ops.origin_image(ORIGIN), before);
    assert!(
        !r.stack.sweep_slot(false).unwrap(),
        "second sweep finds nothing"
    );
}

/// §7.11 during the swing's freeze window, before the flip: the
/// watchdog force-thaw aborts the swing with no retarget and no flip —
/// slot B is garbage, exactly the pre-flip crash arm.
#[test]
fn watchdog_fired_during_swing_aborts_before_flip() {
    let r = rig_with_budget(25);
    let before = seed(&r);
    r.ops.delay_next_freeze(Duration::from_millis(300));

    let mut flipped = false;
    let mut hook = || {
        flipped = true;
        Ok(())
    };
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    let err = r.stack.swing("s", &mut hooks).unwrap_err();
    assert!(err.to_string().contains("watchdog"), "{err}");
    assert!(!flipped, "no flip once the window is blown");
    assert!(
        !r.ops.events().contains(&E::Reload(SHIM.into()))
            && !r
                .ops
                .events()
                .contains(&E::ReloadInFrozenWindow(SHIM.into())),
        "no retarget after a force-thaw"
    );
    assert!(r.ops.events().contains(&E::ForceThaw));
    assert!(!r.ops.frozen());
    // Pre-flip arm: sweep B, stack A intact.
    assert!(r.stack.sweep_slot(false).unwrap());
    assert_eq!(r.ops.origin_image(ORIGIN), before);
    assert_eq!(r.ops.read_dev(SHIM, 0, VOL).unwrap(), before);
}

/// `sweep_slot(true)` from a mid-teardown state (flip durable, A still
/// standing, B still slot-named): finishes the teardown, swaps the
/// file, canonicalizes, preallocates — and replays to a no-op.
#[test]
fn sweep_flipped_converges_from_mid_teardown() {
    let r = rig();
    let before = seed(&r);

    // Hand-build the exact post-flip pre-teardown state the swing's
    // freeze window leaves behind: B side complete, dirty set copied,
    // shim+port retargeted to B.
    let bpath = slot_b_path(&r.live);
    let loop_b = r.ops.prepare_backing(&bpath, META + VOL, false).unwrap();
    r.ops
        .create_device(
            &format!("{POOL}-btm"),
            "SKYNBD-vlin-slot-btm",
            &[(0, META / 512, "linear".into(), format!("{loop_b} 0"))],
        )
        .unwrap();
    r.ops
        .create_device(
            &format!("{POOL}-bda"),
            "SKYNBD-vlin-slot-bda",
            &[(
                0,
                VOL / 512,
                "linear".into(),
                format!("{loop_b} {}", META / 512),
            )],
        )
        .unwrap();
    r.ops
        .create_device(
            &format!("{POOL}-b"),
            "SKYNBD-vlin-slot-pool",
            &[(
                0,
                VOL / 512,
                "thin-pool".into(),
                format!(
                    "/dev/mapper/{POOL}-btm /dev/mapper/{POOL}-bda {} 32 2 \
                     skip_block_zeroing no_discard_passdown",
                    CHUNK / 512
                ),
            )],
        )
        .unwrap();
    r.ops
        .pool_message(&format!("{POOL}-b"), "create_thin 0")
        .unwrap();
    r.ops
        .create_device(
            &format!("{ORIGIN}-b"),
            "SKYNBD-vlin-slot-origin",
            &[(
                0,
                VOL / 512,
                "thin".into(),
                format!("{} 0 {EXT}", mm(&r.ops, &format!("{POOL}-b"))),
            )],
        )
        .unwrap();
    // The copied dirty set (chunks 2 and 6) and the retarget.
    for c in [2u64, 6] {
        let data = r.ops.read_dev(ORIGIN, c * CHUNK, CHUNK).unwrap();
        r.ops
            .write_dev(&format!("{ORIGIN}-b"), c * CHUNK, &data)
            .unwrap();
    }
    let b_mm = mm(&r.ops, &format!("{ORIGIN}-b"));
    for port in [SHIM, PORT] {
        r.ops
            .reload_device(
                port,
                &[(0, VOL / 512, "linear".into(), format!("{b_mm} 0"))],
            )
            .unwrap();
    }

    assert!(r.stack.sweep_slot(true).unwrap(), "teardown work existed");
    for n in r.ops.device_names() {
        assert!(!n.contains("-b") && !n.contains("swb"), "leftover {n}");
    }
    assert_eq!(r.ops.read_dev(SHIM, 0, VOL).unwrap(), before);
    assert_eq!(r.ops.mapped_blocks(ORIGIN), vec![2, 6]);
    let (_, prealloc, _) = r.ops.backing_info(&r.live).unwrap();
    assert!(prealloc);
    assert!(r.ops.backing_info(&bpath).is_none());

    // Idempotent replay: nothing left to sweep.
    assert!(!r.stack.sweep_slot(true).unwrap());
    assert_eq!(r.ops.read_dev(SHIM, 0, VOL).unwrap(), before);
}

/// §7.1 dual-discard eviction: each evicted range is discarded on BOTH
/// the live origin (through the port) and the retained snapshot's thin
/// (through a transient RW activation), so the data blocks really free
/// (last-ref drop) and reads stay byte-identical. FIFO = LBA order,
/// bounded by `max_ranges`.
#[test]
fn evict_dual_discards_and_preserves_reads() {
    let r = rig();
    let base = pat(VOL as usize, 0x10);
    r.ops.set_external_origin(ORIGIN, base.clone());
    // Chunks 1, 3, 4 written before capture; chunk 3 rewritten after —
    // shared = {1} and {4} (two LBA-ordered ranges).
    for (c, seed) in [(1u64, 0x21u8), (3, 0x23), (4, 0x24)] {
        r.ops
            .write_origin(ORIGIN, c * CHUNK, &pat(CHUNK as usize, seed));
    }
    r.stack.capture("s").expect("capture");
    r.ops
        .write_origin(ORIGIN, 3 * CHUNK, &pat(CHUNK as usize, 0x43));
    // The repoint: origin path serves the published-from-S generation.
    let mut published = base;
    for (c, seed) in [(1u64, 0x21u8), (3, 0x23), (4, 0x24)] {
        published[(c * CHUNK) as usize..((c + 1) * CHUNK) as usize]
            .copy_from_slice(&pat(CHUNK as usize, seed));
    }
    r.ops.set_external_origin(ORIGIN, published);
    let before = r.ops.origin_image(ORIGIN);
    let data_before = r.ops.data_block_count();

    // First window, FIFO-bounded to one range: chunk 1 only.
    assert_eq!(r.stack.evict("s", 1).unwrap(), CHUNK);
    assert_eq!(
        r.ops.mapped_blocks(ORIGIN),
        vec![3, 4],
        "live side unmapped"
    );
    assert_eq!(
        r.ops.mapped_blocks("sky-vlin-c-s"),
        vec![3, 4],
        "snapshot side unmapped too (no delta ever spans the eviction)"
    );
    assert_eq!(
        r.ops.data_block_count(),
        data_before - 1,
        "the shared data block really freed (last-ref drop)"
    );
    assert_eq!(r.ops.origin_image(ORIGIN), before, "eviction moved no byte");

    // The snapshot's RO activation is restored: the stack still reads
    // it, and the transient RW device is gone.
    assert!(r.stack.read("s", 3 * CHUNK, CHUNK).is_ok());
    assert!(!r.ops.device_names().iter().any(|n| n.contains("evict")));

    // Second window drains the rest: chunk 4 (chunk 3 was rewritten —
    // not shared, never evictable).
    assert_eq!(r.stack.evict("s", 1024).unwrap(), CHUNK);
    assert_eq!(r.ops.mapped_blocks(ORIGIN), vec![3]);
    assert_eq!(r.ops.origin_image(ORIGIN), before);
    // Nothing shared remains: a further call is a no-op window.
    assert_eq!(r.stack.evict("s", 1024).unwrap(), 0);
    assert!(!r.ops.frozen());
    assert!(!r.ops.meta_snap_reserved());
}

/// Enumerate yields both §7 sets from one dump: R (shared) and D
/// (dirty), consistent with what the executors then act on.
#[test]
fn enumerate_reports_shared_and_dirty() {
    let r = rig();
    seed(&r);
    let sets = r.stack.enumerate("s").unwrap();
    assert_eq!(sets.shared, vec![(CHUNK, CHUNK)], "chunk 1 undirtied");
    assert_eq!(sets.shared_count(), 1);
    assert_eq!(
        sets.dirty,
        vec![(2 * CHUNK, CHUNK), (6 * CHUNK, CHUNK)],
        "chunk 2 rewritten, chunk 6 new"
    );
    assert_eq!(sets.dirty_bytes(), 2 * CHUNK);
    assert!(!r.ops.meta_snap_reserved());

    // Headroom answers on a slot-managed stack, honoring the budget.
    r.ops.set_fs_free(123 << 20);
    let (free, required) = r.stack.headroom().unwrap();
    assert_eq!(free, 123 << 20);
    assert!(required >= META, "transient covers slot metadata");
}

/// The armed-but-retargeted crash arm (§7): a death between the
/// retarget and the flip persist leaves the shim and port aimed at slot
/// B under a record that still names A, with the freeze outliving the
/// process that armed the watchdog. The durable record is the
/// authority, so the pre-flip sweep is also the repair — tables back on
/// A, filesystem thawed — and only then may B be removed. Without the
/// repair the removal is EBUSY behind the shim's own reference, and a
/// swallowed error leaves a permanently frozen mount on the pool the
/// record calls garbage.
#[test]
fn armed_sweep_repairs_a_retargeted_shim_and_thaws() {
    let r = rig();
    seed(&r);
    // Reconstruct exactly what a process death in that window leaves.
    let bpath = slot_b_path(&r.live);
    let loop_b = r.ops.prepare_backing(&bpath, META + VOL, false).unwrap();
    r.ops
        .create_device(
            "skypool-btm",
            "SKYNBD-vlin-slot-btm",
            &[(0, META / 512, "linear".into(), format!("{loop_b} 0"))],
        )
        .unwrap();
    r.ops.write_dev("skypool-btm", 0, &[0u8; 4096]).unwrap();
    r.ops
        .create_device(
            "skypool-bda",
            "SKYNBD-vlin-slot-bda",
            &[(
                0,
                VOL / 512,
                "linear".into(),
                format!("{loop_b} {}", META / 512),
            )],
        )
        .unwrap();
    r.ops
        .create_device(
            "skypool-b",
            "SKYNBD-vlin-slot-pool",
            &[(
                0,
                VOL / 512,
                "thin-pool".into(),
                format!(
                    "{} {} {} 32 2 skip_block_zeroing no_discard_passdown",
                    mm(&r.ops, "skypool-btm"),
                    mm(&r.ops, "skypool-bda"),
                    CHUNK / 512
                ),
            )],
        )
        .unwrap();
    r.ops.pool_message("skypool-b", "create_thin 0").unwrap();
    r.ops
        .create_device(
            "origin-b",
            "SKYNBD-vlin-slot-origin",
            &[(
                0,
                VOL / 512,
                "thin".into(),
                format!("{} 0 {EXT}", mm(&r.ops, "skypool-b")),
            )],
        )
        .unwrap();
    let b_table = vec![(
        0u64,
        VOL / 512,
        "linear".to_string(),
        format!("{} 0", mm(&r.ops, "origin-b")),
    )];
    r.ops.reload_device(SHIM, &b_table).unwrap();
    r.ops.reload_device(PORT, &b_table).unwrap();
    r.ops.freeze_fs(std::path::Path::new("/mnt")).unwrap();
    assert!(r.ops.frozen(), "the freeze outlived the process");

    // A live slot B is a SECOND pool holding a thin, and the fixture
    // map cannot see it — the blindness that would let a matrix's
    // convergence check pass over an unswept slot.
    assert_eq!(
        r.ops.thin_ids(),
        vec![ORIGIN_ID, 2],
        "the fixture pool only"
    );
    assert_eq!(
        r.ops.all_thin_ids(),
        vec![
            (POOL.to_string(), vec![ORIGIN_ID, 2]),
            ("skypool-b".to_string(), vec![0])
        ],
        "slot B's pool and its thin are visible across pools"
    );

    // Everything the sweep itself does, isolated from the reconstruction
    // above (which uses plain reloads deliberately — it is standing in
    // for the dead process, not for this code).
    let before = r.ops.events().len();
    assert!(r.stack.sweep_slot(false).unwrap(), "swept the armed slot");
    let during: Vec<_> = r.ops.events().into_iter().skip(before).collect();

    // The sweep retargets a filesystem that is ALREADY frozen — the state
    // asserted five lines above. A plain reload there takes dm's implicit
    // lockfs on top of that freeze, and FREEZEPROBE measured every such
    // pair leaking the block-device freeze. `thaw_fs` cannot undo it:
    // FITHAW is `thaw_super`, not `bdev_thaw`. The result would be this
    // sweep reporting a successful repair on a volume that can never be
    // mounted again — on the crash path.
    assert!(
        during.contains(&E::ReloadInFrozenWindow(SHIM.into())),
        "the sweep's retarget must suppress dm's lockfs: {during:?}"
    );
    assert!(
        !during.contains(&E::Reload(SHIM.into())) && !during.contains(&E::Reload(PORT.into())),
        "a plain reload during the sweep leaks a freeze the thaw cannot \
         release: {during:?}"
    );
    assert_eq!(
        r.ops.all_thin_ids(),
        vec![(POOL.to_string(), vec![ORIGIN_ID, 2])],
        "the swept slot leaves no pool behind"
    );

    let a_mm = mm(&r.ops, ORIGIN);
    for name in [SHIM, PORT] {
        assert_eq!(
            r.ops.table(name).unwrap()[0].3,
            format!("{a_mm} 0"),
            "{name} is back on slot A — the record's authority"
        );
    }
    assert!(!r.ops.frozen(), "the sweep thawed the orphaned freeze");
    for gone in ["origin-b", "skypool-b", "skypool-btm", "skypool-bda"] {
        assert!(
            !r.ops.device_names().contains(&gone.to_string()),
            "{gone} removed once the shim stopped referencing it"
        );
    }
    assert!(!r.ops.backing_exists(&bpath).unwrap(), "B file deleted");
    // Idempotent: a retry over the repaired state is a clean no-op.
    assert!(!r.stack.sweep_slot(false).unwrap());
}

/// The armed sweep's thaw is the repair, not a courtesy: a filesystem
/// still frozen behind a refused `FITHAW` is a wedged mount, and every
/// removal below it is EBUSY behind the shim's own reference. Reporting
/// that as a successful sweep is what lets the caller clear the record
/// over a live slot B. "Was not frozen" is `Ok` on every
/// implementation, so anything that reaches here is the real thing and
/// must propagate.
#[test]
fn a_refused_thaw_fails_the_armed_sweep_instead_of_passing_it() {
    let r = rig();
    seed(&r);
    // The state a death between retarget and flip leaves: B standing,
    // shim on B, the freeze outliving the process.
    let bpath = slot_b_path(&r.live);
    let loop_b = r.ops.prepare_backing(&bpath, META + VOL, false).unwrap();
    r.ops
        .create_device(
            "skypool-btm",
            "SKYNBD-vlin-slot-btm",
            &[(0, META / 512, "linear".into(), format!("{loop_b} 0"))],
        )
        .unwrap();
    r.ops
        .create_device(
            "skypool-bda",
            "SKYNBD-vlin-slot-bda",
            &[(
                0,
                VOL / 512,
                "linear".into(),
                format!("{loop_b} {}", META / 512),
            )],
        )
        .unwrap();
    r.ops
        .create_device(
            "skypool-b",
            "SKYNBD-vlin-slot-pool",
            &[(
                0,
                VOL / 512,
                "thin-pool".into(),
                format!(
                    "{} {} {} 32 2 skip_block_zeroing no_discard_passdown",
                    mm(&r.ops, "skypool-btm"),
                    mm(&r.ops, "skypool-bda"),
                    CHUNK / 512
                ),
            )],
        )
        .unwrap();
    r.ops.pool_message("skypool-b", "create_thin 0").unwrap();
    r.ops
        .create_device(
            "origin-b",
            "SKYNBD-vlin-slot-origin",
            &[(
                0,
                VOL / 512,
                "thin".into(),
                format!("{} 0 {EXT}", mm(&r.ops, "skypool-b")),
            )],
        )
        .unwrap();
    r.ops.freeze_fs(std::path::Path::new("/mnt")).unwrap();

    r.ops.fail_next_thaw();
    let err = r
        .stack
        .sweep_slot(false)
        .expect_err("a sweep that could not thaw is not a sweep");
    assert!(err.to_string().contains("FITHAW"), "{err}");
    assert!(
        r.ops.frozen(),
        "the mount is still frozen — nothing to hide"
    );
    assert!(
        r.ops.device_names().contains(&"origin-b".to_string()),
        "B is untouched: the caller must not clear the record over it"
    );

    // The retry converges once the thaw takes.
    assert!(
        r.stack.sweep_slot(false).unwrap(),
        "the replayed sweep runs"
    );
    assert!(!r.ops.frozen());
    assert!(!r.ops.device_names().contains(&"origin-b".to_string()));
}

/// §7.1 dual discard, ordered: the retained snapshot goes first. Only
/// mapped-in-live-only survives a gap between the two — a `Data`
/// re-upload. The reverse asymmetry would publish `write_zeroes` over
/// an LBA the filesystem still owns.
#[test]
fn evict_discards_the_snapshot_side_before_the_live_side() {
    let r = rig();
    let base = pat(VOL as usize, 0x10);
    r.ops.set_external_origin(ORIGIN, base.clone());
    r.ops
        .write_origin(ORIGIN, CHUNK, &pat(CHUNK as usize, 0x21));
    r.stack.capture("s").expect("capture");
    let mut published = base;
    published[CHUNK as usize..2 * CHUNK as usize].copy_from_slice(&pat(CHUNK as usize, 0x21));
    r.ops.set_external_origin(ORIGIN, published);

    r.ops.events(); // drain
    assert_eq!(r.stack.evict("s", 1).unwrap(), CHUNK);

    let order: Vec<String> = r
        .ops
        .events()
        .into_iter()
        .filter_map(|e| match e {
            E::Discard(name, _, _) => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        vec!["sky-vlin-evict".to_string(), PORT.to_string()],
        "snapshot side first, then live"
    );
}

/// B3's ordering rationale, made to happen: die BETWEEN the snapshot-
/// side and live-side discards of one range. Exactly one asymmetry
/// survives such a death, and only one of the two is safe —
/// mapped-in-live-only re-uploads its true content as a `Data` range,
/// while mapped-in-S-only would publish `write_zeroes` over an LBA the
/// filesystem still owns. The ordering is the whole guarantee, so it is
/// asserted through the delta the next cycle would actually compute.
#[test]
fn a_death_between_the_dual_discards_leaves_only_the_safe_asymmetry() {
    let r = rig();
    let base = pat(VOL as usize, 0x10);
    r.ops.set_external_origin(ORIGIN, base.clone());
    r.ops
        .write_origin(ORIGIN, CHUNK, &pat(CHUNK as usize, 0x21));
    r.stack.capture("s").expect("capture");
    let mut published = base;
    published[CHUNK as usize..2 * CHUNK as usize].copy_from_slice(&pat(CHUNK as usize, 0x21));
    r.ops.set_external_origin(ORIGIN, published);
    let before = r.ops.origin_image(ORIGIN);

    // The live-side discard is the second of the pair: fail it, and the
    // window closes with the range unmapped on S alone... or on live
    // alone, depending on the order the executor chose.
    r.ops.fail_next_discard_on(PORT);
    let err = r.stack.evict("s", 1).unwrap_err();
    assert!(err.to_string().contains("discard"), "{err}");

    assert_eq!(
        r.ops.mapped_blocks("sky-vlin-c-s"),
        Vec::<u64>::new(),
        "the snapshot side went first, so it is the side that lost it"
    );
    assert_eq!(
        r.ops.mapped_blocks(ORIGIN),
        vec![1],
        "the live side still maps the range — the safe asymmetry"
    );

    // What the next cycle sees. `Data` = re-upload the block's true
    // content (a wasted interval, nothing more); a `Clear` here would
    // be the corrupting publish.
    r.stack.capture("s2").expect("post-failure capture");
    let delta = r.stack.thin_delta("s", "s2").expect("delta");
    assert_eq!(
        delta,
        vec![DeltaRange::Data {
            off: CHUNK,
            len: CHUNK
        }],
        "mapped-in-live-only, never mapped-in-S-only"
    );
    assert!(
        !delta.iter().any(|d| matches!(d, DeltaRange::Clear { .. })),
        "a Clear would write_zeroes over an LBA the fs still owns: {delta:?}"
    );

    // Hygiene: the failed window restored S's RO activation, thawed, and
    // moved no byte.
    assert!(r.stack.read("s", 0, VOL).is_ok(), "S is readable again");
    assert!(!r.ops.device_names().iter().any(|n| n.contains("evict")));
    assert!(!r.ops.frozen());
    assert!(!r.ops.meta_snap_reserved());
    assert_eq!(r.ops.origin_image(ORIGIN), before);
}

/// The sliver between the pre-retarget watchdog check and the suspend:
/// a fire there lets writes onto A that the top-up already read, and the
/// retarget strands them. The retarget is also what makes them
/// recoverable — A is quiescent behind it — so the executor re-diffs and
/// copies once more before the flip. Without that pass the wholesale
/// teardown destroys acked writes.
#[test]
fn late_writes_stranded_by_a_forced_thaw_are_copied_before_the_flip() {
    let r = rig_with_budget(25);
    seed(&r);
    // The retarget stalls past the freeze budget: the watchdog
    // force-thaws with the shim mid-flight.
    r.ops.delay_next_reload(Duration::from_millis(120));

    let mut flipped = false;
    let mut hook = || {
        flipped = true;
        Ok(())
    };
    let mut retargeted = false;
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut retargeted,
    };
    let stats = r.stack.swing("s", &mut hooks).expect("swing completes");

    assert!(r.ops.events().contains(&E::ForceThaw), "the watchdog fired");
    assert!(flipped, "the flip still persists — B is authoritative");
    // The late pass re-reserved the metadata snapshot after the
    // retarget: that reservation is the recovery of the stranded writes.
    let ev = r.ops.events();
    let retarget = ev
        .iter()
        .position(|e| *e == E::ReloadInFrozenWindow(SHIM.into()))
        .unwrap();
    assert!(
        ev.iter().skip(retarget).any(|e| *e == E::ReserveMetaSnap),
        "a second top-up diff ran after the retarget: {ev:?}"
    );
    assert!(stats.copied_bytes >= 2 * CHUNK);
}

/// The flip persist failing *while* the watchdog has fired is the one
/// case where rolling back destroys data: the thaw let guest writes onto
/// B, and restoring A's tables hands them to the armed sweep. The
/// executor must leave the tables on B and stall instead.
#[test]
fn a_failed_flip_after_a_forced_thaw_keeps_the_tables_on_b() {
    let r = rig_with_budget(25);
    seed(&r);

    let mut hook = || {
        // Outlast the freeze budget, then fail the persist.
        std::thread::sleep(Duration::from_millis(120));
        Err(StackError::Fault("injected flip persist failure"))
    };
    let mut retargeted = false;
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut retargeted,
    };
    let err = r.stack.swing("s", &mut hooks).unwrap_err();
    assert!(err.to_string().contains("flip persist"), "{err}");
    assert!(retargeted, "the caller must not fall back to the port");

    // No rollback: the shim still points at B, so the writes the thaw
    // let through are still reachable for recovery.
    let b_mm = mm(&r.ops, &format!("{ORIGIN}-b"));
    assert_eq!(
        r.ops.table(SHIM).unwrap()[0].3,
        format!("{b_mm} 0"),
        "tables stay on B after a fired-watchdog persist failure"
    );
}

/// The swing's in-window retargets must SUPPRESS dm's implicit
/// `lock_fs`, and nothing else was pinning that.
///
/// The bug this guards against is invisible to a mock — a leaked
/// `bd_fsfreeze_count` is a kernel effect — so the temptation is to
/// conclude that no unit test can cover it. That is true of the EFFECT
/// and false of the CALL, and the call is what a refactor changes:
/// reverting the four sites to the plain `reload_device` reintroduces an
/// unmountable volume after every major compaction, and the whole suite
/// stayed green while it did.
///
/// Asserted as "no PLAIN reload of the shim during the swing" rather
/// than by counting the frozen-window ones, because one site falling
/// back is exactly the partial regression a count would still accept.
#[test]
fn the_swings_retargets_suppress_dms_implicit_lockfs() {
    let r = rig();
    seed(&r);
    let mut hook = || Ok(());
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    r.stack.swing("s", &mut hooks).expect("swing");

    let ev = r.ops.events();
    assert!(
        ev.contains(&E::ReloadInFrozenWindow(SHIM.into())),
        "the shim retarget must run with lockfs suppressed: {ev:?}"
    );
    assert!(
        !ev.contains(&E::Reload(SHIM.into())),
        "a plain reload of the shim inside the frozen window takes a SECOND, \
         kernel-held freeze on an already-frozen filesystem — the leak that \
         left the block device unmountable: {ev:?}"
    );
    assert!(
        !ev.contains(&E::Reload(PORT.into())),
        "the port follows the shim through the same window: {ev:?}"
    );
}

/// A sweep that cannot finish its retarget must not leave the guest's
/// filesystem frozen.
///
/// This is the hazard the sweep's own freeze introduced. Before it, no
/// `FIFREEZE` was taken on this arm at all, and `reload_device`'s
/// resume-on-failure released dm's lockfs — so a failed sweep left the
/// volume mountable and writable, merely unrepaired. Owning the window
/// means owning the obligation to close it on every path out.
///
/// It matters because a retarget that fails is usually a PERSISTENT
/// condition, not a transient one: the next attempt re-enters, takes the
/// `AlreadyFrozen` arm, fails in the same place, and leaves it frozen
/// again. A repair that cannot complete would stop the workload instead
/// of merely failing to fix it — which is strictly worse than not
/// running at all.
#[test]
fn a_failed_sweep_retarget_leaves_the_guest_filesystem_thawed() {
    let r = rig();
    seed(&r);
    // A crash BEFORE the frozen window: `origin_b` exists on a LIVE
    // filesystem, so the sweep takes the freeze itself and the window is
    // unambiguously its own to close.
    r.ops
        .create_device(
            "origin-b",
            "SKYNBD-vlin-slot-origin",
            &[(
                0,
                VOL / 512,
                "linear".into(),
                format!("{} 0", mm(&r.ops, ORIGIN)),
            )],
        )
        .unwrap();
    assert!(!r.ops.frozen(), "the crash landed before the window");

    // Fail the port's retarget the way a real one fails — the shim's
    // has already landed by then, so this is the mid-window shape rather
    // than a refusal on entry.
    r.ops.remove_device(PORT).unwrap();

    let err = r.stack.sweep_slot(false).expect_err("the retarget fails");
    assert!(
        format!("{err}").contains("no such device"),
        "the retarget's failure is the CAUSE and must be the error reported, \
         not whatever the thaw on the way out returned: {err}"
    );
    assert!(
        !r.ops.frozen(),
        "the sweep closed its own window on the failure path"
    );
}

/// A capture whose candidate activation is already present must
/// supersede it, not fail forever against its own leftovers.
///
/// This is the §8.2 shape, found by S15j on a kernel. The publish is
/// the call a partition hangs, so a cut cycle dies with its candidate
/// already activated; the candidate's dm name is a pure function of the
/// target, so the NEXT cycle's create answers EBUSY — and keeps
/// answering it, identically, for as long as the volume lives. The one
/// cycle the outage runbook is waiting on is the one that can never
/// run. On the kernel it looked like this, every 5 s:
///
/// ```text
/// cycle failed error=... create sky-vlin15j-c-...-snap-g2: EBUSY
/// ```
///
/// with the thin dev id climbing (48, 49, 50, …) and the name fixed —
/// so each attempt also leaked a snapshot into a pool that §8 has under
/// pressure by construction. Asserted here as "capture twice against
/// the same target succeeds", because that is the property; the EBUSY
/// was only how it announced itself.
#[test]
fn a_capture_supersedes_a_candidate_left_by_a_failed_cycle() {
    let r = rig();
    seed(&r);

    let first = r.stack.capture("snap/g2").expect("first capture");
    // `dm_name` is `sky-{volume}-c-{logical}`, logical verbatim — the
    // pure-function-of-the-target property that makes the collision
    // permanent rather than incidental.
    let name = "sky-vlin-c-snap/g2".to_string();
    assert!(
        r.ops.device_names().contains(&name),
        "the candidate activation exists: {:?}",
        r.ops.device_names()
    );

    // The cycle dies here in the real thing — publish is what hangs,
    // and nothing tears the candidate down on the way out.
    let second = r
        .stack
        .capture("snap/g2")
        .expect("a second capture for the same target must not collide");
    assert_ne!(
        first.thin_dev_id, second.thin_dev_id,
        "the second capture is a genuinely new snapshot"
    );

    // Same name, now resolving to the new snapshot rather than EBUSY.
    assert_eq!(
        r.ops
            .device_names()
            .iter()
            .filter(|n| n.as_str() == name)
            .count(),
        1,
        "exactly one candidate activation for the target"
    );
    assert!(
        !r.ops.thin_ids().contains(&first.thin_dev_id),
        "the superseded candidate's thin id is deleted, or the livelock \
         merely slows down instead of stopping: {:?}",
        r.ops.thin_ids()
    );
}

/// §8.2 step 4's "zero origin reads" as the CONSTRUCTION it claims to
/// be, rather than as a count.
///
/// The runbook does not say the publish happens not to read the origin;
/// it says it cannot, and gives the reason — the delta is the locally
/// mapped set. That reason lives in one line: `activate_ro` builds the
/// candidate through `create_ro_thin`, whose table is `thin <pool>
/// <dev_id>` with no external-origin argument at all. An unmapped read
/// of the candidate therefore returns zeros and has nowhere to fall
/// through to.
///
/// Tested here, and not only in S15j, because the e2e can count what
/// reaches the origin device but cannot attribute it to the publish
/// with certainty: the same device is read by udev's blkid re-probe
/// after each dm table change, asynchronously, landing in a different
/// phase every run, and the harness's phase fences come from log
/// polling at 50 ms while the agent's publish → repoint → reclaim →
/// remount sequence completes in about 10. That count is worth having
/// and it cannot carry this claim. This can, and it is sharper: it
/// fails on the single change — giving the candidate an external
/// origin — that would make step 4 false.
#[test]
fn a_captured_candidate_cannot_read_through_to_the_origin() {
    let r = rig();
    seed(&r);

    // Chunk 4 is never written by `seed`, so it is unmapped in the thin
    // and the EXTERNAL ORIGIN is what serves it — with a pattern that
    // is unmistakable if it ever shows up where it should not.
    let off = 4 * CHUNK;
    let origin = r.ops.origin_image(ORIGIN);
    assert_eq!(
        &origin[off as usize..(off + CHUNK) as usize],
        &pat(VOL as usize, 0x10)[off as usize..(off + CHUNK) as usize],
        "precondition: this range is unmapped, so the origin serves it \
         from its external origin"
    );

    r.stack.capture("snap/g2").expect("capture");
    let got = r
        .stack
        .read("snap/g2", off, CHUNK)
        .expect("read the candidate");
    assert_eq!(
        got,
        vec![0u8; CHUNK as usize],
        "an unmapped read of the candidate must be ZEROS: the candidate \
         carries no external origin, which is what makes §8.2 step 4's \
         'zero origin reads by construction' true by construction rather \
         than by luck"
    );
}

/// Task 15e: the swing on a PROVISIONED stack (agentcycle's slot
/// wiring). A provision-built pool carves its loop into linears named
/// `sky-{v}-tmeta` / `sky-{v}-data` — not the config-rig `-tm`/`-da`
/// convention — and `SlotConfig::{tmeta_name,data_name}` is what makes
/// the swing's A-side teardown remove THOSE devices and its
/// canonicalize rename the B carves back onto THOSE names. Without the
/// override the teardown would skip the real carves (leaking them over
/// a released loop) and the rename would mint `-tm`/`-da` devices that
/// `provision::build`'s idempotent resume, `observe`, and teardown have
/// never heard of.
#[test]
fn swing_on_provision_named_carves_removes_and_reclaims_the_same_names() {
    const TMETA: &str = "sky-vlin-tmeta";
    const DATA: &str = "sky-vlin-data";
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
    let origin_mm = mm(&ops, ORIGIN);
    for (name, part) in [(SHIM, "shim"), (PORT, "rec")] {
        ops.create_device(
            name,
            &format!("SKYNBD-vlin-stack-{part}"),
            &[(0, VOL / 512, "linear".into(), format!("{origin_mm} 0"))],
        )
        .unwrap();
    }
    let live = dir.path().join("pool.img");
    let loop_a = ops.prepare_backing(&live, META + VOL, true).unwrap();
    // The provision-shaped A carves: `[tmeta][data]` linears over the
    // pool file's loop, under provision's names.
    ops.create_device(
        TMETA,
        "SKYNBD-vlin-stack-tmeta",
        &[(0, META / 512, "linear".into(), format!("{loop_a} 0"))],
    )
    .unwrap();
    ops.create_device(
        DATA,
        "SKYNBD-vlin-stack-data",
        &[(
            0,
            VOL / 512,
            "linear".into(),
            format!("{loop_a} {}", META / 512),
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
            discard_name: Some(PORT.into()),
            backing_file: Some(live.clone()),
            shim_name: Some(SHIM.into()),
            mount_name: None,
            slot: Some(SlotConfig {
                live_file: live.clone(),
                data_bytes: VOL,
                meta_bytes: META,
                shim_name: SHIM.into(),
                external_origin: Some(EXT.into()),
                tmeta_name: Some(TMETA.into()),
                data_name: Some(DATA.into()),
            }),
        },
        ops.clone(),
    )
    .unwrap();
    let r = Rig {
        ops,
        stack,
        live,
        _dir: dir,
    };
    let before = seed(&r);

    let mut hook = || Ok(());
    let mut hooks = SwingHooks {
        persist_flip: &mut hook,
        retargeted: &mut false,
    };
    r.stack.swing("s", &mut hooks).expect("swing");

    // The A carves were removed under their provision names, the B
    // carves renamed back onto them — the post-swing device set is
    // exactly a provisioned stack's.
    let ev = r.ops.events();
    let pos = |e: &E| {
        ev.iter()
            .position(|x| x == e)
            .unwrap_or_else(|| panic!("missing {e:?}"))
    };
    let remove_tmeta = pos(&E::Remove(TMETA.into()));
    let rename_tmeta = pos(&E::Rename(format!("{POOL}-btm"), TMETA.into()));
    let rename_data = pos(&E::Rename(format!("{POOL}-bda"), DATA.into()));
    assert!(
        remove_tmeta < rename_tmeta,
        "the canonical name is freed before the rename claims it"
    );
    assert!(pos(&E::Remove(DATA.into())) < rename_data);
    // Slot A's loop was actually released — the carve removal above is
    // what makes that possible (a linear still referencing it would be
    // the leak the name mismatch used to cause).
    assert!(
        ev.contains(&E::ReleaseBacking(loop_a.clone())),
        "slot A's loop released: {ev:?}"
    );
    let names = r.ops.device_names();
    assert!(names.contains(&TMETA.to_string()), "{names:?}");
    assert!(names.contains(&DATA.to_string()), "{names:?}");
    for n in &names {
        assert!(
            !n.ends_with("-tm") && !n.ends_with("-da") && !n.contains("-b"),
            "no config-convention or slot-B leftovers: {n}"
        );
    }
    // The renamed carves are the B side: their tables reference the B
    // loop, never the released A loop.
    let tmeta_table = r.ops.table(TMETA).expect("tmeta table");
    assert!(
        !tmeta_table[0].3.contains(&loop_a),
        "the canonical tmeta is the B carve: {tmeta_table:?}"
    );
    // And the swing stayed content-preserving through all of it.
    assert_eq!(r.ops.read_dev(SHIM, 0, VOL).unwrap(), before);
    let (_, prealloc, loop_dev) = r.ops.backing_info(&r.live).expect("live file");
    assert!(prealloc && loop_dev.is_some());
    assert!(r.ops.backing_info(&slot_b_path(&r.live)).is_none());
}
