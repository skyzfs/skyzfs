//! The test build's refused commit: `serve --refuse-one-commit`, as a
//! queue around the kernel's queue (next to the kernel, below the
//! request log of [`crate::record`]).
//!
//! The first request that is handed over and is of the kind asked for
//! (a read or a write that covers a given byte, or a flush) has its
//! commit sent with no buffer: the driver is told the address 0. A
//! queue of this daemon asks for none of the features under which a
//! commit may name no buffer, so the driver refuses that commit when it
//! is submitted, before it changes anything (`-EINVAL`), as it does for
//! the control rig's `--refuse-one-commit` (#80). The request stays the
//! server's, unanswered; the pump sees the refusal at its next wait and
//! ends ([`Stopped::CommitRefused`](skyzfs_ublk::pump::Stopped)), and
//! serve exits with status 8, leaving the device for the next server.
//! The kernel's side, that the next serve is handed that request again
//! and completes it, is the daemon rig's to show.
//!
//! Only that one commit is sent so; every other call passes through.
//! The buffer the pump gave back with it is kept until the queue is
//! dropped: the driver was told no address, and the request is still
//! the server's. A commit that the library will not put on the ring is
//! made as any other, so that nothing waits on it. The hook does not
//! look at a failure of the volume: given with one, the refused request
//! is answered by the delete of the failed volume's device.
//!
//! The request log, above this queue, writes the commit as any other
//! `commit` line; its refusal follows as an `ended` line for the tag.

use std::io;

use skyzfs_blk::Buf;
use skyzfs_ublk::pump::{Fetched, Queue, QueueDown, Unfetched, Wake};
use skyzfs_ublk_sys::queue::KernelQueue;

use crate::cli::Held;
use crate::delay::is_held;

/// A queue that can commit a request with no buffer.
pub trait NoBuffer: Queue {
    /// Commit `tag` with `result` and no buffer. An error says that
    /// nothing was sent.
    fn commit_with_no_buffer(&mut self, tag: u16, result: i32) -> io::Result<()>;
}

impl NoBuffer for KernelQueue<'_> {
    fn commit_with_no_buffer(&mut self, tag: u16, result: i32) -> io::Result<()> {
        self.commit_with_no_buffer_for_tests(tag, result)
    }
}

/// A queue that sends one request's commit with no buffer, if it was
/// told which: see the [module documentation](self).
pub struct Refusing<Q: NoBuffer> {
    inner: Q,
    /// The request whose commit is sent with no buffer, until it has
    /// been handed over.
    what: Option<Held>,
    /// Its tag, from its hand-over to its commit.
    tag: Option<u16>,
    /// The buffers of the commits sent with no buffer.
    kept: Vec<Option<Buf>>,
}

impl<Q: NoBuffer> Refusing<Q> {
    /// `inner`, with the commit of the first request that `what` names
    /// sent with no buffer, if there is a `what`.
    pub fn new(inner: Q, what: Option<Held>) -> Self {
        Self {
            inner,
            what,
            tag: None,
            kept: Vec::new(),
        }
    }
}

impl<Q: NoBuffer> Queue for Refusing<Q> {
    fn depth(&self) -> u16 {
        self.inner.depth()
    }

    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown> {
        let before = fetched.len();
        let woken = self.inner.wait(fetched, unfetched);
        if let Some(what) = self.what {
            let new = fetched.get(before..).unwrap_or_default();
            if let Some(first) = new.iter().find(|f| is_held(what, &f.desc)) {
                self.tag = Some(first.tag);
                self.what = None;
            }
        }
        woken
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        self.inner.take_buf(tag, blocks)
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        if self.tag == Some(tag) {
            self.tag = None;
            if self.inner.commit_with_no_buffer(tag, result).is_ok() {
                self.kept.push(buf);
                return;
            }
            // Nothing was sent: the request is answered as any other.
        }
        self.inner.commit(tag, result, buf);
    }

    fn flush(&mut self) -> Result<(), QueueDown> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use skyzfs_ublk::translate::{op, Desc};

    use super::*;

    /// A queue that hands over what its script says and notes its
    /// commits, with a buffer or with none.
    #[derive(Default)]
    struct Noting {
        turns: VecDeque<Vec<Fetched>>,
        committed: Vec<(u16, i32, bool)>,
        no_buffer: Vec<(u16, i32)>,
        /// The library will not send a commit with no buffer.
        not_sent: bool,
        flushes: u32,
    }

