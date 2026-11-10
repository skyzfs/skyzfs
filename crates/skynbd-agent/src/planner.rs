//! Recovery planner: V3.2 §15.2 as a pure function (agent design doc §5).
//!
//! `plan(state, local, remote)` performs no I/O and reads no clock. The
//! *observations* are authoritative; the persisted state only refines the
//! decision (op cursor, phase hints). This is what makes the crash matrix
//! (V3.2 §21.1) an ordinary unit-test suite: every row is one call.

use crate::state::{AgentVolumeState, Phase};

/// What startup observation found on the local thin pool for one volume.
#[derive(Debug, Clone, Default)]
pub struct LocalObservation {
    /// Snapshot LVs tagged as this volume's baseline (`B_k`). Sound setups
    /// have zero (pre-bootstrap) or one.
    pub baselines: Vec<String>,
    /// Snapshot LVs tagged as this volume's candidate (`S_n`).
    pub candidates: Vec<String>,
}

/// Status of a remote snapshot ref, as reported by the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSnapStatus {
    Absent,
    /// Hidden prepared root; `op_watermark` is the durable idempotency
    /// watermark (server design §9A.6).
    Prepared {
        watermark: u64,
    },
    Visible {
        pinned: bool,
    },
}

/// What the control plane reports for this volume's lineage.
#[derive(Debug, Clone, Copy)]
pub struct RemoteObservation {
    /// The in-flight target (`P_n` / `G_n`), if the state file names one;
    /// `Absent` when nothing is prepared or visible under that id.
    pub target: RemoteSnapStatus,
    /// The recorded remote base (`G_k`): does it exist and hold our pin?
    pub base_visible: bool,
    pub base_pinned: bool,
    /// Invariant F (task 15h): the control plane answered a transaction
    /// with the epoch-specific `lease_lost` refusal — this stage's epoch
    /// is superseded. Set ONLY by that authenticated refusal, never by
    /// transport errors, 5xx or ordinary conflicts (those keep their
    /// retryable classification).
    pub fenced: bool,
}

