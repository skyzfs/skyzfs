//! The libublk spike of work package M1.3b: a small ublk server that
//! answers the questions in `docs/skyzfs-ublk-spike.md` by running.
//! It is not the binding and nothing of `skyzfsd` is built on it.
//!
//! ```text
//! spike features                  the driver's feature mask
//! spike info N                    device N as the driver reports it
//! spike consts                    the constants of the header the crate ships
//! spike add N [--size-mib M] [--no-start] [--linger S] [--no-wz] [--plain-delete-on-drop]
//! spike takeover N [--die-after-begin]
//! spike delete N                  the asynchronous delete
//! spike rawadd N                  ADD_DEV sent by this program, not the library
//! spike rawinfo N                 GET_DEV_INFO sent by this program
//! spike watch N MS                the device's state every 2 ms, for MS ms
//! spike excl PATH                 an exclusive open that does no I/O
//! ```
//!
//! `add` and `takeover` serve until the file `<image>.stop` appears.
//! The target is a file, `/dev/shm/skz-spike-N.img`, so that what was
//! written outlives a kill of the server. Requests are executed on a
//! worker thread and completed on the queue's thread after an eventfd
//! wake: the shape `pump::Queue` needs.
//!
//! Every answer is a line `spike_<name>=<value>` on stdout.

use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use io_uring::{opcode, squeue, types, IoUring};
use libublk::ctrl::{UblkCtrl, UblkCtrlBuilder};
use libublk::io::{BufDescList, UblkDev, UblkQueue};
use libublk::{sys, BufDesc, UblkError, UblkFlags, UblkIORes};

const DEPTH: u16 = 64;
const MAX_IO: u32 = 1 << 20;
const BLOCK_SHIFT: u8 = 12;
/// The user data of the eventfd's read on the queue's ring: a target
/// I/O (bit 63), not a command of the driver.
const WAKE: u64 = (1 << 63) | 1;

fn say(name: &str, value: impl std::fmt::Display) {
    println!("spike_{name}={value}");
}

fn errno_of(error: &UblkError) -> String {
    match error {
        UblkError::OtherError(n) | UblkError::UringIOError(n) => format!("errno {}", -n),
        UblkError::IOError(e) => format!("io {:?}", e.raw_os_error()),
        other => format!("{other:?}"),
    }
}

fn state_name(state: u16) -> &'static str {
    match u32::from(state) {
        sys::UBLK_S_DEV_DEAD => "DEAD",
        sys::UBLK_S_DEV_LIVE => "LIVE",
        sys::UBLK_S_DEV_QUIESCED => "QUIESCED",
        sys::UBLK_S_DEV_FAIL_IO => "FAIL_IO",
        _ => "unknown",
    }
}

fn image(number: i32) -> String {
    format!("/dev/shm/skz-spike-{number}.img")
}

fn features() {
    match UblkCtrl::get_features() {
        Some(mask) => {
            say("features", format!("{mask:#x}"));
            let reissue = mask & u64::from(sys::UBLK_F_USER_RECOVERY_REISSUE) != 0;
            say("features_reissue", reissue);
        }
        None => say("features", "unreadable"),
    }
}

fn info(number: i32) {
    let ctrl = match UblkCtrl::new_simple(number) {
        Ok(ctrl) => ctrl,
        Err(e) => {
            return say(
                &format!("info_{number}"),
                format!("absent ({})", errno_of(&e)),
            )
        }
    };
    let i = ctrl.dev_info();
    say(
        &format!("info_{number}"),
        format!(
            "state {} ({}) pid {} queues {} depth {} max_io_buf_bytes {} flags {:#x} ublksrv_flags {:#x}",
            state_name(i.state),
            i.state,
            i.ublksrv_pid,
            i.nr_hw_queues,
            i.queue_depth,
            i.max_io_buf_bytes,
            i.flags,
            i.ublksrv_flags
        ),
    );
    let mut params = sys::ublk_params {
        len: std::mem::size_of::<sys::ublk_params>() as u32,
        ..Default::default()
    };
    match ctrl.get_params(&mut params) {
        Ok(_) => say(
            &format!("params_{number}"),
            format!(
                "types {:#x} attrs {:#x} logical_bs_shift {} physical_bs_shift {} max_sectors {} dev_sectors {} max_discard_sectors {} max_write_zeroes_sectors {} discard_granularity {} max_discard_segments {}",
                params.types,
                params.basic.attrs,
                params.basic.logical_bs_shift,
                params.basic.physical_bs_shift,
                params.basic.max_sectors,
                params.basic.dev_sectors,
                params.discard.max_discard_sectors,
                params.discard.max_write_zeroes_sectors,
                params.discard.discard_granularity,
                params.discard.max_discard_segments
            ),
        ),
        Err(e) => say(&format!("params_{number}"), format!("unreadable ({})", errno_of(&e))),
    }
}

