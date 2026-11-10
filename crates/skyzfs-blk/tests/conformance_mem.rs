//! The conformance suite against `MemTarget`: one test per case of the
//! suite, each run against several configurations of the target, so that
//! both completion orders and all three discard behaviours are exercised.
//!
//! Rule numbers refer to the crate documentation of `skyzfs-blk`, which
//! restates design §4.1.

use skyzfs_blk::conformance::{cases, run_all, run_case, Config};
use skyzfs_blk::mem::{DiscardPolicy, MemConfig, MemHarness, Schedule};

/// The `MemTarget` configurations every case must pass against.
fn configurations() -> Vec<MemConfig> {
    let base = MemConfig {
        blocks: 64,
        max_io_blocks: 8,
        queue_depth: 16,
        ..MemConfig::default()
    };
    vec![
        base,
        MemConfig {
            schedule: Schedule::Fifo,
            discard: DiscardPolicy::Zero,
            ..base
        },
        MemConfig {
            discard: DiscardPolicy::Keep,
            seed: 77,
            ..base
        },
        // A target that takes one request at a time and small requests.
        MemConfig {
            blocks: 16,
            max_io_blocks: 1,
            queue_depth: 1,
            ..base
        },
        // A device larger than the suite's window, with large requests.
        MemConfig {
            blocks: 1024,
            max_io_blocks: 256,
            queue_depth: 64,
            seed: 9,
            ..base
        },
    ]
}

fn suite_config() -> Config {
    Config {
        cases: 96,
        ..Config::default()
    }
}

fn passes(name: &str) {
    for config in configurations() {
        let mut harness = MemHarness::new(config);
        match run_case(&mut harness, &suite_config(), name) {
            Some(Ok(())) => {}
            Some(Err(failure)) => panic!("{name} failed against {config:?}:\n{failure}"),
            None => panic!("the suite has no case named {name}"),
        }
    }
}

/// The names of the cases that have a test below.
const TESTED: &[&str] = &[
    "geometry_is_sane",
    "fresh_device_reads_zeros",
    "rejects_misaligned",
    "rejects_out_of_bounds",
    "rejects_zero_length_and_oversize",
    "read_your_writes",
    "write_zeroes_reads_zeros_and_is_durable",
    "discard_reads_old_or_zeros",
    "write_after_discard_wins",
    "flushed_writes_survive_power_cut",
    "unflushed_writes_are_old_or_new_never_torn",
    "flush_with_writes_in_flight",
    "queue_depth_requests_complete_in_any_order",
    "overlapping_writes_apply_in_ack_order",
    "restart_keeps_acknowledged_writes",
    "reissue_after_restart_is_idempotent",
    "prop_invalid_requests_are_rejected_without_effect",
    "prop_concurrent_programs_match_model",
    "prop_power_cuts_and_restarts_match_model",
];

/// Not a contract rule: guards this file. Every case of the suite has a
/// test here, and every case states the rule it verifies.
#[test]
fn every_case_of_the_suite_is_tested_and_names_its_rule() {
    let all = cases::<MemHarness>();
    let names: Vec<&str> = all.iter().map(|c| c.name).collect();
    assert_eq!(names, TESTED);
    for case in &all {
        assert!(case.rule.starts_with('R'), "{} names no rule", case.name);
    }
}

/// Not a contract rule: the suite's one-call entry point reports every
/// case, and all pass against a correct target.
#[test]
fn run_all_passes_and_reports_every_case() {
    let mut harness = MemHarness::new(configurations()[0]);
    let config = Config {
        cases: 16,
        ..Config::default()
    };
    let report = run_all(&mut harness, &config);
    assert!(report.passed(), "{report}");
    assert_eq!(report.outcomes.len(), TESTED.len());
    assert!(report.failed().is_empty());
}

/// R1: the device size and the largest request are non-zero multiples of
/// 4 KiB, and the geometry survives a power cut and a restart.
#[test]
fn geometry_is_sane() {
    passes("geometry_is_sane");
}