/// What recovery should do next. One action per startup; the cycle engine
/// re-plans after executing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// No baseline anywhere: full-volume bootstrap (V3.2 §8.3).
    Bootstrap,
    /// Healthy for this phase; enter the cycle engine at `Phase`.
    Continue(Phase),
    /// `P_n` exists: recompute/replay ops strictly above the watermark
    /// (V3.2 §15.3 — always correct while `B_k` and `S_n` exist).
    ResumeBackfill { from_watermark: u64 },
    /// Crash in the publish window, target still hidden: publish is
    /// idempotent (I8) — re-issue, then promote.
    RepublishThenPromote,
    /// Target already visible+pinned, local promotion incomplete: run the
    /// §15.4 promotion order only.
    PromoteOnly,
    /// Published but the local candidate is gone (matrix row 8): keep any
    /// surviving baseline, re-establish the pair against `remote_base`
    /// without re-upload (sampled-audit verified).
    RebaselineFrom { remote_base: String },
    /// Finish an abort: delete `S_n` if present, `AbortSnapshot(P_n)` if
    /// prepared, keep `B_k`.
    Abort { delete_candidate: bool },
    /// Invariant F (task 15h): this stage's epoch is superseded — a
    /// terminal state, not a retryable error. The cycle abandons: no
    /// retry, no final cycle; local state is quarantined and torn down
    /// by the unstage path (D-M13's fenced branch). Retrying a fenced
    /// publish forever is the one outcome worse than losing an
    /// already-lost delta.
    Abandon { reason: &'static str },
    /// Contradictory observations; do nothing and page an operator.
    Quarantine(&'static str),
}

pub fn plan(
    state: Option<&AgentVolumeState>,
    local: &LocalObservation,
    remote: &RemoteObservation,
) -> RecoveryAction {
    // Invariant F short-circuit (task 15h): a superseded epoch is
    // terminal in EVERY phase — whatever else the observations say,
    // no transaction of this stage can land, so any continuation is a
    // retry loop against a fence. Abandon.
    if remote.fenced {
        return RecoveryAction::Abandon {
            reason: "stage epoch superseded (Invariant F): abandon, don't retry",
        };
    }
    // Contradiction guard first: more than one candidate or baseline LV can
    // only come from operator action or tag corruption. Never guess.
    if local.candidates.len() > 1 {
        return RecoveryAction::Quarantine("multiple candidate snapshot LVs");
    }
    if local.baselines.len() > 1 {
        return RecoveryAction::Quarantine("multiple baseline snapshot LVs");
    }
    let has_baseline = local.baselines.len() == 1;
    let has_candidate = local.candidates.len() == 1;

    // An explicitly persisted abort always finishes first (§15.6: abandon is
    // a client decision — once made, recovery completes it).
    if matches!(state.map(|s| s.phase), Some(Phase::Aborting)) {
        return RecoveryAction::Abort {
            delete_candidate: has_candidate,
        };
    }

    match remote.target {
        // Publish landed. Promotion state decides between rows 5/7/8.
        RemoteSnapStatus::Visible { pinned } => {
            if !pinned {
                // Publish-implies-pin (V3.2 §11.1): a visible-but-unpinned
                // target means someone released our pin already.
                return RecoveryAction::Quarantine("published target lost its publish pin");
            }
            if has_candidate {
                // Row 5 / row 7-first-half: S_n survives → record as B_n,
                // transfer pins, delete old B_k.
                RecoveryAction::PromoteOnly
            } else if let Some(target) = state.and_then(|s| s.prepared.as_ref()) {
                if has_baseline && local.baselines == [target_baseline_name(state)] {
                    // Row 9 (after old B_k deletion): the only local snapshot
                    // is already the promoted baseline — promotion finished.
                    RecoveryAction::Continue(Phase::Idle)
                } else {
                    // Row 8: S_n missing. Rebaseline against the published
                    // target without re-upload.
                    RecoveryAction::RebaselineFrom {
                        remote_base: target.target.clone(),
                    }
                }
            } else {
                // No record of the target but it is visible+pinned under our
                // holder: state file was lost mid-promotion.
                RecoveryAction::RebaselineFrom {
                    remote_base: String::new(),
                }
            }
        }

        // A prepared root exists server-side.
        RemoteSnapStatus::Prepared { watermark } => {
            if has_candidate && has_baseline {
                // Rows 3–4 and the pre-publish half of row 6: delta is
                // recomputable (§15.3); replay strictly above the watermark.
                RecoveryAction::ResumeBackfill {
                    from_watermark: watermark,
                }
            } else if has_candidate && !has_baseline {
                // Bootstrap backfill (base=None) — still resumable: the
                // "delta" is the full mapped set of S_0.
                RecoveryAction::ResumeBackfill {
                    from_watermark: watermark,
                }
            } else {
                // Prepared but S_n is gone: the delta is unrecomputable.
                // Abort the prepared root; baseline (if any) stays.
                RecoveryAction::Abort {
                    delete_candidate: false,
                }
            }
        }

        RemoteSnapStatus::Absent => {
            // Was a publish in flight? state says Publishable/Visible but the
            // target is absent remotely — the prepare never became durable or
            // was aborted; fall through on observations.
            if has_candidate && has_baseline {
                // Row 2: S_n exists, no P_n yet → continue by preparing.
                RecoveryAction::Continue(Phase::Captured)
            } else if has_candidate {
                // Candidate without any baseline: bootstrap capture already
                // done; prepare-from-None comes next.
                RecoveryAction::Continue(Phase::Captured)
            } else if has_baseline {
                // Row 1: quiescent. (If the remote base vanished while local
                // B_k survives, the pair invariant §7.1 is broken.)
                if remote.base_visible {
                    RecoveryAction::Continue(Phase::Idle)
                } else {
                    RecoveryAction::Quarantine("local baseline without remote base")
                }
            } else if remote.base_visible {
                // A rebaselined pair that is already established — the
                // record names a visible, pinned base and nothing is in
                // flight — is quiescent even with no local pair member:
                // the next capture re-establishes the local half. This
                // is the state a `RebaselineFrom` pass persists (what
                // makes re-planning converge) and the state a D-M11
                // fresh-pool stage seeds before the first cycle.
                let established = state.is_some_and(|s| {
                    s.baseline.is_some() && s.candidate.is_none() && s.prepared.is_none()
                });
                if established && remote.base_pinned {
                    RecoveryAction::Continue(Phase::Idle)
                } else {
                    // Local pool lost but the lineage lives remotely:
                    // rebaseline from the recorded base (V3.2 §15.7 DR
                    // path) — also the re-pin path when the pair is
                    // recorded but the base pin went missing.
                    RecoveryAction::RebaselineFrom {
                        remote_base: state
                            .and_then(|s| s.baseline.as_ref())
                            .map(|b| b.remote.clone())
                            .unwrap_or_default(),
                    }
                }
            } else {
                RecoveryAction::Bootstrap
            }
        }
    }
}

/// The LV name the candidate would carry after promotion, per the state
/// record; empty string when unknown (never matches a real LV name).
fn target_baseline_name(state: Option<&AgentVolumeState>) -> String {
    state
        .and_then(|s| s.candidate.as_ref())
        .map(|c| c.lv_name.clone())
        .unwrap_or_default()
}
