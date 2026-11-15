use std::collections::VecDeque;

use super::*;

/// The volume's size in the tests: 1 GiB.
const SIZE: u64 = 1 << 30;

/// A queue that notes its commits and answers its waits from a script.
#[derive(Default)]
struct Scripted {
    committed: Vec<(u16, i32)>,
    flushes: u32,
    waits: VecDeque<Result<Vec<Fetched>, QueueDown>>,
    /// What each wait hands on as ended otherwise than OK, whatever it
    /// returns.
    ends: VecDeque<Vec<Unfetched>>,
    flush_fails: bool,
    /// What the flush hands on as ended otherwise than OK.
    flush_ends: Vec<Unfetched>,
    /// A record to read as each wait or flush begins: its last line then
    /// goes to `last_before`.
    peek: Option<std::path::PathBuf>,
    last_before: Vec<String>,
}

impl Scripted {
    fn note_last_line(&mut self) {
        if let Some(path) = &self.peek {
            let text = std::fs::read_to_string(path).expect("the record");
            let last = text.lines().last().unwrap_or_default();
            let (_time, rest) = last.split_once('\t').expect("a time");
            self.last_before.push(rest.to_string());
        }
    }
}

impl Queue for Scripted {
    fn depth(&self) -> u16 {
        4
    }
    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown> {
        self.note_last_line();
        unfetched.extend(self.ends.pop_front().unwrap_or_default());
        match self.waits.pop_front() {
            Some(Ok(more)) => {
                fetched.extend(more);
                Ok(Wake::Work)
            }
            Some(Err(down)) => Err(down),
            None => Ok(Wake::Work),
        }
    }
    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        let mut buf = Buf::zeroed(blocks);
        fill(&mut buf, tag);
        Some(buf)
    }
    fn commit(&mut self, tag: u16, result: i32, _buf: Option<Buf>) {
        self.committed.push((tag, result));
    }
    fn flush(&mut self, unfetched: &mut Vec<Unfetched>) -> Result<(), QueueDown> {
        self.note_last_line();
        self.flushes += 1;
        unfetched.append(&mut self.flush_ends);
        if self.flush_fails {
            Err(QueueDown)
        } else {
            Ok(())
        }
    }
}

/// What the scripted queue's buffer for `tag` holds.
fn fill(buf: &mut [u8], tag: u16) {
    for (i, byte) in buf.iter_mut().enumerate() {
        *byte = (i as u8).wrapping_mul(7) ^ (tag as u8);
    }
}

fn fetched(tag: u16, code: u8, start_sector: u64, nr_sectors: u32) -> Fetched {
    Fetched {
        tag,
        desc: Desc {
            op_flags: u32::from(code),
            nr_sectors,
            start_sector,
        },
    }
}

/// The lines of the record without their time, which is checked apart.
fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("the record")
        .lines()
        .map(|line| {
            let (time, rest) = line.split_once('\t').expect("a time");
            assert!(time.parse::<u128>().is_ok(), "a time in ns: {line}");
            rest.to_string()
        })
        .collect()
}

fn pid() -> u32 {
    std::process::id()
}

