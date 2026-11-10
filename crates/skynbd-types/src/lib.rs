//! Core identifiers, extents, and hashes.
//!
//! This crate is the vocabulary of the system — see design §4 (“Storage
//! Model”) in `docs/skynbd_design.md`. It must stay dependency-light and
//! IO-free: everything here is `Serialize`/`Deserialize` plain data.

use serde::{Deserialize, Serialize};

/// BLAKE3 content address of a canonical serialized tree node.
pub type NodeHash = [u8; 32];
/// BLAKE3 content address of a canonical serialized commit.
pub type CommitHash = [u8; 32];
/// BLAKE3 content address of a CAS data object's raw payload (design D11).
pub type ObjectHash = [u8; 32];

macro_rules! ulid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub ulid::Ulid);

        impl $name {
            pub fn from_string(s: &str) -> Result<Self, TypeError> {
                Ok(Self(ulid::Ulid::from_string(s).map_err(|_| TypeError::BadId)?))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

ulid_id!(
    /// A volume — attached (writable head) or snapshot lineage (design D14).
    VolumeId
);
ulid_id!(
    /// A packed data segment produced by the live write path (design D2).
    SegmentId
);
ulid_id!(
    /// A snapshot manifest / prepared-snapshot identity (design §9, §9A).
    SnapId
);

/// All NBD offsets and lengths are 512-aligned (design §5.3).
pub const SECTOR: u64 = 512;

/// Where an extent's bytes live (design §4.2, D11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRef {
    /// ULID-named packed segment + byte offset (live NBD write path).
    Seg { segment: SegmentId, offset: u64 },
    /// Content-addressed object + byte offset (snapshot ingestion path).
    Cas { hash: ObjectHash, offset: u64 },
}

/// Extent target: real bytes, or explicit zeros overriding anything inherited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Data(DataRef),
    Zero,
}

/// A variable-length mapping `[lba_start, lba_start + len)` → target.
///
/// Invariants (enforced by constructors and checked by the tree crate):
/// non-zero length, 512-aligned start and length, no u64 overflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    pub lba_start: u64,
    pub len: u64,
    pub target: Target,
}

impl Extent {
    pub fn new(lba_start: u64, len: u64, target: Target) -> Result<Self, TypeError> {
        if len == 0 {
            return Err(TypeError::EmptyExtent);
        }
        if !lba_start.is_multiple_of(SECTOR) || !len.is_multiple_of(SECTOR) {
            return Err(TypeError::Unaligned { lba_start, len });
        }
        if lba_start.checked_add(len).is_none() {
            return Err(TypeError::Overflow { lba_start, len });
        }
        Ok(Self {
            lba_start,
            len,
            target,
        })
    }

    /// One past the last byte covered.
    pub fn end(&self) -> u64 {
        self.lba_start + self.len
    }

    /// Whether `next` continues this extent exactly: adjacent in LBA *and*
    /// contiguous in the same backing object, so the pair is describable
    /// as one extent. Note this is arithmetic on the target rather than
    /// target equality — two 64 KiB writes landing back-to-back in one
    /// segment have different `DataRef`s but describe one run — which is
    /// why no generic interval map can coalesce for us.
    pub fn continues_into(&self, next: &Extent) -> bool {
        if self.end() != next.lba_start {
            return false;
        }
        match (&self.target, &next.target) {
            (Target::Zero, Target::Zero) => true,
            (
                Target::Data(DataRef::Seg { segment, offset }),
                Target::Data(DataRef::Seg {
                    segment: next_seg,
                    offset: next_off,
                }),
            ) => segment == next_seg && offset + self.len == *next_off,
            (
                Target::Data(DataRef::Cas { hash, offset }),
                Target::Data(DataRef::Cas {
                    hash: next_hash,
                    offset: next_off,
                }),
            ) => hash == next_hash && offset + self.len == *next_off,
            _ => false,
        }
    }

    /// Absorb an extent that [`Extent::continues_into`] accepts. Alignment
    /// is preserved: the sum of two sector multiples is one.
    pub fn absorb(&mut self, next: &Extent) {
        debug_assert!(self.continues_into(next));
        self.len += next.len;
    }

    /// The part of this extent covering `[start, end)`, with its target
    /// offset advanced to match.
    ///
    /// This is the arithmetic that makes truncation safe: a target names
    /// bytes at `lba_start`, so a piece beginning `d` bytes later begins
    /// `d` bytes into the same backing object. Getting it wrong returns
    /// the wrong bytes rather than failing, so it lives here with the
    /// invariants rather than being open-coded at each call site.
    ///
    /// Panics if `[start, end)` is not a non-empty sub-range of the
    /// extent — callers clamp first.
    pub fn slice(&self, start: u64, end: u64) -> Extent {
        assert!(
            self.lba_start <= start && start < end && end <= self.end(),
            "slice [{start}, {end}) outside extent [{}, {})",
            self.lba_start,
            self.end()
        );
        let delta = start - self.lba_start;
        let target = match &self.target {
            Target::Zero => Target::Zero,
            Target::Data(DataRef::Seg { segment, offset }) => Target::Data(DataRef::Seg {
                segment: *segment,
                offset: offset + delta,
            }),
            Target::Data(DataRef::Cas { hash, offset }) => Target::Data(DataRef::Cas {
                hash: *hash,
                offset: offset + delta,
            }),
        };
        Extent {
            lba_start: start,
            len: end - start,
            target,
        }
    }

