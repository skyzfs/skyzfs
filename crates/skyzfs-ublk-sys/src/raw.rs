//! The control commands this crate sends itself, on a ring of its own,
//! with the driver's own answer: see the [crate documentation](crate)
//! for why the library's are not used for these.
//!
//! Each command opens `/dev/ublk-control` and makes a small ring for
//! itself. Control commands are rare (a start, a `status`, a `detach`),
//! and nothing is kept between two.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;

use io_uring::{opcode, squeue, types, IoUring};
use libublk::sys;

/// The driver's control device.
pub const CONTROL: &str = "/dev/ublk-control";

/// Send one control command for device `number` and return the
/// driver's result (zero or more) with the structure as the driver
/// left it, or the error the driver answered with, as it answered it.
/// `data` is the structure the command reads or fills, and `word` the
/// one number some commands take in the header instead.
///
/// `T` is one of the plain C structures of `libublk::sys` that the
/// driver defines for the command: this module's callers see to that,
/// and the function is not public.
fn command<T: Copy>(
    op: u32,
    number: u32,
    word: u64,
    data: Option<T>,
) -> io::Result<(i32, Option<T>)> {
    let control = OpenOptions::new().read(true).write(true).open(CONTROL)?;
    let mut ring = IoUring::<squeue::Entry128>::builder().build(2)?;
    // The structure is on the heap, so that it can be left there if the
    // command's end cannot be waited for (below).
    let mut data = data.map(Box::new);
    let (addr, len) = match data.as_deref_mut() {
        Some(data) => (
            std::ptr::from_mut(data) as u64,
            std::mem::size_of::<T>() as u16,
        ),
        None => (0, 0),
    };
    let header = sys::ublksrv_ctrl_cmd {
        dev_id: number,
        queue_id: u16::MAX,
        len,
        addr,
        data: [word],
        ..Default::default()
    };
    let mut bytes = [0u8; 80];
    const HEADER: usize = std::mem::size_of::<sys::ublksrv_ctrl_cmd>();
    const { assert!(HEADER <= 80) };
    // SAFETY: `header` is a live, plain C structure of `HEADER` bytes,
    // every byte of it initialised (it has no padding: the driver's
    // header is 32 bytes of integer fields, and `Default` zeroed it);
    // `bytes` has room for `HEADER` bytes, as asserted above; the two
    // do not overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(
            std::ptr::from_ref(&header).cast::<u8>(),
            bytes.as_mut_ptr(),
            HEADER,
        );
    }
    let entry = opcode::UringCmd80::new(types::Fd(control.as_raw_fd()), op)
        .cmd(bytes)
        .build()
        .user_data(1);
    // SAFETY: the entry names a descriptor (`control`) that stays open
    // until this function returns, and the address of `data`, which is
    // a heap allocation this function owns. The kernel reads and writes
    // `data` until the command completes. On every path below the
    // allocation outlives that: either the completion has been taken
    // from the ring before `data` is read or freed, or `data` is leaked
    // and never freed. Nothing in this function reads `data` before the
    // completion. The entry itself is copied into the ring by `push`.
    let pushed = unsafe { ring.submission().push(&entry) };
    if pushed.is_err() {
        // Nothing was submitted: the kernel has not seen the address.
        return Err(io::Error::other("the ring is full"));
    }
    let result = loop {
        match ring.submit_and_wait(1) {
            // An interrupted wait is taken up again.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                // The command may be in the kernel and still running:
                // the driver executes control commands on a worker of
                // the ring. Its memory is left to it for good.
                if let Some(data) = data {
                    Box::leak(data);
                }
                return Err(e);
            }
            Ok(_) => match ring.completion().next() {
                Some(done) => break done.result(),
                // Woken without the completion: wait again.
                None => continue,
            },
        }
    };
    if result < 0 {
        Err(io::Error::from_raw_os_error(-result))
    } else {
        Ok((result, data.map(|data| *data)))
    }
}

fn filled<T>(data: Option<T>) -> io::Result<T> {
    data.ok_or_else(|| io::Error::other("a control command lost its structure"))
}