/// What the kernel handed over and what it was given back, each in the
/// order it happened: the start, the requests as the wait returned them,
/// each commit with its request again, a `send` line as a wait or a
/// flush begins with commits pending and a `sent` line when it returns.
/// The queue underneath sees every call.
#[test]
fn the_record_has_each_request_and_its_answer() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![
        fetched(0, op::WRITE, 131_016, 24),
        fetched(1, op::WRITE, 131_040, 32),
        fetched(2, op::FLUSH, 0, 0),
    ]));
    inner.waits.push_back(Ok(vec![fetched(0, op::READ, 8, 16)]));
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 7, SIZE);
    let mut got = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Ok(Wake::Work));
    assert_eq!(got.len(), 3);
    queue.commit(1, 16384, None);
    queue.commit(0, 12288, None);
    queue.commit(2, 0, None);
    got.clear();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Ok(Wake::Work));
    queue.commit(0, -5, None);
    assert_eq!(queue.flush(&mut Vec::new()), Ok(()));
    let p = pid();
    assert_eq!(
        lines(&path),
        [
            format!("{p}\tstart\t7\t-\t-\t-\t4"),
            format!("{p}\trecv\t0\twrite\t131016\t24\t-"),
            format!("{p}\trecv\t1\twrite\t131040\t32\t-"),
            format!("{p}\trecv\t2\tflush\t0\t0\t-"),
            format!("{p}\tcommit\t1\twrite\t131040\t32\t16384"),
            format!("{p}\tcommit\t0\twrite\t131016\t24\t12288"),
            format!("{p}\tcommit\t2\tflush\t0\t0\t0"),
            format!("{p}\tsend\t-\t-\t-\t-\t3"),
            format!("{p}\tsent\t-\t-\t-\t-\t3"),
            format!("{p}\trecv\t0\tread\t8\t16\t-"),
            format!("{p}\tcommit\t0\tread\t8\t16\t-5"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tsent\t-\t-\t-\t-\t1"),
        ]
    );
    assert_eq!(
        queue.inner.committed,
        [(1, 16384), (0, 12288), (2, 0), (0, -5)]
    );
    assert_eq!(queue.inner.flushes, 1);
}

/// The `send` line is in the file before the queue underneath is asked
/// to wait or flush, since that call hands the commits to the kernel at
/// its start; the `sent` line only after it returns. A kill inside the
/// call leaves the first and not the second.
#[test]
fn the_send_line_is_written_before_the_commits_go() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let mut inner = Scripted {
        peek: Some(path.clone()),
        ..Scripted::default()
    };
    inner.waits.push_back(Ok(vec![fetched(0, op::WRITE, 0, 8)]));
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 0, SIZE);
    let mut got = Vec::new();
    let _ = queue.wait(&mut got, &mut Vec::new());
    queue.commit(0, 4096, None);
    let _ = queue.wait(&mut got, &mut Vec::new());
    queue.commit(0, 4096, None);
    let _ = queue.flush(&mut Vec::new());
    let p = pid();
    assert_eq!(
        queue.inner.last_before,
        [
            // Nothing pending at the first wait: no `send`.
            format!("{p}\tstart\t0\t-\t-\t-\t4"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
        ]
    );
}

/// The times are `CLOCK_MONOTONIC` and do not go back from one line to
/// the next.
#[test]
fn the_times_are_monotonic() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let mut inner = Scripted::default();
    inner
        .waits
        .push_back(Ok(vec![fetched(3, op::DISCARD, 0, 8)]));
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 0, SIZE);
    let before = mono_ns();
    let mut got = Vec::new();
    let _ = queue.wait(&mut got, &mut Vec::new());
    queue.commit(3, 0, None);
    let _ = queue.flush(&mut Vec::new());
    let after = mono_ns();
    let times: Vec<u128> = std::fs::read_to_string(&path)
        .expect("the record")
        .lines()
        .map(|line| {
            line.split('\t')
                .next()
                .and_then(|t| t.parse().ok())
                .expect("a time")
        })
        .collect();
    // start, recv, commit, send, sent.
    assert_eq!(times.len(), 5);
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    assert!(times[1] >= before && times[4] <= after, "{times:?}");
}

/// Every operation code has a name, and one outside the five says its
/// number.
#[test]
fn every_operation_is_named() {
    assert_eq!(op_name(op::READ), "read");
    assert_eq!(op_name(op::WRITE), "write");
    assert_eq!(op_name(op::FLUSH), "flush");
    assert_eq!(op_name(op::DISCARD), "discard");
    assert_eq!(op_name(op::WRITE_ZEROES), "write_zeroes");
    assert_eq!(op_name(op::WRITE_SAME), "op4");
    assert_eq!(op_name(13), "op13");
}

