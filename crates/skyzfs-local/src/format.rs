//! The write log's bytes. The layout is described in the
//! [crate documentation](crate#layout); this module encodes and decodes
//! it and does no I/O.
//!
//! Every integer is little-endian. Every structure carries a version and
//! reserved bytes, which are written as zero and required to be zero.

use std::fmt;

/// The version of the format this code writes and reads.
pub const VERSION: u32 = 1;

/// The bytes of one superblock copy, of a segment header and of the
/// unit a disk keeps or loses.
pub const SECTOR_LEN: usize = 512;

/// The smallest write unit a log can have: see [`Identity::write_unit`].
pub const MIN_WRITE_UNIT: u32 = 4096;

/// The largest write unit a log can have.
pub const MAX_WRITE_UNIT: u32 = 65536;

/// Where the second superblock copy starts in its file. The first starts
/// at 0. They are a largest write unit apart, so that writing one never
/// rewrites the other.
pub const SUPER_COPY_STRIDE: u64 = MAX_WRITE_UNIT as u64;

/// The bytes of a record header.
pub const RECORD_HEADER: usize = 64;

/// The largest payload a record can carry, whatever the segment size.
pub const MAX_DATA: u32 = 1 << 20;

const SUPER_MAGIC: &[u8; 8] = b"SKYZLOG\0";
const SEGMENT_MAGIC: &[u8; 8] = b"SKYZSEG\0";
const RECORD_MAGIC: &[u8; 4] = b"SKZR";

/// A position in the log: a byte offset in an unbounded space, always on
/// a record boundary. Segment `k` of a log with segments of `S` bytes
/// holds positions `k * S .. (k + 1) * S`, and its first record is one
/// write unit in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lsn(pub u64);

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// Why bytes are not the structure they should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    /// Every byte is zero: nothing was ever written here, or recovery
    /// erased it.
    Blank,
    /// The magic number is wrong.
    Magic,
    /// A version this code does not read.
    Version,
    /// The checksum or the keyed tag does not match. For a record header
    /// this includes a header that does not follow the record before
    /// it, and bytes that only look like a header.
    Checksum,
    /// A field has a value the format does not allow.
    Field,
}

/// The log's key: 128 random bits drawn when the log is created and
/// stored in the superblock, nowhere else. Every record header and
/// segment header carries a tag computed with it, so that nothing a
/// client writes into a payload can pass for a header. It is never
/// shown: not in a debug print, a report or an error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key(pub(crate) [u8; 16]);

impl Key {
    /// A key that everybody knows: what the log would be without one.
    /// Only the tests' deliberately broken log uses it.
    pub(crate) const PUBLIC: Key = Key([0; 16]);

    /// A new key from the operating system's cryptographic generator,
    /// or `None` if it gives nothing. There is no weaker source to fall
    /// back on: without a key no log is made.
    pub(crate) fn random() -> Option<Key> {
        let mut key = [0u8; 16];
        getrandom::fill(&mut key).ok()?;
        Some(Key(key))
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(..)")
    }
}

/// SipHash-2-4 with a 64-bit result (Aumasson and Bernstein, 2012): the
/// keyed function behind every tag.
pub fn siphash24(key: &Key, data: &[u8]) -> u64 {
    let k0 = u64_at(&key.0, 0);
    let k1 = u64_at(&key.0, 8);
    let mut v = [
        k0 ^ 0x736f_6d65_7073_6575,
        k1 ^ 0x646f_7261_6e64_6f6d,
        k0 ^ 0x6c79_6765_6e65_7261,
        k1 ^ 0x7465_6462_7974_6573,
    ];
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13);
        v[1] ^= v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16);
        v[3] ^= v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21);
        v[3] ^= v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17);
        v[1] ^= v[2];
        v[2] = v[2].rotate_left(32);
    }
    let (words, rest) = data.as_chunks::<8>();
    for word in words {
        let m = u64::from_le_bytes(*word);
        v[3] ^= m;
        round(&mut v);
        round(&mut v);
        v[0] ^= m;
    }
    let mut last = [0u8; 8];
    last[..rest.len()].copy_from_slice(rest);
    last[7] = data.len() as u8;
    let m = u64::from_le_bytes(last);
    v[3] ^= m;
    round(&mut v);
    round(&mut v);
    v[0] ^= m;
    v[2] ^= 0xff;
    for _ in 0..4 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

