//! `SKYD1` diff/replication streams (design §9.4–§9.5).
//!
//! Wire form: a framed header, then length-prefixed postcard records,
//! ending with `End { records, payload_blake3 }`. A stream **applies
//! transactionally**: the receiver stages everything in the engine
//! overlay and commits exactly once after the trailer verifies — a torn
//! or corrupted stream changes nothing (scenario S7's kill-mid-stream
//! assertion).
//!
//! Diff semantics are mapping deltas (like `thin_delta`, V3.2 §10.3):
//! runs whose targets differ are emitted; equal-reading targets (explicit
//! zero vs hole) are skipped. Cost is O(extents) over the in-memory maps
//! of the M1 serving model; subtree-hash pruning starts paying off once
//! the engine reads nodes on demand (design §9.4 TODO).

use serde::{Deserialize, Serialize};
use skynbd_engine::{commit_extents, Engine, EngineError};
use skynbd_format::{magic, unframe, Codec, CommitKind};
use skynbd_refs::RefStore;
use skynbd_store::ObjectStore;
use skynbd_tree::ExtentMap;
use skynbd_types::{CommitHash, DataRef, Target, VolumeId};

/// Max payload bytes per DATA record (bounds apply-side buffering).
pub const MAX_DATA_RECORD: u64 = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("engine: {0}")]
    Engine(#[from] EngineError),
    #[error("format: {0}")]
    Format(#[from] skynbd_format::FormatError),
    #[error("malformed stream: {0}")]
    Malformed(String),
    #[error("stream truncated (no END trailer) — nothing was applied")]
    Torn,
    #[error("payload checksum mismatch — nothing was applied")]
    Checksum,
    #[error("target volume mismatch: {0}")]
    TargetMismatch(String),
}

pub type Result<T> = std::result::Result<T, StreamError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub vol_id: VolumeId,
    pub size: u64,
    /// `None` = full baseline stream (apply requires a fresh volume).
    pub from: Option<CommitHash>,
    pub to: CommitHash,
}

impl Codec for Header {
    const MAGIC: [u8; 4] = magic::DIFF_STREAM;
    const VERSION: u16 = 1;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rec {
    Data {
        off: u64,
        payload: Vec<u8>,
    },
    Zero {
        off: u64,
        len: u64,
    },
    Unmap {
        off: u64,
        len: u64,
    },
    End {
        records: u64,
        payload_blake3: [u8; 32],
    },
}

fn push_msg(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn next_msg<'a>(buf: &mut &'a [u8]) -> Result<Option<&'a [u8]>> {
    if buf.is_empty() {
        return Ok(None);
    }
    if buf.len() < 4 {
        return Err(StreamError::Torn);
    }
    let len = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    if buf.len() < 4 + len {
        return Err(StreamError::Torn);
    }
    let msg = &buf[4..4 + len];
    *buf = &buf[4 + len..];
    Ok(Some(msg))
}

// ------------------------------------------------------------------- diff

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Data,
    Zero,
    Unmap,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffStats {
    pub records: u64,
    pub data_bytes: u64,
}

/// Rebase a `(lba0, Target)` map value to position `at`.
fn rebased(v: &(u64, Target), at: u64) -> Target {
    let delta = at - v.0;
    match &v.1 {
        Target::Zero => Target::Zero,
        Target::Data(DataRef::Seg { segment, offset }) => Target::Data(DataRef::Seg {
            segment: *segment,
            offset: offset + delta,
        }),
        Target::Data(DataRef::Cas { hash, offset }) => Target::Data(DataRef::Cas {
            hash: *hash,
            offset: offset + delta,
        }),
    }
}

/// Does position `at` read identically under `a` and `b`?
fn same_reading(a: Option<&(u64, Target)>, b: Option<&(u64, Target)>, at: u64) -> bool {
    let zeroish =
        |v: Option<&(u64, Target)>| matches!(v.map(|v| rebased(v, at)), None | Some(Target::Zero));
    match (a, b) {
        (None, None) => true,
        _ if zeroish(a) && zeroish(b) => true,
        (Some(x), Some(y)) => rebased(x, at) == rebased(y, at),
        _ => false,
    }
}

