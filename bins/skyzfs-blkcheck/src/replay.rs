//! Replays a trace of the tool (`BLKCHECK_TRACE`) through the suite's
//! model: the same submits, deliveries and kills, in the same order,
//! with contents rebuilt from the digests. It says which event the
//! model objects to, and so lets a failure of a kernel run be studied
//! on any machine.
//!
//! ```text
//! skyzfs-blkcheck --replay TRACE VOLUME [SIZE_MIB]
//! ```

use std::collections::BTreeMap;
use std::process::ExitCode;

use skyzfs_blk::model::{InFlight, Model};
use skyzfs_blk::{BlkError, Buf, Completion, Geometry, Op, Request, RequestId};

const BLOCK: usize = 4096;

/// The trace's word for a result the device harness hands the suite as
/// answered before a kill. The tool writes it and the replayer reads
/// it, both from here, so that the two cannot drift apart. A trace from
/// before it has `before_kill` there instead, which the replayer passes
/// over: the suite then dropped such results and issued them again.
pub(crate) const ANSWERED_BEFORE_KILL: &str = "answered_before_kill";

/// Content for a digest: the same digest gives the same block, and
/// "zero" gives zeros, which is all the model compares.
fn content(digests: &str) -> Option<Buf> {
    let mut bytes = Vec::new();
    for digest in digests.split(',').filter(|d| !d.is_empty()) {
        if digest == "zero" {
            bytes.extend(std::iter::repeat_n(0u8, BLOCK));
        } else {
            bytes.extend(digest.as_bytes().iter().cycle().take(BLOCK));
        }
    }
    Buf::from_blocks(&bytes)
}

struct Done {
    result: Result<(), BlkError>,
    seen: String,
}

fn result_of(text: &str) -> Result<(), BlkError> {
    if text.starts_with("Ok") {
        Ok(())
    } else if text.contains("Failed") {
        Err(BlkError::Failed)
    } else {
        Err(BlkError::Io(std::io::ErrorKind::Other))
    }
}

/// Replay volume `volume` of the trace at `path`. Prints one line per
/// event the model is given and what the model said.
pub fn replay(path: &str, volume: &str, size_mib: u64) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(2);
        }
    };
    match replay_text(&text, volume, size_mib) {
        Ok(violations) => ExitCode::from(u8::from(violations > 0)),
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(2)
        }
    }
}

/// The size in MiB and the depth the tool opened the volume with, from
/// the trace's "open" line. A trace without one is replayed at the size
/// given and the tool's own depth.
fn opened_with(text: &str, volume: &str) -> Option<(u64, u32)> {
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() >= 6 && f[1] == volume && f[2] == "open" {
            Some((f[4].parse().ok()?, f[5].parse().ok()?))
        } else {
            None
        }
    })
}