/// A wait or a flush that fails says that the commits it was to send may
/// not have reached the kernel; a commit for a tag that holds no request
/// (the pump answers a refused request with no fetch recorded only in
/// a test) still has its line.
#[test]
fn a_failed_wait_or_flush_says_the_commits_may_not_have_gone() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let mut inner = Scripted::default();
    inner.waits.push_back(Err(QueueDown));
    inner.flush_fails = true;
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 1, SIZE);
    queue.commit(2, -5, None);
    let mut got = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Err(QueueDown));
    queue.commit(3, -5, None);
    assert_eq!(queue.flush(&mut Vec::new()), Err(QueueDown));
    // Nothing committed since: no line.
    assert_eq!(queue.flush(&mut Vec::new()), Err(QueueDown));
    let p = pid();
    assert_eq!(
        lines(&path),
        [
            format!("{p}\tstart\t1\t-\t-\t-\t4"),
            format!("{p}\tcommit\t2\t-\t-\t-\t-5"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tdown\t-\t-\t-\t-\t1"),
            format!("{p}\tcommit\t3\t-\t-\t-\t-5"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tdown\t-\t-\t-\t-\t1"),
        ]
    );
}

/// The instances of one volume append to one file: what the first wrote
/// stays, and the second's lines follow it whole.
#[test]
fn a_second_instance_appends_to_the_same_record() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![fetched(0, op::WRITE, 0, 8)]));
    let mut first = Recorded::new(inner, Some(open(&path).expect("open")), 0, SIZE);
    let mut got = Vec::new();
    let _ = first.wait(&mut got, &mut Vec::new());
    drop(first);
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![fetched(3, op::WRITE, 0, 8)]));
    let mut second = Recorded::new(inner, Some(open(&path).expect("open")), 0, SIZE);
    got.clear();
    let _ = second.wait(&mut got, &mut Vec::new());
    second.commit(3, 4096, None);
    let _ = second.flush(&mut Vec::new());
    let p = pid();
    assert_eq!(
        lines(&path),
        [
            format!("{p}\tstart\t0\t-\t-\t-\t4"),
            format!("{p}\trecv\t0\twrite\t0\t8\t-"),
            format!("{p}\tstart\t0\t-\t-\t-\t4"),
            format!("{p}\trecv\t3\twrite\t0\t8\t-"),
            format!("{p}\tcommit\t3\twrite\t0\t8\t4096"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tsent\t-\t-\t-\t-\t1"),
        ]
    );
}

/// Without a file the queue is the queue underneath: every call passes
/// through and nothing is written.
#[test]
fn without_a_file_every_call_passes_through() {
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![fetched(1, op::READ, 0, 8)]));
    let mut queue = Recorded::new(inner, None, 0, SIZE);
    let mut got = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Ok(Wake::Work));
    assert_eq!(got, [fetched(1, op::READ, 0, 8)]);
    assert!(queue.take_buf(1, 2).is_some_and(|buf| buf.len() == 8192));
    queue.commit(1, 4096, None);
    assert_eq!(queue.flush(&mut Vec::new()), Ok(()));
    assert_eq!(queue.inner.committed, [(1, 4096)]);
    assert_eq!(queue.depth(), 4);
}

/// A path that cannot be opened is an error at `open`, before the
/// serve touches a device.
#[test]
fn a_record_that_cannot_be_opened_is_an_error() {
    let dir = tempfile::tempdir().expect("dir");
    assert!(open(&dir.path().join("no-such-dir").join("requests.tsv")).is_err());
}

