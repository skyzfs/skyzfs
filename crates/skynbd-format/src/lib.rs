//! Framed, versioned codecs for every S3 metadata object (design §5.2).
//!
//! All metadata objects are postcard-serialized structs wrapped in an
//! 8-byte frame: `magic (4B ASCII) | version (u16 LE) | flags (u16 LE)`.
//! Unknown major versions are rejected. CAS data objects are **unframed**
//! raw payloads (their S3 key is `blake3(payload)`); segments carry the
//! `SSEG` magic but a raw-bytes body.
//!
//! TDD note: every new object kind lands here as (1) a struct, (2) a
//! `Codec` impl with magic + version, (3) roundtrip + rejection tests.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use skynbd_types::{CommitHash, Extent, NodeHash, ObjectHash, SegmentId, SnapId, Target, VolumeId};

pub const FRAME_LEN: usize = 8;

/// Object kinds and their magics (design §5.2).
pub mod magic {
    pub const SEGMENT: [u8; 4] = *b"SSEG";
    pub const NODE: [u8; 4] = *b"SNOD";
    pub const COMMIT: [u8; 4] = *b"SCMT";
    pub const SUPERBLOCK: [u8; 4] = *b"SSUP";
    pub const SNAPSHOT: [u8; 4] = *b"SSNP";
    pub const DIFF_STREAM: [u8; 4] = *b"SKYD";
    pub const CLONE_BASE: [u8; 4] = *b"SCLN";
    pub const REFS: [u8; 4] = *b"SREF";
    pub const EMAP: [u8; 4] = *b"SEMP";
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("object too short for frame")]
    Truncated,
    #[error("bad magic: expected {expected:?}, got {got:?}")]
    BadMagic { expected: [u8; 4], got: [u8; 4] },
    #[error("unsupported version {got} (max supported {supported})")]
    UnsupportedVersion { got: u16, supported: u16 },
    #[error("body decode failed: {0}")]
    Body(String),
    #[error("body encode failed: {0}")]
    Encode(String),
}

/// Write `magic | version | flags | body` into one buffer.
pub fn frame(magic: [u8; 4], version: u16, flags: u16, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_LEN + body.len());
    out.extend_from_slice(&magic);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Parse a frame, validating magic and version, returning `(flags, body)`.
pub fn unframe(
    bytes: &[u8],
    expected_magic: [u8; 4],
    max_version: u16,
) -> Result<(u16, &[u8]), FormatError> {
    if bytes.len() < FRAME_LEN {
        return Err(FormatError::Truncated);
    }
    let got: [u8; 4] = bytes[0..4].try_into().unwrap();
    if got != expected_magic {
        return Err(FormatError::BadMagic {
            expected: expected_magic,
            got,
        });
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version > max_version {
        return Err(FormatError::UnsupportedVersion {
            got: version,
            supported: max_version,
        });
    }
    let flags = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    Ok((flags, &bytes[FRAME_LEN..]))
}

/// A framed, postcard-encoded object kind.
pub trait Codec: Serialize + DeserializeOwned {
    const MAGIC: [u8; 4];
    const VERSION: u16;

    fn encode(&self) -> Result<Vec<u8>, FormatError> {
        let body = postcard::to_allocvec(self).map_err(|e| FormatError::Encode(e.to_string()))?;
        Ok(frame(Self::MAGIC, Self::VERSION, 0, &body))
    }

    fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_flags, body) = unframe(bytes, Self::MAGIC, Self::VERSION)?;
        postcard::from_bytes(body).map_err(|e| FormatError::Body(e.to_string()))
    }

    /// Content address of the canonical encoded bytes.
    fn content_hash(&self) -> Result<[u8; 32], FormatError> {
        Ok(*blake3::hash(&self.encode()?).as_bytes())
    }
}

/// Why a commit exists (design §5.2, D12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommitKind {
    Flush,
    Checkpoint,
    Import,
    SnapshotForced,
    Prepared,
}

/// Integrity metadata for a segment introduced by a commit (design D9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentMeta {
    pub id: SegmentId,
    pub len: u64,
    pub blake3: [u8; 32],
}