/// The tag of `fields` behind `before`: a keyed function of both.
fn tag(key: &Key, before: u64, fields: &[u8]) -> u64 {
    let mut input = [0u8; 8 + SECTOR_LEN];
    input[..8].copy_from_slice(&before.to_le_bytes());
    input[8..8 + fields.len()].copy_from_slice(fields);
    siphash24(key, &input[..8 + fields.len()])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(raw)
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

fn zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| *b == 0)
}

/// What a log is for and how it is laid out. Fixed when the log is
/// created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// The volume the log belongs to.
    pub volume: [u8; 16],
    /// The volume's size in bytes.
    pub device_size: u64,
    /// The volume's block size in bytes.
    pub block_size: u32,
    /// What every record's header and payload are padded to: 64 or 4096.
    pub align: u32,
    /// The bytes of one segment.
    pub segment_size: u64,
    /// The write unit: what the layers below rewrite as a whole when a
    /// part of it is written. For buffered I/O that is the kernel's
    /// page, and it is never less than a disk's physical sector. A
    /// power of two from 4096 to 65536. The log never writes again into
    /// a unit that holds a durable record: the first record after a
    /// sync or a recovery starts at the next multiple of this. A
    /// segment's first unit is its header and its records start at the
    /// second.
    pub write_unit: u32,
}

impl Identity {
    /// The write unit to give a new log on this system's files: the
    /// kernel's page size, and at least 4096.
    pub fn system_write_unit() -> u32 {
        crate::fs::page_size().clamp(MIN_WRITE_UNIT, MAX_WRITE_UNIT)
    }

    /// Whether the values can describe a log.
    pub fn valid(&self) -> bool {
        (self.align == 64 || self.align == 4096)
            && self.write_unit.is_power_of_two()
            && (MIN_WRITE_UNIT..=MAX_WRITE_UNIT).contains(&self.write_unit)
            && self.segment_size.is_multiple_of(self.unit())
            && self.segment_size >= 4 * self.unit()
            && self.block_size > 0
    }

    /// The write unit, as a number of bytes.
    pub fn unit(&self) -> u64 {
        u64::from(self.write_unit)
    }

    /// `x` rounded up to a multiple of the write unit.
    pub fn unit_up(&self, x: u64) -> u64 {
        x.div_ceil(self.unit()) * self.unit()
    }

    /// The bytes a record with `data_len` bytes of payload takes.
    pub fn record_size(&self, data_len: u32) -> u64 {
        let align = u64::from(self.align);
        align + u64::from(data_len).div_ceil(align) * align
    }

    /// Whether a record with `data_len` bytes of payload may start at
    /// byte `at` of a segment: behind it, at the next unit boundary,
    /// there must still be room for the marker that closes the segment.
    pub fn fits(&self, at: u64, data_len: u32) -> bool {
        self.unit_up(at + self.record_size(data_len)) + u64::from(self.align) <= self.segment_size
    }

    /// The largest payload a record of this log can carry: the segment's
    /// first unit is its header and its last is kept for the marker that
    /// closes it.
    pub fn max_data(&self) -> u32 {
        let align = u64::from(self.align);
        let room = (self.segment_size - 2 * self.unit()).saturating_sub(align);
        (room / align * align).min(u64::from(MAX_DATA)) as u32
    }

    /// The first position of the segment that holds `position`.
    pub fn base_of(&self, position: Lsn) -> u64 {
        position.0 / self.segment_size * self.segment_size
    }
}

/// The superblock: the only structure that is updated in place. Two
/// copies are written alternately; the valid one with the higher counter
/// counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    /// What the log is for.
    pub identity: Identity,
    /// The log's key.
    pub(crate) key: Key,
    /// Incremented at every write of a copy.
    pub counter: u64,
    /// Incremented at every write of a superblock copy: by every
    /// recovery and every reclaim. A record is written with the epoch
    /// the superblock has at that moment.
    pub epoch: u64,
    /// Where the log starts: everything before it is reclaimed.
    pub floor: Lsn,
    /// A position that was durable when this copy was written. A
    /// recovery that finds less has found corruption.
    pub durable_floor: Lsn,
}

