//! Issue #97 item 1: the pipelined backfill. A batch's objects upload
//! concurrently while its attaches stay in op order, so the published
//! image is bit-exact at any depth, round trips shrink by the depth, and
//! the concurrency is real — asserted as an observed in-flight high-water
//! mark, never inferred from exit status (a regression to serial uploads
//! would otherwise pass forever).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use skynbd_agent::{
    ControlPlane, CpError, CycleEngine, CycleProgress, FakeStack, InprocControlPlane, RangeOp,
    StateStore,
};
use skynbd_refs::MemoryRefStore;
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const CHUNKS: u64 = 256;
const VOL: u64 = CHUNKS * CHUNK;

type Inner = InprocControlPlane<MemoryStore, MemoryRefStore>;

#[derive(Default)]
struct Counters {
    ensure: AtomicU64,
    put: AtomicU64,
    attach: AtomicU64,
    clear: AtomicU64,
    /// `apply` calls, including refused ones.
    apply: AtomicU64,
    /// attach/clear/apply calls in flight now, and at most at once.
    attaching: AtomicU64,
    attach_high_water: AtomicU64,
    /// Set when an `ensure` or `put_object` starts while an attach-family
    /// call is in flight: consecutive batches really overlapped.
    overlapped: std::sync::atomic::AtomicBool,
    in_flight: AtomicU64,
    put_high_water: AtomicU64,
    /// `put_object` calls that reached the store (not cancelled first).
    put_done: AtomicU64,
    /// attach/clear/apply calls that staged on the server.
    attached_ok: AtomicU64,
    /// Highest op id an attach-family call has staged (returned Ok).
    attached_max: AtomicU64,
    /// Progress reports, in call order: (phase, ops_done, attached_max
    /// when the report was sent).
    reports: Mutex<Vec<(&'static str, u64, u64)>>,
    /// Progress reports in flight now.
    reporting: AtomicU64,
    /// Set when an attach-family call starts while a report is in flight.
    attach_while_reporting: std::sync::atomic::AtomicBool,
    /// Op ids of every applied attach/clear/apply call, in call order.
    op_ids: Mutex<Vec<u64>>,
}

/// Delegating plane that counts calls, tracks concurrent `put_object`s,
/// and optionally charges a fixed delay per call (a simulated RTT).
#[derive(Clone)]
struct Probe {
    inner: Inner,
    rtt: Duration,
    put_delay: Duration,
    /// Extra delay on every attach-family call, before it reaches the
    /// server.
    attach_delay: Duration,
    /// Delay on every progress report (a slow status endpoint).
    report_delay: Duration,
    /// Whether `put_delay` and `attach_delay` apply (default true; a test
    /// turns them off to scan or recover quickly).
    slow: Arc<std::sync::atomic::AtomicBool>,
    /// From this (1-based) `ensure` call on, answer with one result too
    /// few (a skewed or broken peer). 0: never.
    short_ensure_from: Arc<AtomicU64>,
    /// Answer `apply` as a server that predates it.
    no_apply: Arc<std::sync::atomic::AtomicBool>,
    c: Arc<Counters>,
}

impl Probe {
    async fn rtt(&self) {
        if !self.rtt.is_zero() {
            tokio::time::sleep(self.rtt).await;
        }
    }

    /// Run an attach-family call, tracking how many are in flight.
    async fn attaching(
        &self,
        f: impl std::future::Future<Output = Result<(), CpError>>,
    ) -> Result<(), CpError> {
        let now = self.c.attaching.fetch_add(1, Ordering::SeqCst) + 1;
        self.c.attach_high_water.fetch_max(now, Ordering::SeqCst);
        if self.c.reporting.load(Ordering::SeqCst) > 0 {
            self.c.attach_while_reporting.store(true, Ordering::SeqCst);
        }
        if !self.attach_delay.is_zero() && self.slow.load(Ordering::SeqCst) {
            tokio::time::sleep(self.attach_delay).await;
        }
        let out = f.await;
        if out.is_ok() {
            self.c.attached_ok.fetch_add(1, Ordering::SeqCst);
        }
        self.c.attaching.fetch_sub(1, Ordering::SeqCst);
        out
    }

