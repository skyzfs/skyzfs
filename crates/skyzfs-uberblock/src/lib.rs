//! Where a ZFS vdev's labels and uberblock rings lie for a volume of a
//! given size, and what in a write to the volume is an uberblock write
//! (design §5.2). No I/O: the daemon's store holds every write it is
//! handed against [`Labels::uberblocks_in`], and the tests hold the same
//! functions against the on-disk format and against pools recorded on
//! real ZFS (M2.1, with the recorded pools of M2.T1).
//!
//! # The on-disk format, as this crate reads it
//!
//! Cited from OpenZFS 2.4.1 (tag `zfs-2.4.1`); a line number is that
//! tag's.
//!
//! - **Four labels of 256 KiB.** `vdev_label_t` is `vl_pad1` (8 KiB),
//!   `vl_be` (8 KiB), `vl_vdev_phys` (112 KiB) and `vl_uberblock`, the
//!   ring (128 KiB): `include/sys/vdev_impl.h` lines 478 to 482 and 535
//!   to 540. `VDEV_LABELS` is 4 (line 562).
//! - **Where they lie.** `vdev_label_offset` (`module/zfs/vdev_label.c`
//!   lines 163 to 170): labels 0 and 1 at the start of the vdev, at 0
//!   and 256 KiB, and labels 2 and 3 at its end, at `psize` − 512 KiB
//!   and `psize` − 256 KiB, where `psize` is the vdev's size rounded
//!   down to a whole label (the function asserts it is a multiple of
//!   `sizeof (vdev_label_t)`; `vdev_open` rounds the size down so).
//! - **The ring's slots.** A slot is `1 << VDEV_UBERBLOCK_SHIFT` bytes,
//!   the top-level vdev's `ashift` held between `UBERBLOCK_SHIFT` (10)
//!   and `MAX_UBERBLOCK_SHIFT` (13): 1 KiB to 8 KiB
//!   (`include/sys/vdev_impl.h` lines 490 to 499,
//!   `include/sys/uberblock_impl.h` line 47). Slot `n` begins `n` slots
//!   into the ring (`VDEV_UBERBLOCK_OFFSET`).
//! - **An uberblock write.** `vdev_uberblock_sync`
//!   (`module/zfs/vdev_label.c` lines 1781 to 1797) writes one slot in
//!   each of the four labels, the slot chosen by the group: the
//!   `uberblock_t` at the slot's start and zeros after it
//!   (`abd_zero_off`). The vdev's own write puts the label's checksum
//!   (`zio_eck_t`, 40 bytes) at the slot's end. Multihost
//!   (`module/zfs/mmp.c`, `mmp_write_uberblock`) writes a copy of the
//!   last synced uberblock, its timestamp and multihost fields changed,
//!   into the last slot of a label chosen at random.
//! - **A new label** (`vdev_label_init`, `module/zfs/vdev_label.c`
//!   lines 1213 to 1218 and 1245) writes the whole ring at once: one
//!   uberblock at its start whose group is 0, and zeros.
//! - **The uberblock** (`struct uberblock`,
//!   `include/sys/uberblock_impl.h` line 122): `ub_magic`,
//!   `ub_version`, `ub_txg`, `ub_guid_sum`, `ub_timestamp`, each 8
//!   bytes, then `ub_rootbp`, a `blkptr_t` of 128 bytes
//!   (`include/sys/spa.h` lines 374 to 382), then fields this crate
//!   does not read. `UBERBLOCK_MAGIC` is `0x00bab10c` (line 46).
//! - **Byte order.** An uberblock is written in the byte order of the
//!   host that wrote it, and read in either: `uberblock_verify`
//!   (`module/zfs/uberblock.c` lines 33 to 42) swaps one whose magic
//!   reads swapped.
//!
//! # The rules, as design §5.2 gives them
//!
//! - **What counts as an uberblock write** ([`Uberblock::counts`]): a
//!   write to an uberblock slot that carries the magic, in either byte
//!   order, and a transaction group above zero. A new label's group is
//!   0.
//! - **What counts as a repeat** ([`is_repeat`]): an uberblock whose
//!   group and root block pointer both equal those of the uberblock
//!   that marks the cut to come, or those of the newest cut (§5.2 as of
//!   2026-11-14). The group alone does not identify a pool state: after
//!   a rewind import some versions write a different state under a
//!   group already used. The newest cut is there for multihost, whose
//!   writes carry the last synced uberblock: one that still carries the
//!   group before can be applied after the group's own uberblock writes
//!   (`mmp_update_uberblock` is called only once they have all
//!   returned, `module/zfs/vdev_label.c` line 2141), and it equals the
//!   newest cut.
//!
//! What the store does with them (the mark of an applied uberblock
//! write, the cut at the barrier of the next flush, the count of cuts)
//! is the store's, in `skyzfs-local`.
//!
//! # Limits
//!
//! - **The labels are where a vdev on the whole volume has them.** The
//!   pool's device is the volume, its first label at byte 0, which the
//!   daemon rig pins (`label_at_byte_0`). A pool made on a partition of
//!   the volume has its labels elsewhere, and none of its uberblock
//!   writes is seen: such a pool makes no cuts.
//! - **The slot's size is not known**, since it follows the pool's
//!   `ashift`, which this crate is not told. Every position in a ring
//!   that is a whole number of the smallest slot (1 KiB) is looked at.
//!   A slot holds its uberblock at its start and zeros after it, up to
//!   the checksum in its last 40 bytes, which never begins at a whole
//!   KiB; so a larger slot shows nothing at its other KiB boundaries.
//! - **The label's checksum is not checked.** §5.2's rule is the magic
//!   and the group, and only ZFS's label writes reach a ring. A write
//!   that holds fewer than [`READ_LEN`] bytes of a slot from its start
//!   is not read: ZFS writes a slot whole.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// The size of a label, `sizeof (vdev_label_t)`.
pub const LABEL_SIZE: u64 = 256 << 10;
/// The labels on a vdev, `VDEV_LABELS`.
pub const LABELS: usize = 4;
/// `VDEV_PAD_SIZE`: `vl_pad1` and `vl_be` are each this long.
const PAD_SIZE: u64 = 8 << 10;
/// `VDEV_PHYS_SIZE`.
const PHYS_SIZE: u64 = 112 << 10;
/// Where the uberblock ring begins in a label,
/// `offsetof (vdev_label_t, vl_uberblock)`.
pub const RING_OFFSET: u64 = 2 * PAD_SIZE + PHYS_SIZE;
/// The ring's size, `VDEV_UBERBLOCK_RING`.
pub const RING_SIZE: u64 = 128 << 10;
/// The smallest slot, `1 << UBERBLOCK_SHIFT`: every slot begins at a
/// whole number of these into the ring.
pub const SLOT_MIN: u64 = 1 << 10;
/// The largest slot, `1 << MAX_UBERBLOCK_SHIFT`.
pub const SLOT_MAX: u64 = 1 << 13;
/// `UBERBLOCK_MAGIC`.
pub const MAGIC: u64 = 0x00ba_b10c;
/// Where `ub_txg` lies in an uberblock.
const TXG_AT: usize = 16;
/// Where `ub_timestamp` lies in an uberblock.
const TIMESTAMP_AT: usize = 32;
/// Where `ub_rootbp` lies in an uberblock.
const ROOT_AT: usize = 40;
/// The 64-bit words of a `blkptr_t`.
pub const ROOT_WORDS: usize = 16;
/// Where `ub_mmp_magic` lies: after the root block pointer and
/// `ub_software_version` (`uberblock_impl.h` 131–134).
const MMP_MAGIC_AT: usize = ROOT_AT + ROOT_WORDS * 8 + 8;
/// Where `ub_mmp_config` lies, after `ub_mmp_delay`
/// (`uberblock_impl.h` 139–159).
const MMP_CONFIG_AT: usize = MMP_MAGIC_AT + 16;
/// `MMP_MAGIC` (`uberblock_impl.h` 48): an uberblock written by a ZFS
/// that knows multihost.
pub const MMP_MAGIC: u64 = 0xa11c_ea11;
/// `MMP_SEQ_VALID_BIT` and `MMP_SEQ_MASK` (`uberblock_impl.h` 51, 55):
/// whether `ub_mmp_config` carries the multihost sequence, and where.
const MMP_SEQ_VALID_BIT: u64 = 0x02;
const MMP_SEQ_MASK: u64 = 0x0000_ffff_0000_0000;
/// The bytes of an uberblock this crate reads, from its start: the five
/// words up to `ub_timestamp`, the root block pointer, and the fields up
/// to `ub_mmp_config`, which ZFS's order among uberblocks reads
/// ([`Rank`]).
pub const READ_LEN: usize = MMP_CONFIG_AT + 8;

