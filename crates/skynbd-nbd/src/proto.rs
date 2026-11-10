//! NBD wire protocol: constants and frame codecs (design §8).
//!
//! Only fixed-newstyle negotiation and simple replies are implemented in
//! M2 (structured replies + BLOCK_STATUS arrive in M4, design §8.3).
//! Every constant is pinned by golden-byte tests against the NBD spec.

/// "NBDMAGIC" — initial server greeting.
pub const NBDMAGIC: u64 = 0x4e42_444d_4147_4943;
/// "IHAVEOPT" — second greeting word + client option magic.
pub const IHAVEOPT: u64 = 0x4948_4156_454f_5054;
/// Server option-reply magic.
pub const OPT_REPLY_MAGIC: u64 = 0x0003_e889_0455_65a9;
/// Transmission request magic.
pub const REQUEST_MAGIC: u32 = 0x2560_9513;
/// Simple reply magic.
pub const SIMPLE_REPLY_MAGIC: u32 = 0x6744_6698;

// Handshake / client flags.
pub const FLAG_FIXED_NEWSTYLE: u16 = 1 << 0;
pub const FLAG_NO_ZEROES: u16 = 1 << 1;
pub const CFLAG_FIXED_NEWSTYLE: u32 = 1 << 0;
pub const CFLAG_NO_ZEROES: u32 = 1 << 1;

// Options.
pub const OPT_EXPORT_NAME: u32 = 1;
pub const OPT_ABORT: u32 = 2;
pub const OPT_LIST: u32 = 3;
pub const OPT_STARTTLS: u32 = 5;
pub const OPT_INFO: u32 = 6;
pub const OPT_GO: u32 = 7;
pub const OPT_STRUCTURED_REPLY: u32 = 8;
pub const OPT_LIST_META_CONTEXT: u32 = 9;
pub const OPT_SET_META_CONTEXT: u32 = 10;

// Option reply types.
pub const REP_ACK: u32 = 1;
pub const REP_SERVER: u32 = 2;
pub const REP_INFO: u32 = 3;
pub const REP_META_CONTEXT: u32 = 4;
pub const REP_ERR_UNSUP: u32 = (1 << 31) + 1;
pub const REP_ERR_INVALID: u32 = (1 << 31) + 3;
/// FORCEDTLS: every option except STARTTLS/ABORT gets this pre-TLS.
pub const REP_ERR_TLS_REQD: u32 = (1 << 31) + 5;
pub const REP_ERR_UNKNOWN: u32 = (1 << 31) + 6;

// NBD_INFO_* types.
pub const INFO_EXPORT: u16 = 0;
pub const INFO_BLOCK_SIZE: u16 = 3;

// Transmission flags (per-export; design §8.2).
pub const TFLAG_HAS_FLAGS: u16 = 1 << 0;
pub const TFLAG_READ_ONLY: u16 = 1 << 1;
pub const TFLAG_SEND_FLUSH: u16 = 1 << 2;
pub const TFLAG_SEND_FUA: u16 = 1 << 3;
pub const TFLAG_SEND_TRIM: u16 = 1 << 5;
pub const TFLAG_SEND_WRITE_ZEROES: u16 = 1 << 6;

// Commands.
pub const CMD_READ: u16 = 0;
pub const CMD_WRITE: u16 = 1;
pub const CMD_DISC: u16 = 2;
pub const CMD_FLUSH: u16 = 3;
pub const CMD_TRIM: u16 = 4;
pub const CMD_CACHE: u16 = 5;
pub const CMD_WRITE_ZEROES: u16 = 6;
pub const CMD_BLOCK_STATUS: u16 = 7;

// Command flags.
pub const CMD_FLAG_FUA: u16 = 1 << 0;
pub const CMD_FLAG_REQ_ONE: u16 = 1 << 3;

// Structured replies (design §8.3, M4).
pub const STRUCTURED_REPLY_MAGIC: u32 = 0x668e_33ef;
pub const REPLY_FLAG_DONE: u16 = 1 << 0;
pub const REPLY_TYPE_NONE: u16 = 0;
pub const REPLY_TYPE_OFFSET_DATA: u16 = 1;
pub const REPLY_TYPE_BLOCK_STATUS: u16 = 5;
pub const REPLY_TYPE_ERROR: u16 = (1 << 15) + 1;

// base:allocation status bits.
pub const STATE_HOLE: u32 = 1 << 0;
pub const STATE_ZERO: u32 = 1 << 1;
pub const BASE_ALLOCATION: &str = "base:allocation";
/// Our context id for base:allocation (server-assigned).
pub const ALLOCATION_CTX_ID: u32 = 1;