impl Superblock {
    /// The bytes of one copy.
    pub fn encode(&self) -> [u8; SECTOR_LEN] {
        let mut out = [0u8; SECTOR_LEN];
        out[0..8].copy_from_slice(SUPER_MAGIC);
        out[8..12].copy_from_slice(&VERSION.to_le_bytes());
        out[16..32].copy_from_slice(&self.identity.volume);
        out[32..40].copy_from_slice(&self.identity.device_size.to_le_bytes());
        out[40..44].copy_from_slice(&self.identity.block_size.to_le_bytes());
        out[44..48].copy_from_slice(&self.identity.align.to_le_bytes());
        out[48..56].copy_from_slice(&self.identity.segment_size.to_le_bytes());
        out[56..64].copy_from_slice(&self.counter.to_le_bytes());
        out[64..72].copy_from_slice(&self.epoch.to_le_bytes());
        out[72..80].copy_from_slice(&self.floor.0.to_le_bytes());
        out[80..88].copy_from_slice(&self.durable_floor.0.to_le_bytes());
        out[88..92].copy_from_slice(&self.identity.write_unit.to_le_bytes());
        out[92..108].copy_from_slice(&self.key.0);
        let crc = crc32c::crc32c(&out[..SECTOR_LEN - 4]);
        out[SECTOR_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// Read one copy.
    pub fn decode(bytes: &[u8; SECTOR_LEN]) -> Result<Self, Bad> {
        if zero(bytes) {
            return Err(Bad::Blank);
        }
        if &bytes[0..8] != SUPER_MAGIC {
            return Err(Bad::Magic);
        }
        if u32_at(bytes, SECTOR_LEN - 4) != crc32c::crc32c(&bytes[..SECTOR_LEN - 4]) {
            return Err(Bad::Checksum);
        }
        if u32_at(bytes, 8) != VERSION {
            return Err(Bad::Version);
        }
        let mut volume = [0u8; 16];
        volume.copy_from_slice(&bytes[16..32]);
        let mut key = [0u8; 16];
        key.copy_from_slice(&bytes[92..108]);
        // No log is made with the key everybody knows.
        if zero(&key) {
            return Err(Bad::Field);
        }
        let block = Self {
            key: Key(key),
            identity: Identity {
                volume,
                device_size: u64_at(bytes, 32),
                block_size: u32_at(bytes, 40),
                align: u32_at(bytes, 44),
                segment_size: u64_at(bytes, 48),
                write_unit: u32_at(bytes, 88),
            },
            counter: u64_at(bytes, 56),
            epoch: u64_at(bytes, 64),
            floor: Lsn(u64_at(bytes, 72)),
            durable_floor: Lsn(u64_at(bytes, 80)),
        };
        let reserved = zero(&bytes[12..16]) && zero(&bytes[108..SECTOR_LEN - 4]);
        if !reserved || !block.identity.valid() || block.floor > block.durable_floor {
            return Err(Bad::Field);
        }
        Ok(block)
    }
}

/// The header at the start of a segment's file. It says which part of
/// the log the file holds now; the file's name says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentHeader {
    /// The volume the log belongs to.
    pub volume: [u8; 16],
    /// The position of the segment's first byte.
    pub base: u64,
    /// The epoch in which the file was given this base.
    pub epoch: u64,
}

impl SegmentHeader {
    /// The header's bytes, tagged with the log's key.
    pub fn encode(&self, key: &Key) -> [u8; SECTOR_LEN] {
        let mut out = [0u8; SECTOR_LEN];
        out[0..8].copy_from_slice(SEGMENT_MAGIC);
        out[8..12].copy_from_slice(&VERSION.to_le_bytes());
        out[16..32].copy_from_slice(&self.volume);
        out[32..40].copy_from_slice(&self.base.to_le_bytes());
        out[40..48].copy_from_slice(&self.epoch.to_le_bytes());
        let tag = tag(key, 0, &out[..SECTOR_LEN - 8]);
        out[SECTOR_LEN - 8..].copy_from_slice(&tag.to_le_bytes());
        out
    }