/// The labels of a vdev that is the whole volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
    /// The vdev's size as ZFS sees it: the volume's, rounded down to a
    /// whole label.
    psize: u64,
}

/// One label's uberblock ring: `RING_SIZE` bytes from `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ring {
    /// Which label, 0 to 3.
    pub label: usize,
    /// Where the ring begins on the volume, in bytes.
    pub start: u64,
}

impl Ring {
    /// Where the ring ends on the volume: the byte after it.
    pub fn end(&self) -> u64 {
        self.start + RING_SIZE
    }
}

impl Labels {
    /// The labels of a vdev on a volume of `size` bytes.
    pub fn for_size(size: u64) -> Self {
        Self {
            psize: size - size % LABEL_SIZE,
        }
    }

    /// The vdev's size as ZFS sees it.
    pub fn psize(&self) -> u64 {
        self.psize
    }

    /// Where label `label` begins (`vdev_label_offset`), or `None` where
    /// there is no such label: a label number above 3, or a volume too
    /// small to hold four labels.
    pub fn label_offset(&self, label: usize) -> Option<u64> {
        let all = LABELS as u64 * LABEL_SIZE;
        if label >= LABELS || self.psize < all {
            return None;
        }
        let label = label as u64;
        Some(if label < LABELS as u64 / 2 {
            label * LABEL_SIZE
        } else {
            label * LABEL_SIZE + self.psize - all
        })
    }

