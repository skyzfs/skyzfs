//! Prepared snapshots — the V3.2 ingestion control plane, server side
//! (design §9A, implementing V3.2 §11.1).
//!
//! A prepared snapshot `P_n` is a hidden ref `prepared/{snap_id}` whose
//! commits are built by the ordinary engine (design D12): attach/clear
//! ops stage in the overlay and checkpoint through `commit()` every
//! [`CHECKPOINT_EVERY_OPS`] batches. Ops above the durable watermark are
//! lost on a crash and replayed idempotently by the coordinator
//! (V3.2 §11.5, §15.2).
//!
//! Pin lifecycle (design §9A.4, I8): prepare pins the base in the same
//! ref transaction that creates the prepared ref; publish creates the
//! visible ref, pins it for the publisher, deletes the prepared ref, and
//! unpins the base — all in one transaction. There is no observable
//! visible-but-unpinned state.

use serde::{Deserialize, Serialize};
use skynbd_format::{Codec, Commit, CommitKind, Node, SnapKind, SnapshotManifest};
use skynbd_refs::{RefOp, RefStore, TxnOutcome};
use skynbd_store::ObjectStore;
use skynbd_types::{CommitHash, SnapId, VolumeId};

use crate::{keys, now_ms, Engine, EngineConfig, EngineError, Result};

/// Checkpoint cadence: overlay folds into a durable `Prepared` commit
/// after this many applied op-batches (bounds replay after a crash).
pub const CHECKPOINT_EVERY_OPS: u32 = 32;

/// One op of a mixed [`PreparedSession::apply`] batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeOp {
    /// `AttachRange`: `len` bytes at `off` map to `obj_off` in CAS `hash`.
    Attach {
        off: u64,
        len: u64,
        hash: [u8; 32],
        obj_off: u64,
    },
    /// `ClearRange`: an explicit zero extent over `[off, off+len)`.
    Clear { off: u64, len: u64 },
}

/// Durable per-prepared-snapshot state (the design §6.1 `prepared` row),
/// stored in the ref store's meta table, postcard-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedMeta {
    pub base: Option<CommitHash>,
    /// Base ref name pinned for this session (e.g. `snap/g0`).
    pub base_ref: Option<String>,
    /// Highest op id folded into a durable checkpoint.
    pub op_watermark: u64,
    /// EnsureObject promises: CAS hashes GC must keep alive (§10.2).
    pub pending_cas: Vec<[u8; 32]>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PreparedStatus {
    pub snap_id: String,
    pub state: String,
    pub base_ref: Option<String>,
    pub op_watermark: u64,
    pub last_commit: String,
}

fn meta_key(snap_id: SnapId) -> String {
    format!("prepared/{snap_id}")
}

fn holder(snap_id: SnapId) -> String {
    format!("prepared:{snap_id}")
}

pub struct PreparedSession<S, R> {
    pub snap_id: SnapId,
    engine: Engine<S, R>,
    meta: PreparedMeta,
    /// Highest op id applied in memory (≥ durable watermark).
    applied_watermark: u64,
    batches_since_checkpoint: u32,
}

