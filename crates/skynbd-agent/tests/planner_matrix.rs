//! The V3.2 §15.2 recovery matrix as a decision table (agent design doc §5).
//!
//! One test per matrix row, plus contradiction/quarantine rows and a small
//! exhaustive sweep asserting the planner is total (never panics) over the
//! observation space.

use skynbd_agent::state::{
    AgentVolumeState, BaselinePair, LocalSnap, OpCursor, Phase, PreparedRef,
};
use skynbd_agent::{plan, LocalObservation, RecoveryAction, RemoteObservation, RemoteSnapStatus};

fn snap(name: &str) -> LocalSnap {
    LocalSnap {
        lv_name: name.into(),
        thin_dev_id: 7,
        pool_tx_id: 3,
        created_at_ms: 1,
    }
}

fn state(phase: Phase) -> AgentVolumeState {
    AgentVolumeState {
        volume: "vlin".into(),
        phase,
        baseline: Some(BaselinePair {
            local: snap("B_k"),
            remote: "snap/g3".into(),
            pin_holder: "baseline:host1".into(),
        }),
        candidate: Some(snap("S_n")),
        prepared: Some(PreparedRef {
            snap_id: "p4".into(),
            target: "snap/g4".into(),
        }),
        cursor: Some(OpCursor { next_op_id: 10 }),
        advancing: false,
        epoch: None,
        advances_since_swing: 0,
        swing_flipped: None,
        candidate_requests: Vec::new(),
        served_requests: Vec::new(),
        lost_requests: Vec::new(),
        updated_at_ms: 2,
    }
}

fn local(baselines: &[&str], candidates: &[&str]) -> LocalObservation {
    LocalObservation {
        baselines: baselines.iter().map(|s| s.to_string()).collect(),
        candidates: candidates.iter().map(|s| s.to_string()).collect(),
    }
}

fn remote(target: RemoteSnapStatus, base_visible: bool, base_pinned: bool) -> RemoteObservation {
    RemoteObservation {
        target,
        base_visible,
        base_pinned,
        fenced: false,
    }
}

const ABSENT: RemoteSnapStatus = RemoteSnapStatus::Absent;

/// Row 1: crash before `S_n` exists → keep `B_k`, no remote action.
#[test]
fn row1_before_candidate() {
    let a = plan(
        Some(&state(Phase::Capturing)),
        &local(&["B_k"], &[]),
        &remote(ABSENT, true, true),
    );
    assert_eq!(a, RecoveryAction::Continue(Phase::Idle));
}

/// Row 2: after `S_n`, before `P_n` → continue by preparing.
#[test]
fn row2_candidate_no_prepared() {
    let a = plan(
        Some(&state(Phase::Captured)),
        &local(&["B_k"], &["S_n"]),
        &remote(ABSENT, true, true),
    );
    assert_eq!(a, RecoveryAction::Continue(Phase::Captured));
}

/// Rows 3–5: after `P_n` — delta incomplete, mid-upload, or mid-clear —
/// all converge to "recompute/replay above the durable watermark" (§15.3).
#[test]
fn rows3_to_5_resume_backfill_from_watermark() {
    for phase in [Phase::Prepared, Phase::Backfilling] {
        let a = plan(
            Some(&state(phase)),
            &local(&["B_k"], &["S_n"]),
            &remote(RemoteSnapStatus::Prepared { watermark: 32 }, true, true),
        );
        assert_eq!(a, RecoveryAction::ResumeBackfill { from_watermark: 32 });
    }
}

/// Bootstrap variant of rows 3–5: no baseline yet, prepared-from-None.
#[test]
fn bootstrap_backfill_resumes() {
    let a = plan(
        Some(&state(Phase::Backfilling)),
        &local(&[], &["S_0"]),
        &remote(RemoteSnapStatus::Prepared { watermark: 5 }, false, false),
    );
    assert_eq!(a, RecoveryAction::ResumeBackfill { from_watermark: 5 });
}

/// Row 6a: crash during publish, target still hidden → re-publish (I8).
#[test]
fn row6_publish_window_still_prepared() {
    // Observationally identical to rows 3–5 with a complete watermark: the
    // planner resumes; the engine's first replay step is the idempotent
    // publish. Asserting the action is Resume keeps publish retry inside the
    // normal path rather than a special case.
    let a = plan(
        Some(&state(Phase::Publishable)),
        &local(&["B_k"], &["S_n"]),
        &remote(RemoteSnapStatus::Prepared { watermark: 64 }, true, true),
    );
    assert_eq!(a, RecoveryAction::ResumeBackfill { from_watermark: 64 });
}