    /// Read a header written with the log's key.
    pub fn decode(bytes: &[u8; SECTOR_LEN], key: &Key) -> Result<Self, Bad> {
        if zero(bytes) {
            return Err(Bad::Blank);
        }
        if &bytes[0..8] != SEGMENT_MAGIC {
            return Err(Bad::Magic);
        }
        if u64_at(bytes, SECTOR_LEN - 8) != tag(key, 0, &bytes[..SECTOR_LEN - 8]) {
            return Err(Bad::Checksum);
        }
        if u32_at(bytes, 8) != VERSION {
            return Err(Bad::Version);
        }
        if !zero(&bytes[12..16]) || !zero(&bytes[48..SECTOR_LEN - 8]) {
            return Err(Bad::Field);
        }
        let mut volume = [0u8; 16];
        volume.copy_from_slice(&bytes[16..32]);
        Ok(Self {
            volume,
            base: u64_at(bytes, 32),
            epoch: u64_at(bytes, 40),
        })
    }
}

/// What a record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Data written to the volume. The payload is the data.
    Write,
    /// A range of the volume set to zeros. No payload.
    WriteZeroes,
    /// A range of the volume discarded. No payload.
    Discard,
    /// The rest of the segment is unused; the log continues in the next.
    /// Written by the log itself and never returned by a replay.
    Pad,
    /// Nothing: a record a recovery writes at the end of the log it
    /// found, to take the place of whatever was there. Never returned by
    /// a replay.
    Mark,
}

impl Kind {
    fn code(self) -> u8 {
        match self {
            Kind::Write => 1,
            Kind::WriteZeroes => 2,
            Kind::Discard => 3,
            Kind::Pad => 4,
            Kind::Mark => 5,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Kind::Write,
            2 => Kind::WriteZeroes,
            3 => Kind::Discard,
            4 => Kind::Pad,
            5 => Kind::Mark,
            _ => return None,
        })
    }
}

/// The header of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    /// What the record is.
    pub kind: Kind,
    /// The record's own position in the log.
    pub position: Lsn,
    /// The epoch the superblock had when the record was written.
    pub epoch: u64,
    /// The byte offset on the volume it applies to.
    pub offset: u64,
    /// The number of bytes of the volume it applies to. For a pad, the
    /// bytes it skips.
    pub length: u64,
    /// The bytes of payload that follow the header.
    pub data_len: u32,
    /// The CRC32C of the payload.
    pub data_crc: u32,
}

/// The value the first record of a segment is chained to: a keyed
/// function of the volume and the segment's base.
pub fn chain_seed(key: &Key, volume: &[u8; 16], base: u64) -> u64 {
    let mut input = [0u8; 24];
    input[..16].copy_from_slice(volume);
    input[16..].copy_from_slice(&base.to_le_bytes());
    siphash24(key, &input)
}

impl RecordHeader {
    /// The header's bytes, chained to `chain`: the tag of the record
    /// before it, or [`chain_seed`] at the start of a segment. Returns
    /// the bytes and this header's tag, which the next record is chained
    /// to.
    ///
    /// The tag is a keyed function of `chain` and of every other byte of
    /// the header: kind, position, epoch, the range of the volume, the
    /// payload's length and the payload's checksum. Nothing a recovery
    /// acts on is outside it.
    pub fn encode(&self, key: &Key, chain: u64) -> ([u8; RECORD_HEADER], u64) {
        let mut out = [0u8; RECORD_HEADER];
        out[0..4].copy_from_slice(RECORD_MAGIC);
        out[4] = VERSION as u8;
        out[5] = self.kind.code();
        out[8..16].copy_from_slice(&self.position.0.to_le_bytes());
        out[16..24].copy_from_slice(&self.epoch.to_le_bytes());
        out[24..32].copy_from_slice(&self.offset.to_le_bytes());
        out[32..40].copy_from_slice(&self.length.to_le_bytes());
        out[40..44].copy_from_slice(&self.data_len.to_le_bytes());
        out[44..48].copy_from_slice(&self.data_crc.to_le_bytes());
        let tag = tag(key, chain, &out[..RECORD_HEADER - 8]);
        out[RECORD_HEADER - 8..].copy_from_slice(&tag.to_le_bytes());
        (out, tag)
    }