    impl Queue for Noting {
        fn depth(&self) -> u16 {
            8
        }
        fn wait(
            &mut self,
            fetched: &mut Vec<Fetched>,
            _unfetched: &mut Vec<Unfetched>,
        ) -> Result<Wake, QueueDown> {
            fetched.extend(self.turns.pop_front().unwrap_or_default());
            Ok(Wake::Work)
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<Buf> {
            Some(Buf::zeroed(blocks))
        }
        fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
            self.committed.push((tag, result, buf.is_some()));
        }
        fn flush(&mut self) -> Result<(), QueueDown> {
            self.flushes += 1;
            Ok(())
        }
    }

    impl NoBuffer for Noting {
        fn commit_with_no_buffer(&mut self, tag: u16, result: i32) -> io::Result<()> {
            if self.not_sent {
                return Err(io::Error::other("not put on the ring"));
            }
            self.no_buffer.push((tag, result));
            Ok(())
        }
    }

    fn request(tag: u16, code: u8, byte: u64) -> Fetched {
        Fetched {
            tag,
            desc: Desc {
                op_flags: u32::from(code),
                nr_sectors: if code == op::FLUSH { 0 } else { 8 },
                start_sector: byte / 512,
            },
        }
    }

    fn wait(queue: &mut Refusing<Noting>) -> Vec<Fetched> {
        let mut fetched = Vec::new();
        queue.wait(&mut fetched, &mut Vec::new()).expect("a wait");
        fetched
    }

    /// The first request of the kind named has its commit sent with no
    /// buffer, once, and its buffer is kept; the others, before and
    /// after it, are committed as they come. A later request of the
    /// same kind on the same tag is committed as any other.
    #[test]
    fn the_request_named_has_its_commit_sent_once_with_no_buffer() {
        let inner = Noting {
            turns: [
                vec![
                    request(1, op::WRITE, 0),
                    request(2, op::WRITE, 8192),
                    request(3, op::READ, 8192),
                ],
                vec![request(2, op::WRITE, 8192)],
            ]
            .into(),
            ..Noting::default()
        };
        let mut queue = Refusing::new(inner, Some(Held::Write(8192)));
        assert_eq!(wait(&mut queue).len(), 3);
        queue.commit(1, 4096, Some(Buf::zeroed(1)));
        queue.commit(2, 4096, Some(Buf::zeroed(1)));
        queue.commit(3, 4096, Some(Buf::zeroed(1)));
        assert_eq!(queue.inner.no_buffer, [(2, 4096)]);
        assert_eq!(queue.inner.committed, [(1, 4096, true), (3, 4096, true)]);
        assert_eq!(queue.kept.len(), 1, "the refused commit's buffer is kept");
        assert_eq!(wait(&mut queue).len(), 1);
        queue.commit(2, 4096, Some(Buf::zeroed(1)));
        assert_eq!(queue.inner.no_buffer.len(), 1, "only once");
        assert_eq!(queue.inner.committed.last(), Some(&(2, 4096, true)));
        assert!(queue.flush().is_ok());
        assert_eq!(queue.inner.flushes, 1);

        // A flush, named so.
        let inner = Noting {
            turns: [vec![request(0, op::READ, 0), request(5, op::FLUSH, 0)]].into(),
            ..Noting::default()
        };
        let mut queue = Refusing::new(inner, Some(Held::Flush));
        wait(&mut queue);
        queue.commit(5, 0, None);
        queue.commit(0, 4096, Some(Buf::zeroed(1)));
        assert_eq!(queue.inner.no_buffer, [(5, 0)]);
        assert_eq!(queue.inner.committed, [(0, 4096, true)]);
    }

    /// A commit with no buffer that the library will not send is made
    /// as any other, so that nothing waits on it.
    #[test]
    fn a_commit_the_library_will_not_send_is_made_as_any_other() {
        let inner = Noting {
            turns: [vec![request(4, op::READ, 4096)]].into(),
            not_sent: true,
            ..Noting::default()
        };
        let mut queue = Refusing::new(inner, Some(Held::Read(4096)));
        wait(&mut queue);
        queue.commit(4, 4096, Some(Buf::zeroed(1)));
        assert!(queue.inner.no_buffer.is_empty());
        assert_eq!(queue.inner.committed, [(4, 4096, true)]);
        assert!(queue.kept.is_empty());
    }

    /// Without a request named, every call passes through.
    #[test]
    fn without_a_request_named_every_commit_passes_through() {
        let inner = Noting {
            turns: [vec![request(1, op::WRITE, 0), request(2, op::FLUSH, 0)]].into(),
            ..Noting::default()
        };
        let mut queue = Refusing::new(inner, None);
        assert_eq!(queue.depth(), 8);
        wait(&mut queue);
        queue.commit(1, 4096, Some(Buf::zeroed(1)));
        queue.commit(2, 0, None);
        assert!(queue.inner.no_buffer.is_empty());
        assert_eq!(queue.inner.committed, [(1, 4096, true), (2, 0, false)]);
    }
}
