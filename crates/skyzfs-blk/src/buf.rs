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
///
/// # Capacity, and an address that does not change
///
/// A buffer has a capacity, fixed when it is made, and a length that
/// can be set to anything up to it ([`Buf::set_blocks`]). What is seen
/// through the buffer is its first `length` bytes.
///
/// **The address of a buffer's first byte never changes**: not when
/// the length is set, and not when the `Buf` is moved, boxed or sent
/// to another thread. A transport that has told the kernel where a
/// request slot's memory is rests on that: it makes one buffer of the
/// largest request's size per slot, and lends it out at each request's
/// length. (A buffer of capacity zero has no memory and no address to
/// speak of.)
///
/// **Setting a longer length shows what was there**, not zeros: the
/// bytes of whatever used the buffer before. That is what such a
/// transport needs, since the kernel has put a write's data there. It
/// is also why a target must fill the whole buffer of a read before it
/// answers `Ok`, as the contract says: otherwise it hands one client's
/// data to another.
pub struct Buf {
    /// Over-allocated so that an aligned run of `cap` bytes exists in
    /// it. Never grown, shrunk or replaced, so the run does not move.
    raw: Vec<u8>,
    start: usize,
    /// The capacity in bytes.
    cap: usize,
    /// The length in bytes: at most `cap`.
    len: usize,
}

/// A length beyond a buffer's capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{asked} blocks do not fit a buffer of {capacity}")]
pub struct TooLong {
    /// The length asked for, in blocks.
    pub asked: usize,
    /// The buffer's capacity, in blocks.
    pub capacity: usize,
}

impl Buf {
    /// The alignment of the buffer's memory, in bytes.
    pub const ALIGN: usize = BLOCK_USIZE;

    /// A buffer of `blocks` blocks, every byte `byte`. Its capacity is
    /// its length.
    pub fn filled(blocks: usize, byte: u8) -> Self {
        let len = blocks * BLOCK_USIZE;
        if len == 0 {
            return Self {
                raw: Vec::new(),
                start: 0,
                cap: 0,
                len: 0,
            };
        }
        let raw = vec![byte; len + Self::ALIGN - 1];
        let misaligned = raw.as_ptr() as usize % Self::ALIGN;
        let start = (Self::ALIGN - misaligned) % Self::ALIGN;
        Self {
            raw,
            start,
            cap: len,
            len,
        }
    }

    /// A buffer of zeros with room for `blocks` blocks, and that
    /// length: to be lent out at shorter lengths with
    /// [`Buf::set_blocks`].
    pub fn with_capacity(blocks: usize) -> Self {
        Self::zeroed(blocks)
    }

    /// The capacity in blocks.
    pub fn capacity_blocks(&self) -> usize {
        self.cap / BLOCK_USIZE
    }

    /// Set the length to `blocks` blocks, which must fit the capacity.
    /// The memory is not touched and does not move: a longer length
    /// shows whatever was there before.
    pub fn set_blocks(&mut self, blocks: usize) -> Result<(), TooLong> {
        if blocks > self.capacity_blocks() {
            return Err(TooLong {
                asked: blocks,
                capacity: self.capacity_blocks(),
            });
        }
        self.len = blocks * BLOCK_USIZE;
        Ok(())
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
    /// A copy of what is seen, in a new, aligned allocation whose
    /// capacity is its length.
    fn clone(&self) -> Self {
        let mut copy = Self::zeroed(self.blocks());
        copy.copy_from_slice(self);
        copy
    }
}

impl PartialEq for Buf {
    /// Equal if what is seen is equal, whatever the capacities.
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

    /// The length moves below the capacity, and the first byte stays
    /// where it is: through every length, a move, a box and another
    /// thread.
    #[test]
    fn the_length_is_set_below_the_capacity_and_the_address_never_changes() {
        let mut buf = Buf::with_capacity(256);
        assert_eq!((buf.blocks(), buf.capacity_blocks()), (256, 256));
        let address = buf.as_ptr() as usize;
        assert_eq!(address % Buf::ALIGN, 0);
        buf.fill(0x11);

        for blocks in [1usize, 0, 7, 256, 3] {
            buf.set_blocks(blocks).expect("fits");
            assert_eq!((buf.blocks(), buf.len()), (blocks, blocks * BLOCK_USIZE));
            assert_eq!(buf.capacity_blocks(), 256);
            if blocks > 0 {
                assert_eq!(buf.as_ptr() as usize, address);
                assert_eq!(buf.as_mut_ptr() as usize, address);
            }
        }
        assert_eq!(
            buf.set_blocks(257),
            Err(TooLong {
                asked: 257,
                capacity: 256
            })
        );
        assert_eq!(buf.blocks(), 3, "a refused length changes nothing");

        // A longer length shows what was there, not zeros.
        buf.set_blocks(1).expect("fits");
        buf.fill(0x22);
        buf.set_blocks(2).expect("fits");
        assert!(buf[..BLOCK_USIZE].iter().all(|b| *b == 0x22));
        assert!(buf[BLOCK_USIZE..].iter().all(|b| *b == 0x11));

        // Moved, boxed, and through another thread and back.
        let moved = buf;
        assert_eq!(moved.as_ptr() as usize, address);
        let boxed = Box::new(moved);
        assert_eq!(boxed.as_ptr() as usize, address);
        let mut back = std::thread::spawn(move || {
            let mut there = *boxed;
            there.set_blocks(256).expect("fits");
            there
        })
        .join()
        .expect("the thread");
        assert_eq!(back.as_ptr() as usize, address);
        assert_eq!(back.blocks(), 256);
        back.set_blocks(2).expect("fits");

        // A clone and a comparison look at what is seen.
        let copy = back.clone();
        assert_eq!((copy.blocks(), copy.capacity_blocks()), (2, 2));
        assert_eq!(copy, back);
        assert_ne!(copy.as_ptr() as usize, address);
        assert_eq!(format!("{back:?}"), format!("{copy:?}"));

        // No memory, no length to set.
        let mut empty = Buf::with_capacity(0);
        assert_eq!(empty.set_blocks(0), Ok(()));
        assert!(empty.set_blocks(1).is_err());
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