/// An immutable point-in-time mapping of a whole volume (design §5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub vol_id: VolumeId,
    pub parent: Option<CommitHash>,
    pub shard_index: NodeHash,
    pub segments: Vec<SegmentMeta>,
    pub flush_seq: u64,
    pub created_at_ms: u64,
    pub kind: CommitKind,
    /// Reserved for consistency groups (design D5). Always `None` in MVP.
    pub group: Option<u64>,
    /// The writer's fence epoch (design D-M5): the volume's lease epoch
    /// at the time the writing engine acquired ownership. 0 = written
    /// without ownership (single-node, pre-lease history, prepared
    /// ingest). This is what makes a fenced writer's commits — same
    /// prefix, complete closure, plausible `flush_seq` — distinguishable
    /// from the winner's: recovery orders by epoch first, so a zombie's
    /// discarded writes can never be resurrected into the head.
    pub epoch: u64,
}

impl Codec for Commit {
    const MAGIC: [u8; 4] = magic::COMMIT;
    const VERSION: u16 = 2;

    /// v1 bodies predate the epoch; they were all written without
    /// ownership, so 0 is a fact about them, not a guess (the same
    /// argument as [`V1_SHARD_BYTES`]).
    fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_flags, body) = unframe(bytes, Self::MAGIC, Self::VERSION)?;
        let version = u16::from_le_bytes(bytes[4..6].try_into().expect("unframed"));
        if version >= 2 {
            return postcard::from_bytes(body).map_err(|e| FormatError::Body(e.to_string()));
        }
        #[derive(Deserialize)]
        struct V1 {
            vol_id: VolumeId,
            parent: Option<CommitHash>,
            shard_index: NodeHash,
            segments: Vec<SegmentMeta>,
            flush_seq: u64,
            created_at_ms: u64,
            kind: CommitKind,
            group: Option<u64>,
        }
        let v1: V1 = postcard::from_bytes(body).map_err(|e| FormatError::Body(e.to_string()))?;
        Ok(Commit {
            vol_id: v1.vol_id,
            parent: v1.parent,
            shard_index: v1.shard_index,
            segments: v1.segments,
            flush_seq: v1.flush_seq,
            created_at_ms: v1.created_at_ms,
            kind: v1.kind,
            group: v1.group,
            epoch: 0,
        })
    }
}

/// Soft cap on entries per node (design §5.3): keeps serialized nodes
/// ≈≤64 KiB and trees shallow.
pub const MAX_NODE_ENTRIES: usize = 256;

/// A COW-tree node (design §4.3, §6.2). Content-addressed via [`Codec`].
///
/// Leaf extents are gap-delta encoded on the wire ([`ExtentEnc`]): each
/// entry stores the gap from the previous extent's end, so dense mappings
/// serialize to near-nothing under postcard varints. The absolute form is
/// reconstructed by [`Node::leaf_extents`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    Leaf(Vec<ExtentEnc>),
    /// `(first_lba, child_hash)` per child, sorted by `first_lba`.
    Internal(Vec<(u64, NodeHash)>),
    /// `shard_no → root` for the whole volume; `None` = empty shard.
    ShardIndex(Vec<Option<NodeHash>>),
}

/// Wire form of a leaf extent: `gap` = bytes between the previous extent's
/// end (or 0 for the first) and this extent's start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtentEnc {
    pub gap: u64,
    pub len: u64,
    pub target: Target,
}

impl Codec for Node {
    const MAGIC: [u8; 4] = magic::NODE;
    const VERSION: u16 = 1;
}

impl Node {
    /// Build a leaf from absolute, sorted, non-overlapping extents.
    /// Errors if input is unsorted/overlapping (defense in depth — the
    /// tree layer should never produce that).
    pub fn leaf_from_extents(extents: &[Extent]) -> Result<Node, FormatError> {
        let mut enc = Vec::with_capacity(extents.len());
        let mut cursor = 0u64;
        for e in extents {
            if e.lba_start < cursor {
                return Err(FormatError::Encode(format!(
                    "extents unsorted or overlapping at lba {}",
                    e.lba_start
                )));
            }
            enc.push(ExtentEnc {
                gap: e.lba_start - cursor,
                len: e.len,
                target: e.target.clone(),
            });
            cursor = e.lba_start + e.len;
        }
        Ok(Node::Leaf(enc))
    }

