//! `skyzfsd`, the SkyZFS daemon (design §4.2): one volume of the local
//! store served as a ublk device.
//!
//! So far: the command line ([`cli`]). The rig that tests the daemon
//! against a kernel is built to it, item by item.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cli;