fn consts() {
    for (name, value) in [
        ("IO_F_FAILFAST_DEV", sys::UBLK_IO_F_FAILFAST_DEV),
        ("IO_F_FAILFAST_TRANSPORT", sys::UBLK_IO_F_FAILFAST_TRANSPORT),
        ("IO_F_FAILFAST_DRIVER", sys::UBLK_IO_F_FAILFAST_DRIVER),
        ("IO_F_META", sys::UBLK_IO_F_META),
        ("IO_F_FUA", sys::UBLK_IO_F_FUA),
        ("IO_F_NOUNMAP", sys::UBLK_IO_F_NOUNMAP),
        ("IO_F_SWAP", sys::UBLK_IO_F_SWAP),
        ("IO_F_NEED_REG_BUF", sys::UBLK_IO_F_NEED_REG_BUF),
        ("IO_F_INTEGRITY", sys::UBLK_IO_F_INTEGRITY),
        ("IO_F_SHMEM_ZC", sys::UBLK_IO_F_SHMEM_ZC),
        ("IO_OP_READ", sys::UBLK_IO_OP_READ),
        ("IO_OP_WRITE", sys::UBLK_IO_OP_WRITE),
        ("IO_OP_FLUSH", sys::UBLK_IO_OP_FLUSH),
        ("IO_OP_DISCARD", sys::UBLK_IO_OP_DISCARD),
        ("IO_OP_WRITE_SAME", sys::UBLK_IO_OP_WRITE_SAME),
        ("IO_OP_WRITE_ZEROES", sys::UBLK_IO_OP_WRITE_ZEROES),
    ] {
        say(&format!("const_{name}"), format!("{value:#x}"));
    }
}

/// One request, as the worker gets it. The buffer is the queue's own
/// for this tag: the worker is the only one that touches it between
/// the fetch and the completion.
struct Job {
    tag: u16,
    op: u32,
    offset: u64,
    bytes: u32,
    buf: *mut u8,
}

// SAFETY: the pointer is to the queue's buffer of one tag, which lives
// as long as the queue's thread serves, and which nothing else reads or
// writes while the request of that tag is with the worker.
unsafe impl Send for Job {}

#[derive(Default)]
struct Counters {
    reads: AtomicU64,
    writes: AtomicU64,
    flushes: AtomicU64,
    discards: AtomicU64,
    write_zeroes: AtomicU64,
    others: AtomicU64,
    fua: AtomicU64,
    largest: AtomicU64,
    idle_wakes: AtomicU64,
    last_idle_ms: AtomicU64,
}

impl Counters {
    fn line(&self) -> String {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        format!(
            "reads {} writes {} flushes {} discards {} write_zeroes {} others {} fua {} largest {} idle_wakes {} last_idle_ms {}\n",
            g(&self.reads),
            g(&self.writes),
            g(&self.flushes),
            g(&self.discards),
            g(&self.write_zeroes),
            g(&self.others),
            g(&self.fua),
            g(&self.largest),
            g(&self.idle_wakes),
            g(&self.last_idle_ms)
        )
    }
}

/// The worker: executes each request on the file and puts the result
/// on the list, then wakes the queue's thread.
fn worker(file: File, jobs: Receiver<Job>, done: Arc<Mutex<Vec<(u16, i32)>>>, wake: Arc<OwnedFd>) {
    let zeros = vec![0u8; MAX_IO as usize];
    for job in jobs {
        let len = job.bytes as usize;
        let result = match job.op {
            sys::UBLK_IO_OP_READ => {
                // SAFETY: see `Job`. The buffer has room for the
                // largest request.
                let buf = unsafe { std::slice::from_raw_parts_mut(job.buf, len) };
                file.read_exact_at(buf, job.offset)
                    .map(|()| job.bytes as i32)
            }
            sys::UBLK_IO_OP_WRITE => {
                // SAFETY: see `Job`.
                let buf = unsafe { std::slice::from_raw_parts(job.buf, len) };
                file.write_all_at(buf, job.offset)
                    .map(|()| job.bytes as i32)
            }
            sys::UBLK_IO_OP_FLUSH => Ok(0),
            sys::UBLK_IO_OP_DISCARD | sys::UBLK_IO_OP_WRITE_ZEROES => {
                let mut at = 0;
                let mut result = Ok(job.bytes as i32);
                while at < len {
                    let n = (len - at).min(zeros.len());
                    if let Err(e) = file.write_all_at(&zeros[..n], job.offset + at as u64) {
                        result = Err(e);
                        break;
                    }
                    at += n;
                }
                result
            }
            _ => Ok(-libc::EOPNOTSUPP),
        };
        let result = result.unwrap_or(-libc::EIO);
        if let Ok(mut list) = done.lock() {
            list.push((job.tag, result));
        }
        let one = 1u64.to_ne_bytes();
        // SAFETY: a write of eight bytes from a live buffer to an
        // eventfd this thread holds a reference to.
        unsafe { libc::write(wake.as_raw_fd(), one.as_ptr().cast(), 8) };
    }
}