    /// Read a header that must follow `chain`. Returns it with its tag.
    pub fn decode(bytes: &[u8; RECORD_HEADER], key: &Key, chain: u64) -> Result<(Self, u64), Bad> {
        if zero(bytes) {
            return Err(Bad::Blank);
        }
        if &bytes[0..4] != RECORD_MAGIC {
            return Err(Bad::Magic);
        }
        let stored = u64_at(bytes, RECORD_HEADER - 8);
        if stored != tag(key, chain, &bytes[..RECORD_HEADER - 8]) {
            return Err(Bad::Checksum);
        }
        if u32::from(bytes[4]) != VERSION {
            return Err(Bad::Version);
        }
        let Some(kind) = Kind::from_code(bytes[5]) else {
            return Err(Bad::Field);
        };
        if !zero(&bytes[6..8]) || !zero(&bytes[48..RECORD_HEADER - 8]) {
            return Err(Bad::Field);
        }
        let header = Self {
            kind,
            position: Lsn(u64_at(bytes, 8)),
            epoch: u64_at(bytes, 16),
            offset: u64_at(bytes, 24),
            length: u64_at(bytes, 32),
            data_len: u32_at(bytes, 40),
            data_crc: u32_at(bytes, 44),
        };
        if kind != Kind::Write && header.data_len != 0 {
            return Err(Bad::Field);
        }
        Ok((header, stored))
    }

