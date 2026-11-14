use std::collections::VecDeque;

use super::*;

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
    fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<Buf> {
        Some(Buf::zeroed(blocks))
    }
    fn commit(&mut self, tag: u16, result: i32, _buf: Option<Buf>) {
        self.committed.push((tag, result));
    }
    fn flush(&mut self) -> Result<(), QueueDown> {
        self.note_last_line();
        self.flushes += 1;
        if self.flush_fails {
            Err(QueueDown)
        } else {
            Ok(())
        }
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
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 7);
    let mut got = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Ok(Wake::Work));
    assert_eq!(got.len(), 3);
    queue.commit(1, 16384, None);
    queue.commit(0, 12288, None);
    queue.commit(2, 0, None);
    got.clear();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Ok(Wake::Work));
    queue.commit(0, -5, None);
    assert_eq!(queue.flush(), Ok(()));
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
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 0);
    let mut got = Vec::new();
    let _ = queue.wait(&mut got, &mut Vec::new());
    queue.commit(0, 4096, None);
    let _ = queue.wait(&mut got, &mut Vec::new());
    queue.commit(0, 4096, None);
    let _ = queue.flush();
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
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 0);
    let before = mono_ns();
    let mut got = Vec::new();
    let _ = queue.wait(&mut got, &mut Vec::new());
    queue.commit(3, 0, None);
    let _ = queue.flush();
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
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 1);
    queue.commit(2, -5, None);
    let mut got = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Err(QueueDown));
    queue.commit(3, -5, None);
    assert_eq!(queue.flush(), Err(QueueDown));
    // Nothing committed since: no line.
    assert_eq!(queue.flush(), Err(QueueDown));
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
    let mut first = Recorded::new(inner, Some(open(&path).expect("open")), 0);
    let mut got = Vec::new();
    let _ = first.wait(&mut got, &mut Vec::new());
    drop(first);
    let mut inner = Scripted::default();
    inner.waits.push_back(Ok(vec![fetched(3, op::WRITE, 0, 8)]));
    let mut second = Recorded::new(inner, Some(open(&path).expect("open")), 0);
    got.clear();
    let _ = second.wait(&mut got, &mut Vec::new());
    second.commit(3, 4096, None);
    let _ = second.flush();
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
    let mut queue = Recorded::new(inner, None, 0);
    let mut got = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut Vec::new()), Ok(Wake::Work));
    assert_eq!(got, [fetched(1, op::READ, 0, 8)]);
    assert!(queue.take_buf(1, 2).is_some_and(|buf| buf.len() == 8192));
    queue.commit(1, 4096, None);
    assert_eq!(queue.flush(), Ok(()));
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
/// its tag, the command, the result. Also when the wait fails, and the
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
    let mut queue = Recorded::new(inner, Some(open(&path).expect("open")), 7);
    queue.commit(1, 4096, None);
    let mut got = Vec::new();
    let mut ended = Vec::new();
    assert_eq!(queue.wait(&mut got, &mut ended), Ok(Wake::Work));
    assert_eq!(ended.len(), 1);
    queue.commit(0, 8192, None);
    assert_eq!(queue.wait(&mut got, &mut ended), Err(QueueDown));
    assert_eq!(ended.len(), 4);
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
        ]
    );
}