    /// Record whether a staging call began under an in-flight attach.
    fn staging(&self) {
        if self.c.attaching.load(Ordering::SeqCst) > 0 {
            self.c.overlapped.store(true, Ordering::SeqCst);
        }
    }
}

impl ControlPlane for Probe {
    async fn prepare(&self, b: Option<&str>) -> Result<String, CpError> {
        self.rtt().await;
        self.inner.prepare(b).await
    }
    async fn find_prepared(&self) -> Result<Option<skynbd_agent::PreparedInfo>, CpError> {
        self.inner.find_prepared().await
    }
    async fn ensure(&self, s: &str, h: &[[u8; 32]]) -> Result<Vec<bool>, CpError> {
        let n = self.c.ensure.fetch_add(1, Ordering::SeqCst) + 1;
        self.staging();
        self.rtt().await;
        let mut out = self.inner.ensure(s, h).await?;
        let from = self.short_ensure_from.load(Ordering::SeqCst);
        if from != 0 && n >= from {
            out.pop();
        }
        Ok(out)
    }
    async fn put_object(&self, h: [u8; 32], d: Vec<u8>) -> Result<(), CpError> {
        self.c.put.fetch_add(1, Ordering::SeqCst);
        self.staging();
        let now = self.c.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.c.put_high_water.fetch_max(now, Ordering::SeqCst);
        // A pending await, so concurrently issued uploads overlap.
        let put_delay = if self.slow.load(Ordering::SeqCst) {
            self.put_delay
        } else {
            Duration::ZERO
        };
        tokio::time::sleep(put_delay.max(self.rtt)).await;
        let r = self.inner.put_object(h, d).await;
        self.c.put_done.fetch_add(1, Ordering::SeqCst);
        self.c.in_flight.fetch_sub(1, Ordering::SeqCst);
        r
    }
    async fn attach(
        &self,
        s: &str,
        o: u64,
        r: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError> {
        self.c.attach.fetch_add(1, Ordering::SeqCst);
        self.c.op_ids.lock().unwrap().push(o);
        let r = self
            .attaching(async {
                self.rtt().await;
                self.inner.attach(s, o, r).await
            })
            .await;
        if r.is_ok() {
            self.c.attached_max.fetch_max(o, Ordering::SeqCst);
        }
        r
    }
    async fn clear(&self, s: &str, o: u64, r: &[(u64, u64)]) -> Result<(), CpError> {
        self.c.clear.fetch_add(1, Ordering::SeqCst);
        self.c.op_ids.lock().unwrap().push(o);
        let r = self
            .attaching(async {
                self.rtt().await;
                self.inner.clear(s, o, r).await
            })
            .await;
        if r.is_ok() {
            self.c.attached_max.fetch_max(o, Ordering::SeqCst);
        }
        r
    }
    async fn apply(&self, s: &str, o: u64, ops: &[RangeOp]) -> Result<(), CpError> {
        self.c.apply.fetch_add(1, Ordering::SeqCst);
        self.attaching(async {
            self.rtt().await;
            if self.no_apply.load(Ordering::SeqCst) {
                return Err(CpError::Unsupported("apply".into()));
            }
            self.c.op_ids.lock().unwrap().push(o);
            self.inner.apply(s, o, ops).await
        })
        .await
        .inspect(|()| {
            self.c.attached_max.fetch_max(o, Ordering::SeqCst);
        })
    }
    async fn report_progress(&self, p: CycleProgress) -> Result<(), CpError> {
        let attached = self.c.attached_max.load(Ordering::SeqCst);
        self.c
            .reports
            .lock()
            .unwrap()
            .push((p.phase, p.ops_done, attached));
        self.c.reporting.fetch_add(1, Ordering::SeqCst);
        if !self.report_delay.is_zero() {
            tokio::time::sleep(self.report_delay).await;
        }
        self.c.reporting.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
    async fn checkpoint(&self, s: &str, o: u64) -> Result<(), CpError> {
        self.rtt().await;
        self.inner.checkpoint(s, o).await
    }
    async fn publish(&self, s: &str, n: &str, p: &str) -> Result<(), CpError> {
        self.rtt().await;
        self.inner.publish(s, n, p).await
    }
    async fn abort(&self, s: &str) -> Result<(), CpError> {
        self.inner.abort(s).await
    }
    async fn ref_exists(&self, r: &str) -> Result<bool, CpError> {
        self.inner.ref_exists(r).await
    }
    async fn pinned_by(&self, r: &str, h: &str) -> Result<bool, CpError> {
        self.inner.pinned_by(r, h).await
    }
    async fn pin(&self, r: &str, h: &str) -> Result<(), CpError> {
        self.inner.pin(r, h).await
    }
    async fn unpin(&self, r: &str, h: &str) -> Result<(), CpError> {
        self.inner.unpin(r, h).await
    }
    async fn read_snapshot(&self, r: &str, o: u64, l: u64) -> Result<Vec<u8>, CpError> {
        self.inner.read_snapshot(r, o, l).await
    }
}

struct Rig {
    stack: FakeStack,
    cp: Probe,
    _dir: tempfile::TempDir,
    eng: CycleEngine<FakeStack, Probe>,
}

async fn rig(depth: usize, rtt: Duration) -> Rig {
    rig_with(depth, rtt, |_| {}).await
}

/// [`rig`] with the probe adjusted before the engine takes its copy.
async fn rig_with(depth: usize, rtt: Duration, tune: impl FnOnce(&mut Probe)) -> Rig {
    let stack = FakeStack::new(VOL, CHUNK);
    let inner = InprocControlPlane::create_volume(
        MemoryStore::new(),
        MemoryRefStore::new(),
        VolumeId(ulid::Ulid::new()),
        VOL,
    )
    .await
    .unwrap();
    let mut cp = Probe {
        inner,
        rtt,
        put_delay: Duration::from_millis(2),
        attach_delay: Duration::ZERO,
        report_delay: Duration::ZERO,
        slow: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        short_ensure_from: Arc::new(AtomicU64::new(0)),
        no_apply: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        c: Arc::new(Counters::default()),
    };
    tune(&mut cp);
    let dir = tempfile::tempdir().unwrap();
    let eng = CycleEngine::open(
        stack.clone(),
        cp.clone(),
        StateStore::new(dir.path()),
        "vlin",
    )
    .unwrap()
    .with_publish_pipeline_depth(depth);
    Rig {
        stack,
        cp,
        _dir: dir,
        eng,
    }
}

/// Distinct content in every chunk: nothing dedups, every op uploads.
fn fill_distinct(stack: &FakeStack, seed: u8) {
    for i in 0..CHUNKS {
        let mut b = vec![seed; CHUNK as usize];
        b[..8].copy_from_slice(&i.to_le_bytes());
        stack.write_origin(i * CHUNK, &b);
    }
}

/// The same history publishes the same bytes at depth 1 and depth 16,
/// with depth-16 sending 1/16th of the ensures and attaches and really
/// running its uploads 16 at a time.
#[tokio::test(flavor = "multi_thread")]
async fn pipelined_publish_is_bit_exact_and_concurrent() {
    let mut images = Vec::new();
    for depth in [1usize, 16] {
        let mut r = rig(depth, Duration::ZERO).await;
        fill_distinct(&r.stack, 1);
        let img = r.stack.origin_image();
        let target = r.eng.run_cycle().await.unwrap();
        assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);
        images.push(img);

        let c = &r.cp.c;
        let batches = CHUNKS / depth as u64;
        assert_eq!(c.put.load(Ordering::SeqCst), CHUNKS, "depth {depth}");
        assert_eq!(c.ensure.load(Ordering::SeqCst), batches, "depth {depth}");
        assert_eq!(c.attach.load(Ordering::SeqCst), batches, "depth {depth}");
        assert_eq!(
            c.put_high_water.load(Ordering::SeqCst),
            depth as u64,
            "depth {depth}: observed upload concurrency"
        );
        // Attach ids name each batch's last op, strictly increasing.
        let ids = c.op_ids.lock().unwrap().clone();
        let want: Vec<u64> = (1..=batches).map(|b| b * depth as u64).collect();
        assert_eq!(ids, want, "depth {depth}");
    }
    assert_eq!(images[0], images[1]);
}

/// Interleave writes and discards over the first 16 chunks, so every
/// same-kind run is one op long.
fn interleave(stack: &FakeStack) {
    for i in 0..16u64 {
        if i % 2 == 0 {
            stack.discard_origin(i * CHUNK, CHUNK);
        } else {
            stack.write_origin(i * CHUNK, &vec![0xEE ^ i as u8; CHUNK as usize]);
        }
    }
}

/// [`interleave`]'s mirror: writes the even chunks (with `seed`) and
/// discards the odd ones, so after it every op is again a one-op run.
fn interleave_with(stack: &FakeStack, seed: u8) {
    for i in 0..16u64 {
        if i % 2 == 0 {
            stack.write_origin(i * CHUNK, &vec![seed ^ i as u8; CHUNK as usize]);
        } else {
            stack.discard_origin(i * CHUNK, CHUNK);
        }
    }
}

/// Interleaved writes and discards: each depth-8 batch is ONE `apply`
/// call under its last op id — not eight alternating attach/clear calls
/// — and the published image stays exact.
#[tokio::test(flavor = "multi_thread")]
async fn a_mixed_batch_is_one_apply_call() {
    let mut r = rig(8, Duration::ZERO).await;
    fill_distinct(&r.stack, 1);
    r.eng.run_cycle().await.unwrap();
    r.cp.c.op_ids.lock().unwrap().clear();
    let (attaches, clears) = (
        r.cp.c.attach.load(Ordering::SeqCst),
        r.cp.c.clear.load(Ordering::SeqCst),
    );

    interleave(&r.stack);
    let img = r.stack.origin_image();
    let target = r.eng.run_cycle().await.unwrap();
    assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);

