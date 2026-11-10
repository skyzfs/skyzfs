//! D-M12: the cycle engine reports phase/op progress through
//! `ControlPlane::report_progress` — ordered phase seams, a final
//! backfill count that matches the delta, and strict best-effort
//! semantics (a reporting failure never fails the cycle) — and the
//! durable request-to-cut bindings those reports carry.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use skynbd_agent::{
    CaptureBinding, ControlPlane, CpError, CycleEngine, CycleProgress, FakeStack, LocalStack,
    ServedRequest, StateStore,
};
use skynbd_refs::MemoryRefStore;
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const VOL: u64 = 32 * CHUNK;

/// One `report_captures` call: (captures, lost).
type CaptureReport = (Vec<CaptureBinding>, Vec<String>);

/// Delegating wrapper that records every progress report (and can be
/// told to fail them, proving the engine ignores reporter errors).
#[derive(Clone)]
struct Recording<C> {
    inner: C,
    reports: Arc<Mutex<Vec<CycleProgress>>>,
    fail_reports: bool,
    /// Fail `prepare` — the §8.2 partition that strands a candidate.
    fail_prepare: Arc<AtomicBool>,
    /// Every `report_captures` call: (captures, lost).
    capture_reports: Arc<Mutex<Vec<CaptureReport>>>,
}

impl<C: ControlPlane> ControlPlane for Recording<C> {
    async fn prepare(&self, base_ref: Option<&str>) -> Result<String, CpError> {
        if self.fail_prepare.load(Ordering::SeqCst) {
            return Err(CpError::Other("partitioned".into()));
        }
        self.inner.prepare(base_ref).await
    }
    async fn find_prepared(&self) -> Result<Option<skynbd_agent::PreparedInfo>, CpError> {
        self.inner.find_prepared().await
    }
    async fn ensure(&self, snap_id: &str, hashes: &[[u8; 32]]) -> Result<Vec<bool>, CpError> {
        self.inner.ensure(snap_id, hashes).await
    }
    async fn put_object(&self, hash: [u8; 32], data: Vec<u8>) -> Result<(), CpError> {
        self.inner.put_object(hash, data).await
    }
    async fn attach(
        &self,
        snap_id: &str,
        op_id: u64,
        ranges: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError> {
        self.inner.attach(snap_id, op_id, ranges).await
    }
    async fn clear(&self, snap_id: &str, op_id: u64, ranges: &[(u64, u64)]) -> Result<(), CpError> {
        self.inner.clear(snap_id, op_id, ranges).await
    }
    async fn apply(
        &self,
        snap_id: &str,
        op_id: u64,
        ops: &[skynbd_agent::RangeOp],
    ) -> Result<(), CpError> {
        self.inner.apply(snap_id, op_id, ops).await
    }
    async fn checkpoint(&self, snap_id: &str, op_id: u64) -> Result<(), CpError> {
        self.inner.checkpoint(snap_id, op_id).await
    }
    async fn publish(&self, snap_id: &str, name: &str, publisher: &str) -> Result<(), CpError> {
        self.inner.publish(snap_id, name, publisher).await
    }
    async fn abort(&self, snap_id: &str) -> Result<(), CpError> {
        self.inner.abort(snap_id).await
    }
    async fn ref_exists(&self, ref_name: &str) -> Result<bool, CpError> {
        self.inner.ref_exists(ref_name).await
    }
    async fn pinned_by(&self, ref_name: &str, holder: &str) -> Result<bool, CpError> {
        self.inner.pinned_by(ref_name, holder).await
    }
    async fn pin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.inner.pin(ref_name, holder).await
    }
    async fn unpin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.inner.unpin(ref_name, holder).await
    }
    async fn read_snapshot(&self, ref_name: &str, off: u64, len: u64) -> Result<Vec<u8>, CpError> {
        self.inner.read_snapshot(ref_name, off, len).await
    }
    async fn report_progress(&self, p: CycleProgress) -> Result<(), CpError> {
        self.reports.lock().unwrap().push(p);
        if self.fail_reports {
            return Err(CpError::Other("reporter down".into()));
        }
        Ok(())
    }
    async fn report_captures(
        &self,
        captures: &[CaptureBinding],
        lost: &[String],
    ) -> Result<(), CpError> {
        self.capture_reports
            .lock()
            .unwrap()
            .push((captures.to_vec(), lost.to_vec()));
        Ok(())
    }
}

async fn recording_cp(
    fail_reports: bool,
) -> (
    Recording<skynbd_agent::InprocControlPlane<MemoryStore, MemoryRefStore>>,
    Arc<Mutex<Vec<CycleProgress>>>,
) {
    let inner = skynbd_agent::InprocControlPlane::create_volume(
        MemoryStore::new(),
        MemoryRefStore::new(),
        VolumeId(ulid::Ulid::new()),
        VOL,
    )
    .await
    .unwrap();
    let reports = Arc::new(Mutex::new(Vec::new()));
    (
        Recording {
            inner,
            reports: reports.clone(),
            fail_reports,
            fail_prepare: Arc::new(AtomicBool::new(false)),
            capture_reports: Arc::new(Mutex::new(Vec::new())),
        },
        reports,
    )
}

