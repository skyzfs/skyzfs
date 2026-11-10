//! Retention thinning (design D-M15 §6.2, task 15f): the pass that turns
//! "delete actually frees bytes" into a property the system maintains by
//! itself instead of a curl someone remembers.
//!
//! The policy in one line: keep every publish for a short
//! restore-granularity window (`publishRetentionWindow`), then only
//! configured snapshots; batch each pass's deletions into ONE refs
//! transaction to keep CAS churn down. Three keep-rules sit above the
//! window and are checked first, because each encodes an invariant that
//! age must never override:
//!
//! - **The newest generation is never a candidate.** It is the volume's
//!   current content and the pointer's restage fallback (D-M15: an
//!   absent or stale pointer resolves to the newest generation —
//!   thinning it would resurrect old data on the next fresh stage).
//! - **A pinned generation is never a candidate.** Pins are the
//!   system's own liveness protocol — the `origin` holder moves with
//!   the pointer row (Invariant D), baseline holders track the node's
//!   retained snapshot, prepared sessions pin their base. The pass
//!   trusts the protocol rather than re-deriving each holder's reason.
//! - **An aliased generation is never a candidate.** A CSI
//!   `CreateSnapshot` gives the generation a second `snap/` name; the
//!   alias is the user's statement that this one is a configured
//!   snapshot on the retention track (15f's promotion op makes it
//!   emap-carrying too — PR2). Either ref keeps the closure alive for
//!   GC, but the generation ref itself also stays: generation numbers
//!   are the monotone order Invariant E compares, and the origin
//!   fallback resolves through them, so a kept snapshot keeps its
//!   generation identity.
//!
//! The same pass sweeps **fenced-abandoned prepared sessions** (#75
//! review C7): a zombie coordinator's `prepared/<id>` ref, its base
//! pin, and its `pending_cas` GC roots (§10.2 resurrection rule) all
//! survive abandonment — nothing else ever collects them, and the
//! pending-CAS roots actively hold garbage alive. Liveness is judged by
//! the session's last durable activity: the newer of the ULID's own
//! timestamp and the hidden ref's commit time (each checkpoint writes a
//! fresh commit), against a deliberately generous TTL — a slow backfill
//! is normal, an untouched session from last week is not.
//!
//! The planner only reads; the single transaction the caller executes
//! carries the authority — the server's own for volumes it may act on,
//! the node's fence epoch (15h `transact_as`) when the agent drives the
//! pass for a lineage it holds. Every deleting `CasRef` names the exact
//! `RefValue` the plan saw, so a ref that moved between plan and
//! execute aborts the whole batch (`Conflict`) instead of deleting a
//! generation the plan never judged.

use std::collections::HashSet;
use std::sync::Arc;

use serde::Serialize;
use skynbd_engine::keys;
use skynbd_format::{Codec, Commit};
use skynbd_refs::{RefOp, RefStore, RefValue};
use skynbd_store::ObjectStore;
use skynbd_types::origin::generation_number;
use skynbd_types::VolumeId;

/// Dials for one pass. Callers fill from their config surface (daemon
/// `[retention]`, agent env) — defaults live there, not here.
#[derive(Debug, Clone, Copy)]
pub struct ThinPolicy {
    /// Publishes younger than this are kept regardless (restore
    /// granularity — §6.2's `publishRetentionWindow`).
    pub publish_window_ms: u64,
    /// A prepared session with no durable activity for this long is
    /// fenced-abandoned garbage.
    pub prepared_ttl_ms: u64,
}

/// What one pass decided, and why — the report is the interface that
/// keeps the pass auditable (counts alone rot into "trust me").
#[derive(Debug, Default, Serialize)]
pub struct ThinPlan {
    /// `snap/g<N>` refs the transaction deletes.
    pub delete_generations: Vec<String>,
    /// Abandoned prepared-session ids the transaction collects.
    pub sweep_sessions: Vec<String>,
    /// Kept: commit also carried by a non-generation `snap/` ref.
    pub kept_configured: usize,
    /// Kept: pin holders present.
    pub kept_pinned: usize,
    /// Kept: younger than the window (or age unreadable — keep is the
    /// fail-safe direction).
    pub kept_window: usize,
    #[serde(skip)]
    pub ops: Vec<RefOp>,
}