    assert_eq!(
        *r.cp.c.op_ids.lock().unwrap(),
        vec![8, 16],
        "one call per batch"
    );
    assert_eq!(r.cp.c.apply.load(Ordering::SeqCst), 2);
    assert_eq!(r.cp.c.attach.load(Ordering::SeqCst), attaches);
    assert_eq!(r.cp.c.clear.load(Ordering::SeqCst), clears);
}

/// Against a server without `apply`, a mixed batch's refusal falls back
/// to one call per same-kind run, in op order — never all attaches
/// first — and the published image stays exact. A refusal is asked once
/// per CYCLE: the rest of the cycle's mixed batches split without asking
/// (a refusal can cost a round trip, e.g. a proxy answering for the
/// path), and the next cycle asks again, so an upgraded server is used
/// from then on.
#[tokio::test(flavor = "multi_thread")]
async fn without_apply_a_mixed_batch_splits_into_runs_in_op_order() {
    let mut r = rig(8, Duration::ZERO).await;
    r.cp.no_apply.store(true, Ordering::SeqCst);
    fill_distinct(&r.stack, 1);
    r.eng.run_cycle().await.unwrap();
    assert_eq!(
        r.cp.c.apply.load(Ordering::SeqCst),
        0,
        "pure batches never apply"
    );
    r.cp.c.op_ids.lock().unwrap().clear();
    let attaches_before = r.cp.c.attach.load(Ordering::SeqCst);

    interleave(&r.stack);
    let img = r.stack.origin_image();
    let target = r.eng.run_cycle().await.unwrap();
    assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);