/// A command the driver ended otherwise than OK has an `ended` line
/// when the wait that returned it ends, before that wait's requests:
/// its tag, the command, the result. Also when the wait fails, and at
/// the last flush (a commit refused when the flush sent it), and the
/// pump is handed every one of them.
#[test]
fn a_command_the_driver_ended_otherwise_than_ok_has_its_line() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![fetched(0, op::READ, 8, 16)]));
    inner.ends.push_back(vec![Unfetched {
        tag: 1,
        command: Command::Commit,
        result: -22,
    }]);
    inner.waits.push_back(Err(QueueDown));
    inner.ends.push_back(vec![
        Unfetched {
            tag: 0,
            command: Command::Commit,
            result: -19,
        },
        Unfetched {
            tag: 2,
            command: Command::Fetch,
            result: -19,
        },
        Unfetched {
            tag: 3,
            command: Command::Other(0x23),
            result: 1,
        },
    ]);
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 7, SIZE);
    queue.commit(1, 4096, None);
    let mut got = Vec::new();
    let mut ended = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut ended), Ok(Wake::Work));
    assert_eq!(ended.len(), 1);
    queue.commit(0, 8192, None);
    assert_eq!(queue.wait(&mut got, &mut ended), Err(QueueDown));
    assert_eq!(ended.len(), 4);
    queue.inner.flush_ends = vec![Unfetched {
        tag: 2,
        command: Command::Commit,
        result: -22,
    }];
    queue.commit(2, 0, None);
    let mut at_flush = Vec::new();
    assert_eq!(queue.flush(&mut at_flush), Ok(()));
    assert_eq!(
        at_flush,
        [Unfetched {
            tag: 2,
            command: Command::Commit,
            result: -22,
        }]
    );
    let p = pid();
    assert_eq!(
        lines(&path),
        [
            format!("{p}\tstart\t7\t-\t-\t-\t4"),
            format!("{p}\tcommit\t1\t-\t-\t-\t4096"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tsent\t-\t-\t-\t-\t1"),
            format!("{p}\tended\t1\tcommit\t-\t-\t-22"),
            format!("{p}\trecv\t0\tread\t8\t16\t-"),
            format!("{p}\tcommit\t0\tread\t8\t16\t8192"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tdown\t-\t-\t-\t-\t1"),
            format!("{p}\tended\t0\tcommit\t-\t-\t-19"),
            format!("{p}\tended\t2\tfetch\t-\t-\t-19"),
            format!("{p}\tended\t3\tcmd35\t-\t-\t1"),
            format!("{p}\tcommit\t2\t-\t-\t-\t0"),
            format!("{p}\tsend\t-\t-\t-\t-\t1"),
            format!("{p}\tsent\t-\t-\t-\t-\t1"),
            format!("{p}\tended\t2\tcommit\t-\t-\t-22"),
        ]
    );
}

/// A write that touches either end of the volume has a `label` line
/// with all its bytes in hex, as the queue hands its buffer over: the
/// first 512 KiB, and the last 512 KiB of the labels' span to the
/// volume's end. A write between them, a write that ends where the
/// last end begins, and a read at an end have none.
#[test]
fn a_write_at_either_end_of_the_volume_has_its_bytes() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    // 512 KiB in sectors, and where the last end begins.
    let end = 1024;
    let tail = SIZE / 512 - end;
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![
        fetched(0, op::WRITE, 0, 8),
        fetched(1, op::WRITE, end, 8),
        fetched(2, op::WRITE, end - 8, 16),
        fetched(3, op::READ, 0, 8),
    ]));
    inner.waits.push_back(Ok(vec![
        fetched(0, op::WRITE, tail, 8),
        fetched(1, op::WRITE, tail - 8, 8),
    ]));
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 0, SIZE);
    let mut got = Vec::new();
    let _ = queue.wait(&mut got, &mut Vec::new());
    for (tag, blocks) in [(0, 1), (1, 1), (2, 2), (3, 1)] {
        assert!(queue.take_buf(tag, blocks).is_some());
    }
    got.clear();
    let _ = queue.wait(&mut got, &mut Vec::new());
    for tag in [0, 1] {
        assert!(queue.take_buf(tag, 1).is_some());
    }
    let bytes = |tag: u16, len: usize| {
        let mut buf = vec![0u8; len];
        fill(&mut buf, tag);
        hex(&buf)
    };
    let p = pid();
    let labels: Vec<String> = lines(&path)
        .into_iter()
        .filter(|line| line.contains("\tlabel\t"))
        .collect();
    assert_eq!(
        labels,
        [
            format!("{p}\tlabel\t0\twrite\t0\t8\t{}", bytes(0, 4096)),
            format!("{p}\tlabel\t2\twrite\t1016\t16\t{}", bytes(2, 8192)),
            format!("{p}\tlabel\t0\twrite\t{tail}\t8\t{}", bytes(0, 4096)),
        ]
    );
    assert_eq!(&hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
}

