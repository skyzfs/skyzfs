//! The ublk transport of SkyZFS (design §4.2): what turns the requests
//! of a ublk device into the operations of the block target contract
//! (`skyzfs-blk`, design §4.1) and back.
//!
//! It is in layers, so that all but the last can be tested without a
//! kernel:
//!
//! - [`translate`]: pure. A ublk request descriptor becomes a plan in
//!   the contract's terms, and an outcome becomes the result the kernel
//!   is given.
//! - [`pump`]: moves requests between one queue and one block target,
//!   generic over both. Tested with a fake queue.
//! - [`ack`]: the record, outside the store, of the position of the
//!   newest acknowledged log record, which every start compares with
//!   what it recovered.
//!
//! The binding to the kernel is built on these.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(unix)]
pub mod ack;
pub mod pump;
pub mod translate;