    /// Whether `bytes` look like a record header that says it is at
    /// `position`, without the key. For reports only: nothing is ever
    /// replayed on the strength of this.
    pub fn looks_like(bytes: &[u8; RECORD_HEADER], position: Lsn) -> bool {
        &bytes[0..4] == RECORD_MAGIC && u64_at(bytes, 8) == position.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            volume: *b"0123456789abcdef",
            device_size: 1 << 30,
            block_size: 4096,
            align: 64,
            segment_size: 1 << 16,
            write_unit: 4096,
        }
    }

    /// The key of the SipHash paper's test vectors: bytes 0 to 15.
    const KEY: Key = Key([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);

    /// The checksum is CRC32C (Castagnoli), by its standard check value,
    /// so the golden bytes below do not rest on one library's word.
    #[test]
    fn the_checksum_is_crc32c() {
        assert_eq!(crc32c::crc32c(b"123456789"), 0xE306_9283);
    }

    /// The keyed function is SipHash-2-4, by the test vectors of its
    /// reference implementation: key 00..0f, messages of bytes 00, 01,
    /// ... of every length from 0 to 15, which crosses a word boundary.
    #[test]
    fn the_tag_function_is_siphash_2_4() {
        let vectors: [u64; 16] = [
            0x726f_db47_dd0e_0e31,
            0x74f8_39c5_93dc_67fd,
            0x0d6c_8009_d9a9_4f5a,
            0x8567_6696_d7fb_7e2d,
            0xcf27_94e0_2771_87b7,
            0x1876_5564_cd99_a68d,
            0xcbc9_466e_58fe_e3ce,
            0xab02_00f5_8b01_d137,
            0x93f5_f579_9a93_2462,
            0x9e00_82df_0ba9_e4b0,
            0x7a5d_bbc5_94dd_b9f3,
            0xf4b3_2f46_226b_ada7,
            0x751e_8fbc_860e_e5fb,
            0x14ea_5627_c084_3d90,
            0xf723_ca90_8e7a_f2ee,
            0xa129_ca61_49be_45e5,
        ];
        let message: Vec<u8> = (0..15).collect();
        for (len, want) in vectors.iter().enumerate() {
            assert_eq!(siphash24(&KEY, &message[..len]), *want, "length {len}");
        }
        // The key matters, and it is never printed.
        assert_ne!(siphash24(&Key::PUBLIC, &message), vectors[15]);
        assert_eq!(format!("{KEY:?}"), "Key(..)");
    }

    /// Format, version 1: the superblock's bytes are fixed. A change to
    /// them needs a new version and a test that still reads these.
    #[test]
    fn superblock_golden_bytes() {
        let block = Superblock {
            identity: identity(),
            key: KEY,
            counter: 3,
            epoch: 5,
            floor: Lsn(4096),
            durable_floor: Lsn(8192),
        };
        let bytes = block.encode();
        assert_eq!(&bytes[..12], b"SKYZLOG\0\x01\0\0\0");
        assert_eq!(&bytes[16..32], b"0123456789abcdef");
        assert_eq!(bytes[32..40], (1u64 << 30).to_le_bytes());
        assert_eq!(bytes[40..44], 4096u32.to_le_bytes());
        assert_eq!(bytes[44..48], 64u32.to_le_bytes());
        assert_eq!(bytes[48..56], (1u64 << 16).to_le_bytes());
        assert_eq!(bytes[56..64], 3u64.to_le_bytes());
        assert_eq!(bytes[64..72], 5u64.to_le_bytes());
        assert_eq!(bytes[72..80], 4096u64.to_le_bytes());
        assert_eq!(bytes[80..88], 8192u64.to_le_bytes());
        assert_eq!(bytes[88..92], 4096u32.to_le_bytes());
        assert_eq!(bytes[92..108], KEY.0);
        assert!(bytes[108..508].iter().all(|b| *b == 0));
        assert_eq!(bytes[508..], 0x7DFB_028Eu32.to_le_bytes());
        assert_eq!(Superblock::decode(&bytes), Ok(block));
        // A debug print of a superblock does not show the key.
        assert!(!format!("{block:?}").contains("15"));
    }

    /// Format, version 1: the segment header's bytes are fixed, and its
    /// tag depends on the key.
    #[test]
    fn segment_header_golden_bytes() {
        let header = SegmentHeader {
            volume: *b"0123456789abcdef",
            base: 1 << 16,
            epoch: 7,
        };
        let bytes = header.encode(&KEY);
        assert_eq!(&bytes[..12], b"SKYZSEG\0\x01\0\0\0");
        assert_eq!(&bytes[16..32], b"0123456789abcdef");
        assert_eq!(bytes[32..40], (1u64 << 16).to_le_bytes());
        assert_eq!(bytes[40..48], 7u64.to_le_bytes());
        assert!(bytes[48..504].iter().all(|b| *b == 0));
        assert_eq!(bytes[504..], 0xA123_3219_BAD1_D58Eu64.to_le_bytes());
        assert_eq!(SegmentHeader::decode(&bytes, &KEY), Ok(header));
        assert_eq!(
            SegmentHeader::decode(&bytes, &Key::PUBLIC),
            Err(Bad::Checksum)
        );
    }

    /// Format, version 1: the record header's bytes are fixed, and its
    /// tag depends on the key and on what it is chained to.
    #[test]
    fn record_header_golden_bytes() {
        let header = RecordHeader {
            kind: Kind::Write,
            position: Lsn(4096),
            epoch: 2,
            offset: 8192,
            length: 4096,
            data_len: 4096,
            data_crc: 0xDEAD_BEEF,
        };
        let seed = chain_seed(&KEY, b"0123456789abcdef", 0);
        assert_eq!(seed, 0x2204_A407_D865_B0FF);
        let (bytes, tag) = header.encode(&KEY, seed);
        assert_eq!(&bytes[..8], b"SKZR\x01\x01\0\0");
        assert_eq!(bytes[8..16], 4096u64.to_le_bytes());
        assert_eq!(bytes[16..24], 2u64.to_le_bytes());
        assert_eq!(bytes[24..32], 8192u64.to_le_bytes());
        assert_eq!(bytes[32..40], 4096u64.to_le_bytes());
        assert_eq!(bytes[40..44], 4096u32.to_le_bytes());
        assert_eq!(bytes[44..48], 0xDEAD_BEEFu32.to_le_bytes());
        assert!(bytes[48..56].iter().all(|b| *b == 0));
        assert_eq!(tag, 0x267C_1166_1407_E8FD);
        assert_eq!(bytes[56..], tag.to_le_bytes());
        assert_eq!(RecordHeader::decode(&bytes, &KEY, seed), Ok((header, tag)));
        // Chained to anything else, or under another key, it is not this
        // record.
        assert_eq!(
            RecordHeader::decode(&bytes, &KEY, seed ^ 1),
            Err(Bad::Checksum)
        );
        assert_eq!(
            RecordHeader::decode(&bytes, &Key::PUBLIC, seed),
            Err(Bad::Checksum)
        );
        assert_ne!(header.encode(&KEY, seed ^ 1).1, tag);
        assert_ne!(chain_seed(&KEY, b"0123456789abcdef", 1 << 16), seed);
        assert!(RecordHeader::looks_like(&bytes, Lsn(4096)));
        assert!(!RecordHeader::looks_like(&bytes, Lsn(4160)));
    }

    /// The tag covers every field a recovery acts on: changing any one
    /// of kind, position, epoch, the volume range, the payload length or
    /// the payload checksum under a genuine tag is refused.
    #[test]
    fn the_tag_covers_every_field() {
        let header = RecordHeader {
            kind: Kind::Write,
            position: Lsn(4096),
            epoch: 2,
            offset: 8192,
            length: 4096,
            data_len: 4096,
            data_crc: 0xDEAD_BEEF,
        };
        let (genuine, tag) = header.encode(&KEY, 9);
        let others = [
            RecordHeader {
                kind: Kind::Discard,
                data_len: 0,
                ..header
            },
            RecordHeader {
                position: Lsn(4160),
                ..header
            },
            RecordHeader { epoch: 3, ..header },
            RecordHeader {
                offset: 0,
                ..header
            },
            RecordHeader {
                length: 8192,
                ..header
            },
            RecordHeader {
                data_len: 4097,
                ..header
            },
            RecordHeader {
                data_crc: 1,
                ..header
            },
        ];
        for other in others {
            // The other record's fields under the genuine record's tag.
            let (mut forged, other_tag) = other.encode(&KEY, 9);
            assert_ne!(other_tag, tag);
            forged[56..].copy_from_slice(&genuine[56..]);
            assert_eq!(
                RecordHeader::decode(&forged, &KEY, 9),
                Err(Bad::Checksum),
                "{other:?}"
            );
        }
    }

    /// Format: every way a structure can be wrong is told apart, and a
    /// single flipped bit anywhere is never accepted.
    #[test]
    fn damaged_structures_are_refused() {
        let block = Superblock {
            identity: identity(),
            key: KEY,
            counter: 1,
            epoch: 1,
            floor: Lsn(4096),
            durable_floor: Lsn(4096),
        };
        let good = block.encode();
        assert_eq!(Superblock::decode(&[0; SECTOR_LEN]), Err(Bad::Blank));
        // The key everybody knows is no key.
        let unkeyed = Superblock {
            key: Key::PUBLIC,
            ..block
        };
        assert_eq!(Superblock::decode(&unkeyed.encode()), Err(Bad::Field));
        for bit in 0..SECTOR_LEN * 8 {
            let mut bytes = good;
            bytes[bit / 8] ^= 1 << (bit % 8);
            assert!(Superblock::decode(&bytes).is_err(), "bit {bit}");
        }
        let segment = SegmentHeader {
            volume: [1; 16],
            base: 0,
            epoch: 1,
        }
        .encode(&KEY);
        assert_eq!(
            SegmentHeader::decode(&[0; SECTOR_LEN], &KEY),
            Err(Bad::Blank)
        );
        for bit in 0..SECTOR_LEN * 8 {
            let mut bytes = segment;
            bytes[bit / 8] ^= 1 << (bit % 8);
            assert!(SegmentHeader::decode(&bytes, &KEY).is_err(), "bit {bit}");
        }
        let header = RecordHeader {
            kind: Kind::Discard,
            position: Lsn(4096),
            epoch: 1,
            offset: 0,
            length: 4096,
            data_len: 0,
            data_crc: 0,
        };
        let (record, _) = header.encode(&KEY, 9);
        assert_eq!(
            RecordHeader::decode(&[0; RECORD_HEADER], &KEY, 9),
            Err(Bad::Blank)
        );
        for bit in 0..RECORD_HEADER * 8 {
            let mut bytes = record;
            bytes[bit / 8] ^= 1 << (bit % 8);
            assert!(RecordHeader::decode(&bytes, &KEY, 9).is_err(), "bit {bit}");
        }
        // Fields the format does not allow, under a valid tag.
        let retag = |mut bytes: [u8; RECORD_HEADER]| {
            let tag = tag(&KEY, 9, &bytes[..RECORD_HEADER - 8]);
            bytes[RECORD_HEADER - 8..].copy_from_slice(&tag.to_le_bytes());
            RecordHeader::decode(&bytes, &KEY, 9)
        };
        let mut bytes = record;
        bytes[5] = 9;
        assert_eq!(retag(bytes), Err(Bad::Field));
        let mut bytes = record;
        bytes[4] = 2;
        assert_eq!(retag(bytes), Err(Bad::Version));
        let mut bytes = record;
        bytes[50] = 1;
        assert_eq!(retag(bytes), Err(Bad::Field));
        let mut bytes = record;
        bytes[40] = 1;
        assert_eq!(retag(bytes), Err(Bad::Field));
    }

    /// Layout arithmetic: record sizes at both alignments, the largest
    /// payload a segment allows, and what an identity must satisfy.
    #[test]
    fn sizes_follow_the_alignment() {
        let small = identity();
        assert_eq!(small.record_size(0), 64);
        assert_eq!(small.record_size(1), 128);
        assert_eq!(small.record_size(4096), 64 + 4096);
        let direct = Identity {
            align: 4096,
            ..small
        };
        assert_eq!(direct.record_size(0), 4096);
        assert_eq!(direct.record_size(4096), 8192);
        assert_eq!(direct.record_size(4097), 3 * 4096);
        assert_eq!(small.max_data(), (1 << 16) - 2 * 4096 - 64);
        assert_eq!(direct.max_data(), (1 << 16) - 3 * 4096);
        // The largest record fits at the start of a segment and nowhere
        // later; the last block stays free for the closing marker.
        assert!(small.fits(4096, small.max_data()));
        assert!(!small.fits(4160, small.max_data()));
        assert!(direct.fits(4096, direct.max_data()));
        assert!(!direct.fits(8192, direct.max_data()));
        assert!(small.fits((1 << 16) - 8192, 4032));
        assert!(!small.fits((1 << 16) - 8192, 4033));
        assert_eq!(small.unit_up(4096), 4096);
        assert_eq!(small.unit_up(4097), 8192);
        // A larger write unit: the header and the closing marker take a
        // unit each.
        let paged = Identity {
            write_unit: 16384,
            segment_size: 1 << 17,
            ..small
        };
        assert!(paged.valid());
        assert_eq!(paged.unit_up(4097), 16384);
        assert_eq!(paged.max_data(), (1 << 17) - 2 * 16384 - 64);
        assert!(paged.fits(16384, paged.max_data()));
        assert!(!paged.fits(16384 + 64, paged.max_data()));
        for write_unit in [0, 512, 2048, 4097, 12288, 131072] {
            assert!(
                !Identity {
                    write_unit,
                    ..paged
                }
                .valid(),
                "{write_unit}"
            );
        }
        assert!(!Identity {
            segment_size: (1 << 17) + 4096,
            ..paged
        }
        .valid());
        let big = Identity {
            segment_size: 64 << 20,
            ..small
        };
        assert_eq!(big.max_data(), MAX_DATA);
        assert_eq!(small.base_of(Lsn((1 << 16) + 5000)), 1 << 16);
        assert!(small.valid() && direct.valid() && big.valid());
        assert!(!Identity {
            align: 512,
            ..small
        }
        .valid());
        assert!(!Identity {
            segment_size: 5000,
            ..small
        }
        .valid());
        assert!(!Identity {
            segment_size: 3 * 4096,
            ..small
        }
        .valid());
        assert_eq!(Lsn(7).to_string(), "@7");
    }
}
