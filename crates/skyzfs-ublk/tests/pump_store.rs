//! The pump over the real store behind its worker thread, with a queue
//! of the test's own: the pair that the binding will put together.
//! What the fakes in the pump's unit tests stand in for is here for
//! real: the store's completions, its acknowledged position, and the
//! record of it on a file.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use skyzfs_blk::{Base, Buf};
use skyzfs_local::format::{Identity, Lsn};
use skyzfs_local::fs::FsDir;
use skyzfs_local::store::{OpenOptions, Served, Store, StoreConfig, StoreError};
use skyzfs_ublk::ack::{self, AckFile};
use skyzfs_ublk::pump::{AckRecord, Fetched, Pump, Queue, QueueDown, Stopped, Wake};
use skyzfs_ublk::translate::{op, Desc};

const BOOT: [u8; 16] = [7; 16];

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

    fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown> {
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

    fn flush(&mut self) -> Result<(), QueueDown> {
        Ok(())
    }

    fn pause(&mut self) {}
}

/// The record on a file, noting each position as it is stored.
struct Noting {
    file: AckFile,
    stored: Vec<u64>,
}

impl AckRecord for Noting {
    fn advance(&mut self, position: u64) -> std::io::Result<()> {
        self.file.advance(position)?;
        self.stored.push(position);
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
fn the_pump_serves_a_real_store_and_what_it_stored_is_what_the_next_open_must_reach() {
    let dir = tempfile::tempdir().expect("a directory");
    let volume = dir.path().join("volume");
    std::fs::create_dir(&volume).expect("the volume's directory");
    let config = StoreConfig {
        volume: *b"a volume on disk",
        size_mib: 8,
        base: Base::Empty,
        align: 4096,
        segment_size: 8 << 20,
        write_unit: Identity::system_write_unit(),
    };
    let store = Store::create(
        FsDir::new(&volume).expect("the directory"),
        &config,
        OpenOptions::default(),
    )
    .expect("create");
    let identity = store.identity();
    let served = store.serve().expect("serve");

    let ack_path = dir.path().join("acknowledged");
    let record = Noting {
        file: AckFile::open(&ack_path, identity, BOOT).expect("the record"),
        stored: Vec::new(),
    };
    let mut queue = ScriptQueue::default();
    queue.script.push_back(Step::Fetch(vec![
        request(0, op::WRITE, 0, 1),
        request(1, op::WRITE, 1, 2),
        request(2, op::FLUSH, 0, 0),
    ]));
    let mut pump: Pump<ScriptQueue, Served, Noting> = Pump::new(queue, served, record);
    for _ in 0..10_000 {
        if pump.queue().committed.len() == 3 {
            break;
        }
        assert_eq!(pump.turn(), None);
    }
    let mut results = pump.queue().committed.clone();
    results.sort_unstable();
    assert_eq!(results, [(0, 4096), (1, 8192), (2, 0)]);

    // What the pump stored is what the store says it acknowledged, and
    // it is on the file.
    let acknowledged = pump.target().acknowledged().expect("two writes");
    assert_eq!(pump.ack().stored.last().copied(), Some(acknowledged.0));
    let on_file = ack::read(&ack_path).expect("read").expect("a record");
    assert_eq!(on_file.position_for(&identity, &BOOT), Some(acknowledged.0));

    // The data comes back through the pump, in the queue's buffers.
    pump.queue().script.push_back(Step::Fetch(vec![
        request(3, op::READ, 0, 1),
        request(4, op::READ, 1, 2),
        request(5, op::READ, 3, 1),
    ]));
    pump.queue().script.push_back(Step::Stop);
    assert_eq!(pump.run(), Stopped::Orderly);
    let returned = &pump.queue().returned;
    assert!(returned[&3].iter().all(|b| *b == 0x40));
    assert!(returned[&4].iter().all(|b| *b == 0x41));
    assert!(returned[&5].iter().all(|b| *b == 0), "never written");
    assert_eq!(pump.counters().in_flight, 0);
    drop(pump);

    // The next open must reach the stored position, and does. One that
    // is told a position beyond the log is refused.
    let reopen = |position: u64| {
        let mut options = OpenOptions::default();
        options.must_reach = Some(Lsn(position));
        options.must_reach_identity = Some(identity);
        Store::open(FsDir::new(&volume).expect("the directory"), options).map(|_| ())
    };
    reopen(acknowledged.0).expect("the log reaches what was acknowledged");
    assert!(matches!(
        reopen(acknowledged.0 + (1 << 30)),
        Err(StoreError::Log(_))
    ));
}