    /// Reconstruct absolute extents from a leaf. Errors on other variants
    /// or on entries that violate extent invariants.
    pub fn leaf_extents(&self) -> Result<Vec<Extent>, FormatError> {
        let Node::Leaf(entries) = self else {
            return Err(FormatError::Body("not a leaf node".into()));
        };
        let mut out = Vec::with_capacity(entries.len());
        let mut cursor = 0u64;
        for e in entries {
            let start = cursor
                .checked_add(e.gap)
                .ok_or_else(|| FormatError::Body("leaf extent start overflow".into()))?;
            let ext = Extent::new(start, e.len, e.target.clone())
                .map_err(|err| FormatError::Body(err.to_string()))?;
            cursor = ext.end();
            out.push(ext);
        }
        Ok(out)
    }
}

/// Volume superblock (design §5.2): the self-describing anchor that makes
/// refs reconstructible from S3 alone (§6.5).
///
/// `shard_bytes` is here because the shard index is a *positional* array —
/// slot `i` means `[i·shard_bytes, (i+1)·shard_bytes)` — and without it
/// recorded, the array is only interpretable with an out-of-band parameter
/// the reader supplies. Two readers that disagree then disagree about what
/// the tree says, and a commit under the wrong geometry rewrites part of
/// the array on a different grid. The volume owns its geometry; config
/// only chooses it at create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Superblock {
    pub vol_id: VolumeId,
    pub size: u64,
    pub format_major: u16,
    pub created_at_ms: u64,
    /// Logical shard size (design D3), fixed for the volume's lifetime.
    pub shard_bytes: u64,
}

/// What a v1 superblock — written before the geometry was recorded — is
/// known to have used. It was never configurable, so this is a fact about
/// every such volume rather than a guess.
pub const V1_SHARD_BYTES: u64 = 1 << 30;

impl Codec for Superblock {
    const MAGIC: [u8; 4] = magic::SUPERBLOCK;
    const VERSION: u16 = 2;

    /// v1 bodies lack the trailing `shard_bytes`, and postcard is not
    /// self-describing, so the version selects the shape to decode.
    fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_flags, body) = unframe(bytes, Self::MAGIC, Self::VERSION)?;
        let version = u16::from_le_bytes(bytes[4..6].try_into().expect("unframed"));
        if version >= 2 {
            return postcard::from_bytes(body).map_err(|e| FormatError::Body(e.to_string()));
        }
        #[derive(Deserialize)]
        struct V1 {
            vol_id: VolumeId,
            size: u64,
            format_major: u16,
            created_at_ms: u64,
        }
        let v1: V1 = postcard::from_bytes(body).map_err(|e| FormatError::Body(e.to_string()))?;
        Ok(Superblock {
            vol_id: v1.vol_id,
            size: v1.size,
            format_major: v1.format_major,
            created_at_ms: v1.created_at_ms,
            shard_bytes: V1_SHARD_BYTES,
        })
    }
}

/// Snapshot provenance (design §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapKind {
    Manual,
    Rpo,
    Imported,
}

/// Snapshot manifest (design §9.2): the immutable record that makes refs
/// reconstructible (§6.5) and lineage explicit. Written *before* the
/// `snap/<name>` ref is CASed, so a ref never dangles (§9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub snap_id: SnapId,
    pub vol_id: VolumeId,
    /// Full ref name, e.g. `snap/nightly-2026-07-21` or `rpo`.
    pub ref_name: String,
    pub commit: CommitHash,
    /// Lineage: previous snapshot on this line. `None` until the diff/
    /// send machinery (M5) and ingestion (M6) populate it.
    pub parent_snap: Option<SnapId>,
    pub kind: SnapKind,
    pub created_at_ms: u64,
}

impl Codec for SnapshotManifest {
    const MAGIC: [u8; 4] = magic::SNAPSHOT;
    const VERSION: u16 = 1;
}