/// The replay of a trace's text: how many events the model objected
/// to, or why the trace could not be replayed.
fn replay_text(text: &str, volume: &str, size_mib: u64) -> Result<usize, String> {
    let (size_mib, depth) = opened_with(text, volume).unwrap_or((size_mib, crate::device::DEPTH));
    let Ok(geometry) = Geometry::new(size_mib, depth, false) else {
        return Err(format!("not a geometry: {size_mib} MiB, depth {depth}"));
    };
    let mut model = Model::new(geometry);
    let mut ops: BTreeMap<u64, String> = BTreeMap::new();
    let mut done: BTreeMap<u64, Done> = BTreeMap::new();
    let mut violations = 0;
    // Ids the model has been given and whose result it has not.
    let mut outstanding = std::collections::BTreeSet::new();
    // Between "kill" and "settled" the tool collects the results of the
    // calls that were outstanding; the suite has not seen them yet.
    let mut settling = false;
    // The calls that had returned when the old server was gone, in the
    // order the tool sorted them: the suite takes them as answered
    // before the loss, when the kill is over and before its model is
    // told of it. An older trace says `before_kill` for them, when the
    // tool dropped them and the suite issued them again; those lines
    // are passed over, as the suite passed over the results.
    let mut answered_before: Vec<u64> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 3 || f[1] != volume {
            continue;
        }
        let (ms, event) = (f[0], f[2]);
        let id = f.get(3).and_then(|s| s.parse::<u64>().ok());
        let said = match (event, id) {
            ("submit", Some(id)) if f.len() >= 7 => {
                let first: u64 = f[5].parse().unwrap_or(0);
                let blocks: usize = f[6].parse().unwrap_or(0);
                let offset = first * BLOCK as u64;
                let len = (blocks * BLOCK) as u64;
                let op = match f[4] {
                    "read" => Some(Op::Read {
                        offset,
                        buf: Buf::zeroed(blocks),
                    }),
                    "write" => content(f.get(7).copied().unwrap_or(""))
                        .map(|data| Op::Write { offset, data }),
                    "flush" => Some(Op::Flush),
                    "discard" => Some(Op::Discard { offset, len }),
                    "write_zeroes" => Some(Op::WriteZeroes { offset, len }),
                    _ => None,
                };
                let Some(op) = op else {
                    return Err(format!("{ms}: cannot rebuild request {id}: {line}"));
                };
                ops.insert(id, format!("{} {first}+{blocks}", f[4]));
                if !outstanding.insert(id) {
                    // The suite's reissue after a kill, which the tool
                    // issued: the model is not told of it a second time.
                    println!(
                        "{ms}\tsubmit\t#{id} {} {first}+{blocks}\tissued again\tworlds {}",
                        f[4],
                        model.worlds()
                    );
                    continue;
                }
                format!(
                    "{:?}",
                    model.submit(&Request {
                        id: RequestId(id),
                        op
                    })
                )
            }
            ("done", Some(id)) => {
                done.insert(
                    id,
                    Done {
                        result: result_of(f.get(4).copied().unwrap_or("")),
                        seen: f.get(5).copied().unwrap_or("").to_string(),
                    },
                );
                continue;
            }
            ("kill", _) => {
                settling = true;
                continue;
            }
            (ANSWERED_BEFORE_KILL, Some(id)) if settling => {
                answered_before.push(id);
                continue;
            }
            ("delivered", _) if settling => continue,
            ("delivered", Some(id)) => {
                let Some(d) = done.remove(&id) else {
                    return Err(format!("{ms}: {id} delivered before it was done"));
                };
                outstanding.remove(&id);
                let buf = if d.seen.is_empty() {
                    None
                } else {
                    content(&d.seen)
                };
                let completion = Completion {
                    id: RequestId(id),
                    result: d.result,
                    buf,
                };
                format!("{:?}", model.complete(&completion))
            }
            ("settled", _) => {
                settling = false;
                for id in std::mem::take(&mut answered_before) {
                    let Some(d) = done.remove(&id) else {
                        return Err(format!(
                            "{ms}: {id} answered before the kill but never done"
                        ));
                    };
                    outstanding.remove(&id);
                    let buf = if d.seen.is_empty() {
                        None
                    } else {
                        content(&d.seen)
                    };
                    let said = model.complete(&Completion {
                        id: RequestId(id),
                        result: d.result,
                        buf,
                    });
                    let what = ops.get(&id).cloned().unwrap_or_default();
                    println!(
                        "{ms}\tanswered_before_kill\t#{id} {what}\t{said:?}\tworlds {}",
                        model.worlds()
                    );
                    if said.is_err() {
                        violations += 1;
                    }
                }
                // The trace is a block device's: a call the lost daemon
                // completed can return after the kill.
                model.kill(InFlight::MayHaveCompleted);
                "the model is told of the kill".to_string()
            }
            ("fail", _) => {
                model.log_failed();
                "the model is told the log failed".to_string()
            }
            ("reopen", _) => {
                // Nothing is outstanding at a volume that is opened
                // again: an id from before is a new request.
                outstanding.clear();
                done.clear();
                model.reopened();
                "the model is told of the reopen".to_string()
            }
            _ => continue,
        };
        let what = id.and_then(|id| ops.get(&id)).cloned().unwrap_or_default();
        let id = id.map_or_else(String::new, |id| format!("#{id} "));
        println!(
            "{ms}\t{event}\t{id}{what}\t{said}\tworlds {}",
            model.worlds()
        );
        if said.starts_with("Err") {
            violations += 1;
        }
    }
    println!("{violations} violations");
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::{opened_with, replay_text};

    /// A write, the log fails, and a read that the kernel answers with
    /// an I/O error: the suite is given `Failed` for it, and the trace
    /// has that first and the system call's own result after it.
    fn failed_log(read_result: &str) -> String {
        [
            "1\t7\tsubmit\t1\twrite\t3\t1\taaaa",
            "2\t7\tdone\t1\tOk(())\t\tOk(())",
            "3\t7\tdelivered\t1\t",
            "4\t7\tfail\t\t",
            "5\t7\tsubmit\t2\tread\t3\t1\t",
            &format!("6\t7\tdone\t2\t{read_result}\t\tErr(Io(Other))"),
            "7\t7\tdelivered\t2\t",
        ]
        .join("\n")
    }

    #[test]
    fn a_failed_log_replays_as_the_suite_saw_it() {
        assert_eq!(replay_text(&failed_log("Err(Failed)"), "7", 64), Ok(0));
    }

    /// What the replayer was given before: the system call's result. The
    /// model does not accept an I/O error from a volume, failed or not.
    #[test]
    fn the_raw_result_of_a_failed_volume_is_a_violation() {
        assert_eq!(replay_text(&failed_log("Err(Io(Other))"), "7", 64), Ok(1));
    }

    /// The geometry is the one the volume was opened with. The model
    /// does not use the depth today; it is read all the same, so that
    /// the replay and the live run cannot come apart when it does.
    #[test]
    fn the_geometry_is_read_from_the_open_line() {
        let trace = "1\t7\topen\t\t128\t16\n1\t8\topen\t\t64\t32";
        assert_eq!(opened_with(trace, "7"), Some((128, 16)));
        assert_eq!(opened_with(trace, "8"), Some((64, 32)));
        assert_eq!(opened_with(trace, "9"), None);
        assert_eq!(opened_with("1\t7\topen\t\tx\t16", "7"), None);
    }

    /// An id that was outstanding when the volume failed is a new
    /// request after the volume is opened again, not one issued again.
    #[test]
    fn an_id_outstanding_before_a_reopen_is_new_after_it() {
        let trace = [
            "1\t7\tsubmit\t1\twrite\t3\t1\taaaa",
            "2\t7\tfail\t\t",
            "3\t7\treopen\t\t",
            "4\t7\tsubmit\t1\tread\t3\t1\t",
            "5\t7\tdone\t1\tOk(())\tzero\tOk(())",
            "6\t7\tdelivered\t1\t",
        ]
        .join("\n");
        assert_eq!(replay_text(&trace, "7", 64), Ok(0));
    }

    /// A write the old server answered before its death, whose call had
    /// returned when it was gone: under the word of the tool that hands
    /// it over, the suite took it as answered before the loss and did
    /// not issue it again, so a read after the kill that misses it is a
    /// violation. Under the older word the tool had dropped the result
    /// and the suite took the write as in flight at the death, which a
    /// read of the old content does not contradict.
    fn answered_before(word: &str) -> String {
        [
            "1\t7\tsubmit\t1\twrite\t3\t1\taaaa",
            "2\t7\tdone\t1\tOk(())\t\tOk(())",
            "3\t7\tkill\t\t",
            &format!("4\t7\t{word}\t1\tOk(())"),
            "4\t7\tdelivered\t1\t",
            "5\t7\tsettled\t\t",
            "6\t7\tsubmit\t2\tread\t3\t1\t",
            "7\t7\tdone\t2\tOk(())\tzero\tOk(())",
            "8\t7\tdelivered\t2\t",
        ]
        .join("\n")
    }

    #[test]
    fn a_write_answered_before_the_kill_must_survive_it() {
        assert_eq!(
            replay_text(&answered_before("answered_before_kill"), "7", 64),
            Ok(1)
        );
        assert_eq!(replay_text(&answered_before("before_kill"), "7", 64), Ok(0));
        // And the write read back after the kill is no violation.
        let kept = answered_before("answered_before_kill").replace("\tzero\t", "\taaaa\t");
        assert_eq!(replay_text(&kept, "7", 64), Ok(0));
    }

    #[test]
    fn another_volume_of_the_trace_is_not_replayed() {
        assert_eq!(replay_text(&failed_log("Err(Io(Other))"), "8", 64), Ok(0));
    }

    // M1.T28. Volume 60 of check c's case `prop_failures_match_model`
    // (seed 0x5ca1ab1e), from two failed passes on ubuntu-26.04: the
    // daemon rig's run 43430825700 and the diagnostic run 43455086383,
    // pass 3. In both, writes #20 (16372+8), #21 (16377+7) and #17
    // (write-zeroes 16383+1) are in flight together, #20 and #17 are
    // acknowledged, and the tool kills the server while #21's call has
    // not returned. The second run's kernel trace and request log show
    // that the old server was handed #21 whole, applied it before #17
    // and completed both before the kill; the call returned after the
    // restart only because a udev read of block 16382, requeued at the
    // death, held the page the direct write's return waits on. The tool
    // counted #21 as in flight at the death and the model required it
    // to be issued again after #20 and #17, so that it wins the blocks
    // it shares with them. It need not: a call outstanding at a kill
    // was either applied by the old server while it was in flight, in
    // any order with what was in flight with it, or is issued again.
    // See skyzfs/skyzfs#58 (issuecomment-6858276171).
    const RUN_43430825700: &str = include_str!("testdata/c-trace-43430825700-v60.tsv");
    const RUN_43455086383: &str = include_str!("testdata/c-trace-43455086383-v60.tsv");
    // The daemon rig's run 43453345076, the pass with a commit delay of
    // 20 ms: the same case with the roles swapped. #21 is acknowledged
    // before the kill and #20's call returns after it.
    const RUN_43453345076_DELAY: &str = include_str!("testdata/cd-trace-43453345076-v60.tsv");

    /// The trace with block `index` of the read `id`'s result replaced.
    fn with_read_block(trace: &str, id: u64, index: usize, digest: &str) -> String {
        let mut changed = 0;
        let out = trace
            .lines()
            .map(|line| {
                let mut f: Vec<String> = line.split('\t').map(str::to_string).collect();
                if f.len() >= 6 && f[2] == "done" && f[3] == id.to_string() {
                    let mut blocks: Vec<&str> = f[5].split(',').collect();
                    blocks[index] = digest;
                    f[5] = blocks.join(",");
                    changed += 1;
                }
                f.join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(changed, 1, "the trace has one result of read #{id}");
        out
    }

    /// Run 43430825700: blocks 16377 to 16379 read #20's data, the
    /// order the old server applied the two in.
    #[test]
    fn a_call_returned_after_the_kill_may_have_been_applied_before_it_43430825700() {
        assert_eq!(replay_text(RUN_43430825700, "60", 64), Ok(0));
    }

    /// Run 43455086383: block 16383 reads zero, #17 applied after #21
    /// by the old server.
    #[test]
    fn a_call_returned_after_the_kill_may_have_been_applied_before_it_43455086383() {
        assert_eq!(replay_text(RUN_43455086383, "60", 64), Ok(0));
    }

    /// Run 43430825700 as the model wanted it: blocks 16377 to 16379
    /// hold #21's data, as if #21 had been issued again after #20.
    fn reissued_43430825700() -> String {
        let mut trace = RUN_43430825700.to_string();
        for (i, digest) in ["964cfc9ff9404b25", "6bafe44ab8087b25", "29b89f4b2c577325"]
            .iter()
            .enumerate()
        {
            trace = with_read_block(&trace, 30, 17 + i, digest);
            trace = with_read_block(&trace, 31, 9 + i, digest);
        }
        trace
    }

    /// Run 43455086383 as the model wanted it: block 16383 holds #21's
    /// data, as if #21 had been issued again after #17.
    fn reissued_43455086383() -> String {
        let trace = with_read_block(RUN_43455086383, 33, 23, "6c22afe639c02325");
        with_read_block(&trace, 34, 15, "6c22afe639c02325")
    }

    /// The order of a reissue stays allowed.
    #[test]
    fn a_call_issued_again_after_the_kill_is_still_allowed() {
        assert_eq!(replay_text(&reissued_43430825700(), "60", 64), Ok(0));
        assert_eq!(replay_text(&reissued_43455086383(), "60", 64), Ok(0));
    }

    /// Run 43453345076, the pass with the delay: blocks 16377 to 16379
    /// read #21's data; #20, whose call returned after the kill, was
    /// applied before #21 by the old server.
    #[test]
    fn a_call_returned_after_the_kill_may_have_been_applied_before_it_43453345076() {
        assert_eq!(replay_text(RUN_43453345076_DELAY, "60", 64), Ok(0));
    }

    // Controls: each changes one block of a trace that is allowed (the
    // reissue's order, above) into what no order allows. The model must
    // refuse it, now and after M1.T28.

    /// Block 16377 holds data that no call wrote.
    #[test]
    fn a_block_no_call_wrote_is_still_refused() {
        let trace = with_read_block(&reissued_43430825700(), 31, 9, "0123456789abcdef");
        assert!(matches!(replay_text(&trace, "60", 64), Ok(n) if n > 0));
    }

    /// Block 16382 reads zero where #21 is the only write: #21 was
    /// applied by the old server or issued again, never dropped.
    #[test]
    fn the_outstanding_call_is_not_taken_as_lost() {
        let trace = with_read_block(&reissued_43455086383(), 34, 14, "zero");
        assert!(matches!(replay_text(&trace, "60", 64), Ok(n) if n > 0));
    }

    /// Block 16377 holds #21's data after #27, submitted once #21 had
    /// been delivered, was acknowledged over it: whichever way #21 was
    /// applied, it came before #27.
    #[test]
    fn the_outstanding_call_does_not_land_after_a_later_call() {
        let trace = with_read_block(&reissued_43455086383(), 34, 9, "964cfc9ff9404b25");
        assert!(matches!(replay_text(&trace, "60", 64), Ok(n) if n > 0));
    }
}