/// Produce a `SKYD1` stream for `from → to` into `out`. `cfg` is the
/// reader's fetch tuning — pass the volume's [`EngineConfig::tuning`] so
/// a diff reads with the volume's widths and shares its GET permits.
///
/// [`EngineConfig::tuning`]: skynbd_engine::EngineConfig::tuning
pub async fn send<S, R>(
    store: S,
    refs: R,
    vol: VolumeId,
    from: Option<CommitHash>,
    to: CommitHash,
    cfg: skynbd_engine::EngineConfig,
    out: &mut Vec<u8>,
) -> Result<DiffStats>
where
    S: ObjectStore + Clone,
    R: RefStore + Clone,
{
    let reader = Engine::open_at_commit(store.clone(), refs, vol, to, cfg).await?;
    let size = reader.size();

    let mut a_map: ExtentMap<(u64, Target)> = ExtentMap::new();
    if let Some(f) = &from {
        for e in commit_extents(&store, vol, f).await? {
            a_map.insert(e.lba_start, e.len, (e.lba_start, e.target));
        }
    }
    let mut b_map: ExtentMap<(u64, Target)> = ExtentMap::new();
    for e in commit_extents(&store, vol, &to).await? {
        b_map.insert(e.lba_start, e.len, (e.lba_start, e.target));
    }

    // Boundary walk: every run edge from either map is a comparison edge.
    let mut edges: Vec<u64> = vec![0, size];
    for r in a_map.query(0, size) {
        edges.push(r.start);
    }
    for r in b_map.query(0, size) {
        edges.push(r.start);
    }
    edges.sort_unstable();
    edges.dedup();

    // Classify + merge adjacent same-kind runs.
    let mut runs: Vec<(u64, u64, RunKind)> = Vec::new();
    for w in edges.windows(2) {
        let (p, q) = (w[0], w[1]);
        if q <= p {
            continue;
        }
        let a = a_map
            .query(p, q - p)
            .into_iter()
            .next()
            .and_then(|r| r.value);
        let b = b_map
            .query(p, q - p)
            .into_iter()
            .next()
            .and_then(|r| r.value);
        if same_reading(a.as_ref(), b.as_ref(), p) {
            continue;
        }
        let kind = match b.as_ref().map(|v| rebased(v, p)) {
            Some(Target::Data(_)) => RunKind::Data,
            Some(Target::Zero) => RunKind::Zero,
            None => RunKind::Unmap,
        };
        match runs.last_mut() {
            Some((rs, rl, rk)) if *rk == kind && *rs + *rl == p => *rl += q - p,
            _ => runs.push((p, q - p, kind)),
        }
    }

    // Emit.
    let header = Header {
        vol_id: vol,
        size,
        from,
        to,
    };
    push_msg(out, &header.encode()?);
    let mut hasher = blake3::Hasher::new();
    let mut stats = DiffStats {
        records: 0,
        data_bytes: 0,
    };
    for (start, len, kind) in runs {
        match kind {
            RunKind::Data => {
                let mut off = start;
                let end = start + len;
                while off < end {
                    let chunk = (end - off).min(MAX_DATA_RECORD);
                    let payload = reader.read(off, chunk).await?;
                    hasher.update(&payload);
                    stats.data_bytes += chunk;
                    let rec = Rec::Data { off, payload };
                    push_msg(
                        out,
                        &postcard::to_allocvec(&rec)
                            .map_err(|e| StreamError::Malformed(e.to_string()))?,
                    );
                    stats.records += 1;
                    off += chunk;
                }
            }
            RunKind::Zero | RunKind::Unmap => {
                let rec = if kind == RunKind::Zero {
                    Rec::Zero { off: start, len }
                } else {
                    Rec::Unmap { off: start, len }
                };
                push_msg(
                    out,
                    &postcard::to_allocvec(&rec)
                        .map_err(|e| StreamError::Malformed(e.to_string()))?,
                );
                stats.records += 1;
            }
        }
    }
    let end = Rec::End {
        records: stats.records,
        payload_blake3: *hasher.finalize().as_bytes(),
    };
    push_msg(
        out,
        &postcard::to_allocvec(&end).map_err(|e| StreamError::Malformed(e.to_string()))?,
    );
    Ok(stats)
}