    let ids = r.cp.c.op_ids.lock().unwrap().clone();
    assert_eq!(
        ids,
        (1..=16).collect::<Vec<u64>>(),
        "one call per op, in order"
    );
    assert_eq!(
        r.cp.c.apply.load(Ordering::SeqCst),
        1,
        "asked once per cycle, not per mixed batch"
    );
    assert_eq!(r.cp.c.clear.load(Ordering::SeqCst), 8);
    assert_eq!(r.cp.c.attach.load(Ordering::SeqCst) - attaches_before, 8);
    assert_eq!(
        r.cp.c.ensure.load(Ordering::SeqCst),
        CHUNKS / 8 + 2,
        "one per batch"
    );

    // The server gains the call: the next cycle asks again and uses it.
    r.cp.no_apply.store(false, Ordering::SeqCst);
    interleave_with(&r.stack, 0x40);
    let img = r.stack.origin_image();
    let target = r.eng.run_cycle().await.unwrap();
    assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);
    assert_eq!(
        r.cp.c.apply.load(Ordering::SeqCst),
        3,
        "the next cycle asked again, and both its mixed batches applied"
    );
}

/// Against a server without `apply`, a crash can
/// land between a split batch's runs. Crash the interleaved cycle at
/// every one of its points and record how many attach-family calls had
/// been made: every count from 0 to 16 must occur — a crash state right
/// after each call — not just batch boundaries, which is all a crash
/// seam deferred to the end of a batch would give.
#[tokio::test(flavor = "multi_thread")]
async fn without_apply_a_crash_can_land_between_a_split_batchs_runs() {
    async fn calls_at_crash(at: Option<u32>) -> (usize, u32) {
        let mut r = rig(8, Duration::ZERO).await;
        r.cp.no_apply.store(true, Ordering::SeqCst);
        fill_distinct(&r.stack, 1);
        r.eng.run_cycle().await.unwrap();
        let (before, base) = (r.cp.c.op_ids.lock().unwrap().len(), r.eng.crash.counter);
        interleave(&r.stack);
        r.eng.crash.at = at.map(|k| base + k);
        let out = r.eng.run_cycle().await;
        assert_eq!(out.is_err(), at.is_some(), "{at:?}: {out:?}");
        let calls = r.cp.c.op_ids.lock().unwrap().len() - before;
        (calls, r.eng.crash.counter - base)
    }
    let (all, points) = calls_at_crash(None).await;
    assert_eq!(all, 16, "one call per op");
    let mut seen = std::collections::BTreeSet::new();
    for k in 0..points {
        seen.insert(calls_at_crash(Some(k)).await.0);
    }
    assert!(
        (0..=16).all(|n| seen.contains(&n)),
        "crash states after each call: {seen:?}"
    );
}

/// Identical content within a batch uploads once.
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_content_in_a_batch_uploads_once() {
    let mut r = rig(16, Duration::ZERO).await;
    for i in 0..16u64 {
        r.stack.write_origin(i * CHUNK, &vec![7u8; CHUNK as usize]);
    }
    let img = r.stack.origin_image();
    let target = r.eng.run_cycle().await.unwrap();
    assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);
    assert_eq!(r.cp.c.put.load(Ordering::SeqCst), 1);
    assert_eq!(r.cp.c.attach.load(Ordering::SeqCst), 1);
}

