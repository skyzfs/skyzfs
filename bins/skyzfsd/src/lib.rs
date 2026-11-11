//! `skyzfsd`, the SkyZFS daemon (design §4.2): one volume of the local
//! store served as a ublk device.
//!
//! So far: the command line ([`cli`]), and `status` and `detach` over
//! a control interface that is given to them ([`commands`]). The
//! kernel's own control interface is not built yet, so the binary
//! still runs no command but `help`. The rig that tests the daemon
//! against a kernel is built to it, item by item.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cli;
pub mod commands;