// Errors (design §8.3).
pub const EPERM: u32 = 1;
pub const EIO: u32 = 5;
pub const EINVAL: u32 = 22;
pub const ENOSPC: u32 = 28;
pub const ENOTSUP: u32 = 95;
pub const ESHUTDOWN: u32 = 108;

/// Advertised block sizes (design §5.3, §8.1).
pub const BLOCK_MIN: u32 = 512;
pub const BLOCK_PREFERRED: u32 = 4096;
pub const BLOCK_MAX_IO: u32 = 1 << 20;

/// Hard cap on READ/WRITE payloads: larger is a protocol error given the
/// advertised max (we still accept up to 32 MiB before erroring cleanly).
pub const HARD_MAX_IO: u32 = 32 << 20;

/// Cap on option payloads during negotiation.
pub const MAX_OPT_LEN: u32 = 4096;

/// A parsed transmission request header (28 bytes on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub flags: u16,
    pub cmd: u16,
    pub cookie: u64,
    pub offset: u64,
    pub len: u32,
}

impl Request {
    pub const WIRE_LEN: usize = 28;

    pub fn decode(b: &[u8; Self::WIRE_LEN]) -> Result<Self, u32> {
        let magic = u32::from_be_bytes(b[0..4].try_into().unwrap());
        if magic != REQUEST_MAGIC {
            return Err(EINVAL);
        }
        Ok(Self {
            flags: u16::from_be_bytes(b[4..6].try_into().unwrap()),
            cmd: u16::from_be_bytes(b[6..8].try_into().unwrap()),
            cookie: u64::from_be_bytes(b[8..16].try_into().unwrap()),
            offset: u64::from_be_bytes(b[16..24].try_into().unwrap()),
            len: u32::from_be_bytes(b[24..28].try_into().unwrap()),
        })
    }

    #[cfg(test)]
    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut out = [0u8; Self::WIRE_LEN];
        out[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
        out[4..6].copy_from_slice(&self.flags.to_be_bytes());
        out[6..8].copy_from_slice(&self.cmd.to_be_bytes());
        out[8..16].copy_from_slice(&self.cookie.to_be_bytes());
        out[16..24].copy_from_slice(&self.offset.to_be_bytes());
        out[24..28].copy_from_slice(&self.len.to_be_bytes());
        out
    }
}

/// Simple reply header (16 bytes), optionally followed by READ payload.
pub fn simple_reply(err: u32, cookie: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&SIMPLE_REPLY_MAGIC.to_be_bytes());
    out[4..8].copy_from_slice(&err.to_be_bytes());
    out[8..16].copy_from_slice(&cookie.to_be_bytes());
    out
}

/// Option reply header (20 bytes), followed by `data_len` bytes.
pub fn opt_reply_header(option: u32, reply_type: u32, data_len: u32) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[0..8].copy_from_slice(&OPT_REPLY_MAGIC.to_be_bytes());
    out[8..12].copy_from_slice(&option.to_be_bytes());
    out[12..16].copy_from_slice(&reply_type.to_be_bytes());
    out[16..20].copy_from_slice(&data_len.to_be_bytes());
    out
}

/// Structured reply header (20 bytes), followed by `data_len` payload.
pub fn structured_reply(flags: u16, reply_type: u16, cookie: u64, data_len: u32) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&STRUCTURED_REPLY_MAGIC.to_be_bytes());
    out[4..6].copy_from_slice(&flags.to_be_bytes());
    out[6..8].copy_from_slice(&reply_type.to_be_bytes());
    out[8..16].copy_from_slice(&cookie.to_be_bytes());
    out[16..20].copy_from_slice(&data_len.to_be_bytes());
    out
}

/// Structured error payload: u32 errno + u16 message length (no message).
pub fn structured_error_payload(err: u32) -> [u8; 6] {
    let mut out = [0u8; 6];
    out[0..4].copy_from_slice(&err.to_be_bytes());
    out
}

/// `NBD_INFO_EXPORT` payload: info type + size + transmission flags.
pub fn info_export(size: u64, tflags: u16) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[0..2].copy_from_slice(&INFO_EXPORT.to_be_bytes());
    out[2..10].copy_from_slice(&size.to_be_bytes());
    out[10..12].copy_from_slice(&tflags.to_be_bytes());
    out
}

