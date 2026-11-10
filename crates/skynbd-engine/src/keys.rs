//! S3 key scheme (design §5.1). All keys live under `skynbd/v1/{vol}/`.

use skynbd_types::{NodeHash, SegmentId, VolumeId};

pub const PREFIX: &str = "skynbd/v1";

fn hex(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn volume_prefix(vol: VolumeId) -> String {
    format!("{PREFIX}/{vol}/")
}

pub fn superblock(vol: VolumeId, gen: u64) -> String {
    format!("{PREFIX}/{vol}/sb/{gen}")
}

pub fn segment(vol: VolumeId, seg: SegmentId) -> String {
    format!("{PREFIX}/{vol}/seg/{seg}")
}

pub fn snapshot(vol: VolumeId, snap: skynbd_types::SnapId) -> String {
    format!("{PREFIX}/{vol}/snap/{snap}")
}

/// Clone provenance object (CSI design §4.1): fixed key, present only on
/// clone volumes.
pub fn clone_base(vol: VolumeId) -> String {
    format!("{PREFIX}/{vol}/base")
}

pub fn cas_object(vol: VolumeId, hash: &[u8; 32]) -> String {
    format!("{PREFIX}/{vol}/obj/{}", hex(hash))
}

pub fn node(vol: VolumeId, hash: &NodeHash) -> String {
    format!("{PREFIX}/{vol}/node/{}", hex(hash))
}

pub fn commit(vol: VolumeId, hash: &[u8; 32]) -> String {
    format!("{PREFIX}/{vol}/commit/{}", hex(hash))
}

/// The materialized extent map for exactly that commit (design D-M7).
/// Immutable; GC sweeps it with the commit that names it.
pub fn emap(vol: VolumeId, commit: &[u8; 32]) -> String {
    format!("{PREFIX}/{vol}/emap/{}", hex(commit))
}

/// The name-index marker for a volume (design D-M9): `volume_id` is
/// `blake3(name)`, so name → prefix needs no listing — this closes the
/// reverse direction, making the export namespace reconstructible after
/// total refs loss. Written at create, removed at delete; lives outside
/// every volume prefix, so per-volume GC never sees it.
pub fn vol_index(name: &str) -> String {
    format!("{PREFIX}/index/vol/{name}")
}

/// The listing prefix for [`vol_index`] markers.
pub fn vol_index_prefix() -> String {
    format!("{PREFIX}/index/vol/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable_and_prefixed() {
        let vol = VolumeId(ulid::Ulid::from_parts(1, 2));
        let seg = SegmentId(ulid::Ulid::from_parts(3, 4));
        assert!(superblock(vol, 0).starts_with(&volume_prefix(vol)));
        assert!(segment(vol, seg).contains("/seg/"));
        assert!(node(vol, &[0xAB; 32]).ends_with(&"ab".repeat(32)));
        assert!(commit(vol, &[1; 32]).contains("/commit/"));
        assert!(cas_object(vol, &[2; 32]).contains("/obj/"));
    }
}