impl<S: ObjectStore + Clone, R: RefStore + Clone> PreparedSession<S, R> {
    /// `PrepareSnapshotFromBase` (design §9A.2): create the hidden ref at
    /// the base commit (or a fresh empty commit for bootstrap), pin the
    /// base, and persist the meta row — one transaction (I8).
    pub async fn create(
        store: S,
        refs: R,
        vol: VolumeId,
        snap_id: SnapId,
        base: Option<(String, CommitHash)>, // (snap ref name, commit)
    ) -> Result<Self> {
        let ref_name = format!("prepared/{snap_id}");
        if refs.get(vol, &ref_name).await?.is_some() {
            return Err(EngineError::SnapshotExists(format!("prepared/{snap_id}")));
        }

        let base_commit = match &base {
            Some((_, c)) => *c,
            None => {
                // Bootstrap: an empty mapping (V3.2 §8.3 `P_0`).
                let sb_bytes = store.get(&keys::superblock(vol, 0), None).await?;
                let sb = skynbd_format::Superblock::decode(&sb_bytes)?;
                // The volume's geometry, not this process's config: the
                // shard index is positional, so a bootstrap built on the
                // wrong grid would be read back on a different one.
                let shard_count = sb.size.div_ceil(sb.shard_bytes) as usize;
                let index = Node::ShardIndex(vec![None; shard_count]);
                let index_hash = index.content_hash()?;
                store
                    .put(&keys::node(vol, &index_hash), index.encode()?.into())
                    .await?;
                let commit = Commit {
                    vol_id: vol,
                    parent: None,
                    shard_index: index_hash,
                    segments: vec![],
                    flush_seq: 0,
                    created_at_ms: now_ms(),
                    kind: CommitKind::Prepared,
                    group: None,
                    // Ingest bootstrap is unowned by design: hidden refs
                    // never compete for head, and epoch 0 keeps it that
                    // way under the D-M5 recovery ordering too.
                    epoch: 0,
                };
                let hash = commit.content_hash()?;
                store
                    .put(&keys::commit(vol, &hash), commit.encode()?.into())
                    .await?;
                hash
            }
        };

        let meta = PreparedMeta {
            base: base.as_ref().map(|(_, c)| *c),
            base_ref: base.as_ref().map(|(n, _)| n.clone()),
            op_watermark: 0,
            pending_cas: vec![],
        };
        let mut ops = vec![
            RefOp::CasRef {
                name: ref_name.clone(),
                expected: None,
                new_commit: Some(base_commit),
            },
            RefOp::MetaPut {
                key: meta_key(snap_id),
                value: postcard::to_allocvec(&meta)
                    .map_err(|e| EngineError::Corrupt(e.to_string()))?,
            },
        ];
        if let Some((base_ref, _)) = &base {
            ops.push(RefOp::Pin {
                name: base_ref.clone(),
                holder: holder(snap_id),
            });
        }
        match refs.transact(vol, ops).await? {
            TxnOutcome::Committed => {}
            TxnOutcome::Conflict { .. } => {
                return Err(EngineError::SnapshotExists(format!("prepared/{snap_id}")));
            }
        }

        let engine = Engine::open_ref(store, refs, vol, &ref_name, EngineConfig::default()).await?;
        Ok(Self {
            snap_id,
            engine,
            applied_watermark: meta.op_watermark,
            meta,
            batches_since_checkpoint: 0,
        })
    }

    /// Reopen after a restart (S13 resume): ref + meta row are the truth.
    pub async fn open_existing(store: S, refs: R, vol: VolumeId, snap_id: SnapId) -> Result<Self> {
        let Some(bytes) = refs.meta_get(vol, &meta_key(snap_id)).await? else {
            return Err(EngineError::PreparedNotFound(snap_id.to_string()));
        };
        let meta: PreparedMeta =
            postcard::from_bytes(&bytes).map_err(|e| EngineError::Corrupt(e.to_string()))?;
        let ref_name = format!("prepared/{snap_id}");
        let engine = Engine::open_ref(store, refs, vol, &ref_name, EngineConfig::default()).await?;
        Ok(Self {
            snap_id,
            engine,
            applied_watermark: meta.op_watermark,
            meta,
            batches_since_checkpoint: 0,
        })
    }

    /// Fan-out for [`PreparedSession::ensure`]'s existence checks
    /// ([`EngineConfig::walk_concurrency`]; 0 is treated as 1). Only the
    /// tuning knob is taken from the serving volume's config — the
    /// session's engine keeps its own defaults otherwise (epoch 0).
    pub fn with_walk_concurrency(mut self, n: usize) -> Self {
        // One width: the resolver's is what walks use; the config field
        // follows it so the two never disagree.
        self.engine.resolver.walk = n.clamp(1, crate::MAX_FANOUT);
        self.engine.cfg.walk_concurrency = self.engine.resolver.walk;
        self
    }

    pub fn status(&self) -> PreparedStatus {
        PreparedStatus {
            snap_id: self.snap_id.to_string(),
            state: if self.meta.op_watermark == 0 && self.applied_watermark == 0 {
                "prepared".into()
            } else {
                "backfilling".into()
            },
            base_ref: self.meta.base_ref.clone(),
            op_watermark: self.meta.op_watermark,
            last_commit: hex(&self.engine.head_commit_hash()),
        }
    }