/// The driver's features (`UBLK_U_CMD_GET_FEATURES`, Linux 6.5), asked
/// for under a device number that is never a device's: `None` when the
/// driver has no such command, which it then answers with `ENODEV` (see
/// [`skyzfs_ublk::features`]).
pub fn features() -> io::Result<Option<u64>> {
    match command(sys::UBLK_U_CMD_GET_FEATURES, u32::MAX, 0, Some(0u64)) {
        Ok((_, data)) => filled(data).map(Some),
        Err(e) => match e.raw_os_error() {
            Some(code) => {
                skyzfs_ublk::features::answered(Err(code)).map_err(io::Error::from_raw_os_error)
            }
            None => Err(e),
        },
    }
}

/// Add a device under `number` with one queue of `depth`, the flags
/// `flags` and the largest request `max_io` bytes, served by this
/// process. An error is the driver's: `EEXIST` when the number is
/// taken, also by a device that is deleted and still held.
///
/// Nothing owns the device afterwards but the kernel: it is deleted
/// only by [`delete`].
pub fn add(number: u32, depth: u16, max_io: u32, flags: u64) -> io::Result<()> {
    let info = sys::ublksrv_ctrl_dev_info {
        dev_id: number,
        nr_hw_queues: 1,
        queue_depth: depth,
        max_io_buf_bytes: max_io,
        ublksrv_pid: std::process::id() as i32,
        flags,
        ..Default::default()
    };
    command(sys::UBLK_U_CMD_ADD_DEV, number, 0, Some(info)).map(|_| ())
}

/// The device under `number` as the driver reports it. An error is the
/// driver's: `ENODEV` when there is none.
pub fn info(number: u32) -> io::Result<sys::ublksrv_ctrl_dev_info> {
    let info = sys::ublksrv_ctrl_dev_info {
        dev_id: number,
        ..Default::default()
    };
    filled(command(sys::UBLK_U_CMD_GET_DEV_INFO, number, 0, Some(info))?.1)
}

/// The parameters of the device under `number`. A device that was
/// never started has none: the driver then answers with zeros.
pub fn params(number: u32) -> io::Result<sys::ublk_params> {
    let params = sys::ublk_params {
        len: std::mem::size_of::<sys::ublk_params>() as u32,
        ..Default::default()
    };
    filled(command(sys::UBLK_U_CMD_GET_PARAMS, number, 0, Some(params))?.1)
}

/// Give the device under `number` these parameters. The answer is the
/// driver's own: it takes parameters only for a device that was never
/// started.
///
/// The daemon does not call this: the parameters of a device it added
/// are set, with the same command and structure, by the library's
/// control object just before its start ([`crate::queue::Device::start`]).
/// It is here for the test program, which shows what the driver
/// answers for a device that is live or waits.
pub fn set_params(number: u32, mut params: sys::ublk_params) -> io::Result<()> {
    params.len = std::mem::size_of::<sys::ublk_params>() as u32;
    command(sys::UBLK_U_CMD_SET_PARAMS, number, 0, Some(params)).map(|_| ())
}

/// Delete the device under `number` without waiting for whoever holds
/// it: the driver's asynchronous delete. A device that is held keeps
/// its number until the holder lets go.
pub fn delete(number: u32) -> io::Result<()> {
    command::<u8>(sys::UBLK_U_CMD_DEL_DEV_ASYNC, number, 0, None).map(|_| ())
}

/// End the recovery of the device under `number`, whose queue this
/// process has given its fetches: the device is live again, served by
/// this process. The driver waits until the queue is ready.
///
/// This is sent from here and not through the library because the
/// library ends a recovery only through a control object made with its
/// recover flag, and such an object writes a file of the library's own
/// (`/run/ublksrvd/NNNN.json`) when it does. Without the flag the
/// library writes nothing: see the crate documentation.
pub fn end_recovery(number: u32) -> io::Result<()> {
    command::<u8>(
        sys::UBLK_U_CMD_END_USER_RECOVERY,
        number,
        u64::from(std::process::id()),
        None,
    )
    .map(|_| ())
}

/// Begin the recovery of the device under `number`, which waits for a
/// server. An error is the driver's own: `EBUSY` while the server
/// before is not yet wholly gone, `ENODEV` when the device is.
pub fn begin_recovery(number: u32) -> io::Result<()> {
    command::<u8>(sys::UBLK_U_CMD_START_USER_RECOVERY, number, 0, None).map(|_| ())
}