    /// The four rings, in the labels' order; none on a volume too small
    /// to hold four labels.
    pub fn rings(&self) -> impl Iterator<Item = Ring> + '_ {
        (0..LABELS).filter_map(|label| {
            self.label_offset(label).map(|at| Ring {
                label,
                start: at + RING_OFFSET,
            })
        })
    }

    /// Whether a write of `len` bytes at `offset` reaches a ring: the one
    /// cheap question asked of every write.
    pub fn reaches_a_ring(&self, offset: u64, len: u64) -> bool {
        let end = offset.saturating_add(len);
        self.rings()
            .any(|ring| offset < ring.end() && ring.start < end)
    }

    /// The uberblocks a write of `data` at `offset` carries, in the order
    /// of their place on the volume: at each whole KiB of a ring that the
    /// write covers with at least [`READ_LEN`] bytes, where the magic is,
    /// in either byte order. A group of 0 is among them
    /// ([`Uberblock::counts`] says whether it counts).
    pub fn uberblocks_in(&self, offset: u64, data: &[u8]) -> Vec<Uberblock> {
        let end = offset.saturating_add(data.len() as u64);
        let mut found = Vec::new();
        for ring in self.rings() {
            let from = offset.max(ring.start);
            let to = end.min(ring.end());
            // The first whole KiB at or after `from`; the rings begin at
            // one.
            let mut at = from.div_ceil(SLOT_MIN) * SLOT_MIN;
            while at + READ_LEN as u64 <= to {
                let i = (at - offset) as usize;
                if let Some(uberblock) = read_at(&data[i..i + READ_LEN], at, ring.label) {
                    found.push(uberblock);
                }
                at += SLOT_MIN;
            }
        }
        found
    }
}

