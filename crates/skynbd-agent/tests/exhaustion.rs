//! Task 15j part A: the exhaustion watermark's two surfaces (design
//! doc §8.1 rule 2), measured against the contract-enforcing mock
//! kernel.
//!
//! Claims under test:
//! - a fully preallocated backing does NOT bind the filesystem surface,
//!   however little the filesystem has left — its space is reserved, so
//!   counting it would advance on a shortage that cannot reach the pool;
//! - a sparse backing DOES bind it, and `effective_free` becomes the
//!   filesystem's number — this is the case that surfaces as loop EIO
//!   under a pool that still believes it has free blocks;
//! - when the pool is the tighter surface it binds, sparse or not;
//! - the surface is keyed on `backing_file`, not on the slot, so it is
//!   live on today's slot-less CSI stacks — with the slot's live file as
//!   the fallback for stacks that carry one.

use std::path::{Path, PathBuf};

use skynbd_agent::dm::{DmConfig, DmStack, SlotConfig};
use skynbd_agent::dm_mock::MockDmOps;
use skynbd_agent::DmOps;

const CHUNK: u64 = 4096;
/// 16 pool blocks — the whole pool is 64 KiB, so a filesystem measured
/// in MiB is unambiguously the looser surface and vice versa.
const VOL: u64 = 16 * CHUNK;
const POOL: &str = "skypool";
const ORIGIN: &str = "origin";
const ORIGIN_ID: u64 = 1;

struct Rig {
    ops: MockDmOps,
    stack: DmStack<MockDmOps>,
    backing: PathBuf,
    _dir: tempfile::TempDir,
}

/// A stack whose pool data lives in `pool.img`, prepared `prealloc`.
/// `slot_file` is the slot's live file when the stack carries a slot at
/// all — the fallback path.
fn rig(prealloc: bool, backing: bool, slot_file: Option<&str>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
    let file = dir.path().join("pool.img");
    ops.prepare_backing(&file, VOL, prealloc).unwrap();
    let slot = slot_file.map(|name| {
        let p = dir.path().join(name);
        ops.prepare_backing(&p, VOL, false).unwrap();
        SlotConfig {
            live_file: p,
            data_bytes: VOL,
            meta_bytes: 4 * CHUNK,
            shim_name: "shim".into(),
            external_origin: None,
            tmeta_name: None,
            data_name: None,
        }
    });
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
            backing_file: backing.then(|| file.clone()),
            shim_name: None,
            mount_name: None,
            slot,
        },
        ops.clone(),
    )
    .unwrap();
    Rig {
        ops,
        stack,
        backing: file,
        _dir: dir,
    }
}

