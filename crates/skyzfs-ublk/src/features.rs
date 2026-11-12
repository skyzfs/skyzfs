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
//! # The asynchronous delete
//!
//! The daemon deletes a device with `UBLK_U_CMD_DEL_DEV_ASYNC`, first in
//! v6.9. It has no feature bit, and a driver that lacks it answers it,
//! for a device that does not exist, as one that has it does
//! (`ENODEV`): no command tells it apart before the daemon needs it.
//! The features word proves a kernel of at least 6.5, so the start also
//! reads the kernel's release and refuses one below 6.9, unless the
//! operator says that the driver has the command
//! ([`ASYNC_DELETE_OPTION`]): a distribution kernel can carry a newer
//! driver under an older number. No other flag is taken as a sign of
//! it, and there is no fallback to the synchronous delete.

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
/// the daemon deletes a device with: what a failed volume gives up its
/// device with while a client still holds it (the plain delete waits
/// for the last holder, and a suspended pool never lets go). It has no
/// feature bit, so the start reads the kernel's release instead
/// ([`check`]); see the [module documentation](self).
pub const DELETE_KERNEL: (u32, u32) = (6, 9);

/// The oldest Linux release the daemon runs on: the newest of the
/// releases above.
pub const OLDEST_KERNEL: &str = "6.9";

/// The option by which an operator says that the kernel's ublk driver
/// has `UBLK_U_CMD_DEL_DEV_ASYNC` although its release is older (a
/// distribution kernel with the driver backported). With it the release
/// is not checked; the features are.
pub const ASYNC_DELETE_OPTION: &str = "--kernel-has-async-delete";

/// One thing the daemon needs and this kernel lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lack {
    /// The driver has no command that says its features.
    NoFeatures,
    /// The driver lacks this flag.
    Flag(Flag),
    /// The kernel's release, as `uname` says it, is below
    /// [`DELETE_KERNEL`].
    Release(String),
    /// The kernel's release does not begin with a major and a minor
    /// number.
    UnreadableRelease(String),
}

/// Why the daemon does not serve on this kernel: every lack found, in
/// the order of the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported(pub Vec<Lack>);

impl fmt::Display for Lack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (major, minor) = DELETE_KERNEL;
        let delete = format!(
            "UBLK_U_CMD_DEL_DEV_ASYNC came with Linux {major}.{minor}, and without it the \
             daemon cannot give up a failed volume's device while a client holds it. If this \
             kernel's ublk driver has it all the same (a distribution kernel with the driver \
             backported), say so with {ASYNC_DELETE_OPTION}"
        );
        match self {
            Self::NoFeatures => write!(
                f,
                "the kernel's ublk driver cannot say its features \
                 (UBLK_U_CMD_GET_FEATURES, Linux {FEATURES_KERNEL})"
            ),
            Self::Flag(flag) => write!(
                f,
                "the kernel's ublk driver lacks {} (Linux {}: {})",
                flag.name, flag.kernel, flag.why
            ),
            Self::Release(release) => {
                write!(
                    f,
                    "the kernel's release {release} is older than Linux {major}.{minor}: {delete}"
                )
            }
            Self::UnreadableRelease(release) => write!(
                f,
                "the kernel's release {release:?} does not begin with a major and a minor \
                 number, so it cannot be compared with Linux {major}.{minor}: {delete}"
            ),
        }
    }
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, lack) in self.0.iter().enumerate() {
            if n > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{lack}")?;
        }
        write!(f, ". This daemon needs Linux {OLDEST_KERNEL} or later")
    }
}

/// The major and minor number at the start of a kernel's release as
/// `uname -r` says it ("6.8.0-45-generic", "7.0.0",
/// "5.14.0-570.el9.x86_64"): digits, a dot, digits, and then the end or
/// anything but a digit. Anything else is `None`.
pub fn release_number(release: &str) -> Option<(u32, u32)> {
    let (major, rest) = release.split_once('.')?;
    let minor_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    let minor = &rest[..minor_len];
    let digits = |s: &str| !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(major) || !digits(minor) {
        return None;
    }
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Whether the daemon serves on a kernel whose ublk driver has the
/// features `features` (`None` when it has no command that says them)
/// and whose release is `release`. With `asserted` the operator has
/// said that the driver has `UBLK_U_CMD_DEL_DEV_ASYNC` whatever the
/// release, and the release is not read; the features are checked all
/// the same.
pub fn check(features: Option<u64>, release: &str, asserted: bool) -> Result<(), Unsupported> {
    let mut lacks = Vec::new();
    match features {
        None => lacks.push(Lack::NoFeatures),
        Some(features) => lacks.extend(
            NEEDED
                .iter()
                .filter(|flag| features & flag.bit == 0)
                .map(|flag| Lack::Flag(*flag)),
        ),
    }
    if !asserted {
        match release_number(release) {
            Some(number) if number >= DELETE_KERNEL => {}
            Some(_) => lacks.push(Lack::Release(release.to_string())),
            None => lacks.push(Lack::UnreadableRelease(release.to_string())),
        }
    }
    if lacks.is_empty() {
        Ok(())
    } else {
        Err(Unsupported(lacks))
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
