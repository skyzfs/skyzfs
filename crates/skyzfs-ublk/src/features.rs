//! The ublk driver's features the daemon needs, and the kernels that
//! have them (design §4.2 "Kernels"). This is the one place that names
//! them: the start asks the driver for its features before anything
//! else ([`check`]), and refuses on a kernel that lacks one, naming it
//! and the oldest kernel that has it.
//!
//! # Where the kernels come from
//!
//! Read, not remembered (2026-11-12): `include/uapi/linux/ublk_cmd.h`
//! at every release tag from v6.0 to v7.0 of the kernel's tree (the
//! mirror at github.com/torvalds/linux, read through
//! raw.githubusercontent.com; git.kernel.org could not be reached from
//! the container that did this). The kernel given for a name is the
//! first release whose header defines it. What the driver answers was
//! read in `drivers/block/ublk_drv.c` at v6.1, v6.3, v6.4, v6.5, v6.8,
//! v6.9 and v7.0, and what the flags mean in
//! `Documentation/block/ublk.rst` at v7.0. The commits that added each
//! name were not looked up: the release tags bound them.
//!
//! - From v6.5 the driver answers `UBLK_U_CMD_GET_FEATURES` before it
//!   looks for any device, with every flag it supports (`UBLK_F_ALL`,
//!   which holds the three flags in [`NEEDED`]).
//! - Before v6.5 it has no such command, and the command, sent for a
//!   device number that is never a device's, answers `ENODEV`: the
//!   driver looks the device up before it matches the command. That
//!   answer is read as "the command is missing" ([`answered`]).
//!
//! # What is not checked here
//!
//! The daemon deletes a device with `UBLK_U_CMD_DEL_DEV_ASYNC`, first in
//! v6.9. It has no feature bit, and a driver that lacks it answers it,
//! for a device that does not exist, as one that has it does
//! (`ENODEV`): nothing tells it apart before the daemon needs it. The
//! features word proves a kernel of at least 6.5; [`OLDEST_KERNEL`] is
//! 6.9.

use std::fmt;

/// A driver feature flag the daemon needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    /// Its name in the kernel's header.
    pub name: &'static str,
    /// Its bit in the features word and in a device's flags.
    pub bit: u64,
    /// The first Linux release that has it.
    pub kernel: &'static str,
    /// Why the daemon needs it.
    pub why: &'static str,
}

/// The flags the daemon needs, in the order a refusal names them. The
/// first two it asks for when it adds a device ([`crate::lifecycle::Wanted::flags`]);
/// the third it relies on: its own control commands are sent in the
/// ioctl encoding (`UBLK_U_CMD_*`) and not tried again in the legacy
/// one. The library under the queue (libublk 0.4.8) asks for no flag of
/// its own for a device the daemon added: it uses the ioctl encoding
/// for the queue's commands when the device reports this flag, and the
/// legacy one when not.
pub const NEEDED: [Flag; 3] = [
    Flag {
        name: "UBLK_F_USER_RECOVERY",
        bit: 1 << 3,
        kernel: "6.1",
        why: "a device that waits for a server when its server ends",
    },
    Flag {
        name: "UBLK_F_USER_RECOVERY_REISSUE",
        bit: 1 << 4,
        kernel: "6.1",
        why: "requests in flight at a server's end go to the next server",
    },
    Flag {
        name: "UBLK_F_CMD_IOCTL_ENCODE",
        bit: 1 << 6,
        kernel: "6.4",
        why: "the daemon's control commands are ioctl-encoded",
    },
];

/// The first Linux release with `UBLK_U_CMD_GET_FEATURES`.
pub const FEATURES_KERNEL: &str = "6.5";

/// The first Linux release with `UBLK_U_CMD_DEL_DEV_ASYNC`, the command
/// the daemon deletes a device with. Not checked: see the [module
/// documentation](self).
pub const DELETE_KERNEL: &str = "6.9";

/// The oldest Linux release the daemon runs on: the newest of the
/// releases above.
pub const OLDEST_KERNEL: &str = DELETE_KERNEL;

/// Why the daemon does not serve on this kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    /// The driver has no command that says its features.
    NoFeatures,
    /// The driver lacks these flags.
    Missing(Vec<Flag>),
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFeatures => write!(
                f,
                "the kernel's ublk driver cannot say its features \
                 (UBLK_U_CMD_GET_FEATURES, Linux {FEATURES_KERNEL}): this daemon needs \
                 Linux {OLDEST_KERNEL} or later"
            ),
            Self::Missing(flags) => {
                write!(f, "the kernel's ublk driver lacks ")?;
                for (n, flag) in flags.iter().enumerate() {
                    if n > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} (Linux {}: {})", flag.name, flag.kernel, flag.why)?;
                }
                write!(f, ": this daemon needs Linux {OLDEST_KERNEL} or later")
            }
        }
    }
}

/// Whether the daemon serves with a driver whose features are
/// `features`: `None` when the driver has no command that says them.
pub fn check(features: Option<u64>) -> Result<(), Unsupported> {
    let Some(features) = features else {
        return Err(Unsupported::NoFeatures);
    };
    let missing: Vec<Flag> = NEEDED
        .iter()
        .filter(|flag| features & flag.bit == 0)
        .copied()
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Unsupported::Missing(missing))
    }
}

/// What the answer to `UBLK_U_CMD_GET_FEATURES`, sent for a device
/// number that is never a device's, says: the features, `None` when the
/// driver has no such command (it answers `ENODEV`), or the error when
/// it is anything else.
pub fn answered(answer: Result<u64, i32>) -> Result<Option<u64>, i32> {
    const ENODEV: i32 = 19;
    match answer {
        Ok(features) => Ok(Some(features)),
        Err(ENODEV) => Ok(None),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests;
