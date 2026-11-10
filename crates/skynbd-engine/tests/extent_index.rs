//! What the committed extent map costs, in the shape it has today versus
//! the immutable per-shard index of `skynbd-multinode-design.md` D-M10.
//!
//! Four things are measured, all against the same extent lists:
//!
//! - **resident bytes per extent** — `BTreeMap<u64, (u64, Placed)>` against
//!   sorted columnar shards, via a counting global allocator;
//! - **build time** — what `load_commit_state` pays after fetching nodes;
//! - **point-query time** — the read path's map lookup;
//! - **commit-path cost** — today's `committed.clone()` + one `insert()`
//!   per overlay entry, against merging the overlay into the shards it
//!   touches and sharing the rest.
//!
//! The packing claim is checked rather than assumed: within a 1 GiB shard
//! the start, the length, the interned target index and the offset within
//! an 8 MiB segment must all fit in `u32`, and the harness reports the
//! maxima it actually saw.
//!
//! Measurement only: run with
//! `cargo test -p skynbd-engine --test extent_index -- --ignored --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use skynbd_tree::ExtentMap;
use skynbd_types::{DataRef, Extent, SegmentId, Target};

// ---------------------------------------------------------------- alloc

static LIVE: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size() as u64, Ordering::Relaxed);
        TOTAL.fetch_add(l.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as u64, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        LIVE.fetch_add(new as u64, Ordering::Relaxed);
        LIVE.fetch_sub(l.size() as u64, Ordering::Relaxed);
        TOTAL.fetch_add(new.saturating_sub(l.size()) as u64, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn live() -> u64 {
    LIVE.load(Ordering::Relaxed)
}

/// Live bytes the closure's result holds, and total bytes it churned.
fn measure<T>(f: impl FnOnce() -> T) -> (T, u64, u64) {
    let (l0, t0) = (live(), TOTAL.load(Ordering::Relaxed));
    let v = f();
    (
        v,
        live().saturating_sub(l0),
        TOTAL.load(Ordering::Relaxed) - t0,
    )
}

// ------------------------------------------------------ today's shape

/// `skynbd_engine::Placed`, which is private — same fields, same size.
/// The fields are never read here; the point is what they cost to store.
#[derive(Clone)]
#[allow(dead_code)]
struct Placed {
    lba0: u64,
    target: Target,
}

fn build_btree(extents: &[Extent]) -> ExtentMap<Placed> {
    let mut m = ExtentMap::new();
    for e in extents {
        m.insert(
            e.lba_start,
            e.len,
            Placed {
                lba0: e.lba_start,
                target: e.target.clone(),
            },
        );
    }
    m
}

// ----------------------------------------------------- the D-M10 shape

const SHARD: u64 = 1 << 30;

/// One shard's extents: sorted, immutable, columnar. Targets are interned,
/// so a segment backing 128 extents is stored once.
#[derive(Default)]
struct ShardIndex {
    /// Offset of each extent within the shard.
    start: Vec<u32>,
    len: Vec<u32>,
    /// Index into the *volume-level* target table; `u32::MAX` is
    /// `Target::Zero`. Volume-level rather than per-shard: a segment's
    /// 128 extents scatter across shards, so a per-shard table would
    /// hold a near-complete copy of the segment list in every shard.
    target: Vec<u32>,
    /// Offset within the backing object.
    offset: Vec<u32>,
}

const ZERO: u32 = u32::MAX;

impl ShardIndex {
    fn push(&mut self, e: &Extent, shard_base: u64, targets: &mut Interner) {
        self.start.push((e.lba_start - shard_base) as u32);
        self.len.push(e.len as u32);
        match &e.target {
            Target::Zero => {
                self.target.push(ZERO);
                self.offset.push(0);
            }
            Target::Data(d) => {
                let (key, off) = match d {
                    DataRef::Seg { segment, offset } => (format!("s{segment}"), *offset),
                    DataRef::Cas { hash, offset } => (format!("c{hash:?}"), *offset),
                };
                self.target.push(targets.intern(key, d));
                self.offset.push(off as u32);
            }
        }
    }

    /// The extent covering `at` (offset within the shard), if any.
    fn lookup(&self, at: u32) -> Option<usize> {
        let i = self.start.partition_point(|&s| s <= at);
        if i == 0 {
            return None;
        }
        let i = i - 1;
        (at - self.start[i] < self.len[i]).then_some(i)
    }

    fn bytes(&self) -> usize {
        (self.start.capacity()
            + self.len.capacity()
            + self.target.capacity()
            + self.offset.capacity())
            * std::mem::size_of::<u32>()
    }
}

/// Distinct backing objects, shared by every shard of a commit.
#[derive(Default)]
struct Interner {
    by_key: HashMap<String, u32>,
    refs: Vec<DataRef>,
}

impl Interner {
    fn intern(&mut self, key: String, d: &DataRef) -> u32 {
        let next = self.refs.len() as u32;
        let idx = *self.by_key.entry(key).or_insert(next);
        if idx == next {
            self.refs.push(d.clone());
        }
        idx
    }
}

fn build_columnar(extents: &[Extent], vol_size: u64) -> (Vec<ShardIndex>, Interner) {
    let mut shards: Vec<ShardIndex> = (0..vol_size.div_ceil(SHARD))
        .map(|_| ShardIndex::default())
        .collect();
    let mut targets = Interner::default();
    for e in extents {
        let s = (e.lba_start / SHARD) as usize;
        shards[s].push(e, s as u64 * SHARD, &mut targets);
    }
    // The lookup table the shards index into stays; the string keys that
    // built it do not.
    targets.by_key = HashMap::new();
    (shards, targets)
}

/// Largest values seen, to check the `u32` packing empirically.
fn packing_maxima(extents: &[Extent]) -> (u64, u64, u64) {
    let mut max_off_in_shard = 0;
    let mut max_len = 0;
    let mut max_target_off = 0;
    for e in extents {
        max_off_in_shard = max_off_in_shard.max(e.lba_start % SHARD);
        max_len = max_len.max(e.len);
        if let Target::Data(DataRef::Seg { offset, .. } | DataRef::Cas { offset, .. }) = &e.target {
            max_target_off = max_target_off.max(*offset);
        }
    }
    (max_off_in_shard, max_len, max_target_off)
}

// ------------------------------------------------------------ workloads

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Turn a write stream into the extents a commit would leave: segment and
/// offset assigned in arrival order (8 MiB seal), sorted by LBA,
/// last-writer-wins on exact repeats, then coalesced exactly as
/// `commit()` does.
fn extents_for(ios: &[(u64, u64)]) -> Vec<Extent> {
    let mut placed: HashMap<u64, (u64, SegmentId, u64)> = HashMap::new();
    let (mut seg, mut seg_off) = (SegmentId(ulid::Ulid::from(1u128)), 0u64);
    let mut seg_no = 1u128;
    for &(lba, len) in ios {
        if seg_off + len > (8 << 20) {
            seg_no += 1;
            seg = SegmentId(ulid::Ulid::from(seg_no));
            seg_off = 0;
        }
        placed.insert(lba, (len, seg, seg_off));
        seg_off += len;
    }
    let mut keys: Vec<u64> = placed.keys().copied().collect();
    keys.sort_unstable();
    let mut out: Vec<Extent> = Vec::with_capacity(keys.len());
    for k in keys {
        let (len, segment, offset) = placed[&k];
        let e = Extent::new(k, len, Target::Data(DataRef::Seg { segment, offset })).unwrap();
        match out.last_mut() {
            Some(prev) if prev.continues_into(&e) => prev.absorb(&e),
            _ => out.push(e),
        }
    }
    out
}

fn seq(io: u64, total: u64) -> Vec<(u64, u64)> {
    (0..total / io).map(|i| (i * io, io)).collect()
}

fn random(io: u64, count: u64, span: u64, seed: u64) -> Vec<(u64, u64)> {
    let mut rng = Rng(seed);
    (0..count)
        .map(|_| ((rng.next() % (span / io)) * io, io))
        .collect()
}

fn runs(io: u64, run: u64, count: u64, span: u64, seed: u64) -> Vec<(u64, u64)> {
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    for _ in 0..count {
        let base = (rng.next() % (span / run)) * run;
        out.extend((0..run / io).map(|i| (base + i * io, io)));
    }
    out
}

// ------------------------------------------------------------- report

fn report(label: &str, vol_size: u64, ios: Vec<(u64, u64)>) {
    let extents = extents_for(&ios);
    let n = extents.len().max(1);

    let t = Instant::now();
    let (btree, btree_live, _) = measure(|| build_btree(&extents));
    let btree_build = t.elapsed();

    let t = Instant::now();
    let ((columns, targets), col_live, _) = measure(|| build_columnar(&extents, vol_size));
    let col_build = t.elapsed();
    let col_cols: usize = columns.iter().map(|s| s.bytes()).sum();

    // Point queries at random covered offsets — the read path's lookup.
    let probes: Vec<u64> = {
        let mut rng = Rng(99);
        (0..20_000)
            .map(|_| extents[(rng.next() % n as u64) as usize].lba_start)
            .collect()
    };
    let t = Instant::now();
    for &p in &probes {
        std::hint::black_box(btree.query(p, 512));
    }
    let btree_q = t.elapsed();
    let t = Instant::now();
    for &p in &probes {
        let s = (p / SHARD) as usize;
        std::hint::black_box(columns[s].lookup((p % SHARD) as u32));
    }
    let col_q = t.elapsed();

    // Commit path: 8 MiB of fresh 64 KiB writes landing in one shard.
    let overlay: Vec<Extent> = extents_for(&seq(64 << 10, 8 << 20));
    let t = Instant::now();
    let (_folded, _, btree_churn) = measure(|| {
        let mut folded = btree.clone();
        for e in &overlay {
            folded.insert(
                e.lba_start,
                e.len,
                Placed {
                    lba0: e.lba_start,
                    target: e.target.clone(),
                },
            );
        }
        folded
    });
    let btree_commit = t.elapsed();
    // Per-shard: rebuild only the touched shard, Arc-share the rest.
    let touched: Vec<Extent> = extents
        .iter()
        .filter(|e| e.lba_start < SHARD)
        .cloned()
        .collect();
    let t = Instant::now();
    let (_new_shard, _, col_churn) = measure(|| {
        let mut merged = Vec::with_capacity(touched.len() + overlay.len());
        merged.extend(touched.iter().cloned());
        merged.extend(overlay.iter().cloned());
        merged.sort_by_key(|e| e.lba_start);
        build_columnar(&merged, SHARD)
    });
    let col_commit = t.elapsed();

    let (max_off, max_len, max_toff) = packing_maxima(&extents);
    let fits =
        max_off <= u32::MAX as u64 && max_len <= u32::MAX as u64 && max_toff <= u32::MAX as u64;

    println!(
        "\n{label}  extents={n}\n  \
         live      btree={:>9} ({:>5.1} B/ext)   columnar={:>9} ({:>5.1} B/ext)   {:.1}x smaller\n  \
         build     btree={:>9?}                  columnar={:>9?}\n  \
         query20k  btree={:>9?}                  columnar={:>9?}\n  \
         commit    btree={:>7} KiB churned in {:?}   per-shard={:>7} KiB in {:?}\n  \
         columns   {:>7} KiB in the four u32 columns + {} interned targets\n  \
         packing   max_shard_off={max_off} max_len={max_len} max_target_off={max_toff} fits_u32={fits}",
        btree_live,
        btree_live as f64 / n as f64,
        col_live,
        col_live as f64 / n as f64,
        btree_live as f64 / col_live.max(1) as f64,
        btree_build,
        col_build,
        btree_q,
        col_q,
        btree_churn / 1024,
        btree_commit,
        col_churn / 1024,
        col_commit,
        col_cols / 1024,
        targets.refs.len(),
    );
    drop(btree);
    drop(columns);
    drop(targets);
}

#[test]
#[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
fn extent_index_cost() {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    println!(
        "shard={}GiB segment_max=8MiB; BTreeMap live bytes include node overhead",
        SHARD / GIB
    );

    report("seq-64k / 4GiB vol", 4 * GIB, seq(64 << 10, 512 * MIB));
    report(
        "runs-1m-of-64k / 4GiB",
        4 * GIB,
        runs(64 << 10, MIB, 512, 4 * GIB, 3),
    );
    report(
        "rand-4k / 4GiB vol",
        4 * GIB,
        random(4 << 10, 32_768, 4 * GIB, 1),
    );
    // The reference volume from the design doc: 100 GiB, 50 GiB live,
    // 64 KiB average extent, scattered — the fragmented case that sets
    // the memory ceiling.
    report(
        "rand-64k / 100GiB vol (reference)",
        100 * GIB,
        random(64 << 10, 819_200, 100 * GIB, 7),
    );
}

// ------------------------------------------------------- the real thing

/// The prototype above says what the representation *should* cost. This
/// measures what the engine actually holds and churns once it is wired in,
/// so the claim is about shipped code rather than a sketch.
#[tokio::test(flavor = "current_thread")]
#[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
async fn engine_open_and_commit_cost() {
    use skynbd_engine::{Engine, EngineConfig};
    use skynbd_format::CommitKind;
    use skynbd_refs::{MemoryRefStore, RefStore};
    use skynbd_store::{MemoryStore, ObjectStore};
    use skynbd_types::VolumeId;

    const GIB: u64 = 1 << 30;
    let size = 4 * GIB;
    let store = std::sync::Arc::new(MemoryStore::new());
    let refs: std::sync::Arc<dyn RefStore> = std::sync::Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(11u128));

    let mut e = Engine::create_volume(
        std::sync::Arc::clone(&store) as std::sync::Arc<dyn ObjectStore>,
        std::sync::Arc::clone(&refs),
        vol,
        size,
        EngineConfig::default(),
    )
    .await
    .expect("create");
    let buf = [0x5au8; 4096];
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut since = 0u64;
    for _ in 0..32_768 {
        e.write((rng.next() % (size / 4096)) * 4096, &buf)
            .expect("write");
        since += 4096;
        if since >= (8 << 20) {
            e.commit(CommitKind::Flush).await.expect("commit");
            since = 0;
        }
    }
    e.commit(CommitKind::Checkpoint).await.expect("commit");
    drop(e);

    // Resident: everything an opened engine holds, which at this size is
    // dominated by the committed index.
    let (mut opened, open_live, _) = {
        let (l0, t0) = (live(), TOTAL.load(Ordering::Relaxed));
        let v = Engine::open(
            std::sync::Arc::clone(&store) as std::sync::Arc<dyn ObjectStore>,
            std::sync::Arc::clone(&refs),
            vol,
            EngineConfig::default(),
        )
        .await
        .expect("open");
        (
            v,
            live().saturating_sub(l0),
            TOTAL.load(Ordering::Relaxed) - t0,
        )
    };

    // Churn of one flush: 8 MiB of fresh writes, one shard touched.
    for i in 0..128u64 {
        opened
            .write(i * (64 << 10), &[0x11u8; 64 << 10])
            .expect("write");
    }
    let (l0, t0) = (live(), TOTAL.load(Ordering::Relaxed));
    let t = Instant::now();
    opened.commit(CommitKind::Flush).await.expect("commit");
    let commit_time = t.elapsed();
    let commit_churn = TOTAL.load(Ordering::Relaxed) - t0;
    let commit_live = live() as i64 - l0 as i64;

    println!(
        "\nengine (4GiB volume, 32k random 4 KiB writes)\n  \
         open      holds {open_live} B live\n  \
         commit    churned {} KiB, live delta {commit_live} B, in {commit_time:?}",
        commit_churn / 1024,
    );
}