/// Row 6b: crash during publish, target visible → promote only.
#[test]
fn row6_publish_window_became_visible() {
    let a = plan(
        Some(&state(Phase::Publishable)),
        &local(&["B_k"], &["S_n"]),
        &remote(RemoteSnapStatus::Visible { pinned: true }, true, true),
    );
    assert_eq!(a, RecoveryAction::PromoteOnly);
}

/// Row 7: after publish, before baseline promotion → record `S_n` as `B_n`,
/// then delete old `B_k`.
#[test]
fn row7_promotion_incomplete() {
    let a = plan(
        Some(&state(Phase::Promoting)),
        &local(&["B_k"], &["S_n"]),
        &remote(RemoteSnapStatus::Visible { pinned: true }, true, true),
    );
    assert_eq!(a, RecoveryAction::PromoteOnly);
}

/// Row 8: after publish, `S_n` missing → snapshot valid remotely;
/// rebaseline later (no re-upload).
#[test]
fn row8_published_candidate_lost() {
    let a = plan(
        Some(&state(Phase::Promoting)),
        &local(&["B_k"], &[]),
        &remote(RemoteSnapStatus::Visible { pinned: true }, true, true),
    );
    assert_eq!(
        a,
        RecoveryAction::RebaselineFrom {
            remote_base: "snap/g4".into()
        }
    );
}

/// Row 9: after old `B_k` deletion — `S_n` already recorded as `B_n` (its LV
/// now carries the baseline tag) → promotion is complete.
#[test]
fn row9_promotion_complete() {
    let a = plan(
        Some(&state(Phase::Promoting)),
        &local(&["S_n"], &[]), // S_n's LV, now tagged baseline
        &remote(RemoteSnapStatus::Visible { pinned: true }, true, true),
    );
    assert_eq!(a, RecoveryAction::Continue(Phase::Idle));
}

/// §15.6: a persisted abort decision always completes, regardless of how far
/// the backfill got.
#[test]
fn persisted_abort_finishes() {
    let a = plan(
        Some(&state(Phase::Aborting)),
        &local(&["B_k"], &["S_n"]),
        &remote(RemoteSnapStatus::Prepared { watermark: 9 }, true, true),
    );
    assert_eq!(
        a,
        RecoveryAction::Abort {
            delete_candidate: true
        }
    );
}

/// Prepared root but the candidate LV is gone: delta unrecomputable → abort
/// the prepared root, keep the baseline.
#[test]
fn prepared_without_candidate_aborts() {
    let a = plan(
        Some(&state(Phase::Backfilling)),
        &local(&["B_k"], &[]),
        &remote(RemoteSnapStatus::Prepared { watermark: 3 }, true, true),
    );
    assert_eq!(
        a,
        RecoveryAction::Abort {
            delete_candidate: false
        }
    );
}

/// Fresh host, empty lineage → bootstrap (V3.2 §8.3).
#[test]
fn fresh_everything_bootstraps() {
    let a = plan(None, &local(&[], &[]), &remote(ABSENT, false, false));
    assert_eq!(a, RecoveryAction::Bootstrap);
}

/// Local pool lost, lineage alive remotely → DR rebaseline (V3.2 §15.7),
/// even with a torn/absent state file.
#[test]
fn local_loss_rebaselines_from_remote() {
    let s = state(Phase::Idle);
    let a = plan(Some(&s), &local(&[], &[]), &remote(ABSENT, true, true));
    assert_eq!(
        a,
        RecoveryAction::RebaselineFrom {
            remote_base: "snap/g3".into()
        }
    );
    // No state record at all: still recoverable, base unknown → engine must
    // list refs; planner signals with an empty base rather than guessing.
    let a = plan(None, &local(&[], &[]), &remote(ABSENT, true, true));
    assert_eq!(
        a,
        RecoveryAction::RebaselineFrom {
            remote_base: String::new()
        }
    );
}

/// Quarantine rows: contradictory observations must never be "resolved".
#[test]
fn contradictions_quarantine() {
    let two_candidates = plan(
        Some(&state(Phase::Backfilling)),
        &local(&["B_k"], &["S_n", "S_rogue"]),
        &remote(RemoteSnapStatus::Prepared { watermark: 1 }, true, true),
    );
    assert!(matches!(two_candidates, RecoveryAction::Quarantine(_)));

    let two_baselines = plan(
        Some(&state(Phase::Idle)),
        &local(&["B_k", "B_old"], &[]),
        &remote(ABSENT, true, true),
    );
    assert!(matches!(two_baselines, RecoveryAction::Quarantine(_)));

    // Baseline-pair invariant (§7.1) broken: local B_k without remote base.
    let no_remote_base = plan(
        Some(&state(Phase::Idle)),
        &local(&["B_k"], &[]),
        &remote(ABSENT, false, false),
    );
    assert!(matches!(no_remote_base, RecoveryAction::Quarantine(_)));

    // Publish-implies-pin violated.
    let unpinned = plan(
        Some(&state(Phase::Promoting)),
        &local(&["B_k"], &["S_n"]),
        &remote(RemoteSnapStatus::Visible { pinned: false }, true, true),
    );
    assert!(matches!(unpinned, RecoveryAction::Quarantine(_)));
}