/// The ends follow the labels: on a volume whose size is not a multiple
/// of 256 KiB the labels end before the volume does, and the last end
/// runs from 512 KiB before the labels' end to the volume's end, which
/// holds the last 512 KiB of the volume too.
#[test]
fn the_last_end_runs_from_the_labels_to_the_volumes_end() {
    let size = SIZE + (100 << 10);
    let ends = Ends::of(size);
    assert!(ends.touched(SIZE - (512 << 10), 4096));
    assert!(!ends.touched(SIZE - (512 << 10) - 4096, 4096));
    assert!(ends.touched(size - 4096, 4096));
    assert!(ends.touched(size - (512 << 10), 4096));
    assert!(ends.touched(0, 1));
    assert!(!ends.touched(512 << 10, 4096));
    assert!(!ends.touched(0, 0));
}

/// The store's lines go to the same file through a second handle, in
/// the order of the queue's own: `applied` for a write, `barrier` for a
/// flush (`fua_flush` for the flush the pump adds after an operation
/// with FUA, whose id carries the pump's flag), and `cut` with the
/// cut's group after its `barrier`.
#[test]
fn the_stores_lines_name_the_tag_and_fall_among_the_queues() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("requests.tsv");
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![
        fetched(0, op::WRITE, 4096, 8),
        fetched(1, op::FLUSH, 0, 0),
    ]));
    let file = open(&path).expect("open");
    let mut watcher = watcher(Lines::another(&file));
    let mut queue = Recorded::new(inner, Some(file), 0, SIZE);
    let mut got = Vec::new();
    let _ = queue.wait(&mut got, &mut Vec::new());
    watcher(Seen::Applied(RequestId(0)));
    queue.commit(0, 4096, None);
    watcher(Seen::Barrier(RequestId(1)));
    let cut = skyzfs_local::inplace::Cut {
        number: 1,
        group: 77,
        itself: true,
    };
    watcher(Seen::Cut(RequestId(1), cut));
    queue.commit(1, 0, None);
    // The flush the pump adds after an operation of tag 3 with FUA.
    let added = RequestId(3 | 1 << 32);
    assert_eq!(tag_of(added), (3, true));
    watcher(Seen::Barrier(added));
    watcher(Seen::Cut(
        added,
        skyzfs_local::inplace::Cut {
            itself: false,
            ..cut
        },
    ));
    let p = pid();
    assert_eq!(
        lines(&path),
        [
            format!("{p}\tstart\t0\t-\t-\t-\t4"),
            format!("{p}\trecv\t0\twrite\t4096\t8\t-"),
            format!("{p}\trecv\t1\tflush\t0\t0\t-"),
            format!("{p}\tapplied\t0\twrite\t-\t-\t-"),
            format!("{p}\tcommit\t0\twrite\t4096\t8\t4096"),
            format!("{p}\tbarrier\t1\tflush\t-\t-\t-"),
            format!("{p}\tcut\t1\tflush\titself\t-\t77"),
            format!("{p}\tcommit\t1\tflush\t0\t0\t0"),
            format!("{p}\tbarrier\t3\tfua_flush\t-\t-\t-"),
            format!("{p}\tcut\t3\tfua_flush\trewind\t-\t77"),
        ]
    );
}
