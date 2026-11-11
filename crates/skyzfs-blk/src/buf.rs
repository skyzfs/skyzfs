//! The buffer type of the contract.

use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::BLOCK_USIZE;

/// An owned buffer of whole 4 KiB blocks whose memory is aligned to
/// 4096 bytes.
///
/// Reads and writes carry one, and the completion hands it back. The
/// alignment is part of the contract so that a store can pass the buffer
/// straight to a write log opened with direct I/O: with an unaligned
/// buffer "one copy per request" would become two.
///
/// It is deliberately minimal: allocate, view as bytes, length in whole
/// blocks. No pooling and no zero-copy; a transport that wants a pool
/// keeps the buffers it gets back.
pub struct Buf {
    /// Over-allocated so that an aligned run of `len` bytes exists in it.
    /// Never grown, so the run does not move.
    raw: Vec<u8>,
    start: usize,
    len: usize,
}

impl Buf {
    /// The alignment of the buffer's memory, in bytes.
    pub const ALIGN: usize = BLOCK_USIZE;

    /// A buffer of `blocks` blocks, every byte `byte`.
    pub fn filled(blocks: usize, byte: u8) -> Self {
        let len = blocks * BLOCK_USIZE;
        if len == 0 {
            return Self {
                raw: Vec::new(),
                start: 0,
                len: 0,
            };
        }
        let raw = vec![byte; len + Self::ALIGN - 1];
        let misaligned = raw.as_ptr() as usize % Self::ALIGN;
        let start = (Self::ALIGN - misaligned) % Self::ALIGN;
        Self { raw, start, len }
    }

    /// A buffer of `blocks` blocks of zeros.
    pub fn zeroed(blocks: usize) -> Self {
        Self::filled(blocks, 0)
    }

    /// A buffer holding a copy of `bytes`, or `None` if `bytes` is not a
    /// whole number of blocks.
    pub fn from_blocks(bytes: &[u8]) -> Option<Self> {
        if !bytes.len().is_multiple_of(BLOCK_USIZE) {
            return None;
        }
        let mut buf = Self::zeroed(bytes.len() / BLOCK_USIZE);
        buf.copy_from_slice(bytes);
        Some(buf)
    }

    /// The length in blocks.
    pub fn blocks(&self) -> usize {
        self.len / BLOCK_USIZE
    }
}

impl Deref for Buf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.raw[self.start..self.start + self.len]
    }
}

impl DerefMut for Buf {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.raw[self.start..self.start + self.len]
    }
}

impl Clone for Buf {
    /// A copy in a new, aligned allocation.
    fn clone(&self) -> Self {
        let mut copy = Self::zeroed(self.blocks());
        copy.copy_from_slice(self);
        copy
    }
}

impl PartialEq for Buf {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for Buf {}

impl fmt::Debug for Buf {
    /// The length and the first bytes: a megabyte of hex helps nobody.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let head = &self[..self.len.min(8)];
        write!(f, "Buf({} blocks, {head:02x?}..)", self.blocks())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R1: buffers are whole blocks and their memory is 4096-aligned, also
    /// after a clone and a move.
    #[test]
    fn buffers_are_aligned_whole_blocks() {
        for blocks in [0usize, 1, 2, 7, 256] {
            let buf = Buf::filled(blocks, 0xAB);
            let moved = buf.clone();
            let boxed = Box::new(buf);
            for b in [&*boxed, &moved] {
                assert_eq!(b.len(), blocks * BLOCK_USIZE);
                assert_eq!(b.blocks(), blocks);
                assert!(b.iter().all(|x| *x == 0xAB));
                if blocks > 0 {
                    assert_eq!(b.as_ptr() as usize % Buf::ALIGN, 0);
                }
            }
        }
    }

    /// R1: a buffer cannot hold a partial block.
    #[test]
    fn partial_blocks_are_refused() {
        assert!(Buf::from_blocks(&[0; 512]).is_none());
        assert!(Buf::from_blocks(&[0; BLOCK_USIZE + 1]).is_none());
        let data: Vec<u8> = (0..2 * BLOCK_USIZE).map(|i| i as u8).collect();
        assert_eq!(&Buf::from_blocks(&data).unwrap()[..], &data[..]);
    }
}
