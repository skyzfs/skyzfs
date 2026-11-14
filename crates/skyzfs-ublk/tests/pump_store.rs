//! The pump over the in-place store on the simulated disk, with a queue
//! of the test's own: the pair that the binding puts together. What the
//! fakes in the pump's unit tests stand in for is here for real: the
//! store's completions from its I/O threads, and its flushes on the
//! file's helper. What the log store's version held of the next open,
//! that it reaches what the pump stored as acknowledged (a record
//! removed in M1.3t-b with the log store's last plumbing), is here what
//! the in-place store promises instead: after a power cut, the next
//! start finds what the pump's flush covered.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use skyzfs_blk::{Base, Buf};
use skyzfs_local::inplace::{create, lock, InPlace, OpenOptions, Opened, VolumeConfig};
use skyzfs_local::sim::{LoseAll, SimDisk};
use skyzfs_ublk::pump::{Fetched, Pump, Queue, QueueDown, Stopped, Unfetched, Wake};
use skyzfs_ublk::translate::{op, Desc};

enum Step {
    Fetch(Vec<Fetched>),
    Stop,
}

/// A queue that hands over what its script says, and otherwise waits a
/// millisecond: it has no waker.
#[derive(Default)]
struct ScriptQueue {
    script: VecDeque<Step>,
    committed: Vec<(u16, i32)>,
    returned: HashMap<u16, Buf>,
}

impl Queue for ScriptQueue {
    fn depth(&self) -> u16 {
        16
    }

    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        _unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown> {
        match self.script.pop_front() {
            Some(Step::Fetch(more)) => {
                fetched.extend(more);
                Ok(Wake::Work)
            }
            Some(Step::Stop) => Ok(Wake::Stop),
            None => {
                std::thread::sleep(Duration::from_millis(1));
                Ok(Wake::Work)
            }
        }
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        Some(Buf::filled(blocks, 0x40 + tag as u8))
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        self.committed.push((tag, result));
        if let Some(buf) = buf {
            self.returned.insert(tag, buf);
        }
    }

    fn flush(&mut self, _unfetched: &mut Vec<Unfetched>) -> Result<(), QueueDown> {
        Ok(())
    }
}

fn request(tag: u16, operation: u8, block: u64, blocks: u32) -> Fetched {
    Fetched {
        tag,
        desc: Desc {
            op_flags: u32::from(operation),
            nr_sectors: blocks * 8,
            start_sector: block * 8,
        },
    }
}

#[test]
fn the_pump_serves_the_in_place_store() {
    let disk = SimDisk::new(1);
    let config = VolumeConfig {
        volume: *b"a volume on disk",
        size_mib: 8,
        base: Base::Empty,
        write_unit: 4096,
    };
    create(&disk, &config).expect("create");
    let opened = Opened::open(
        disk.clone(),
        lock(&disk).expect("lock"),
        OpenOptions::default(),
    )
    .map_err(|(e, _)| e)
    .expect("open");
    let store = opened.start(None).expect("start");

    let mut queue = ScriptQueue::default();
    queue.script.push_back(Step::Fetch(vec![
        request(0, op::WRITE, 0, 1),
        request(1, op::WRITE, 1, 2),
        request(2, op::FLUSH, 0, 0),
    ]));
    let mut pump: Pump<ScriptQueue, InPlace<SimDisk>> = Pump::new(queue, store);
    for _ in 0..10_000 {
        if pump.queue().committed.len() == 3 {
            break;
        }
        assert_eq!(pump.turn(), None);
    }
    let mut results = pump.queue().committed.clone();
    results.sort_unstable();
    assert_eq!(results, [(0, 4096), (1, 8192), (2, 0)]);

    // The pump's own times: three requests submitted and committed,
    // none of them a read; the times of a record that is no more stay
    // zero.
    let timing = pump.timing();
    assert_eq!(timing.to_submit.count, 3, "{timing:?}");
    assert_eq!(timing.other_to_commit.count, 3, "{timing:?}");
    assert_eq!(timing.read_to_commit.count, 0, "{timing:?}");
    assert_eq!(timing.store.count, 0, "{timing:?}");
    assert_eq!(timing.behind_store, 0, "{timing:?}");
    for spent in [timing.to_submit, timing.other_to_commit] {
        assert!(spent.max_ns <= spent.sum_ns, "{timing:?}");
    }

    // The data comes back through the pump, in the queue's buffers. A
    // write that no flush covers goes along.
    pump.queue().script.push_back(Step::Fetch(vec![
        request(3, op::READ, 0, 1),
        request(4, op::READ, 1, 2),
        request(5, op::READ, 3, 1),
        request(6, op::WRITE, 5, 1),
    ]));
    pump.queue().script.push_back(Step::Stop);
    assert_eq!(pump.run(), Stopped::Orderly);
    let after = pump.timing();
    assert_eq!(after.to_submit.count, 7, "{after:?}");
    assert_eq!(after.read_to_commit.count, 3, "{after:?}");
    assert_eq!(after.other_to_commit.count, 4, "{after:?}");
    let returned = &pump.queue().returned;
    assert!(returned[&3].iter().all(|b| *b == 0x40));
    assert!(returned[&4].iter().all(|b| *b == 0x41));
    assert!(returned[&5].iter().all(|b| *b == 0), "never written");
    assert_eq!(pump.counters().in_flight, 0);
    assert_eq!(pump.counters().unstored, 0);

    // The power is cut after the stop: the next start finds what the
    // pump's flush covered. The write after it is lost, as the cut
    // keeps nothing that no sync covered: the cut is a real one.
    drop(pump.into_target());
    disk.power_cut(LoseAll);
    let opened = Opened::open(
        disk.clone(),
        lock(&disk).expect("lock"),
        OpenOptions::default(),
    )
    .map_err(|(e, _)| e)
    .expect("open again");
    let store = opened.start(None).expect("start again");
    let mut queue = ScriptQueue::default();
    queue.script.push_back(Step::Fetch(vec![
        request(7, op::READ, 0, 1),
        request(8, op::READ, 1, 2),
        request(9, op::READ, 5, 1),
    ]));
    queue.script.push_back(Step::Stop);
    let mut pump: Pump<ScriptQueue, InPlace<SimDisk>> = Pump::new(queue, store);
    assert_eq!(pump.run(), Stopped::Orderly);
    let returned = &pump.queue().returned;
    assert!(returned[&7].iter().all(|b| *b == 0x40), "flushed");
    assert!(returned[&8].iter().all(|b| *b == 0x41), "flushed");
    assert!(returned[&9].iter().all(|b| *b == 0), "not flushed, and cut");
}