/// R3: a block never written reads as zeros.
#[test]
fn fresh_device_reads_zeros() {
    passes("fresh_device_reads_zeros");
}

/// R2: an offset or length that is not a multiple of 4 KiB is rejected,
/// for all four ranged operations, without effect.
#[test]
fn rejects_misaligned() {
    passes("rejects_misaligned");
}

/// R2: a request reaching past the end of the device is rejected without
/// effect, and one ending exactly at the end is accepted.
#[test]
fn rejects_out_of_bounds() {
    passes("rejects_out_of_bounds");
}

/// R2: a zero-length request and a read or write longer than the stated
/// maximum are rejected; the maximum itself is accepted, and a discard and
/// a write-zeroes may span the device (open readings 2 and 3).
#[test]
fn rejects_zero_length_and_oversize() {
    passes("rejects_zero_length_and_oversize");
}

/// R3, R4: once a write is acknowledged, reads return it, and an overwrite
/// replaces it.
#[test]
fn read_your_writes() {
    passes("read_your_writes");
}

/// R7: write-zeroes reads as zeros and has the durability of a write.
#[test]
fn write_zeroes_reads_zeros_and_is_durable() {
    passes("write_zeroes_reads_zeros_and_is_durable");
}

/// R3, R6: a discarded block reads as its last write or as zeros, also
/// after a power cut, and its neighbours are untouched (open reading 1).
#[test]
fn discard_reads_old_or_zeros() {
    passes("discard_reads_old_or_zeros");
}

/// R6, R8: a write acknowledged after a discard is what reads return.
#[test]
fn write_after_discard_wins() {
    passes("write_after_discard_wins");
}

/// R5, R10: every write acknowledged before a completed flush was
/// submitted survives a power cut.
#[test]
fn flushed_writes_survive_power_cut() {
    passes("flushed_writes_survive_power_cut");
}

/// R4, R10: after a power cut a block holds its flushed content or one
/// later write, never a mix of two; what is read then is stable and
/// survives a second power cut (open reading 8).
#[test]
fn unflushed_writes_are_old_or_new_never_torn() {
    passes("unflushed_writes_are_old_or_new_never_torn");
}

/// R5: a flush submitted while a write is in flight completes and covers
/// at least the writes acknowledged before it was submitted (open reading
/// 4).
#[test]
fn flush_with_writes_in_flight() {
    passes("flush_with_writes_in_flight");
}

/// R8: the target takes `queue_depth` requests in flight, completes each
/// exactly once in whatever order, and refuses an id already in flight.
#[test]
fn queue_depth_requests_complete_in_any_order() {
    passes("queue_depth_requests_complete_in_any_order");
}

/// R8: writes to one block are applied in acknowledgement order.
#[test]
fn overlapping_writes_apply_in_ack_order() {
    passes("overlapping_writes_apply_in_ack_order");
}

/// R9: a write acknowledged before the daemon is lost is still there when
/// it returns, and a later flush makes it durable (open reading 5).
#[test]
fn restart_keeps_acknowledged_writes() {
    passes("restart_keeps_acknowledged_writes");
}

/// R9: requests in flight when the daemon is lost are reissued under the
/// same ids and complete as if issued once.
#[test]
fn reissue_after_restart_is_idempotent() {
    passes("reissue_after_restart_is_idempotent");
}

/// R2, as a property: any invalid request is rejected and changes nothing.
#[test]
fn prop_invalid_requests_are_rejected_without_effect() {
    passes("prop_invalid_requests_are_rejected_without_effect");
}

/// R3, R4, R6, R7, R8, as a property: random overlapping requests reaped
/// at random moments, with completions out of order, read back as the
/// reference model allows.
#[test]
fn prop_concurrent_programs_match_model() {
    passes("prop_concurrent_programs_match_model");
}

/// R5, R9, R10, as a property: random programs with power cuts and daemon
/// restarts at random moments read back as the reference model allows.
#[test]
fn prop_power_cuts_and_restarts_match_model() {
    passes("prop_power_cuts_and_restarts_match_model");
}