    async fn persist_meta(&mut self) -> Result<()> {
        self.persist(&self.meta).await
    }

    /// Write `meta` as this session's durable row (without adopting it).
    async fn persist(&self, meta: &PreparedMeta) -> Result<()> {
        self.engine
            .refs()
            .transact(
                self.engine.vol_id(),
                vec![RefOp::MetaPut {
                    key: meta_key(self.snap_id),
                    value: postcard::to_allocvec(meta)
                        .map_err(|e| EngineError::Corrupt(e.to_string()))?,
                }],
            )
            .await?;
        Ok(())
    }

    async fn maybe_checkpoint(&mut self, op_id: u64) -> Result<()> {
        self.batches_since_checkpoint += 1;
        if self.batches_since_checkpoint >= CHECKPOINT_EVERY_OPS {
            self.checkpoint(op_id).await?;
        }
        Ok(())
    }

    /// Fold staged ops into a durable `Prepared` commit and advance the
    /// durable watermark (design D12).
    pub async fn checkpoint(&mut self, op_id: u64) -> Result<()> {
        self.engine.commit(CommitKind::Prepared).await?;
        self.meta.op_watermark = op_id;
        self.persist_meta().await?;
        self.batches_since_checkpoint = 0;
        Ok(())
    }

    /// `AttachRange` batch (design §9A.2). Ops at or below the durable
    /// watermark are replay no-ops; last-writer-wins within the root.
    pub async fn attach(
        &mut self,
        ranges: &[(u64, u64, [u8; 32], u64)], // (off, len, hash, obj_offset)
        op_id: u64,
    ) -> Result<bool> {
        let ops: Vec<RangeOp> = ranges
            .iter()
            .map(|&(off, len, hash, obj_off)| RangeOp::Attach {
                off,
                len,
                hash,
                obj_off,
            })
            .collect();
        self.apply(&ops, op_id).await
    }

    /// `ClearRange` batch (design §9A.3): explicit zero extents override
    /// the cloned base — never hole-punching.
    pub async fn clear(&mut self, ranges: &[(u64, u64)], op_id: u64) -> Result<bool> {
        let ops: Vec<RangeOp> = ranges
            .iter()
            .map(|&(off, len)| RangeOp::Clear { off, len })
            .collect();
        self.apply(&ops, op_id).await
    }

    /// A mixed attach/clear batch under ONE op id (issue #97 follow-up):
    /// what lets the agent send a batch of interleaved writes and
    /// discards as a single call instead of one per same-kind run. Ops
    /// stage in order (last-writer-wins, as across separate calls); the
    /// batch is replay-skipped, staged and counted toward the checkpoint
    /// cadence as a unit, exactly like an `attach` or `clear` batch.
    ///
    /// Staging is all-or-nothing against the watermark: every op is
    /// validated before any lands, so a refused batch leaves the overlay
    /// as it was and cannot be half-covered by a later checkpoint.
    pub async fn apply(&mut self, ops: &[RangeOp], op_id: u64) -> Result<bool> {
        if op_id <= self.meta.op_watermark {
            return Ok(false); // durable replay: skip
        }
        for op in ops {
            let (RangeOp::Attach { off, len, .. } | RangeOp::Clear { off, len }) = *op;
            self.engine.check_io(off, len)?;
        }
        for op in ops {
            match *op {
                RangeOp::Attach {
                    off,
                    len,
                    hash,
                    obj_off,
                } => self.engine.attach_cas(off, len, hash, obj_off)?,
                RangeOp::Clear { off, len } => self.engine.write_zeroes(off, len)?,
            }
        }
        self.applied_watermark = self.applied_watermark.max(op_id);
        self.maybe_checkpoint(op_id).await?;
        Ok(true)
    }

