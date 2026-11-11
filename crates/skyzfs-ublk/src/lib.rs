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
//!
//! The pump between a queue and a target, and the binding to the
//! kernel, are built on it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod translate;
