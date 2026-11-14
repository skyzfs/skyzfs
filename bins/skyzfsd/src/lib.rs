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

/// Write `text` to standard output and flush it. A write that fails
/// (the reader is gone) is ignored: `println!` panics on it, and a
/// panic would replace the command's exit status with 101.
pub fn to_stdout(text: &str) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

/// Write `text` to standard error. A write that fails is ignored, as
/// in [`to_stdout`].
pub fn to_stderr(text: &str) {
    use std::io::Write as _;
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
}

pub mod cli;
pub mod commands;
pub mod delay;
#[cfg(all(target_os = "linux", feature = "test-hooks"))]
pub mod record;
#[cfg(all(target_os = "linux", feature = "test-hooks"))]
pub mod refuse;
#[cfg(target_os = "linux")]
pub mod serve;
pub mod timing;
