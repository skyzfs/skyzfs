//! One device's queue over the library, as the pump's [`Queue`].
//!
//! # The buffers: one copy per request
//!
//! The queue owns one [`Buf`] per tag, of the largest request's size,
//! locked in memory (and unlocked again when the queue is dropped).
//! [`Queue::take_buf`] lends the tag's buffer out at the request's
//! length, the request carries it through the target, the completion
//! brings it back, and [`Queue::commit`] puts it in its place before it
//! tells the driver the buffer's address for the tag's next request. So
//! the driver copies a write's data straight into the memory the store
//! writes from, and takes a read's data straight from the memory the
//! store read into.
//!
//! **Between a tag's commit and the completion of its next fetch
//! nothing in the process holds a reference into that tag's memory**,
//! because the kernel writes there in that time (a write's data, on the
//! queue's thread, before the fetch completes). The buffer sits in its
//! slot as an owned value; it is not read, not written and not
//! borrowed. That is what makes it sound to give its address away, and
//! it needs no `unsafe` here: the library's commit takes a slice and
//! uses only its address.
//!
//! # The first request of each tag
//!
//! The library gives the driver the address of a buffer of its own with
//! each tag's first fetch, and only a commit can name another. So the
//! first request of every tag after a start, a take-over included,
//! arrives in the library's buffer. If it is a write, its data is
//! copied once into the tag's own buffer, and counted
//! ([`Stopper::first_write_copies`]): at most `depth` copies per start.
//! A read needs none: its data is taken from the address its commit
//! names. From a tag's first commit on, the address is ours.
//!
//! # Threads
//!
//! The library's queue lives on one thread and cannot leave it.
//! [`Device::serve`] starts that thread and runs the caller's body on
//! it with the queue; everything else of a [`Device`] is called from
//! the thread that made it.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use io_uring::{opcode, types};
use libublk::ctrl::{UblkCtrl, UblkCtrlBuilder};
use libublk::helpers::IoBuf;
use libublk::io::{BufDescList, UblkDev, UblkQueue};
use libublk::{sys, BufDesc, UblkError, UblkFlags, UblkIORes};
use skyzfs_blk::{Buf, BLOCK_SIZE};
use skyzfs_ublk::pump::{Fetched, Queue, QueueDown, Wake as Woken};
use skyzfs_ublk::translate::Desc;

use crate::mem::{self, Locked};
use crate::{io_error, Wake, FLAGS, MAX_IO};

/// The user data of the wake descriptor's poll on the queue's ring:
/// bit 63 marks it as not a command of the driver.
const WAKE: u64 = (1 << 63) | 1;

/// How long [`Queue::pause`] sleeps.
pub const PAUSE: Duration = Duration::from_millis(100);

/// The name the library is given for the target. It appears in the
/// library's own file of a take-over and nowhere else.
const NAME: &str = "skyzfsd";

/// The blocks of the largest request.
const MAX_BLOCKS: usize = (MAX_IO as u64 / BLOCK_SIZE) as usize;

/// A device that exists, about to be served by this process: one this
/// instance added and has not started, or one that waits for a server
/// and whose recovery has begun.
///
/// Dropping it deletes nothing and stops nothing: the control object in
/// it is made without the library's add flag.
pub struct Device {
    control: UblkCtrl,
    dev: Arc<UblkDev>,
    number: u32,
    take_over: bool,
}

/// How a device is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Open {
    /// The device's number.
    pub number: u32,
    /// The size in bytes the device has, or is given by its first
    /// start.
    pub size: u64,
    /// Whether the device waits for a server and is taken over (its
    /// recovery has begun), or was added by this instance and never
    /// started.
    pub take_over: bool,
}