/// The byte order an uberblock was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ByteOrder {
    /// Little endian: the magic reads `0x00bab10c` as little endian.
    Little,
    /// Big endian: the magic reads `0x00bab10c` as big endian.
    Big,
}

impl ByteOrder {
    fn word(self, bytes: &[u8]) -> u64 {
        let mut word = [0; 8];
        word.copy_from_slice(&bytes[..8]);
        match self {
            ByteOrder::Little => u64::from_le_bytes(word),
            ByteOrder::Big => u64::from_be_bytes(word),
        }
    }
}

/// The pool state an uberblock names: its transaction group and its
/// root block pointer, which §5.2's repeat compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct State {
    /// `ub_txg`.
    pub txg: u64,
    /// `ub_rootbp`, as 16 words in the uberblock's byte order.
    pub root: [u64; ROOT_WORDS],
}

/// Where an uberblock stands in ZFS's order among uberblocks, the order
/// in which an import chooses the one it opens
/// (`vdev_uberblock_compare`, `vdev_label.c` 1495): the transaction
/// group, then the timestamp, then the multihost sequence. The sequence
/// counts only where the uberblock carries `MMP_MAGIC` and its config
/// says the sequence is valid (`MMP_VALID` and `MMP_SEQ_VALID`); it is 0
/// otherwise, as there. The derived order compares the fields in that
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rank {
    /// `ub_txg`.
    pub txg: u64,
    /// `ub_timestamp`.
    pub timestamp: u64,
    /// `MMP_SEQ`, or 0.
    pub seq: u64,
}

/// An uberblock found in a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uberblock {
    /// Where its slot begins on the volume, in bytes.
    pub at: u64,
    /// The label whose ring holds it.
    pub label: usize,
    /// The byte order it was written in.
    pub order: ByteOrder,
    /// Its group and root block pointer.
    pub state: State,
    /// Its place in ZFS's order among uberblocks.
    pub rank: Rank,
}

impl Uberblock {
    /// Whether it counts as an uberblock write (§5.2): a group above
    /// zero. ZFS writes 0 into a new label's ring.
    pub fn counts(&self) -> bool {
        self.state.txg > 0
    }
}

/// Whether `next` is a repeat (§5.2): its group and root block pointer
/// both equal those of `mark`, the uberblock that marks the cut to come,
/// or those of `newest`, the newest cut's. Where there is neither, it is
/// not.
pub fn is_repeat(mark: Option<&State>, newest: Option<&State>, next: &State) -> bool {
    mark == Some(next) || newest == Some(next)
}

/// The uberblock at the start of `bytes` ([`READ_LEN`] of them), at `at`
/// on the volume in label `label`, if the magic is there.
fn read_at(bytes: &[u8], at: u64, label: usize) -> Option<Uberblock> {
    let order = [ByteOrder::Little, ByteOrder::Big]
        .into_iter()
        .find(|order| order.word(bytes) == MAGIC)?;
    let mut root = [0; ROOT_WORDS];
    for (n, word) in root.iter_mut().enumerate() {
        *word = order.word(&bytes[ROOT_AT + 8 * n..]);
    }
    let txg = order.word(&bytes[TXG_AT..]);
    let config = order.word(&bytes[MMP_CONFIG_AT..]);
    let seq = if order.word(&bytes[MMP_MAGIC_AT..]) == MMP_MAGIC && config & MMP_SEQ_VALID_BIT != 0
    {
        (config & MMP_SEQ_MASK) >> 32
    } else {
        0
    };
    Some(Uberblock {
        at,
        label,
        order,
        state: State { txg, root },
        rank: Rank {
            txg,
            timestamp: order.word(&bytes[TIMESTAMP_AT..]),
            seq,
        },
    })
}