fn pat(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

/// Provision `blocks` pool blocks so occupancy is a known number.
fn fill(r: &Rig, blocks: u64) {
    for c in 0..blocks {
        r.ops
            .write_origin(ORIGIN, c * CHUNK, &pat(CHUNK as usize, c as u8));
    }
}

#[test]
fn a_preallocated_backing_does_not_bind_the_filesystem_surface() {
    let r = rig(true, true, None);
    fill(&r, 4);
    // The filesystem is nearly empty — and irrelevant: every byte the
    // pool can still provision is already committed to the file.
    r.ops.set_fs_free(0);
    let p = r.stack.pool_pressure().expect("pressure");
    assert_eq!(p.used_bytes, 4 * CHUNK);
    assert_eq!(p.total_bytes, VOL);
    assert!((p.occupancy() - 0.25).abs() < 1e-9);
    assert!(!p.fs_bound, "a reserved backing cannot lose the race");
    assert_eq!(p.effective_free, VOL - 4 * CHUNK);
}

#[test]
fn a_sparse_backing_binds_the_filesystem_surface() {
    let r = rig(false, true, None);
    fill(&r, 4);
    // 8 KiB left under a pool that thinks it has 48 KiB: the shortage
    // reaches the pool as loop EIO, below the whole queue/timeout ladder.
    r.ops.set_fs_free(2 * CHUNK);
    let p = r.stack.pool_pressure().expect("pressure");
    assert!(p.fs_bound, "a sparse backing is the exposed case");
    assert_eq!(p.effective_free, 2 * CHUNK);
    // Occupancy is untouched by the second surface — the two terms of
    // the watermark stay independent.
    assert!((p.occupancy() - 0.25).abs() < 1e-9);

    // Partially allocated is still sparse: allocation has to COVER the
    // logical length before the surface stops binding.
    r.ops.set_backing_allocated(&r.backing, VOL - CHUNK);
    assert!(r.stack.pool_pressure().unwrap().fs_bound);
    r.ops.set_backing_allocated(&r.backing, VOL);
    assert!(!r.stack.pool_pressure().unwrap().fs_bound);
}

#[test]
fn the_pool_binds_when_it_is_the_tighter_surface() {
    let r = rig(false, true, None);
    fill(&r, 15); // one block left: 4 KiB
    r.ops.set_fs_free(64 << 20);
    let p = r.stack.pool_pressure().expect("pressure");
    assert!(!p.fs_bound, "the filesystem is the looser surface here");
    assert_eq!(p.effective_free, CHUNK);
}

/// §8.1 rule 2 as built: the surface is carried by the BACKING FILE, so
/// it works on today's Shape-B stacks (`slot: None`) — and a
/// slot-configured stack that predates the config still answers through
/// its live file.
#[test]
fn the_backing_file_carries_the_surface_and_the_slot_is_only_a_fallback() {
    // Slot-less, backing configured: bound. This is every CSI volume.
    let r = rig(false, true, None);
    r.ops.set_fs_free(CHUNK);
    assert!(r.stack.pool_pressure().unwrap().fs_bound);

    // Slot-configured, backing unset: the slot's live file answers.
    let r = rig(false, false, Some("slot.img"));
    r.ops.set_fs_free(CHUNK);
    assert!(r.stack.pool_pressure().unwrap().fs_bound);

    // Neither: nothing file-bounded, so pressure is pool occupancy alone
    // no matter how little the filesystem has.
    let r = rig(false, false, None);
    r.ops.set_fs_free(0);
    let p = r.stack.pool_pressure().unwrap();
    assert!(!p.fs_bound);
    assert_eq!(p.effective_free, VOL);
}

/// A relative `backing_file` still has a filesystem under it.
/// `Path::parent` of a bare filename is `Some("")`, and `statvfs("")` is
/// ENOENT — so a stack configured with one used to lose the second
/// surface silently, which is the exact shape §8.1 rule 2 exists to
/// prevent (a surface that is off announces nothing).
#[test]
fn a_relative_backing_path_still_resolves_its_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
    let file = PathBuf::from("pool.img"); // no parent component at all
    ops.prepare_backing(&file, VOL, false).unwrap();
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
            backing_file: Some(file),
            shim_name: None,
            mount_name: None,
            slot: None,
        },
        ops.clone(),
    )
    .unwrap();
    ops.set_fs_free(CHUNK);
    let p = stack.pool_pressure().expect("pressure");
    assert!(
        p.fs_bound,
        "the surface must bind: a relative path resolves against the cwd, \
         not against the empty path"
    );
    assert_eq!(p.effective_free, CHUNK);
}

/// A backing the stack cannot stat degrades to the pool surface — never
/// to "unknown", which would take the occupancy watermark down with it.
#[test]
fn an_unmeasurable_backing_leaves_the_occupancy_watermark_standing() {
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, ORIGIN_ID, VOL, CHUNK);
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
            // Never registered with the mock: `backing_alloc` errors.
            backing_file: Some(Path::new("/nonexistent/pool.img").to_path_buf()),
            shim_name: None,
            mount_name: None,
            slot: None,
        },
        ops.clone(),
    )
    .unwrap();
    for c in 0..8 {
        ops.write_origin(ORIGIN, c * CHUNK, &pat(CHUNK as usize, c as u8));
    }
    let p = stack.pool_pressure().expect("occupancy still answers");
    assert!((p.occupancy() - 0.5).abs() < 1e-9);
    assert!(!p.fs_bound);
    assert_eq!(p.effective_free, VOL / 2);
}