impl Device {
    /// Open the device for serving. The device's parameters are fixed
    /// here: 4096-byte blocks, a volatile write cache (so that flushes
    /// arrive), requests of at most [`MAX_IO`], and
    /// discard and write-zeroes of at most that, one segment each.
    pub fn open(open: &Open) -> io::Result<Self> {
        let Ok(id) = i32::try_from(open.number) else {
            return Err(io::Error::other("no such device number"));
        };
        let current = crate::raw::info(open.number)?;
        let control = UblkCtrlBuilder::default()
            .name(NAME)
            .id(id)
            .nr_queues(current.nr_hw_queues)
            .depth(current.queue_depth)
            .io_buf_bytes(current.max_io_buf_bytes)
            .ctrl_flags(FLAGS)
            // No flag: not the add flag, so that the object owns no
            // device and deletes none, and not the recover flag, so
            // that the library writes no file of its own. See the
            // crate documentation.
            .dev_flags(UblkFlags::empty())
            .build()
            .map_err(io_error)?;
        let size = open.size;
        let dev = UblkDev::new(
            NAME.to_string(),
            |dev: &mut UblkDev| {
                dev.tgt.dev_size = size;
                dev.tgt.params = parameters(size);
                Ok(())
            },
            &control,
        )
        .map_err(io_error)?;
        Ok(Self {
            control,
            dev: Arc::new(dev),
            number: open.number,
            take_over: open.take_over,
        })
    }

    /// The device's number.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// The depth of the device's queue.
    pub fn depth(&self) -> u16 {
        self.control.dev_info().queue_depth
    }

    /// Start the queue's thread and run `body` on it with the queue.
    /// Returns when the queue has given the driver its fetches, so that
    /// [`Device::start`] can follow.
    ///
    /// `wake` wakes the queue's wait from other threads: give the same
    /// one to the target's waker. With `require_locked`, a buffer that
    /// cannot be locked in memory is an error; without, the queue runs
    /// with it unlocked.
    pub fn serve<R, F>(&self, wake: &Wake, require_locked: bool, body: F) -> io::Result<Serving<R>>
    where
        R: Send + 'static,
        F: for<'q> FnOnce(KernelQueue<'q>) -> R + Send + 'static,
    {
        let stopper = Stopper {
            stop: Arc::new(AtomicBool::new(false)),
            copies: Arc::new(AtomicU64::new(0)),
            wake: wake.clone(),
        };
        let (ready, is_ready) = mpsc::channel();
        let (dev, theirs) = (self.dev.clone(), stopper.clone());
        let thread = std::thread::Builder::new()
            .name("skyzfsd-queue".into())
            .spawn(move || {
                let tid = UblkCtrl::init_queue_thread();
                match KernelQueue::new(&dev, theirs, require_locked) {
                    Ok(queue) => {
                        let _ = ready.send(Ok(tid));
                        Some(body(queue))
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        None
                    }
                }
            })?;
        let tid = is_ready
            .recv()
            .map_err(|_| io::Error::other("the queue's thread ended before it was ready"))??;
        self.control
            .configure_queue(&self.dev, 0, tid)
            .map_err(io_error)?;
        Ok(Serving { thread, stopper })
    }

    /// Start the device: a device that was never started gets its
    /// parameters and is started; a device that is taken over has its
    /// recovery ended. From here on the kernel issues requests.
    pub fn start(&self) -> io::Result<()> {
        if self.take_over {
            // What the library's start does before its command: wait
            // until the queue's thread has given the driver its
            // buffers. Then the end of the recovery, sent from here.
            self.dev.wait_for_buffer_registration(1).map_err(io_error)?;
            return crate::raw::end_recovery(self.number);
        }
        match self.control.start_dev(&self.dev).map_err(io_error)? {
            refused if refused < 0 => Err(io::Error::from_raw_os_error(-refused)),
            _ => Ok(()),
        }
    }
}