#[tokio::test]
async fn cycle_reports_ordered_phases_and_full_backfill_count() {
    let stack = FakeStack::new(VOL, CHUNK);
    let (cp, reports) = recording_cp(false).await;
    let dir = tempfile::tempdir().unwrap();
    let mut eng =
        CycleEngine::open(stack.clone(), cp, StateStore::new(dir.path()), "vlin").unwrap();

    stack.write_origin(0, &vec![7u8; 3 * CHUNK as usize]);
    eng.set_capture_requests(vec!["csi-1".into()]);
    eng.run_cycle().await.unwrap();

    let reports = reports.lock().unwrap();
    let phases: Vec<&str> = reports.iter().map(|p| p.phase).collect();
    // Each seam reported, in cycle order (backfilling repeats).
    let mut want = [
        "capturing",
        "captured",
        "backfilling",
        "publishable",
        "visible",
        "promoting",
    ]
    .into_iter()
    .peekable();
    for p in &phases {
        if want.peek() == Some(p) {
            want.next();
        }
    }
    assert!(
        want.peek().is_none(),
        "missing phases {:?} in report stream {phases:?}",
        want.collect::<Vec<_>>()
    );
    // Bindings appear once the cut exists, never before: `capturing`
    // precedes the freeze, so it cannot promise a point in time.
    let capturing = reports.iter().find(|p| p.phase == "capturing").unwrap();
    assert!(capturing.captures.is_empty());
    let captured = reports.iter().find(|p| p.phase == "captured").unwrap();
    assert_eq!(captured.captures.len(), 1);
    assert_eq!(captured.captures[0].name, "csi-1");
    let capture_time_ms = captured.captures[0].capture_time_ms;
    assert!(capture_time_ms > 0);

    // The backfill total gets known and completed: the last backfilling
    // report carries the delta size, and the terminal reports agree.
    let last_backfill = reports.iter().rfind(|p| p.phase == "backfilling").unwrap();
    assert!(last_backfill.ops_total > 0, "delta never counted");
    let visible = reports.iter().find(|p| p.phase == "visible").unwrap();
    assert_eq!(visible.ops_done, visible.ops_total);
    assert_eq!(visible.ops_total, last_backfill.ops_total);
    assert_eq!(
        visible.captures, captured.captures,
        "same cut, every report"
    );

    // Published: the request is owed an alias to exactly g0, at the
    // original cut time, until the loop settles it.
    assert_eq!(
        eng.served_requests(),
        vec![ServedRequest {
            name: "csi-1".into(),
            target: "snap/g0".into(),
            capture_time_ms,
        }]
    );
    assert!(eng.is_request_bound("csi-1"));
    eng.finish_served("csi-1").unwrap();
    assert!(!eng.is_request_bound("csi-1"));
    assert!(eng.served_requests().is_empty());
}