struct Shared {
    stop: AtomicBool,
    counters: Counters,
}

/// The queue's thread: fetch, hand to the worker, complete on the wake.
fn queue(
    dev: &UblkDev,
    jobs: Sender<Job>,
    done: Arc<Mutex<Vec<(u16, i32)>>>,
    wake: Arc<OwnedFd>,
    shared: Arc<Shared>,
) -> Result<(), UblkError> {
    let bufs = Rc::new(dev.alloc_queue_io_bufs());
    let q =
        UblkQueue::new(0, dev)?.submit_fetch_commands_unified(BufDescList::Slices(Some(&bufs)))?;
    // The eventfd's counter is read into this, again after every wake.
    let mut counter = [0u8; 8];
    let arm = |q: &UblkQueue, counter: &mut [u8; 8]| {
        let read = opcode::Read::new(types::Fd(wake.as_raw_fd()), counter.as_mut_ptr(), 8)
            .build()
            .user_data(WAKE);
        q.ublk_submit_sqe_sync(read)
    };
    arm(&q, &mut counter)?;

    let mut with_worker = 0u32;
    let mut fetched: Vec<u16> = Vec::new();
    let mut woken;
    let mut last_return = Instant::now();
    loop {
        fetched.clear();
        woken = false;
        let got = q.flush_and_wake_io_tasks(
            |user_data, cqe, _last| {
                if user_data == WAKE {
                    woken = true;
                } else if user_data & (1 << 63) == 0 && cqe.result() == sys::UBLK_IO_RES_OK as i32 {
                    fetched.push((user_data & 0xffff) as u16);
                }
            },
            1,
        );
        let c = &shared.counters;
        match got {
            Ok(0) => {
                // Nothing arrived: the library's own time-out.
                c.idle_wakes.fetch_add(1, Ordering::Relaxed);
                c.last_idle_ms
                    .store(last_return.elapsed().as_millis() as u64, Ordering::Relaxed);
            }
            Ok(_) => {}
            Err(UblkError::QueueIsDown) => return Ok(()),
            Err(e) => return Err(e),
        }
        last_return = Instant::now();
        for tag in fetched.drain(..) {
            let iod = q.get_iod(tag);
            let op = iod.op_flags & 0xff;
            let bytes = iod.nr_sectors << 9;
            match op {
                sys::UBLK_IO_OP_READ => &c.reads,
                sys::UBLK_IO_OP_WRITE => &c.writes,
                sys::UBLK_IO_OP_FLUSH => &c.flushes,
                sys::UBLK_IO_OP_DISCARD => &c.discards,
                sys::UBLK_IO_OP_WRITE_ZEROES => &c.write_zeroes,
                _ => &c.others,
            }
            .fetch_add(1, Ordering::Relaxed);
            if iod.op_flags & sys::UBLK_IO_F_FUA != 0 {
                c.fua.fetch_add(1, Ordering::Relaxed);
            }
            c.largest.fetch_max(u64::from(bytes), Ordering::Relaxed);
            let job = Job {
                tag,
                op,
                offset: iod.start_sector << 9,
                bytes,
                buf: bufs[tag as usize].as_mut_ptr(),
            };
            if jobs.send(job).is_ok() {
                with_worker += 1;
            }
        }
        if woken {
            let finished: Vec<(u16, i32)> = match done.lock() {
                Ok(mut list) => std::mem::take(&mut *list),
                Err(_) => Vec::new(),
            };
            for (tag, result) in finished {
                with_worker -= 1;
                // The slice's address is what the driver is given for
                // the next request of this tag: its length is not used.
                let buf = BufDesc::Slice(bufs[tag as usize].as_slice());
                q.complete_io_cmd_unified(tag, buf, Ok(UblkIORes::Result(result)))?;
            }
            arm(&q, &mut counter)?;
        }
        if shared.stop.load(Ordering::Relaxed) && with_worker == 0 {
            // An orderly stop: nothing is with the worker. The commits
            // above are flushed, and the thread returns with the device
            // as it is.
            let _ = q.flush_and_wake_io_tasks(|_, _, _| {}, 0);
            return Ok(());
        }
    }
}