/// Invariant F rows (task 15h): a fenced observation — the epoch-specific
/// `lease_lost` refusal was seen — is TERMINAL in every phase and under
/// every local/remote shape: the planner short-circuits to `Abandon`
/// before any other rule (quarantine guards included) can suggest a
/// retryable continuation.
#[test]
fn fenced_observation_abandons_in_every_phase() {
    let phases = [
        Phase::Idle,
        Phase::Capturing,
        Phase::Captured,
        Phase::Prepared,
        Phase::Backfilling,
        Phase::Publishable,
        Phase::Visible,
        Phase::Promoting,
        Phase::Advancing,
        Phase::Aborting,
    ];
    let locals = [
        local(&[], &[]),
        local(&["B_k"], &[]),
        local(&["B_k"], &["S_n"]),
        local(&["B_k", "B2"], &["S_n", "S2"]), // even contradictions
    ];
    let targets = [
        ABSENT,
        RemoteSnapStatus::Prepared { watermark: 7 },
        RemoteSnapStatus::Visible { pinned: true },
    ];
    for p in phases {
        for l in &locals {
            for t in targets {
                let mut r = remote(t, true, true);
                r.fenced = true;
                let a = plan(Some(&state(p)), l, &r);
                assert!(
                    matches!(a, RecoveryAction::Abandon { .. }),
                    "phase {p:?} × {l:?} × {t:?} must abandon when fenced, got {a:?}"
                );
                let a = plan(None, l, &r);
                assert!(matches!(a, RecoveryAction::Abandon { .. }), "{a:?}");
            }
        }
    }
}

/// The precision rule: only the epoch-specific refusal fences. A
/// transient/transport-shaped failure never sets `fenced`, so no
/// non-fenced observation may plan an `Abandon` — those keep their
/// retryable classification.
#[test]
fn transient_errors_never_abandon() {
    let locals = [
        local(&[], &[]),
        local(&["B_k"], &[]),
        local(&[], &["S_n"]),
        local(&["B_k"], &["S_n"]),
        local(&["S_n"], &[]),
    ];
    let targets = [
        ABSENT,
        RemoteSnapStatus::Prepared { watermark: 0 },
        RemoteSnapStatus::Visible { pinned: true },
        RemoteSnapStatus::Visible { pinned: false },
    ];
    let phases = [
        Phase::Idle,
        Phase::Capturing,
        Phase::Captured,
        Phase::Prepared,
        Phase::Backfilling,
        Phase::Publishable,
        Phase::Visible,
        Phase::Promoting,
        Phase::Advancing,
        Phase::Aborting,
    ];
    for l in &locals {
        for t in targets {
            for bv in [false, true] {
                for bp in [false, true] {
                    let r = remote(t, bv, bp);
                    assert!(!matches!(plan(None, l, &r), RecoveryAction::Abandon { .. }));
                    for p in phases {
                        assert!(
                            !matches!(plan(Some(&state(p)), l, &r), RecoveryAction::Abandon { .. }),
                            "un-fenced observation must never abandon (phase {p:?})"
                        );
                    }
                }
            }
        }
    }
}

/// Totality sweep: the planner returns (no panic) for every combination of
/// a coarse observation grid × phases × state presence.
#[test]
fn planner_is_total() {
    let locals = [
        local(&[], &[]),
        local(&["B_k"], &[]),
        local(&[], &["S_n"]),
        local(&["B_k"], &["S_n"]),
        local(&["B_k", "B2"], &["S_n", "S2"]),
        local(&["S_n"], &[]),
    ];
    let targets = [
        ABSENT,
        RemoteSnapStatus::Prepared { watermark: 0 },
        RemoteSnapStatus::Visible { pinned: true },
        RemoteSnapStatus::Visible { pinned: false },
    ];
    let phases = [
        Phase::Idle,
        Phase::Capturing,
        Phase::Captured,
        Phase::Prepared,
        Phase::Backfilling,
        Phase::Publishable,
        Phase::Visible,
        Phase::Promoting,
        Phase::Aborting,
    ];
    let mut n = 0;
    for l in &locals {
        for t in targets {
            for bv in [false, true] {
                for bp in [false, true] {
                    let r = remote(t, bv, bp);
                    let _ = plan(None, l, &r);
                    for p in phases {
                        let _ = plan(Some(&state(p)), l, &r);
                        n += 1;
                    }
                }
            }
        }
    }
    assert_eq!(n, locals.len() * targets.len() * 2 * 2 * phases.len());
}