/// `NBD_INFO_BLOCK_SIZE` payload: info type + min/preferred/max.
pub fn info_block_size() -> [u8; 14] {
    let mut out = [0u8; 14];
    out[0..2].copy_from_slice(&INFO_BLOCK_SIZE.to_be_bytes());
    out[2..6].copy_from_slice(&BLOCK_MIN.to_be_bytes());
    out[6..10].copy_from_slice(&BLOCK_PREFERRED.to_be_bytes());
    out[10..14].copy_from_slice(&BLOCK_MAX_IO.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The magics as literal spec bytes — if these change, every NBD
    /// client on earth disagrees with us.
    #[test]
    fn golden_magics() {
        assert_eq!(&NBDMAGIC.to_be_bytes(), b"NBDMAGIC");
        assert_eq!(&IHAVEOPT.to_be_bytes(), b"IHAVEOPT");
        assert_eq!(REQUEST_MAGIC, 0x25609513);
        assert_eq!(SIMPLE_REPLY_MAGIC, 0x67446698);
        assert_eq!(
            OPT_REPLY_MAGIC.to_be_bytes()[2..],
            [0xe8, 0x89, 0x04, 0x55, 0x65, 0xa9]
        );
    }

    #[test]
    fn golden_request_roundtrip() {
        let req = Request {
            flags: CMD_FLAG_FUA,
            cmd: CMD_WRITE,
            cookie: 0xDEAD_BEEF,
            offset: 4096,
            len: 512,
        };
        let wire = req.encode();
        // Spot-check the layout against the spec offsets.
        assert_eq!(&wire[0..4], &[0x25, 0x60, 0x95, 0x13]);
        assert_eq!(&wire[6..8], &[0x00, 0x01]); // CMD_WRITE
        assert_eq!(Request::decode(&wire).unwrap(), req);
    }

    #[test]
    fn bad_request_magic_is_einval() {
        let mut wire = Request {
            flags: 0,
            cmd: CMD_READ,
            cookie: 1,
            offset: 0,
            len: 512,
        }
        .encode();
        wire[0] = 0xFF;
        assert_eq!(Request::decode(&wire), Err(EINVAL));
    }

    #[test]
    fn golden_simple_reply_layout() {
        let r = simple_reply(EIO, 0x0102_0304_0506_0708);
        assert_eq!(&r[0..4], &[0x67, 0x44, 0x66, 0x98]);
        assert_eq!(&r[4..8], &[0, 0, 0, 5]);
        assert_eq!(&r[8..16], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn golden_info_payloads() {
        let e = info_export(1 << 30, TFLAG_HAS_FLAGS | TFLAG_SEND_FLUSH);
        assert_eq!(&e[0..2], &[0, 0]); // INFO_EXPORT
        assert_eq!(u64::from_be_bytes(e[2..10].try_into().unwrap()), 1 << 30);
        assert_eq!(u16::from_be_bytes(e[10..12].try_into().unwrap()), 0b101);

        let b = info_block_size();
        assert_eq!(&b[0..2], &[0, 3]); // INFO_BLOCK_SIZE
        assert_eq!(u32::from_be_bytes(b[2..6].try_into().unwrap()), 512);
        assert_eq!(u32::from_be_bytes(b[6..10].try_into().unwrap()), 4096);
        assert_eq!(u32::from_be_bytes(b[10..14].try_into().unwrap()), 1 << 20);
    }

    #[test]
    fn golden_structured_reply_layout() {
        let h = structured_reply(REPLY_FLAG_DONE, REPLY_TYPE_OFFSET_DATA, 0x0102, 8 + 512);
        assert_eq!(&h[0..4], &[0x66, 0x8e, 0x33, 0xef]);
        assert_eq!(&h[4..6], &[0, 1]); // DONE
        assert_eq!(&h[6..8], &[0, 1]); // OFFSET_DATA
        assert_eq!(u64::from_be_bytes(h[8..16].try_into().unwrap()), 0x0102);
        assert_eq!(u32::from_be_bytes(h[16..20].try_into().unwrap()), 520);

        assert_eq!(REPLY_TYPE_ERROR, 0x8001);
        assert_eq!(structured_error_payload(EIO), [0, 0, 0, 5, 0, 0]);
        assert_eq!((STATE_HOLE, STATE_ZERO), (1, 2));
        assert_eq!(
            (
                OPT_LIST_META_CONTEXT,
                OPT_SET_META_CONTEXT,
                REP_META_CONTEXT
            ),
            (9, 10, 4)
        );
    }

    /// STARTTLS constants against the NBD spec (auth design §3.1).
    #[test]
    fn golden_starttls_constants() {
        assert_eq!(OPT_STARTTLS, 5);
        assert_eq!(REP_ERR_TLS_REQD, 0x8000_0005);
    }

    #[test]
    fn transmission_flags_match_spec_bits() {
        assert_eq!(TFLAG_HAS_FLAGS, 1);
        assert_eq!(TFLAG_READ_ONLY, 2);
        assert_eq!(TFLAG_SEND_FLUSH, 4);
        assert_eq!(TFLAG_SEND_FUA, 8);
        assert_eq!(TFLAG_SEND_TRIM, 32);
        assert_eq!(TFLAG_SEND_WRITE_ZEROES, 64);
    }
}