struct Options {
    size_mib: u64,
    no_start: bool,
    linger: u64,
    no_wz: bool,
    keep_owned: bool,
    die_after_begin: bool,
}

fn serve(number: i32, takeover: bool, options: &Options) -> Result<(), String> {
    let path = image(number);
    let stop_path = format!("{path}.stop");
    let _ = std::fs::remove_file(&stop_path);
    let flags = u64::from(sys::UBLK_F_USER_RECOVERY | sys::UBLK_F_USER_RECOVERY_REISSUE);

    if takeover {
        let old = UblkCtrl::new_simple(number).map_err(|e| format!("lookup: {}", errno_of(&e)))?;
        let began = Instant::now();
        let result = old.start_user_recover();
        say(
            "begin_recovery",
            format!("{result:?} after {} ms", began.elapsed().as_millis()),
        );
        result.map_err(|e| format!("begin recovery: {}", errno_of(&e)))?;
        if options.die_after_begin {
            return Ok(());
        }
    }
    let ctrl = UblkCtrlBuilder::default()
        .name("skz-spike")
        .id(number)
        .nr_queues(1)
        .depth(DEPTH)
        .io_buf_bytes(MAX_IO)
        .ctrl_flags(flags)
        .dev_flags(if takeover {
            UblkFlags::UBLK_DEV_F_RECOVER_DEV
        } else {
            UblkFlags::UBLK_DEV_F_ADD_DEV
        })
        .build();
    let ctrl = match ctrl {
        Ok(ctrl) => ctrl,
        Err(e) => {
            say("add", format!("failed ({})", errno_of(&e)));
            return Err(format!("add: {}", errno_of(&e)));
        }
    };
    if !options.keep_owned {
        // At once: a control object that still owns its device deletes
        // it when it is dropped, with the delete that waits.
        ctrl.disown();
    }
    let i = ctrl.dev_info();
    say(
        "add",
        format!(
            "ok id {} asked flags {flags:#x} reported flags {:#x} queues {} depth {} max_io_buf_bytes {}",
            i.dev_id, i.flags, i.nr_hw_queues, i.queue_depth, i.max_io_buf_bytes
        ),
    );
    if options.linger > 0 {
        std::thread::sleep(Duration::from_secs(options.linger));
    }
    if options.no_start {
        return Ok(());
    }

    let size = if takeover {
        std::fs::metadata(&path)
            .map_err(|e| format!("{path}: {e}"))?
            .len()
    } else {
        options.size_mib << 20
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("{path}: {e}"))?;
    file.set_len(size).map_err(|e| format!("{path}: {e}"))?;

    let no_wz = options.no_wz;
    let dev = UblkDev::new(
        "skz-spike".to_string(),
        |dev: &mut UblkDev| {
            dev.tgt.dev_size = size;
            dev.tgt.params = sys::ublk_params {
                types: sys::UBLK_PARAM_TYPE_BASIC | sys::UBLK_PARAM_TYPE_DISCARD,
                basic: sys::ublk_param_basic {
                    attrs: sys::UBLK_ATTR_VOLATILE_CACHE,
                    logical_bs_shift: BLOCK_SHIFT,
                    physical_bs_shift: BLOCK_SHIFT,
                    io_opt_shift: BLOCK_SHIFT,
                    io_min_shift: BLOCK_SHIFT,
                    max_sectors: MAX_IO >> 9,
                    dev_sectors: size >> 9,
                    ..Default::default()
                },
                discard: sys::ublk_param_discard {
                    discard_granularity: 1 << BLOCK_SHIFT,
                    max_discard_sectors: MAX_IO >> 9,
                    max_write_zeroes_sectors: if no_wz { 0 } else { MAX_IO >> 9 },
                    max_discard_segments: 1,
                    ..Default::default()
                },
                ..Default::default()
            };
            Ok(())
        },
        &ctrl,
    )
    .map_err(|e| format!("the device: {}", errno_of(&e)))?;
    let dev = Arc::new(dev);

    // SAFETY: eventfd takes no pointers; the descriptor it returns is
    // owned by nothing else.
    let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
    if raw < 0 {
        return Err("eventfd".into());
    }
    // SAFETY: `raw` is a descriptor this function just made.
    let wake = Arc::new(unsafe { OwnedFd::from_raw_fd(raw) });
    let done = Arc::new(Mutex::new(Vec::new()));
    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        counters: Counters::default(),
    });
    let (jobs, jobs_rx) = mpsc::channel();
    let worker_thread = {
        let (done, wake) = (done.clone(), wake.clone());
        std::thread::spawn(move || worker(file, jobs_rx, done, wake))
    };
    let (tid_tx, tid_rx) = mpsc::channel();
    let queue_thread = {
        let (dev, done, wake, shared) = (dev.clone(), done.clone(), wake.clone(), shared.clone());
        std::thread::spawn(move || {
            let tid = UblkCtrl::init_queue_thread();
            let _ = tid_tx.send(tid);
            queue(&dev, jobs, done, wake, shared).map_err(|e| errno_of(&e))
        })
    };
    let tid = tid_rx
        .recv()
        .map_err(|e| format!("the queue's thread: {e}"))?;
    ctrl.configure_queue(&dev, 0, tid)
        .map_err(|e| format!("configure the queue: {}", errno_of(&e)))?;
    let began = Instant::now();
    let started = ctrl.start_dev(&dev);
    say(
        if takeover { "end_recovery" } else { "start" },
        format!("{started:?} after {} ms", began.elapsed().as_millis()),
    );
    started.map_err(|e| format!("start: {}", errno_of(&e)))?;
    println!("ready /dev/ublkb{number}");

    // Serve until the stop file appears; keep the counters on a file
    // that outlives a kill.
    let stats = format!("{path}.stats");
    let mut last = String::new();
    while !std::path::Path::new(&stop_path).exists() {
        if queue_thread.is_finished() {
            break;
        }
        let line = shared.counters.line();
        if line != last {
            let _ = std::fs::write(&stats, &line);
            last = line;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let stopping = Instant::now();
    shared.stop.store(true, Ordering::Relaxed);
    let one = 1u64.to_ne_bytes();
    // SAFETY: a write of eight bytes from a live buffer to an eventfd
    // this function holds a reference to.
    unsafe { libc::write(wake.as_raw_fd(), one.as_ptr().cast(), 8) };
    let ended = queue_thread
        .join()
        .map_err(|_| "the queue's thread panicked".to_string())?;
    say(
        "stop",
        format!("{ended:?} after {} ms", stopping.elapsed().as_millis()),
    );
    let _ = std::fs::write(&stats, shared.counters.line());
    drop(dev);
    let _ = worker_thread.join();
    Ok(())
}

/// One control command sent by this program on a ring of its own, with
/// the driver's own result: the library retries a failed command with
/// the legacy opcode and returns that second result, so on a kernel
/// without the legacy opcodes every failure reads as "not supported".
fn raw_ctrl(op: u32, info: &mut sys::ublksrv_ctrl_dev_info) -> Result<i32, String> {
    let control = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/ublk-control")
        .map_err(|e| format!("/dev/ublk-control: {e}"))?;
    let mut ring = IoUring::<squeue::Entry128>::builder()
        .build(4)
        .map_err(|e| format!("a ring: {e}"))?;
    let command = sys::ublksrv_ctrl_cmd {
        dev_id: info.dev_id,
        queue_id: u16::MAX,
        len: std::mem::size_of::<sys::ublksrv_ctrl_dev_info>() as u16,
        addr: std::ptr::from_mut(info) as u64,
        ..Default::default()
    };
    let mut bytes = [0u8; 80];
    // SAFETY: the command is a plain C struct of 32 bytes, copied into
    // a buffer of 80.
    unsafe {
        std::ptr::copy_nonoverlapping(
            std::ptr::from_ref(&command).cast::<u8>(),
            bytes.as_mut_ptr(),
            std::mem::size_of::<sys::ublksrv_ctrl_cmd>(),
        );
    }
    let sqe = opcode::UringCmd80::new(types::Fd(control.as_raw_fd()), op)
        .cmd(bytes)
        .build()
        .user_data(1);
    // SAFETY: the entry points at `info`, which outlives the wait
    // below, and at a descriptor that is open until this returns.
    unsafe { ring.submission().push(&sqe) }.map_err(|e| format!("push: {e}"))?;
    ring.submit_and_wait(1)
        .map_err(|e| format!("submit: {e}"))?;
    let result = ring.completion().next().map(|cqe| cqe.result());
    result.ok_or_else(|| "no completion".to_string())
}

fn raw(number: i32, add: bool) -> Result<(), String> {
    let flags = u64::from(sys::UBLK_F_USER_RECOVERY | sys::UBLK_F_USER_RECOVERY_REISSUE);
    let mut info = sys::ublksrv_ctrl_dev_info {
        dev_id: number as u32,
        ..Default::default()
    };
    if add {
        info.nr_hw_queues = 1;
        info.queue_depth = DEPTH;
        info.max_io_buf_bytes = MAX_IO;
        info.ublksrv_pid = std::process::id() as i32;
        info.flags = flags;
    }
    let op = if add {
        sys::UBLK_U_CMD_ADD_DEV
    } else {
        sys::UBLK_U_CMD_GET_DEV_INFO
    };
    let began = Instant::now();
    let result = raw_ctrl(op, &mut info)?;
    say(
        if add { "rawadd" } else { "rawinfo" },
        format!(
            "{number}: result {result} after {} ms; id {} state {} ({}) pid {} flags {:#x}",
            began.elapsed().as_millis(),
            info.dev_id,
            state_name(info.state),
            info.state,
            info.ublksrv_pid,
            info.flags
        ),
    );
    Ok(())
}

fn delete(number: i32) -> Result<(), String> {
    let ctrl = UblkCtrl::new_simple(number).map_err(|e| format!("lookup: {}", errno_of(&e)))?;
    let began = Instant::now();
    let result = ctrl.del_dev_async();
    say(
        "delete_async",
        format!("{result:?} after {} ms", began.elapsed().as_millis()),
    );
    result.map(|_| ()).map_err(|e| errno_of(&e))
}

fn watch(number: i32, ms: u64) {
    let began = Instant::now();
    let mut last = String::new();
    while began.elapsed() < Duration::from_millis(ms) {
        let now = match UblkCtrl::new_simple(number) {
            Ok(ctrl) => {
                let i = ctrl.dev_info();
                format!("{} pid {}", state_name(i.state), i.ublksrv_pid)
            }
            Err(e) => format!("absent ({})", errno_of(&e)),
        };
        if now != last {
            say(
                "watch",
                format!("{} ms: {now}", began.elapsed().as_millis()),
            );
            last = now;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn excl(path: &str) {
    let began = Instant::now();
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_EXCL | libc::O_NONBLOCK)
        .open(path);
    let result = match opened {
        Ok(_) => "opened".to_string(),
        Err(e) => format!("refused ({:?})", e.raw_os_error()),
    };
    say(
        "excl",
        format!("{result} after {} ms", began.elapsed().as_millis()),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let number = args.get(1).and_then(|n| n.parse::<i32>().ok());
    let options = Options {
        size_mib: after("--size-mib").unwrap_or(256),
        no_start: has("--no-start"),
        linger: after("--linger").unwrap_or(0),
        no_wz: has("--no-wz"),
        keep_owned: has("--plain-delete-on-drop"),
        die_after_begin: has("--die-after-begin"),
    };
    let result = match (args.first().map(String::as_str), number) {
        (Some("features"), _) => {
            features();
            Ok(())
        }
        (Some("consts"), _) => {
            consts();
            Ok(())
        }
        (Some("info"), Some(n)) => {
            info(n);
            Ok(())
        }
        (Some("add"), Some(n)) => serve(n, false, &options),
        (Some("takeover"), Some(n)) => serve(n, true, &options),
        (Some("delete"), Some(n)) => delete(n),
        (Some("rawadd"), Some(n)) => raw(n, true),
        (Some("rawinfo"), Some(n)) => raw(n, false),
        (Some("watch"), Some(n)) => {
            let ms = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(2000);
            watch(n, ms);
            Ok(())
        }
        (Some("excl"), _) => {
            excl(args.get(1).map(String::as_str).unwrap_or(""));
            Ok(())
        }
        _ => Err(
            "usage: spike features|consts|info N|add N|takeover N|delete N|watch N MS|excl PATH"
                .into(),
        ),
    };
    if let Err(why) = result {
        eprintln!("spike: {why}");
        std::process::exit(1);
    }
}