/// The measurement gate in miniature: publish time for a fixed delta at
/// a simulated per-call RTT, depth 1 vs depth 16, and depth 16 must be
/// at least twelve times faster (it is 21.4 times: 1.542 s and 72 ms).
///
/// Why twelve. The factor used to be three, loose for a noisy runner;
/// the clock below is exact, so the factor can say what it means. With
/// the effective depth capped in the engine the same publish at "depth
/// 16" takes 520 ms at an effective depth of 2 (a factor of 2.97),
/// 264 ms at 4 (5.8) and 136 ms at 8 (11.3). Three caught a pipeline
/// that had fallen back to depth 1 or 2 and nothing less; twelve fails
/// one that works at half its depth. (The capped figures are the
/// reviewer's, measured for skyzfs/skyzfs#26.)
///
/// The time judged is the runtime's clock, paused: it moves only when
/// every task waits, and then straight to the next timer. So it counts
/// the waits for the control plane (the RTTs, as many as the pipeline
/// left one after another) and nothing of the machine: the same number
/// on every run. The test used to compare two wall clocks, and under
/// the coverage build on a shared runner the CPU time of 256 ops, which
/// no depth shortens, brought the factor to 2.5 (run 43154941145). The
/// wall clocks are still printed, for the record.
#[tokio::test(start_paused = true)]
async fn pipelining_compresses_publish_time_at_rtt() {
    let rtt = Duration::from_millis(2);
    let mut at_rtt = Vec::new();
    for depth in [1usize, 16] {
        let mut r = rig(depth, rtt).await;
        fill_distinct(&r.stack, 3);
        let wall = Instant::now();
        let t = tokio::time::Instant::now();
        r.eng.run_cycle().await.unwrap();
        let waited = t.elapsed();
        println!(
            "depth={depth} ops={CHUNKS} rtt={rtt:?} publish_time_at_rtt={waited:?} wall={:?}",
            wall.elapsed()
        );
        // The clock is the simulated one: a publish of 256 ops that
        // waited for less than one RTT per batch did not go through
        // the probe's waits, and the comparison below would be of
        // nothing.
        assert!(
            waited >= rtt * (CHUNKS / depth as u64) as u32,
            "depth {depth}: {waited:?} is less than one RTT per batch"
        );
        at_rtt.push(waited);
    }
    assert!(
        at_rtt[1] * 12 < at_rtt[0],
        "depth 16 should be at least twelve times faster: {at_rtt:?}"
    );
}

/// Issue #97 follow-up: above depth 1, batch k+1 stages (its `ensure`
/// and uploads) while batch k attaches — observed, not inferred — yet
/// attach-family calls never overlap EACH OTHER and their op ids only
/// rise. At depth 1 nothing overlaps: the historical sequence.
#[tokio::test(flavor = "multi_thread")]
async fn consecutive_batches_overlap_but_attaches_stay_serial_and_ordered() {
    for (depth, want_overlap) in [(16usize, true), (1, false)] {
        let mut r = rig(depth, Duration::from_millis(3)).await;
        fill_distinct(&r.stack, 5);
        let img = r.stack.origin_image();
        let target = r.eng.run_cycle().await.unwrap();
        assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);

        let c = &r.cp.c;
        assert_eq!(
            c.overlapped.load(Ordering::SeqCst),
            want_overlap,
            "depth {depth}: staging under an in-flight attach"
        );
        assert_eq!(
            c.attach_high_water.load(Ordering::SeqCst),
            1,
            "depth {depth}"
        );
        let ids = c.op_ids.lock().unwrap().clone();
        assert!(
            ids.windows(2).all(|w| w[0] < w[1]),
            "depth {depth}: {ids:?}"
        );
        assert_eq!(*ids.last().unwrap(), CHUNKS, "every op attached");
    }
}

/// Overlap hides a round trip per batch. With uploads costing one RTT,
/// a batch is ensure + uploads (2 RTTs) then attach (1): ~3 RTTs per
/// batch staged-then-attached, ~2 when batch k+1 stages beside batch k's
/// attach. Measured as a ratio of the same publish with overlap off, so
/// the cycle's non-backfill round trips weigh on both sides alike.
///
/// The time is the runtime's paused clock, as in
/// `pipelining_compresses_publish_time_at_rtt`: the waits for the
/// control plane and nothing of the machine. It gives 1.98 s with
/// overlap off and 1.36 s with it on, the same on every run: 620 ms
/// saved, one 20 ms round trip at each of the 31 places where a batch
/// follows another, a ratio of 0.687. Asserted below 0.70, which fails
/// when two of those 31 are lost (0.707). The threshold used to be 0.85
/// against two wall clocks.
#[tokio::test(start_paused = true)]
async fn overlap_saves_a_round_trip_per_batch() {
    let rtt = Duration::from_millis(20);
    let depth = 8usize;
    let mut walls = Vec::new();
    for overlap in [false, true] {
        let mut r = rig(depth, rtt).await;
        r.eng = r.eng.with_publish_overlap(overlap);
        fill_distinct(&r.stack, 9);
        let img = r.stack.origin_image();
        let wall = Instant::now();
        let t = tokio::time::Instant::now();
        let target = r.eng.run_cycle().await.unwrap();
        let waited = t.elapsed();
        let wall = wall.elapsed();
        assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);
        assert_eq!(
            r.cp.c.overlapped.load(Ordering::SeqCst),
            overlap,
            "staging under an in-flight attach"
        );
        println!(
            "depth={depth} batches={} rtt={rtt:?} overlap={overlap} publish_time_at_rtt={waited:?} wall={wall:?}",
            CHUNKS / depth as u64
        );
        walls.push(waited);
    }
    let ratio = walls[1].as_secs_f64() / walls[0].as_secs_f64();
    assert!(
        ratio < 0.70,
        "overlap saved too little: {walls:?} ({ratio:.2})"
    );
}