fn parameters(size: u64) -> sys::ublk_params {
    const SHIFT: u8 = 12;
    sys::ublk_params {
        types: sys::UBLK_PARAM_TYPE_BASIC | sys::UBLK_PARAM_TYPE_DISCARD,
        basic: sys::ublk_param_basic {
            attrs: sys::UBLK_ATTR_VOLATILE_CACHE,
            logical_bs_shift: SHIFT,
            physical_bs_shift: SHIFT,
            io_opt_shift: SHIFT,
            io_min_shift: SHIFT,
            max_sectors: MAX_IO >> 9,
            dev_sectors: size >> 9,
            ..Default::default()
        },
        discard: sys::ublk_param_discard {
            discard_granularity: 1 << SHIFT,
            max_discard_sectors: MAX_IO >> 9,
            max_write_zeroes_sectors: MAX_IO >> 9,
            max_discard_segments: 1,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A queue's thread, running.
pub struct Serving<R> {
    thread: JoinHandle<Option<R>>,
    stopper: Stopper,
}

impl<R> Serving<R> {
    /// The handle that stops the queue and reads its counter.
    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    /// Whether the queue's thread has ended.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Wait for the queue's thread and return what the body returned.
    pub fn join(self) -> io::Result<R> {
        match self.thread.join() {
            Ok(Some(result)) => Ok(result),
            Ok(None) => Err(io::Error::other("the queue was never made")),
            Err(_) => Err(io::Error::other("the queue's thread panicked")),
        }
    }
}

/// Asks a queue to stop, from any thread.
#[derive(Debug, Clone)]
pub struct Stopper {
    stop: Arc<AtomicBool>,
    copies: Arc<AtomicU64>,
    wake: Wake,
}

impl Stopper {
    /// Ask for an orderly stop: the queue's wait returns
    /// [`Wake::Stop`](skyzfs_ublk::pump::Wake::Stop) from now on.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake.wake();
    }

    /// How many first writes were copied from the library's buffer
    /// since this queue started: at most its depth.
    pub fn first_write_copies(&self) -> u64 {
        self.copies.load(Ordering::Relaxed)
    }
}

/// The queue, on its thread. See the [module documentation](self).
pub struct KernelQueue<'q> {
    // Declared first, so dropped first: the ring's commands are gone
    // before the buffers they name.
    queue: UblkQueue<'q>,
    /// The library's buffers: where each tag's first request arrives.
    first: Vec<IoBuf<u8>>,
    /// Each tag's own buffer; `None` while it is lent out.
    own: Vec<Option<Buf>>,
    /// Whether the driver has been told the address of the tag's own
    /// buffer: after the tag's first commit.
    ours: Vec<bool>,
    /// What was locked: unlocked when the queue is dropped.
    _locked: Vec<Locked>,
    stopper: Stopper,
    /// Tags of the last wait, kept to spare an allocation.
    tags: Vec<u16>,
}

impl<'q> KernelQueue<'q> {
    fn new(dev: &'q UblkDev, stopper: Stopper, require_locked: bool) -> io::Result<Self> {
        let first = dev.alloc_queue_io_bufs();
        let depth = first.len();
        let mut own = Vec::with_capacity(depth);
        let mut locked = Vec::with_capacity(depth);
        for _ in 0..depth {
            let buf = Buf::with_capacity(MAX_BLOCKS);
            match mem::lock(&buf) {
                Ok(lock) => locked.push(lock),
                Err(e) if require_locked => {
                    return Err(io::Error::new(
                        e.kind(),
                        format!("locking a request buffer in memory: {e}"),
                    ))
                }
                Err(_) => {}
            }
            own.push(Some(buf));
        }
        let queue = UblkQueue::new(0, dev)
            .map_err(io_error)?
            .submit_fetch_commands_unified(BufDescList::Slices(Some(&first)))
            .map_err(io_error)?;
        let this = Self {
            queue,
            first,
            own,
            ours: vec![false; depth],
            _locked: locked,
            stopper,
            tags: Vec::with_capacity(depth),
        };
        this.arm()?;
        Ok(this)
    }

    /// Put a poll of the wake descriptor on the ring: it completes
    /// when the descriptor is next written. A poll names no memory.
    fn arm(&self) -> io::Result<()> {
        let poll = opcode::PollAdd::new(types::Fd(self.stopper.wake.fd()), libc::POLLIN as u32)
            .build()
            .user_data(WAKE);
        self.queue.ublk_submit_sqe_sync(poll).map_err(io_error)
    }

    fn is_write(&self, tag: u16) -> bool {
        self.queue.get_iod(tag).op_flags & 0xff == sys::UBLK_IO_OP_WRITE
    }
}

impl Queue for KernelQueue<'_> {
    fn depth(&self) -> u16 {
        self.own.len() as u16
    }

    fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Woken, QueueDown> {
        let mut woken = false;
        let tags = &mut self.tags;
        tags.clear();
        // With nothing arriving this returns `Ok(0)` after the
        // library's idle time-out (20 s): a turn with nothing to do,
        // which is what lets the pump try again what it could not
        // store.
        let waited = self.queue.flush_and_wake_io_tasks(
            |user_data, done, _last| {
                if user_data == WAKE {
                    woken = true;
                } else if user_data & (1 << 63) == 0 && done.result() == sys::UBLK_IO_RES_OK as i32
                {
                    tags.push((user_data & 0xffff) as u16);
                }
            },
            1,
        );
        match waited {
            Ok(_) => {}
            Err(UblkError::QueueIsDown) => return Err(QueueDown),
            // An interrupted wait is a turn with nothing.
            Err(UblkError::IOError(e)) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(UblkError::UringIOError(n)) if n.abs() == libc::EINTR => {}
            // Anything else: the ring cannot be waited on, so nothing
            // can be fetched or committed any more.
            Err(_) => return Err(QueueDown),
        }
        for &tag in self.tags.iter() {
            let iod = self.queue.get_iod(tag);
            fetched.push(Fetched {
                tag,
                desc: Desc {
                    op_flags: iod.op_flags,
                    nr_sectors: iod.nr_sectors,
                    start_sector: iod.start_sector,
                },
            });
        }
        if woken {
            self.stopper.wake.drain();
            if self.arm().is_err() {
                return Err(QueueDown);
            }
        }
        if self.stopper.stop.load(Ordering::SeqCst) {
            Ok(Woken::Stop)
        } else {
            Ok(Woken::Work)
        }
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Buf {
        let slot = usize::from(tag);
        // The pump takes a tag's buffer once per request, and the
        // driver was told the largest request: neither fallback below
        // is reached while both keep their word. Each still gives a
        // buffer of the right length with the right data, at the cost
        // of an allocation and, at the commit, a new address.
        let mut buf = match self.own.get_mut(slot).and_then(Option::take) {
            Some(buf) => buf,
            None => Buf::with_capacity(MAX_BLOCKS.max(blocks)),
        };
        if buf.set_blocks(blocks).is_err() {
            buf = Buf::with_capacity(blocks);
        }
        let ours = self.ours.get(slot).copied().unwrap_or(false);
        if !ours && self.is_write(tag) {
            // The tag's first request since the start: the driver put
            // the data into the library's buffer. No fetch of this tag
            // is outstanding, so the kernel does not write there now.
            if let Some(first) = self.first.get(slot) {
                let from = first.as_slice();
                let len = buf.len().min(from.len());
                buf[..len].copy_from_slice(&from[..len]);
                self.stopper.copies.fetch_add(1, Ordering::Relaxed);
            }
        }
        buf
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        let slot = usize::from(tag);
        let Some(place) = self.own.get_mut(slot) else {
            return;
        };
        if let Some(buf) = buf {
            *place = Some(buf);
        }
        // A request whose buffer never came back (the pump commits a
        // failed one without it) gets a new one: unlocked, and never
        // seen in a run. The driver is told whatever address is here.
        let own = place.get_or_insert_with(|| Buf::with_capacity(MAX_BLOCKS));
        // The whole buffer, so that the slice is not empty: the library
        // gives the driver the address of a slice that has a length,
        // and none otherwise.
        if own.set_blocks(own.capacity_blocks()).is_err() || own.is_empty() {
            *own = Buf::with_capacity(MAX_BLOCKS);
        }
        // The borrow ends with this call. After it the kernel may write
        // to the buffer at any time until the tag's next fetch
        // completes, and nothing here touches it until then: see the
        // module documentation.
        let told = self.queue.complete_io_cmd_unified(
            tag,
            BufDesc::Slice(&own[..]),
            Ok(UblkIORes::Result(result)),
        );
        if told.is_ok() {
            if let Some(ours) = self.ours.get_mut(slot) {
                *ours = true;
            }
        }
    }

    fn pause(&mut self) {
        std::thread::sleep(PAUSE);
    }
}