impl ThinPlan {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// Build one pass's plan. Read-only: refs lists/pins and commit-object
/// GETs for ages; the caller owns executing `plan.ops` as ONE
/// transaction under the right authority.
pub async fn plan(
    store: &Arc<dyn ObjectStore>,
    refs: &Arc<dyn RefStore>,
    vol: VolumeId,
    policy: ThinPolicy,
    now_ms: u64,
) -> Result<ThinPlan, skynbd_engine::EngineError> {
    let all = refs.list(vol).await?;
    let mut plan = ThinPlan::default();

    // Commits carried by non-generation snap refs (aliases / manual
    // snapshots): these mark their generation "configured".
    let aliased: HashSet<_> = all
        .iter()
        .filter(|(n, _)| n.starts_with("snap/") && generation_number(n).is_none())
        .map(|(_, rv)| rv.commit)
        .collect();

    let mut generations: Vec<(u64, &String, &RefValue)> = all
        .iter()
        .filter_map(|(n, rv)| generation_number(n).map(|g| (g, n, rv)))
        .collect();
    generations.sort_by_key(|(g, _, _)| *g);
    let newest = generations.last().map(|(g, _, _)| *g);

    for (g, name, rv) in &generations {
        if Some(*g) == newest {
            // The restage fallback — never a candidate, not even counted.
            continue;
        }
        if aliased.contains(&rv.commit) {
            plan.kept_configured += 1;
            continue;
        }
        if !refs.pins(vol, name).await?.is_empty() {
            plan.kept_pinned += 1;
            continue;
        }
        // Age from the commit object itself (refs carry no clock). An
        // unreadable commit keeps the ref: deleting on missing evidence
        // is the one direction this pass must never take.
        let age_ok = match store.get(&keys::commit(vol, &rv.commit), None).await {
            Ok(bytes) => match Commit::decode(&bytes) {
                Ok(c) => now_ms.saturating_sub(c.created_at_ms) >= policy.publish_window_ms,
                Err(_) => false,
            },
            Err(_) => false,
        };
        if !age_ok {
            plan.kept_window += 1;
            continue;
        }
        plan.ops.push(RefOp::CasRef {
            name: (*name).clone(),
            expected: Some(*(*rv)),
            new_commit: None,
        });
        plan.delete_generations.push((*name).clone());
    }

    // Fenced-abandoned prepared sessions (#75 C7). Last durable
    // activity = the newer of the session id's ULID timestamp and its
    // hidden ref's commit time (checkpoints write fresh commits).
    for (key, bytes) in refs.meta_list(vol, "prepared/").await? {
        let Some(id) = key.strip_prefix("prepared/") else {
            continue;
        };
        let Ok(meta) = postcard::from_bytes::<skynbd_engine::prepared::PreparedMeta>(&bytes) else {
            continue;
        };
        let mut last_ms = ulid::Ulid::from_string(id)
            .map(|u| u.timestamp_ms())
            .unwrap_or(0);
        let hidden = refs.get(vol, &key).await?;
        if let Some(rv) = &hidden {
            // Same fail-safe direction as the generation rule: the
            // hidden ref names a commit we cannot read, so the
            // session's last activity is invisible this pass — skip it
            // rather than judge it on the older ULID bound. (The
            // CAS-exact delete cannot protect a live session here: its
            // ref hasn't moved, the evidence read failed.) A zombie
            // skipped once costs a week of one session's garbage; a
            // live session swept costs a publish.
            let Ok(cb) = store.get(&keys::commit(vol, &rv.commit), None).await else {
                continue;
            };
            let Ok(c) = Commit::decode(&cb) else {
                continue;
            };
            last_ms = last_ms.max(c.created_at_ms);
        }
        if last_ms == 0 || now_ms.saturating_sub(last_ms) < policy.prepared_ttl_ms {
            continue;
        }
        if let Some(rv) = hidden {
            plan.ops.push(RefOp::CasRef {
                name: key.clone(),
                expected: Some(rv),
                new_commit: None,
            });
        }
        if let Some(base_ref) = &meta.base_ref {
            plan.ops.push(RefOp::Unpin {
                name: base_ref.clone(),
                holder: format!("prepared:{id}"),
            });
        }
        plan.ops.push(RefOp::MetaDelete { key: key.clone() });
        plan.sweep_sessions.push(id.to_string());
    }

    Ok(plan)
}

/// The "sweep owed" marker: set (inside the thinning transaction) when
/// refs were deleted but the actor could not run GC — a node-held
/// lineage's agent-driven pass, or a dirty engine. Cleared by the next
/// successful GC. The marker is what keeps "thinned but never swept"
/// from becoming an invisible steady state.
pub const GC_OWED_KEY: &str = "gc/owed";

/// The value is a fresh nonce per marker: a debt recorded while a
/// sweep is already mid-flight was not seen by that sweep's mark
/// phase, so the clear is guarded on the exact value read before the
/// sweep began — a marker that changed underneath survives.
pub fn owed_marker() -> RefOp {
    RefOp::MetaPut {
        key: GC_OWED_KEY.into(),
        value: ulid::Ulid::new().to_bytes().to_vec(),
    }
}
