//! Extent-map shape under realistic write patterns: how many extents a
//! workload leaves behind, and how many of those are describable as one
//! (same segment, contiguous in both LBA and segment offset).
//!
//! Extent count is the multiplier on leaf-node count, on the open walk
//! in `load_commit_state`, on a future flat extent-map object, and on
//! resident memory, so what each workload leaves behind is worth
//! tracking. `ios/extents` is how many IOs the fold collapsed into one
//! extent; the ceiling is one extent per *segment*, since an extent
//! cannot span the 8 MiB seal boundary.
//!
//! Measurement only: run with
//! `cargo test -p skynbd-engine --test extent_shape -- --ignored --nocapture`.

use std::sync::Arc;

use skynbd_engine::{commit_extents, Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::{Extent, VolumeId};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
/// `EngineConfig::default()`; restated so the derived numbers are readable.
const SHARD: u64 = 1 << 30;
const MAX_NODE_ENTRIES: u64 = 256;

/// splitmix64 — deterministic, no `rand` dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Aligned offset in `[0, span - len)`.
    fn offset(&mut self, span: u64, len: u64) -> u64 {
        (self.next() % ((span - len) / len)) * len
    }
}

/// One (lba, len) write.
type Io = (u64, u64);

fn seq(io: u64, total: u64) -> Vec<Io> {
    (0..total / io).map(|i| (i * io, io)).collect()
}

fn random(io: u64, count: u64, span: u64, seed: u64) -> Vec<Io> {
    let mut rng = Rng(seed);
    (0..count).map(|_| (rng.offset(span, io), io)).collect()
}

/// Sequential runs placed at random offsets — the shape an allocator or a
/// log-structured writer produces.
fn runs(io: u64, run: u64, count: u64, span: u64, seed: u64) -> Vec<Io> {
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    for _ in 0..count {
        let base = rng.offset(span, run);
        out.extend((0..run / io).map(|i| (base + i * io, io)));
    }
    out
}

/// No adjacent pair may still be mergeable: the fold is maximal, which
/// is what makes the tree a function of content rather than of write
/// history (equal content ⇒ equal nodes ⇒ sharing across snapshots).
fn assert_maximal(extents: &[Extent]) {
    for pair in extents.windows(2) {
        assert!(
            !pair[0].continues_into(&pair[1]),
            "unmerged adjacent extents: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
}

/// Leaf + internal node count for `n` extents spread over `shards`, and
/// the sequential GETs `load_commit_state` pays to open it.
fn walk_cost(extents: &[Extent], size: u64) -> (u64, u64) {
    let shards = size.div_ceil(SHARD);
    let mut per_shard = vec![0u64; shards as usize];
    for e in extents {
        per_shard[(e.lba_start / SHARD) as usize] += 1;
    }
    let mut nodes = 0;
    for count in per_shard {
        if count > 0 {
            nodes += 1 + count.div_ceil(MAX_NODE_ENTRIES);
        }
    }
    (nodes, nodes + 2)
}

async fn build(size: u64, ios: &[Io]) -> (Arc<MemoryStore>, VolumeId, Vec<Extent>) {
    let store = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(1u128));
    let mut e = Engine::create_volume(
        Arc::clone(&store) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        size,
        EngineConfig::default(),
    )
    .await
    .expect("create");

    let mut buf = Vec::new();
    let mut since = 0u64;
    for &(lba, len) in ios {
        if buf.len() < len as usize {
            buf.resize(len as usize, 0x5a);
        }
        e.write(lba, &buf[..len as usize]).expect("write");
        since += len;
        // Commit at the segment seal threshold, as a flushing fs would.
        if since >= 8 * MIB {
            e.commit(CommitKind::Flush).await.expect("commit");
            since = 0;
        }
    }
    let hash = e.commit(CommitKind::Checkpoint).await.expect("commit");
    let extents = commit_extents(&store, vol, &hash).await.expect("extents");
    (store, vol, extents)
}

async fn report(label: &str, size: u64, ios: Vec<Io>) {
    let written: u64 = ios.iter().map(|(_, l)| l).sum();
    let (_store, _vol, extents) = build(size, &ios).await;
    assert_maximal(&extents);
    let (nodes, gets) = walk_cost(&extents, size);
    // Interned + delta-encoded flat map: ~8 B/extent; the tree encoding
    // is ~37 B/extent.
    let emap_kib = (extents.len() as u64 * 8) / 1024;
    println!(
        "{label:<22} ios={:<7} written={:>4}MiB | extents={:<7} ios/extent={:>7.1} \
         | nodes={nodes:<6} open_gets={gets:<6} emap={emap_kib}KiB",
        ios.len(),
        written / MIB,
        extents.len(),
        ios.len() as f64 / extents.len().max(1) as f64,
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
async fn extent_shape_by_workload() {
    let size = 4 * GIB;
    println!(
        "volume={}GiB shard={}GiB segment_max=8MiB",
        size / GIB,
        SHARD / GIB
    );

    report("seq-64k", size, seq(64 << 10, 512 * MIB)).await;
    report("seq-4k", size, seq(4 << 10, 128 * MIB)).await;
    report("rand-4k", size, random(4 << 10, 32_768, size, 1)).await;
    report("rand-64k", size, random(64 << 10, 8_192, size, 2)).await;
    report("runs-1m-of-64k", size, runs(64 << 10, MIB, 512, size, 3)).await;
    report("runs-8m-of-64k", size, runs(64 << 10, 8 * MIB, 64, size, 4)).await;

    // Sequential fill, then a random overwrite pass over it.
    let mut churn = seq(64 << 10, 256 * MIB);
    churn.extend(random(4 << 10, 8_192, 256 * MIB, 5));
    report("seq-then-rand-churn", size, churn).await;
}
