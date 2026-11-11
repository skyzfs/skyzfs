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
//! - [`lifecycle`]: the order of steps at every start: the store is
//!   opened first, then a device is taken over or added. Generic over
//!   the kernel's control interface, the store and the records, and
//!   tested with fakes.
//! - [`records`]: the lifecycle's records as files: which device is a
//!   volume's, in the volume's directory, and the note for each device
//!   in the run directory.
//!
//! The binding to the kernel is built on these.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(unix)]
pub mod ack;
pub mod lifecycle;
pub mod pump;
#[cfg(unix)]
pub mod records;
pub mod translate;
