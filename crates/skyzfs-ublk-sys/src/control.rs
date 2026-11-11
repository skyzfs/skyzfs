//! The lifecycle's [`Control`] over the real driver.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;

use libublk::ctrl::UblkCtrl;
use libublk::sys;
use skyzfs_ublk::lifecycle::{AddError, Control, DeviceInfo, DeviceState, Shape};

use crate::{raw, FLAGS, MAX_IO};

/// The kernel's ublk driver, as the lifecycle sees it.
///
/// # How the driver's words are read
///
/// - **States.** The driver has three: `LIVE` is
///   [`DeviceState::Live`], `QUIESCED` is [`DeviceState::Waiting`],
///   `DEAD` is [`DeviceState::Dead`]. Only the state decides: a device
///   whose server was just killed reports `LIVE` with a server pid of
///   -1 for some milliseconds, and that is still live. Any other state
///   (the driver has one more, for a mode this daemon never asks for)
///   is an error: nothing is done to a device that is not understood.
/// - **Flags** are reported under the mask of the bits the daemon asks
///   for ([`FLAGS`]): the driver adds its own.
/// - **The size** is in the device's parameters, which a device has
///   from its first start on. A device that was never started reports
///   size zero. (The lifecycle deletes a dead device with this volume's
///   note whatever its values, and leaves one without a note whatever
///   its values, so a size of zero changes no decision about a device
///   that was never started.)
/// - **A taken number** is the driver's `EEXIST`, **nothing there** is
///   its `ENODEV`. Every other answer is an error and stops whatever
///   asked.
/// - **Whether a client holds the device** is asked with an exclusive
///   open of the block device that does no I/O. It is refused with
///   `EBUSY` while another exclusive holder exists (a pool is one):
///   held. It succeeds with no holder and with a holder that is not
///   exclusive, so anything else is "cannot be told", never "not held".
#[derive(Debug, Default, Clone, Copy)]
pub struct Kernel;

impl Kernel {
    /// The driver, if its control device is there.
    pub fn new() -> io::Result<Self> {
        std::fs::metadata(raw::CONTROL).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("{}: {e} (is the ublk driver loaded?)", raw::CONTROL),
            )
        })?;
        Ok(Self)
    }
}

/// How often the begin of a recovery is asked again while the driver
/// answers that the device is busy: 50 times, 5 s with [`BUSY_PAUSE`].
pub const BUSY_RETRIES: u32 = 50;

/// How long between two such tries.
pub const BUSY_PAUSE: std::time::Duration = std::time::Duration::from_millis(100);

/// The path of the block device with this number.
pub fn block_device(number: u32) -> String {
    format!("/dev/ublkb{number}")
}

impl Control for Kernel {
    fn can_reissue(&mut self) -> io::Result<bool> {
        let features = UblkCtrl::get_features()
            .ok_or_else(|| io::Error::other("the driver's features cannot be read"))?;
        Ok(features & u64::from(sys::UBLK_F_USER_RECOVERY_REISSUE) != 0)
    }

    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        let info = match raw::info(number) {
            Ok(info) => info,
            Err(e) if e.raw_os_error() == Some(libc::ENODEV) => return Ok(None),
            Err(e) => return Err(e),
        };
        let state = match u32::from(info.state) {
            sys::UBLK_S_DEV_LIVE => DeviceState::Live,
            sys::UBLK_S_DEV_QUIESCED => DeviceState::Waiting,
            sys::UBLK_S_DEV_DEAD => DeviceState::Dead,
            other => {
                return Err(io::Error::other(format!(
                    "device {number} is in state {other}, which this daemon does not know"
                )))
            }
        };
        // Gone between the two commands: there is no device.
        let params = match raw::params(number) {
            Ok(params) => params,
            Err(e) if e.raw_os_error() == Some(libc::ENODEV) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(Some(DeviceInfo {
            state,
            shape: Shape {
                size: params.basic.dev_sectors << 9,
                queues: info.nr_hw_queues,
                depth: info.queue_depth,
                flags: info.flags & FLAGS,
            },
        }))
    }

    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError> {
        if shape.queues != 1 {
            return Err(AddError::Other(io::Error::other(
                "this version adds devices of one queue",
            )));
        }
        match raw::add(number, shape.depth, MAX_IO, shape.flags) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => Err(AddError::Taken),
            Err(e) => Err(AddError::Other(e)),
        }
    }

    /// Nothing to do: a device added by [`raw::add`] is owned by
    /// nothing but the kernel from the start.
    fn disown(&mut self, _number: u32) {}

    fn delete(&mut self, number: u32) -> io::Result<()> {
        raw::delete(number)
    }

    /// The driver answers `EBUSY` while the server before is still
    /// being torn down, which can outlast the moment the device is
    /// first reported as waiting. That is asked again, up to
    /// [`BUSY_RETRIES`] times, [`BUSY_PAUSE`] apart; every other answer
    /// is passed on at once. (The retry is this crate's reading: the
    /// library does the same, for up to 30 s.)
    fn begin_recovery(&mut self, number: u32) -> io::Result<()> {
        let mut tries = 0;
        loop {
            match raw::begin_recovery(number) {
                Err(e) if e.raw_os_error() == Some(libc::EBUSY) && tries < BUSY_RETRIES => {
                    tries += 1;
                    std::thread::sleep(BUSY_PAUSE);
                }
                other => return other,
            }
        }
    }

    fn client_holds(&mut self, number: u32) -> Option<bool> {
        let opened = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_EXCL | libc::O_NONBLOCK)
            .open(block_device(number));
        match opened {
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => Some(true),
            _ => None,
        }
    }
}