/// Clone provenance (CSI design §4.1, option A): a volume created as a
/// fast clone records its base here, at a fixed per-volume key. Readers
/// resolve content-addressed objects missing from the clone's namespace
/// by falling through to the base volume's namespace; the base commit is
/// kept live in the *source* volume by a `clone:<name>` pin on the source
/// snapshot ref. A volume without this object is not a clone. Additive:
/// no existing format changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneBase {
    pub base_vol: VolumeId,
    pub base_commit: CommitHash,
    pub created_at_ms: u64,
}

impl Codec for CloneBase {
    const MAGIC: [u8; 4] = magic::CLONE_BASE;
    const VERSION: u16 = 1;
}

/// A volume's mutable state as one object (design D-M1/D-M2): every ref,
/// pin and meta row, serialized as rows rather than as any backend's file
/// format. Written with a conditional PUT, so the single write *is* the
/// multi-op transaction of D13/I8 — which is why the granularity is the
/// volume and not the ref.
///
/// `owner` is the D-M3 lease (holder, fence epoch, expiry); reserved here
/// so occupying it later is not a format change, unpopulated until
/// ownership lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RefsObject {
    /// `(name, commit, generation)` — the §6.1 refs rows.
    pub refs: Vec<(String, CommitHash, u64)>,
    /// `(ref name, holder)` — refcount is row count per name (§9A.4).
    pub pins: Vec<(String, String)>,
    /// The §6.1 meta rows (`prepared/*`, the provisioner catalog).
    pub meta: Vec<(String, Vec<u8>)>,
    /// Per-volume ownership (D-M3): `(holder, epoch, expires_at_ms)`.
    pub owner: Option<(String, u64, u64)>,
}

impl Codec for RefsObject {
    const MAGIC: [u8; 4] = magic::REFS;
    const VERSION: u16 = 1;
}

// ------------------------------------------------------------------ emap
//
// The materialized extent map for exactly one commit (design D-M7):
// `emap/{hex(commit)}`, immutable like everything else under the volume
// prefix. Present → the open walk collapses to one header GET plus one
// ranged GET per touched shard; absent → the tree walk, unchanged. No
// CAS, no staleness question — the commit hash in the key is the whole
// consistency story — and GC sweeps it with its commit.
//
// The layout is *not* one postcard struct, deliberately: a reader must
// be able to load a single shard without transferring the object, so
// the header carries per-shard byte ranges into the body. On the wire:
//
//   frame (8 B) | header_len: u32 LE | postcard(EmapHeader) | body
//
// where the body is the concatenation of postcard-encoded [`EmapShard`]s
// and `EmapHeader::shards[i]` is `(offset, len)` into the body —
// `(0, 0)` for an empty shard. Columns are `u32`s (the D-M10 argument:
// everything in a shard fits), starts are gap-encoded like tree leaves,
// and the object table is per volume, interned once for the whole map.
// Loading a shard is one linear pass that lands directly in the D-M10
// columnar structure — there is no map to build.

/// A backing object's identity in the emap's interned table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmapObj {
    Seg(SegmentId),
    Cas(ObjectHash),
}

/// One shard's columns. `gap[0]` is the first extent's offset from the
/// shard base; each later gap is the hole after the previous extent.
/// `target[i]` indexes [`EmapHeader::objects`], or [`EMAP_ZERO`] for
/// explicit zeroes (in which case `offset[i]` is 0).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmapShard {
    pub gap: Vec<u32>,
    pub len: Vec<u32>,
    pub target: Vec<u32>,
    pub offset: Vec<u32>,
}

/// `target` value for an extent of explicit zeroes.
pub const EMAP_ZERO: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmapHeader {
    /// The commit this map materializes — must match the key.
    pub commit: CommitHash,
    /// The shard grid the columns are relative to.
    pub shard_bytes: u64,
    /// The volume-wide interned object table.
    pub objects: Vec<EmapObj>,
    /// Per shard: `(byte offset, byte len)` into the body; `(0, 0)` =
    /// empty shard.
    pub shards: Vec<(u32, u32)>,
}

/// How much of the object a reader should fetch first: enough for the
/// frame, the length word, and any realistic header (the object table
/// dominates it at ~17 B per distinct backing object).
pub const EMAP_HEADER_PROBE: u64 = 256 * 1024;