    /// `EnsureObject` (design §9A.2 + §10.2): a positive answer records
    /// the hash in the pending set (a GC root) *before* replying, so the
    /// promise survives any sweep.
    ///
    /// The existence checks of a batch run concurrently (issue #97: the
    /// agent's pipelined backfill sends a batch per `ensure`, and serial
    /// HEADs would just move its round-trip chain server-side). Order is
    /// preserved, and the promises are recorded after all checks return.
    pub async fn ensure(&mut self, hashes: &[[u8; 32]]) -> Result<Vec<bool>> {
        use futures_util::stream::{StreamExt, TryStreamExt};
        let (store, vol) = (self.engine.store(), self.engine.vol_id());
        // Keys are built up front so the closure takes owned values: one
        // borrowing a hash makes the future's `Send` bound higher-ranked,
        // which the API's handlers cannot prove (see `fetch_nodes`).
        let keys: Vec<String> = hashes.iter().map(|h| keys::cas_object(vol, h)).collect();
        let out: Vec<bool> = futures_util::stream::iter(
            keys.into_iter()
                .map(|key| async move { store.exists(&key).await }),
        )
        .buffered(self.engine.walk_concurrency())
        .try_collect()
        .await?;
        // The promises go durable BEFORE they count in memory. Were the
        // in-memory set updated first, a persist that fails — or is
        // cancelled, which an agent dropping its overlapped staging makes
        // routine (issue #97) — would leave a hash that every later
        // `ensure` finds "already promised", answers positive for and
        // never persists, while GC roots only the durable row: the agent
        // skips the upload, attaches, and a sweep before the next
        // checkpoint deletes the object under a publish.
        let mut meta = self.meta.clone();
        for (h, &exists) in hashes.iter().zip(&out) {
            if exists && !meta.pending_cas.contains(h) {
                meta.pending_cas.push(*h);
            }
        }
        if meta.pending_cas.len() != self.meta.pending_cas.len() {
            self.persist(&meta).await?; // promise durable before the reply
            self.meta = meta;
        }
        Ok(out)
    }

    /// `PublishSnapshot` (design §9A.2): final checkpoint, manifest PUT,
    /// then one transaction: visible ref + publisher pin + prepared ref
    /// delete + base unpin + meta delete (I8, I9).
    pub async fn publish(&mut self, name: &str, publisher: &str) -> Result<CommitHash> {
        self.publish_fenced(name, publisher, None).await
    }

    /// [`Self::publish`] with an optional Invariant F fence (task 15h):
    /// when `fence = Some((holder, epoch))`, the I8 transaction itself
    /// runs through the fenced `transact_as` — atomically with the
    /// publish, the volume's lease row must still be exactly that
    /// `(holder, epoch)`, or nothing lands and the actual owner comes
    /// back as [`EngineError::Fenced`].
    pub async fn publish_fenced(
        &mut self,
        name: &str,
        publisher: &str,
        fence: Option<(String, u64)>,
    ) -> Result<CommitHash> {
        crate::validate_snapshot_name(name)?;
        self.publish_ref(format!("snap/{name}"), Some(publisher), fence)
            .await
    }

    /// The §8.2 salvage branch (task 15h): publish a *fenced* session's
    /// delta as a named fork outside the lineage —
    /// `salvage/<name>-e<request_epoch>` — executed under the **current**
    /// lease's `(holder, epoch)` (the server re-stamps at acceptance).
    /// Write-once (an existing salvage ref conflicts), takes **no**
    /// lineage pins (the ref itself keeps the closure alive, and no
    /// resolution path ever adopts `salvage/*`), and writes nothing
    /// beyond the manifest — whose `ref_name` is the salvage ref, so GC
    /// keeps it exactly as long as the ref lives. The ref name is
    /// server-constructed: `validate_snapshot_name` still guards `name`,
    /// and the `salvage/` prefix never passes the public validator.
    pub async fn publish_salvage(
        &mut self,
        name: &str,
        request_epoch: u64,
        current: (String, u64),
    ) -> Result<CommitHash> {
        crate::validate_snapshot_name(name)?;
        self.publish_ref(
            format!("salvage/{name}-e{request_epoch}"),
            None,
            Some(current),
        )
        .await
    }