    /// Do two extents cover any common byte?
    pub fn overlaps(&self, other: &Extent) -> bool {
        self.lba_start < other.end() && other.lba_start < self.end()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TypeError {
    #[error("extent length must be non-zero")]
    EmptyExtent,
    #[error("extent not 512-aligned: start={lba_start} len={len}")]
    Unaligned { lba_start: u64, len: u64 },
    #[error("extent overflows u64: start={lba_start} len={len}")]
    Overflow { lba_start: u64, len: u64 },
    #[error("malformed identifier")]
    BadId,
}

/// Origin-pointer ordering (multi-node design D-M15, Invariant E). Pure
/// naming/ordering policy shared by every pointer writer — the API route
/// (via `skynbd_nbd::origin`, which re-exports these) and the agent's
/// in-process control-plane mirror — so the monotone decision cannot
/// drift between them.
pub mod origin {
    /// `snap/g<N>` → N: the generation refs the advancing pointer moves
    /// over.
    pub fn generation_number(ref_name: &str) -> Option<u64> {
        let digits = ref_name.strip_prefix("snap/g")?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    }

    /// Invariant E's monotone rule: the pointer only moves forward along
    /// the generation chain.
    ///
    /// - `Err(reason)`: the target is not a generation ref — Invariant E
    ///   restricts pointer targets to `snap/g<N>`, the totally ordered
    ///   namespace (aliases share commits with generations, so nothing
    ///   is lost).
    /// - `Ok(false)`: the row already names the target's generation or a
    ///   descendant. The caller must **not** write — a replayed,
    ///   reordered, or duplicated repoint no-ops instead of regressing
    ///   the export (serving a descendant satisfies any older target:
    ///   the Invariant A induction, run forward).
    /// - `Ok(true)`: write, guarded by a `MetaCas` expecting exactly the
    ///   row this decision read — monotonicity is only sound when the
    ///   compare and the write are one atomic step. A row that predates
    ///   the restriction (a legacy alias) imposes no ordering; moving
    ///   off it is a one-way door into the ordered namespace.
    pub fn repoint_forward(old_row: Option<&str>, target: &str) -> Result<bool, String> {
        let t = generation_number(target).ok_or_else(|| {
            format!("origin target {target:?} must be a generation ref (snap/g<N>)")
        })?;
        match old_row.and_then(generation_number) {
            Some(o) if o >= t => Ok(false),
            _ => Ok(true),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{generation_number, repoint_forward};

        #[test]
        fn generation_numbers() {
            assert_eq!(generation_number("snap/g0"), Some(0));
            assert_eq!(generation_number("snap/g17"), Some(17));
            assert_eq!(generation_number("snap/golden"), None);
            assert_eq!(generation_number("snap/g"), None);
            assert_eq!(generation_number("snap/g1x"), None);
            assert_eq!(generation_number("snap/x1"), None);
            assert_eq!(generation_number("head"), None);
        }

        #[test]
        fn monotone_repoint_decisions() {
            // Initialization and forward moves write.
            assert_eq!(repoint_forward(None, "snap/g0"), Ok(true));
            assert_eq!(repoint_forward(Some("snap/g0"), "snap/g1"), Ok(true));
            assert_eq!(repoint_forward(Some("snap/g2"), "snap/g10"), Ok(true));
            // Replay/regression no-ops — the pointer never moves backwards.
            assert_eq!(repoint_forward(Some("snap/g1"), "snap/g1"), Ok(false));
            assert_eq!(repoint_forward(Some("snap/g7"), "snap/g5"), Ok(false));
            // Non-generation targets are refused outright.
            assert!(repoint_forward(None, "snap/golden").is_err());
            assert!(repoint_forward(Some("snap/g1"), "head").is_err());
            // A legacy alias row imposes no ordering: one-way upgrade.
            assert_eq!(repoint_forward(Some("snap/golden"), "snap/g0"), Ok(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(start: u64, len: u64) -> Extent {
        Extent::new(start, len, Target::Zero).unwrap()
    }

    #[test]
    fn extent_construction_validates() {
        assert_eq!(Extent::new(0, 0, Target::Zero), Err(TypeError::EmptyExtent));
        assert!(matches!(
            Extent::new(100, 512, Target::Zero),
            Err(TypeError::Unaligned { .. })
        ));
        assert!(matches!(
            Extent::new(512, 100, Target::Zero),
            Err(TypeError::Unaligned { .. })
        ));
        assert!(matches!(
            Extent::new(u64::MAX - 511, 1024, Target::Zero),
            Err(TypeError::Overflow { .. })
        ));
        assert!(Extent::new(0, 512, Target::Zero).is_ok());
    }

    #[test]
    fn extent_overlap_semantics() {
        // Adjacent extents do not overlap; any shared byte does.
        assert!(!ext(0, 512).overlaps(&ext(512, 512)));
        assert!(ext(0, 1024).overlaps(&ext(512, 512)));
        assert!(ext(512, 512).overlaps(&ext(0, 1024)));
        assert!(ext(0, 512).overlaps(&ext(0, 512)));
    }

    #[test]
    fn types_roundtrip_postcard() {
        // The format crate owns framing; this just pins that core types
        // remain postcard-serializable (design §5.2).
        let e = Extent::new(
            4096,
            8192,
            Target::Data(DataRef::Cas {
                hash: [7u8; 32],
                offset: 512,
            }),
        )
        .unwrap();
        let bytes = postcard::to_allocvec(&e).unwrap();
        let back: Extent = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(e, back);
    }
}