/// An attach failure — here an injected crash right after batch 1's
/// attach — cancels batch 2's overlapped staging where it stands rather
/// than letting it read, ensure and upload a whole batch first. With
/// batch 2's uploads slow and batch 1's attach slower than its ensure,
/// the crash freezes batch 2 mid-upload: ensured, uploads started, none
/// landed. That state must recover to the exact image, re-uploading
/// batch 2 (the crashed run's PUTs never reached the store).
#[tokio::test(flavor = "multi_thread")]
async fn an_attach_crash_cancels_the_overlapped_staging_mid_flight() {
    let depth = 8usize;
    let mut tune = |p: &mut Probe| {
        p.put_delay = Duration::from_millis(400);
        p.attach_delay = Duration::from_millis(100);
    };
    // Find the crash point right after batch 1's attach: the first one
    // at which an attach has staged. Crash points are hit in the same
    // order whatever the timing, so the scan runs without the delays.
    let mut crash_at = None;
    for k in 0..16u32 {
        let mut r = rig_with(depth, Duration::ZERO, &mut tune).await;
        r.cp.slow.store(false, Ordering::SeqCst);
        fill_distinct(&r.stack, 4);
        r.eng.crash.at = Some(k);
        match r.eng.run_cycle().await {
            Err(skynbd_agent::AgentError::InjectedCrash(_)) => {}
            other => panic!("crash point {k} did not fire: {other:?}"),
        }
        if r.cp.c.attached_ok.load(Ordering::SeqCst) == 1 {
            crash_at = Some(k);
            break;
        }
    }
    let crash_at = crash_at.expect("no crash point follows batch 1's attach");

    let mut r = rig_with(depth, Duration::ZERO, &mut tune).await;
    fill_distinct(&r.stack, 4);
    let img = r.stack.origin_image();
    r.eng.crash.at = Some(crash_at);
    let t = Instant::now();
    let err = r.eng.run_cycle().await.unwrap_err();
    let waited = t.elapsed();
    assert!(
        matches!(err, skynbd_agent::AgentError::InjectedCrash(_)),
        "{err}"
    );
    let c = &r.cp.c;
    assert_eq!(c.attached_ok.load(Ordering::SeqCst), 1, "batch 1 attached");
    assert_eq!(c.ensure.load(Ordering::SeqCst), 2, "batch 2 was ensured");
    assert_eq!(
        c.put.load(Ordering::SeqCst),
        2 * depth as u64,
        "batch 2's uploads had started"
    );
    assert_eq!(
        c.put_done.load(Ordering::SeqCst),
        depth as u64,
        "batch 2 was cancelled mid-upload, not run to completion"
    );
    // Batch 1's uploads (400 ms) + its attach (100 ms); batch 2's
    // uploads would have taken until ~900 ms.
    assert!(waited < Duration::from_millis(800), "waited {waited:?}");

    r.cp.slow.store(false, Ordering::SeqCst);
    let mut eng = CycleEngine::open(
        r.stack.clone(),
        r.cp.clone(),
        StateStore::new(r._dir.path()),
        "vlin",
    )
    .unwrap()
    .with_publish_pipeline_depth(depth);
    eng.recover().await.expect("recovery converges");
    let target = if r.cp.ref_exists("snap/g0").await.unwrap() {
        "snap/g0".to_string()
    } else {
        eng.run_cycle().await.unwrap()
    };
    assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);
}