    /// Shared publish tail: final checkpoint, manifest PUT, one I8
    /// transaction (fenced when a principal is given). `publisher: None`
    /// skips the publish pin — the salvage shape.
    async fn publish_ref(
        &mut self,
        visible_ref: String,
        publisher: Option<&str>,
        fence: Option<(String, u64)>,
    ) -> Result<CommitHash> {
        let vol = self.engine.vol_id();
        if self.engine.refs().get(vol, &visible_ref).await?.is_some() {
            return Err(EngineError::SnapshotExists(visible_ref));
        }
        // The published commit stays at epoch 0 even under a fence,
        // deliberately: prepared-session commits must never compete for
        // head, and `recover_head`'s epoch-equality scan would adopt a
        // session commit stamped with the lease's live epoch as the
        // volume's head. The fence's authority lives in the transact
        // below, not in the commit stamp; a fenced-out session's commits
        // are unreferenced garbage either way.
        let commit_hash = self.engine.commit(CommitKind::Prepared).await?;

        let manifest = SnapshotManifest {
            snap_id: self.snap_id,
            vol_id: vol,
            ref_name: visible_ref.clone(),
            commit: commit_hash,
            parent_snap: None, // lineage: base manifest linkage is post-MVP
            kind: SnapKind::Imported,
            created_at_ms: now_ms(),
        };
        self.engine
            .store()
            .put(
                &keys::snapshot(vol, self.snap_id),
                manifest.encode()?.into(),
            )
            .await?;

        let prepared_ref = format!("prepared/{}", self.snap_id);
        let prepared_cur = self.engine.refs().get(vol, &prepared_ref).await?;
        let mut ops = vec![RefOp::CasRef {
            name: visible_ref.clone(),
            expected: None,
            new_commit: Some(commit_hash),
        }];
        if let Some(publisher) = publisher {
            ops.push(RefOp::Pin {
                name: visible_ref.clone(),
                holder: publisher.to_string(),
            });
        }
        ops.push(RefOp::CasRef {
            name: prepared_ref,
            expected: prepared_cur,
            new_commit: None,
        });
        ops.push(RefOp::MetaDelete {
            key: meta_key(self.snap_id),
        });
        if let Some(base_ref) = &self.meta.base_ref {
            ops.push(RefOp::Unpin {
                name: base_ref.clone(),
                holder: holder(self.snap_id),
            });
        }
        let outcome = match &fence {
            None => self.engine.refs().transact(vol, ops).await?,
            Some((h, e)) => match self.engine.refs().transact_as(vol, ops, h, *e).await {
                Ok(out) => out,
                Err(skynbd_refs::RefError::LeaseLost { holder, epoch, .. }) => {
                    return Err(EngineError::Fenced { holder, epoch })
                }
                Err(other) => return Err(other.into()),
            },
        };
        match outcome {
            TxnOutcome::Committed => Ok(commit_hash),
            TxnOutcome::Conflict { op_index: 0, .. } => Err(EngineError::SnapshotExists(
                visible_ref
                    .strip_prefix("snap/")
                    .unwrap_or(&visible_ref)
                    .to_string(),
            )),
            TxnOutcome::Conflict { .. } => Err(EngineError::Conflict),
        }
    }

    /// `AbortSnapshot` (design §9A.2): drop the hidden ref, unpin the
    /// base, delete the meta row — visible state untouched (I9). The
    /// accumulated commits become epoch-aged garbage for GC.
    pub async fn abort(self) -> Result<()> {
        let vol = self.engine.vol_id();
        let prepared_ref = format!("prepared/{}", self.snap_id);
        let cur = self.engine.refs().get(vol, &prepared_ref).await?;
        let mut ops = vec![
            RefOp::CasRef {
                name: prepared_ref,
                expected: cur,
                new_commit: None,
            },
            RefOp::MetaDelete {
                key: meta_key(self.snap_id),
            },
        ];
        if let Some(base_ref) = &self.meta.base_ref {
            ops.push(RefOp::Unpin {
                name: base_ref.clone(),
                holder: holder(self.snap_id),
            });
        }
        match self.engine.refs().transact(vol, ops).await? {
            TxnOutcome::Committed => Ok(()),
            TxnOutcome::Conflict { .. } => Err(EngineError::Conflict),
        }
    }
}

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// List durable prepared-session ids for a volume (daemon restart sweep).
pub async fn list_prepared<R: RefStore>(refs: &R, vol: VolumeId) -> Result<Vec<SnapId>> {
    let mut out = Vec::new();
    for (key, _) in refs.meta_list(vol, "prepared/").await? {
        if let Some(id) = key.strip_prefix("prepared/") {
            if let Ok(sid) = SnapId::from_string(id) {
                out.push(sid);
            }
        }
    }
    Ok(out)
}
