//! The binding of SkyZFS to the kernel's ublk driver (design §4.2):
//! the lifecycle's [`Control`](skyzfs_ublk::lifecycle::Control) and the
//! pump's [`Queue`](skyzfs_ublk::pump::Queue) over a real kernel.
//!
//! **This is the only SkyZFS crate that may hold `unsafe`.** Every
//! `unsafe` block says why it is sound. `skyzfs-ublk` and the daemon
//! forbid it.
//!
//! # What is the library's and what is ours
//!
//! The queue is `libublk` `=0.4.8`: the fetch, the commit, the ring and
//! its idle time-out. So are four control steps, through a control
//! object of the library that is made **only for a device that already
//! exists and never with the library's add flag**: the parameters, the
//! start, and the begin and end of a recovery.
//!
//! Three control commands are sent by this crate itself ([`raw`]): the
//! add, the lookup (a device's info and its parameters) and the
//! asynchronous delete. The kernel spike (`docs/skyzfs-ublk-spike.md`)
//! showed why. The library answers every failed control command with
//! "not supported", so a taken number cannot be told from any other
//! failure; and its failed add deletes whatever device stands under the
//! number, with the delete that waits for the holder. Sent from here
//! the driver's own answers come through: `EEXIST` for a taken number,
//! also one that is deleted and still held; `ENODEV` for nothing there.
//! The opcodes and the structures are the ones of the bindings the
//! library ships (`libublk::sys`), not numbers copied by hand.
//!
//! # What the library does on its own, and cannot be told not to
//!
//! - Making a control object for a device prints `device reload json
//!   failed` on standard error, once. It means nothing here: the
//!   library looks for a file of its own that this daemon never needs.
//! - A take-over (a control object with the library's recover flag)
//!   writes `/run/ublksrvd/NNNN.json` when the device is started. The
//!   daemon reads nothing from it. A device this instance added is
//!   started through an object with no flag, which writes no file.
//! - `UblkCtrl::run_target` is not used: it stops the device when the
//!   queue threads end, and a device of ours must outlive its server.
//! - The library allocates one buffer per tag of its own and gives the
//!   driver their addresses with the first fetch. They stay allocated;
//!   see [`queue`] for how each tag moves to a buffer of ours.
//!
//! # Linux only
//!
//! ublk is a Linux driver. On another system the crate is empty.

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
pub mod control;
#[cfg(target_os = "linux")]
mod mem;
#[cfg(target_os = "linux")]
pub mod queue;
#[cfg(target_os = "linux")]
pub mod raw;
#[cfg(target_os = "linux")]
pub mod signals;
#[cfg(target_os = "linux")]
mod wake;

#[cfg(target_os = "linux")]
pub use mem::lock_all as lock_all_memory;

#[cfg(target_os = "linux")]
pub use wake::Wake;

/// The driver flags the daemon asks for when it adds a device: user
/// recovery, with reissue. They are also the mask under which a
/// device's flags are reported: the driver adds bits of its own (asked
/// `0x18`, reported `0x2605a` on the kernels of the spike), and what is
/// recorded and compared is what was asked.
#[cfg(target_os = "linux")]
pub const FLAGS: u64 =
    (libublk::sys::UBLK_F_USER_RECOVERY | libublk::sys::UBLK_F_USER_RECOVERY_REISSUE) as u64;

/// The largest request a device of ours takes: the contract's largest
/// read or write.
pub const MAX_IO: u32 = skyzfs_blk::MAX_IO_BYTES as u32;

#[cfg(target_os = "linux")]
fn io_error(error: libublk::UblkError) -> std::io::Error {
    use libublk::UblkError;
    match error {
        UblkError::OtherError(n) | UblkError::UringIOError(n) => {
            std::io::Error::from_raw_os_error(n.abs())
        }
        UblkError::IOError(e) => e,
        other => std::io::Error::other(format!("{other:?}")),
    }
}
