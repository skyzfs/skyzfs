//! `skyzfsd`, the SkyZFS daemon (design §4.2): one volume of the local
//! store served as a ublk device.
//!
//! The command line is [`cli`]. `create`, `status` and `detach` are in
//! [`commands`], over a control interface that is given to them, so
//! that they are tested without a kernel. `serve` is in [`serve`], and
//! is tested by the kernel rig (`tests/kernel/daemon-rig.sh`): it needs
//! the driver.
//!
//! The binding to the kernel is the crate `skyzfs-ublk-sys`, the only
//! one with `unsafe`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cli;
pub mod commands;
pub mod delay;
#[cfg(target_os = "linux")]
pub mod serve;
pub mod timing;