// ------------------------------------------------------------------ apply

/// Apply a `SKYD1` stream to `engine` transactionally: one `Import`
/// commit after the trailer verifies, or nothing at all. Returns the new
/// commit. The engine must be clean (no un-flushed writes to clobber).
///
/// Base checking (M5): a `from = None` baseline requires a never-written
/// volume. For incremental streams the caller guarantees the target
/// matches `from`'s *content* — commit hashes don't transfer across
/// volumes; the M6 prepared machinery does same-volume base cloning with
/// real hash checks (design §9A).
pub async fn apply<S, R>(engine: &mut Engine<S, R>, mut stream: &[u8]) -> Result<CommitHash>
where
    S: ObjectStore,
    R: RefStore,
{
    if !engine.is_clean() {
        return Err(StreamError::TargetMismatch(
            "un-flushed writes on target".into(),
        ));
    }
    let Some(hmsg) = next_msg(&mut stream)? else {
        return Err(StreamError::Torn);
    };
    let (_, body) = unframe(hmsg, magic::DIFF_STREAM, Header::VERSION)?;
    let header: Header =
        postcard::from_bytes(body).map_err(|e| StreamError::Malformed(e.to_string()))?;
    if header.size != engine.size() {
        return Err(StreamError::TargetMismatch(format!(
            "stream size {} != volume size {}",
            header.size,
            engine.size()
        )));
    }
    if header.from.is_none() && engine.flush_seq() != 0 {
        return Err(StreamError::TargetMismatch(
            "baseline stream requires a fresh volume".into(),
        ));
    }

    let result = async {
        let mut hasher = blake3::Hasher::new();
        let mut count = 0u64;
        loop {
            let Some(msg) = next_msg(&mut stream)? else {
                return Err(StreamError::Torn); // no END: torn
            };
            let rec: Rec =
                postcard::from_bytes(msg).map_err(|e| StreamError::Malformed(e.to_string()))?;
            match rec {
                Rec::Data { off, payload } => {
                    if payload.is_empty() || payload.len() as u64 > MAX_DATA_RECORD {
                        return Err(StreamError::Malformed("bad DATA payload size".into()));
                    }
                    hasher.update(&payload);
                    engine.write(off, &payload)?;
                    count += 1;
                }
                Rec::Zero { off, len } | Rec::Unmap { off, len } => {
                    engine.write_zeroes(off, len)?;
                    count += 1;
                }
                Rec::End {
                    records,
                    payload_blake3,
                } => {
                    if records != count {
                        return Err(StreamError::Malformed(format!(
                            "record count mismatch: trailer {records}, seen {count}"
                        )));
                    }
                    if payload_blake3 != *hasher.finalize().as_bytes() {
                        return Err(StreamError::Checksum);
                    }
                    if next_msg(&mut stream)?.is_some() {
                        return Err(StreamError::Malformed("data after END".into()));
                    }
                    return Ok(());
                }
            }
        }
    }
    .await;

    match result {
        Ok(()) => Ok(engine.commit(CommitKind::Import).await?),
        Err(e) => {
            engine.discard_uncommitted(); // torn stream changes nothing
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_framing_roundtrip_and_truncation() {
        let mut buf = Vec::new();
        push_msg(&mut buf, b"hello");
        push_msg(&mut buf, b"");
        let mut view = buf.as_slice();
        assert_eq!(next_msg(&mut view).unwrap(), Some(&b"hello"[..]));
        assert_eq!(next_msg(&mut view).unwrap(), Some(&b""[..]));
        assert_eq!(next_msg(&mut view).unwrap(), None);

        let mut torn = &buf[..buf.len() - 1];
        next_msg(&mut torn).unwrap(); // first ok
        assert!(matches!(next_msg(&mut torn), Err(StreamError::Torn)));
    }
}
