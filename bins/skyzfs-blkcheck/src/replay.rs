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

use skyzfs_blk::model::Model;
use skyzfs_blk::{BlkError, Buf, Completion, Geometry, Op, Request, RequestId};

const BLOCK: usize = 4096;

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
                model.kill();
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

    #[test]
    fn another_volume_of_the_trace_is_not_replayed() {
        assert_eq!(replay_text(&failed_log("Err(Io(Other))"), "8", 64), Ok(0));
    }
}