/// A staging failure of batch 2, overlapped with batch 1's attach, waits
/// for that attach (its ops precede) and then fails the cycle: batch 1
/// is attached, nothing of batch 2 is uploaded or attached, and batch 1's
/// slow attach is neither cancelled nor masked by batch 2's error.
#[tokio::test(flavor = "multi_thread")]
async fn an_overlapped_staging_failure_surfaces_after_the_earlier_attach() {
    let depth = 8usize;
    let mut r = rig_with(depth, Duration::ZERO, |p| {
        p.attach_delay = Duration::from_millis(100);
        p.short_ensure_from.store(2, Ordering::SeqCst);
    })
    .await;
    fill_distinct(&r.stack, 6);
    let err = r.eng.run_cycle().await.unwrap_err();
    assert!(
        err.to_string()
            .contains("ensure answered 7 results for 8 hashes"),
        "{err}"
    );
    let c = &r.cp.c;
    assert_eq!(c.ensure.load(Ordering::SeqCst), 2);
    assert_eq!(c.attached_ok.load(Ordering::SeqCst), 1, "batch 1 attached");
    assert_eq!(*c.op_ids.lock().unwrap(), vec![depth as u64]);
    assert_eq!(
        c.put.load(Ordering::SeqCst),
        depth as u64,
        "nothing of batch 2 uploaded"
    );
}

/// Crash with a window in flight AFTER the server has checkpointed (it
/// does every 32 attach calls), then lose the server's non-durable staged
/// ops too: the resume must restart from the durable watermark — which a
/// pipeline may only ever advance over contiguous, attached ops — and
/// still publish the exact image. Re-uploads of objects the crashed run
/// already sent are `ensure` hits.
#[tokio::test(flavor = "multi_thread")]
async fn pipelined_resume_after_a_durable_checkpoint_is_exact() {
    let depth = 4usize;
    // 2 crash points per all-data batch (after the uploads, after the
    // attach); 40 batches in is past the checkpoint at attach call 32.
    for crash_at in [80u32, 81, 85] {
        let mut r = rig(depth, Duration::ZERO).await;
        fill_distinct(&r.stack, 5);
        let img = r.stack.origin_image();
        r.eng.crash.at = Some(crash_at);
        match r.eng.run_cycle().await {
            Err(skynbd_agent::AgentError::InjectedCrash(_)) => {}
            other => panic!("crash point {crash_at} did not fire: {other:?}"),
        }
        let puts_before = r.cp.c.put.load(Ordering::SeqCst);
        r.cp.inner.drop_sessions().await;

        let mut eng = CycleEngine::open(
            r.stack.clone(),
            r.cp.clone(),
            StateStore::new(r._dir.path()),
            "vlin",
        )
        .unwrap()
        .with_publish_pipeline_depth(depth);
        eng.recover().await.expect("recovery converges");
        let target = if r.cp.ref_exists("snap/g0").await.unwrap() {
            "snap/g0".to_string()
        } else {
            eng.run_cycle().await.unwrap()
        };
        assert_eq!(
            r.cp.read_snapshot(&target, 0, VOL).await.unwrap(),
            img,
            "crash point {crash_at}"
        );
        assert_eq!(
            r.cp.c.put.load(Ordering::SeqCst),
            CHUNKS,
            "crash point {crash_at}: objects uploaded before the crash are ensure hits"
        );
        assert!(
            puts_before > 32 * depth as u64,
            "the crash landed past a checkpoint"
        );
    }
}

/// A short `ensure` answer fails the batch before anything attaches. Were
/// it zipped against the batch's objects, the objects past its end would
/// never upload while their ops still attached — a published snapshot
/// referencing missing objects.
#[tokio::test(flavor = "multi_thread")]
async fn a_short_ensure_answer_fails_the_batch_instead_of_skipping_uploads() {
    let mut r = rig(8, Duration::ZERO).await;
    fill_distinct(&r.stack, 9);
    r.cp.short_ensure_from.store(1, Ordering::SeqCst);
    let err = r.eng.run_cycle().await.unwrap_err();
    assert!(
        err.to_string()
            .contains("ensure answered 7 results for 8 hashes"),
        "{err}"
    );
    assert_eq!(r.cp.c.put.load(Ordering::SeqCst), 0, "nothing uploaded");
    assert_eq!(r.cp.c.attach.load(Ordering::SeqCst), 0, "nothing attached");
}

/// However deep the batch, at most `PIPELINE_MAX_UPLOADS` objects upload
/// at once — a deep batch of small ops must not open thousands of PUTs.
#[tokio::test(flavor = "multi_thread")]
async fn upload_concurrency_is_capped_whatever_the_depth() {
    let mut r = rig(CHUNKS as usize, Duration::ZERO).await;
    fill_distinct(&r.stack, 11);
    let img = r.stack.origin_image();
    let target = r.eng.run_cycle().await.unwrap();
    assert_eq!(r.cp.read_snapshot(&target, 0, VOL).await.unwrap(), img);
    assert_eq!(r.cp.c.ensure.load(Ordering::SeqCst), 1, "one batch");
    assert_eq!(
        r.cp.c.put_high_water.load(Ordering::SeqCst),
        skynbd_agent::PIPELINE_MAX_UPLOADS as u64
    );
}