/// Reporting is best-effort by contract: every report erroring changes
/// nothing about the cycle's outcome.
#[tokio::test]
async fn reporting_failures_do_not_fail_the_cycle() {
    let stack = FakeStack::new(VOL, CHUNK);
    let (cp, reports) = recording_cp(true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut eng =
        CycleEngine::open(stack.clone(), cp, StateStore::new(dir.path()), "vlin").unwrap();

    stack.write_origin(0, &vec![9u8; CHUNK as usize]);
    let target = eng.run_cycle().await.unwrap();
    assert_eq!(target, "snap/g0");
    assert!(
        !reports.lock().unwrap().is_empty(),
        "reports were attempted"
    );
}

/// Strand a candidate the way §8.2 does: capture succeeds, `prepare`
/// fails. Returns the engine and its control-plane handles.
async fn stranded_with(
    requests: &[&str],
) -> (
    CycleEngine<
        FakeStack,
        Recording<skynbd_agent::InprocControlPlane<MemoryStore, MemoryRefStore>>,
    >,
    FakeStack,
    Recording<skynbd_agent::InprocControlPlane<MemoryStore, MemoryRefStore>>,
    tempfile::TempDir,
) {
    let stack = FakeStack::new(VOL, CHUNK);
    let (cp, _) = recording_cp(false).await;
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CycleEngine::open(
        stack.clone(),
        cp.clone(),
        StateStore::new(dir.path()),
        "vlin",
    )
    .unwrap();
    stack.write_origin(0, &vec![1u8; CHUNK as usize]);
    eng.set_capture_requests(requests.iter().map(|r| r.to_string()).collect());
    cp.fail_prepare.store(true, Ordering::SeqCst);
    eng.run_cycle().await.unwrap_err();
    cp.fail_prepare.store(false, Ordering::SeqCst);
    for r in requests {
        assert!(eng.is_request_bound(r), "{r} bound to the stranded cut");
    }
    assert!(eng.has_candidate_requests());
    (eng, stack, cp, dir)
}

/// A request filed while a stranded candidate waits is not folded into
/// that candidate's cut: resuming publishes only the requests bound
/// before the freeze, and the late one gets a cut of its own.
#[tokio::test]
async fn resumed_candidate_serves_only_its_own_requests() {
    let (mut eng, stack, _cp, _dir) = stranded_with(&["early"]).await;
    let early_time = eng.capture_bindings()[0].capture_time_ms;

    eng.set_capture_requests(vec!["late".into()]);
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    assert_eq!(
        eng.served_requests(),
        vec![ServedRequest {
            name: "early".into(),
            target: "snap/g0".into(),
            capture_time_ms: early_time,
        }]
    );
    assert!(!eng.is_request_bound("late"), "late waits for its own cut");

    stack.write_origin(0, &vec![2u8; CHUNK as usize]);
    eng.set_capture_requests(vec!["late".into()]);
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    let served = eng.served_requests();
    assert_eq!(served.len(), 2);
    assert_eq!(served[1].name, "late");
    assert_eq!(served[1].target, "snap/g1");
    assert!(served[1].capture_time_ms >= early_time);
}

/// The review's finding: a discarded candidate's generation name is
/// reused by the next capture. Its requests must become lost — never be
/// aliased to the later cut published under the same `snap/gN`.
#[tokio::test]
async fn swept_candidate_requests_are_lost_not_served_by_the_recapture() {
    let (mut eng, stack, cp, dir) = stranded_with(&["s1"]).await;
    // The activation is swept, so the next cycle cannot resume it and
    // re-captures under the same deterministic name.
    let cand = eng.state().unwrap().candidate.clone().unwrap().lv_name;
    stack.snap_delete(&cand).unwrap();

    stack.write_origin(0, &vec![3u8; CHUNK as usize]);
    eng.set_capture_requests(vec!["s2".into()]);
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    let served = eng.served_requests();
    assert_eq!(served.len(), 1, "{served:?}");
    assert_eq!(served[0].name, "s2");
    assert_eq!(eng.state().unwrap().lost_requests, vec!["s1".to_string()]);
    assert!(
        eng.is_request_bound("s1"),
        "a lost request is never re-captured"
    );

    // The binding survives a restart of the agent.
    drop(eng);
    let mut eng = CycleEngine::open(
        stack.clone(),
        cp.clone(),
        StateStore::new(dir.path()),
        "vlin",
    )
    .unwrap();
    assert!(eng.is_request_bound("s1"));
    assert!(eng.has_lost_requests());

    // Reported while pending, and kept after the report is accepted: the
    // API's status is in memory, so after its restart the controller's
    // re-POST would otherwise look like a fresh request.
    let pending = vec!["s1".to_string(), "s2".to_string()];
    eng.report_captures(&pending).await.unwrap();
    let reports = cp.capture_reports.lock().unwrap().clone();
    assert_eq!(reports.last().unwrap().1, vec!["s1".to_string()]);
    assert_eq!(
        reports.last().unwrap().0.len(),
        1,
        "s2's binding re-announced"
    );
    assert!(eng.has_lost_requests());
    assert!(
        eng.is_request_bound("s1"),
        "a tombstone outlives its report"
    );

    // Not pending (the API marked it lost, so it left the queue): nothing
    // to report until a re-POST brings it back.
    eng.report_captures(&["s2".to_string()]).await.unwrap();
    let reports = cp.capture_reports.lock().unwrap().clone();
    assert!(reports.last().unwrap().1.is_empty());
    eng.report_captures(&pending).await.unwrap();
    let reports = cp.capture_reports.lock().unwrap().clone();
    assert_eq!(reports.last().unwrap().1, vec!["s1".to_string()]);

    // Only an explicit forget (D-M13's reused reserved name) releases it.
    eng.forget_lost("s1").unwrap();
    assert!(!eng.is_request_bound("s1"));
}

/// An aborted in-flight cycle discards its cut: its requests are lost.
#[tokio::test]
async fn aborted_candidate_requests_are_lost() {
    let (mut eng, _stack, _cp, _dir) = stranded_with(&["s1", "s2"]).await;
    eng.abort_in_flight().await.unwrap();
    assert!(!eng.has_candidate_requests());
    assert!(eng.served_requests().is_empty());
    assert_eq!(
        eng.state().unwrap().lost_requests,
        vec!["s1".to_string(), "s2".to_string()]
    );
    assert!(eng.capture_bindings().is_empty());
}