/// The uberblocks in the four rings of a vdev on the whole volume, as
/// far as they are known: what an import chooses among
/// (`vdev_uberblock_load_impl`, `vdev_label.c` 1568, reads every slot
/// of every label and keeps the greatest by [`Rank`]). It is kept for
/// every whole KiB of the rings, the positions [`Labels::uberblocks_in`]
/// reads, since the slot's size is not known.
///
/// A position is known once a read or a write has covered its
/// [`READ_LEN`] bytes, and holds the uberblock found there, or none. A
/// read or write that covers only part of them leaves it unknown: what
/// is there then is not known. (The store's requests are whole 4 KiB
/// blocks, and no position's bytes cross a block, so that does not
/// happen through it.)
///
/// What it does not see: the label's checksum is not checked, so a slot
/// that ZFS rejects for its checksum is still in the view. That can only
/// make [`View::chosen`] say no where ZFS would open the state, not the
/// other way. And only this volume's labels are in it: a pool whose
/// other vdevs hold a greater uberblock is not seen.
#[derive(Debug, Clone)]
pub struct View {
    labels: Labels,
    /// The uberblock at each known position that holds one.
    held: std::collections::BTreeMap<u64, Uberblock>,
    /// The positions whose content is not known.
    unknown: std::collections::BTreeSet<u64>,
}

impl View {
    /// The view of rings whose content is not known yet.
    pub fn unknown(labels: Labels) -> Self {
        let unknown = labels
            .rings()
            .flat_map(|ring| (ring.start..ring.end()).step_by(SLOT_MIN as usize))
            .collect();
        Self {
            labels,
            held: Default::default(),
            unknown,
        }
    }

    /// The view of rings known to hold no uberblock.
    pub fn empty(labels: Labels) -> Self {
        Self {
            labels,
            held: Default::default(),
            unknown: Default::default(),
        }
    }

    /// Whether every position is known.
    pub fn is_known(&self) -> bool {
        self.unknown.is_empty()
    }

    /// The uberblocks in the view, by position.
    pub fn uberblocks(&self) -> impl Iterator<Item = &Uberblock> {
        self.held.values()
    }

    /// `data` was read or written at `offset`.
    pub fn saw(&mut self, offset: u64, data: &[u8]) {
        let found = self.labels.uberblocks_in(offset, data);
        self.changed(offset, data.len() as u64, &found);
    }

    /// `len` bytes at `offset` now read as zeros.
    pub fn zeroed(&mut self, offset: u64, len: u64) {
        self.changed(offset, len, &[]);
    }

    /// `len` bytes at `offset` were written, and `found` is what
    /// [`Labels::uberblocks_in`] found in them: for a caller that parsed
    /// the write already.
    pub fn changed(&mut self, offset: u64, len: u64, found: &[Uberblock]) {
        let end = offset.saturating_add(len);
        for ring in self.labels.rings() {
            let mut at = offset.max(ring.start) / SLOT_MIN * SLOT_MIN;
            while at < end.min(ring.end()) {
                let whole = offset <= at && at + READ_LEN as u64 <= end;
                if at + READ_LEN as u64 > offset {
                    self.held.remove(&at);
                    if whole {
                        self.unknown.remove(&at);
                    } else {
                        self.unknown.insert(at);
                    }
                }
                at += SLOT_MIN;
            }
        }
        for uberblock in found {
            self.held.insert(uberblock.at, *uberblock);
        }
    }

    /// Whether an import of the volume as the view has it would open the
    /// pool state `state`: every position is known, and every uberblock
    /// that is greatest by ZFS's order carries `state`. Where two of
    /// different states are equal by the order, ZFS keeps whichever it
    /// read first, so neither is said to be chosen. With `group_only`,
    /// the order is the group alone (a defect, for tests).
    pub fn chosen(&self, state: &State, group_only: bool) -> bool {
        let rank = |uberblock: &Uberblock| {
            if group_only {
                Rank {
                    txg: uberblock.rank.txg,
                    timestamp: 0,
                    seq: 0,
                }
            } else {
                uberblock.rank
            }
        };
        let Some(best) = self.held.values().map(rank).max() else {
            return false;
        };
        self.is_known()
            && self
                .held
                .values()
                .filter(|uberblock| rank(uberblock) == best)
                .all(|uberblock| uberblock.state == *state)
    }
}

#[cfg(test)]
mod tests;