/// Without `apply`, a split batch's runs are separate calls, and the
/// server checkpoints every 32 calls — so the durable watermark can land
/// INSIDE a batch. Alternate writes and discards over 48 chunks at depth
/// 6: every batch is six one-op runs, and call 32 is the second run of
/// the sixth batch (op 32 of 31..=36). Crash the agent at each point
/// after that, lose the server's non-durable staged ops too, and the
/// resume — from a watermark that is not a batch end — must publish the
/// exact image.
#[tokio::test(flavor = "multi_thread")]
async fn a_watermark_inside_a_split_batch_resumes_exactly() {
    let depth = 6usize;
    let mut mid_batch = 0;
    for crash_at in 0..120u32 {
        let mut r = rig(depth, Duration::ZERO).await;
        r.cp.no_apply.store(true, Ordering::SeqCst);
        for i in 0..48u64 {
            r.stack
                .write_origin(i * CHUNK, &vec![0x11 ^ i as u8; CHUNK as usize]);
        }
        r.eng.run_cycle().await.unwrap();
        for i in 0..48u64 {
            if i % 2 == 0 {
                r.stack
                    .write_origin(i * CHUNK, &vec![0x77 ^ i as u8; CHUNK as usize]);
            } else {
                r.stack.discard_origin(i * CHUNK, CHUNK);
            }
        }
        let img = r.stack.origin_image();
        let base = r.eng.crash.counter;
        r.eng.crash.at = Some(base + crash_at);
        match r.eng.run_cycle().await {
            Err(skynbd_agent::AgentError::InjectedCrash(_)) => {}
            Ok(_) => break, // past the cycle's last point
            Err(e) => panic!("crash point {crash_at}: {e}"),
        }
        let Some(info) = r.cp.find_prepared().await.unwrap() else {
            continue; // before prepare, or after publish
        };
        let w = info.op_watermark;
        if w == 0 || w % depth as u64 == 0 {
            continue; // not the case under test
        }
        mid_batch += 1;
        r.cp.inner.drop_sessions().await;

        let mut eng = CycleEngine::open(
            r.stack.clone(),
            r.cp.clone(),
            StateStore::new(r._dir.path()),
            "vlin",
        )
        .unwrap()
        .with_publish_pipeline_depth(depth);
        eng.recover().await.expect("recovery converges");
        let target = if r.cp.ref_exists("snap/g1").await.unwrap() {
            "snap/g1".to_string()
        } else {
            eng.run_cycle().await.unwrap()
        };
        assert_eq!(
            r.cp.read_snapshot(&target, 0, VOL).await.unwrap(),
            img,
            "crash point {crash_at}, durable watermark {w} (mid-batch)"
        );
    }
    assert!(mid_batch > 0, "no crash left the watermark inside a batch");
}

/// Progress reports never claim ops ahead of what is attached, and the
/// backfill's last report comes before "publishable" — at depths 8 and
/// 64 over 256 ops (four 64-op marks).
#[tokio::test(flavor = "multi_thread")]
async fn progress_reports_trail_the_attaches_and_precede_publishable() {
    for depth in [8usize, 64] {
        let mut r = rig(depth, Duration::from_millis(1)).await;
        fill_distinct(&r.stack, 0x21);
        r.eng.run_cycle().await.unwrap();
        let reports = r.cp.c.reports.lock().unwrap().clone();
        let backfill: Vec<_> = reports
            .iter()
            .enumerate()
            .filter(|(_, (phase, done, _))| *phase == "backfilling" && *done > 0)
            .collect();
        assert!(!backfill.is_empty(), "depth {depth}: no progress reported");
        for (_, (_, done, attached)) in &backfill {
            assert!(
                done <= attached,
                "depth {depth}: reported {done} ops with only {attached} attached"
            );
        }
        let publishable = reports
            .iter()
            .position(|(phase, ..)| *phase == "publishable")
            .expect("publishable reported");
        assert!(
            backfill.last().unwrap().0 < publishable,
            "depth {depth}: a backfill report after publishable: {reports:?}"
        );
    }
}

/// A slow status endpoint does not hold up the backfill: with every
/// report taking 300 ms, attaches go on while a report is in flight
/// (they used to wait for it, batch by batch).
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_progress_report_does_not_gate_the_next_batch() {
    let mut r = rig_with(8, Duration::from_millis(1), |p| {
        p.report_delay = Duration::from_millis(300);
    })
    .await;
    fill_distinct(&r.stack, 0x22);
    let img = r.stack.origin_image();
    let t = r.eng.run_cycle().await.unwrap();
    assert_eq!(r.cp.read_snapshot(&t, 0, VOL).await.unwrap(), img);
    assert!(
        r.cp.c.attach_while_reporting.load(Ordering::SeqCst),
        "every attach waited for the progress report in flight"
    );
}