/// Encode a whole emap object. `shards[i] = None` = empty shard.
pub fn encode_emap(
    commit: CommitHash,
    shard_bytes: u64,
    objects: Vec<EmapObj>,
    shards: Vec<Option<EmapShard>>,
) -> Result<Vec<u8>, FormatError> {
    let mut body: Vec<u8> = Vec::new();
    let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(shards.len());
    for shard in &shards {
        match shard {
            None => ranges.push((0, 0)),
            Some(s) => {
                let blob =
                    postcard::to_allocvec(s).map_err(|e| FormatError::Encode(e.to_string()))?;
                let off = u32::try_from(body.len())
                    .map_err(|_| FormatError::Encode("emap body over 4 GiB".into()))?;
                let len = u32::try_from(blob.len())
                    .map_err(|_| FormatError::Encode("emap shard blob over 4 GiB".into()))?;
                ranges.push((off, len));
                body.extend_from_slice(&blob);
            }
        }
    }
    let header = EmapHeader {
        commit,
        shard_bytes,
        objects,
        shards: ranges,
    };
    let header_bytes =
        postcard::to_allocvec(&header).map_err(|e| FormatError::Encode(e.to_string()))?;
    let header_len = u32::try_from(header_bytes.len())
        .map_err(|_| FormatError::Encode("emap header over 4 GiB".into()))?;
    let mut out = frame(magic::EMAP, 1, 0, &[]);
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Bytes of prefix needed before [`decode_emap_header`] can succeed, or
/// `Truncated` when even the length word is missing. The body starts at
/// exactly this offset.
pub fn emap_header_end(prefix: &[u8]) -> Result<usize, FormatError> {
    let (_flags, rest) = unframe(prefix, magic::EMAP, 1)?;
    if rest.len() < 4 {
        return Err(FormatError::Truncated);
    }
    let header_len = u32::from_le_bytes(rest[..4].try_into().expect("checked")) as usize;
    Ok(FRAME_LEN + 4 + header_len)
}

/// Decode the header from a prefix of at least [`emap_header_end`] bytes.
pub fn decode_emap_header(prefix: &[u8]) -> Result<EmapHeader, FormatError> {
    let end = emap_header_end(prefix)?;
    if prefix.len() < end {
        return Err(FormatError::Truncated);
    }
    postcard::from_bytes(&prefix[FRAME_LEN + 4..end]).map_err(|e| FormatError::Body(e.to_string()))
}

/// Decode one shard's blob (the bytes at its header range).
pub fn decode_emap_shard(blob: &[u8]) -> Result<EmapShard, FormatError> {
    let s: EmapShard = postcard::from_bytes(blob).map_err(|e| FormatError::Body(e.to_string()))?;
    let n = s.gap.len();
    if s.len.len() != n || s.target.len() != n || s.offset.len() != n {
        return Err(FormatError::Body(format!(
            "emap shard columns disagree: gap {n}, len {}, target {}, offset {}",
            s.len.len(),
            s.target.len(),
            s.offset.len()
        )));
    }
    Ok(s)
}

// TODO(M5, design §9.4): SKYD1 stream records.

#[cfg(test)]
mod tests {
    use super::*;

    /// A v1 superblock predates the recorded geometry; it must still open,
    /// with the only value it could have had.
    #[test]
    fn v1_superblock_decodes_with_the_geometry_it_was_written_under() {
        #[derive(Serialize)]
        struct V1 {
            vol_id: VolumeId,
            size: u64,
            format_major: u16,
            created_at_ms: u64,
        }
        let vol = VolumeId(ulid::Ulid::from(9u128));
        let body = postcard::to_allocvec(&V1 {
            vol_id: vol,
            size: 4 << 30,
            format_major: 1,
            created_at_ms: 42,
        })
        .unwrap();
        let framed = frame(magic::SUPERBLOCK, 1, 0, &body);

        let sb = Superblock::decode(&framed).unwrap();
        assert_eq!(sb.vol_id, vol);
        assert_eq!(sb.size, 4 << 30);
        assert_eq!(sb.created_at_ms, 42);
        assert_eq!(sb.shard_bytes, V1_SHARD_BYTES);
    }

    /// Emap: header + per-shard ranged blobs round-trip, empty shards
    /// take no body bytes, and a shard decodes alone from its range —
    /// the property the ranged-GET reader depends on.
    #[test]
    fn emap_roundtrips_and_shards_decode_from_their_ranges() {
        let s0 = EmapShard {
            gap: vec![0, 4096],
            len: vec![4096, 8192],
            target: vec![0, EMAP_ZERO],
            offset: vec![0, 0],
        };
        let s2 = EmapShard {
            gap: vec![512],
            len: vec![512],
            target: vec![1],
            offset: vec![65536],
        };
        let objects = vec![
            EmapObj::Seg(SegmentId(ulid::Ulid::from_parts(1, 1))),
            EmapObj::Cas([7u8; 32]),
        ];
        let bytes = encode_emap(
            [9u8; 32],
            1 << 30,
            objects.clone(),
            vec![Some(s0.clone()), None, Some(s2.clone())],
        )
        .unwrap();

        let body_start = emap_header_end(&bytes).unwrap();
        let h = decode_emap_header(&bytes).unwrap();
        assert_eq!(h.commit, [9u8; 32]);
        assert_eq!(h.shard_bytes, 1 << 30);
        assert_eq!(h.objects, objects);
        assert_eq!(h.shards.len(), 3);
        assert_eq!(h.shards[1], (0, 0), "empty shard takes no body bytes");

        for (i, expect) in [(0usize, &s0), (2, &s2)] {
            let (off, len) = h.shards[i];
            let blob = &bytes[body_start + off as usize..body_start + (off + len) as usize];
            assert_eq!(&decode_emap_shard(blob).unwrap(), expect);
        }

        // A short prefix says Truncated, not garbage.
        assert_eq!(
            decode_emap_header(&bytes[..body_start - 1]).unwrap_err(),
            FormatError::Truncated
        );
        // Mismatched columns are refused.
        let bad = postcard::to_allocvec(&EmapShard {
            gap: vec![1, 2],
            len: vec![1],
            target: vec![0, 0],
            offset: vec![0, 0],
        })
        .unwrap();
        assert!(decode_emap_shard(&bad).is_err());
    }

    /// A v1 commit predates the fence epoch; it decodes with 0 — the only
    /// epoch anything unowned ever wrote under.
    #[test]
    fn v1_commit_decodes_with_epoch_zero() {
        #[derive(Serialize)]
        struct V1 {
            vol_id: VolumeId,
            parent: Option<CommitHash>,
            shard_index: NodeHash,
            segments: Vec<SegmentMeta>,
            flush_seq: u64,
            created_at_ms: u64,
            kind: CommitKind,
            group: Option<u64>,
        }
        let vol = VolumeId(ulid::Ulid::from(11u128));
        let body = postcard::to_allocvec(&V1 {
            vol_id: vol,
            parent: Some([3u8; 32]),
            shard_index: [4u8; 32],
            segments: vec![SegmentMeta {
                id: SegmentId(ulid::Ulid::from_parts(7, 7)),
                len: 512,
                blake3: [5u8; 32],
            }],
            flush_seq: 9,
            created_at_ms: 42,
            kind: CommitKind::Flush,
            group: None,
        })
        .unwrap();
        let framed = frame(magic::COMMIT, 1, 0, &body);

        let c = Commit::decode(&framed).unwrap();
        assert_eq!(c.vol_id, vol);
        assert_eq!(c.parent, Some([3u8; 32]));
        assert_eq!(c.flush_seq, 9);
        assert_eq!(c.segments.len(), 1);
        assert_eq!(c.epoch, 0, "pre-epoch commits were all unowned");
    }

    /// And a v2 commit round-trips its epoch.
    #[test]
    fn v2_commit_roundtrips_epoch() {
        let c = Commit {
            vol_id: VolumeId(ulid::Ulid::from(12u128)),
            parent: None,
            shard_index: [0u8; 32],
            segments: vec![],
            flush_seq: 1,
            created_at_ms: 1,
            kind: CommitKind::Flush,
            group: None,
            epoch: 5,
        };
        let out = Commit::decode(&c.encode().unwrap()).unwrap();
        assert_eq!(out, c);
    }

    /// And a v2 superblock is rejected by a reader that only knows v1 —
    /// the property the version bump exists for.
    #[test]
    fn v2_superblock_is_refused_by_a_v1_reader() {
        let sb = Superblock {
            vol_id: VolumeId(ulid::Ulid::from(9u128)),
            size: 1 << 30,
            format_major: 1,
            created_at_ms: 0,
            shard_bytes: 64 << 20,
        };
        let bytes = sb.encode().unwrap();
        assert!(matches!(
            unframe(&bytes, magic::SUPERBLOCK, 1),
            Err(FormatError::UnsupportedVersion {
                got: 2,
                supported: 1
            })
        ));
        assert_eq!(Superblock::decode(&bytes).unwrap(), sb);
    }

    use proptest::prelude::*;

    fn commit_fixture() -> Commit {
        Commit {
            vol_id: VolumeId(ulid::Ulid::from_parts(1, 2)),
            parent: Some([3u8; 32]),
            shard_index: [4u8; 32],
            segments: vec![SegmentMeta {
                id: SegmentId(ulid::Ulid::from_parts(5, 6)),
                len: 8 << 20,
                blake3: [7u8; 32],
            }],
            flush_seq: 42,
            created_at_ms: 1_700_000_000_000,
            kind: CommitKind::Flush,
            group: None,
            epoch: 3,
        }
    }

    #[test]
    fn commit_roundtrips() {
        let c = commit_fixture();
        let bytes = c.encode().unwrap();
        assert_eq!(&bytes[0..4], b"SCMT");
        assert_eq!(Commit::decode(&bytes).unwrap(), c);
    }

    #[test]
    fn content_hash_is_deterministic_and_input_sensitive() {
        let a = commit_fixture();
        let mut b = commit_fixture();
        assert_eq!(a.content_hash().unwrap(), b.content_hash().unwrap());
        b.flush_seq += 1;
        assert_ne!(a.content_hash().unwrap(), b.content_hash().unwrap());
    }

    #[test]
    fn rejects_bad_magic_truncation_and_future_version() {
        let good = commit_fixture().encode().unwrap();

        let mut bad_magic = good.clone();
        bad_magic[0..4].copy_from_slice(b"XXXX");
        assert!(matches!(
            Commit::decode(&bad_magic),
            Err(FormatError::BadMagic { .. })
        ));

        assert_eq!(Commit::decode(&good[0..4]), Err(FormatError::Truncated));

        let mut future = good.clone();
        future[4..6].copy_from_slice(&999u16.to_le_bytes());
        assert!(matches!(
            Commit::decode(&future),
            Err(FormatError::UnsupportedVersion { got: 999, .. })
        ));
    }

    proptest! {
        #[test]
        fn frame_roundtrips_any_body(body in proptest::collection::vec(any::<u8>(), 0..4096),
                                     version in 0u16..8, flags: u16) {
            let framed = frame(magic::NODE, version, flags, &body);
            let (got_flags, got_body) = unframe(&framed, magic::NODE, 8).unwrap();
            prop_assert_eq!(got_flags, flags);
            prop_assert_eq!(got_body, &body[..]);
        }
    }
}

#[cfg(test)]
mod clone_base_tests {
    use super::*;

    #[test]
    fn clone_base_roundtrips_and_rejects_future_version() {
        let cb = CloneBase {
            base_vol: VolumeId(ulid::Ulid::from_parts(4, 4)),
            base_commit: [5u8; 32],
            created_at_ms: 1_753_000_000_000,
        };
        let bytes = cb.encode().unwrap();
        assert_eq!(&bytes[0..4], b"SCLN");
        assert_eq!(CloneBase::decode(&bytes).unwrap(), cb);

        let mut future = bytes.clone();
        future[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert!(matches!(
            CloneBase::decode(&future),
            Err(FormatError::UnsupportedVersion { .. })
        ));
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_and_rejects_future_version() {
        let m = SnapshotManifest {
            snap_id: SnapId(ulid::Ulid::from_parts(9, 9)),
            vol_id: VolumeId(ulid::Ulid::from_parts(1, 1)),
            ref_name: "snap/nightly".into(),
            commit: [7u8; 32],
            parent_snap: Some(SnapId(ulid::Ulid::from_parts(8, 8))),
            kind: SnapKind::Manual,
            created_at_ms: 1_753_000_000_000,
        };
        let bytes = m.encode().unwrap();
        assert_eq!(&bytes[0..4], b"SSNP");
        assert_eq!(SnapshotManifest::decode(&bytes).unwrap(), m);

        let mut future = bytes.clone();
        future[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert!(matches!(
            SnapshotManifest::decode(&future),
            Err(FormatError::UnsupportedVersion { .. })
        ));
    }
}

#[cfg(test)]
mod node_tests {
    use super::*;
    use proptest::prelude::*;
    use skynbd_types::{DataRef, SECTOR};

    fn seg_extent(start: u64, len: u64, seg_ts: u64, offset: u64) -> Extent {
        Extent::new(
            start,
            len,
            Target::Data(DataRef::Seg {
                segment: SegmentId(ulid::Ulid::from_parts(seg_ts, 0)),
                offset,
            }),
        )
        .unwrap()
    }

    #[test]
    fn leaf_roundtrips_and_gap_encodes() {
        // Two extents with a gap, one adjacent (gap 0).
        let extents = vec![
            seg_extent(0, 4096, 1, 0),
            seg_extent(8192, 512, 1, 4096), // gap 4096
            seg_extent(8704, 1024, 2, 0),   // adjacent: gap 0
        ];
        let node = Node::leaf_from_extents(&extents).unwrap();
        if let Node::Leaf(enc) = &node {
            assert_eq!(enc[0].gap, 0);
            assert_eq!(enc[1].gap, 4096);
            assert_eq!(enc[2].gap, 0);
        } else {
            panic!("expected leaf");
        }
        let bytes = node.encode().unwrap();
        let back = Node::decode(&bytes).unwrap();
        assert_eq!(back.leaf_extents().unwrap(), extents);
    }

    #[test]
    fn leaf_rejects_unsorted_or_overlapping_input() {
        let unsorted = vec![seg_extent(4096, 512, 1, 0), seg_extent(0, 512, 1, 512)];
        assert!(Node::leaf_from_extents(&unsorted).is_err());
        let overlapping = vec![seg_extent(0, 1024, 1, 0), seg_extent(512, 1024, 1, 1024)];
        assert!(Node::leaf_from_extents(&overlapping).is_err());
    }

    #[test]
    fn internal_and_shard_index_roundtrip() {
        let internal = Node::Internal(vec![(0, [1u8; 32]), (1 << 30, [2u8; 32])]);
        assert_eq!(Node::decode(&internal.encode().unwrap()).unwrap(), internal);

        let idx = Node::ShardIndex(vec![Some([3u8; 32]), None, Some([4u8; 32])]);
        assert_eq!(Node::decode(&idx.encode().unwrap()).unwrap(), idx);
    }

    #[test]
    fn node_content_hash_distinguishes_variants() {
        let leaf = Node::Leaf(vec![]);
        let idx = Node::ShardIndex(vec![]);
        assert_ne!(leaf.content_hash().unwrap(), idx.content_hash().unwrap());
    }

    prop_compose! {
        // Sorted, non-overlapping, 512-aligned extents with mixed targets.
        fn arb_extents()(raw in proptest::collection::vec((1u64..64, 1u64..16, any::<bool>()), 0..64))
            -> Vec<Extent> {
            let mut out = Vec::new();
            let mut cursor = 0u64;
            for (gap_sectors, len_sectors, zero) in raw {
                let start = cursor + gap_sectors * SECTOR;
                let len = len_sectors * SECTOR;
                let target = if zero {
                    Target::Zero
                } else {
                    Target::Data(DataRef::Cas { hash: [9u8; 32], offset: cursor })
                };
                out.push(Extent::new(start, len, target).unwrap());
                cursor = start + len;
            }
            out
        }
    }

    proptest! {
        #[test]
        fn leaf_roundtrip_any_extents(extents in arb_extents()) {
            let node = Node::leaf_from_extents(&extents).unwrap();
            let back = Node::decode(&node.encode().unwrap()).unwrap();
            prop_assert_eq!(back.leaf_extents().unwrap(), extents);
        }
    }
}
