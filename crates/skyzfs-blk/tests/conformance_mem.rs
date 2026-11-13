//! The conformance suite against `MemTarget`: one test per case of the
//! suite, each run against several configurations of the target, so that
//! both completion orders and every discard behaviour are exercised.
//!
//! Rule numbers refer to the crate documentation of `skyzfs-blk`, which
//! restates design §4.1.

use skyzfs_blk::conformance::{cases, run_all, run_case, Config, Status};
use skyzfs_blk::mem::{DiscardPolicy, LogFailure, MemConfig, MemHarness, Schedule, Timing};

/// The `MemTarget` configurations every case must pass against. Between
/// them they use the freedoms the contract gives a target: completions
/// in or out of order, the log appended on receipt or at the
/// acknowledgement, reads taken on receipt or at completion, and each
/// discard behaviour.
fn configurations() -> Vec<MemConfig> {
    let base = MemConfig {
        queue_depth: 16,
        ..MemConfig::default()
    };
    vec![
        // Everything from the seed.
        base,
        // The plainest target: in order, everything at the acknowledgement.
        MemConfig {
            schedule: Schedule::Fifo,
            apply: Timing::AtAck,
            read: Timing::AtAck,
            discard: DiscardPolicy::Zero,
            ..base
        },
        // A write log: appended on receipt, acknowledged later in any
        // order, so the log's order is not the acknowledgements' order.
        MemConfig {
            apply: Timing::AtSubmit,
            read: Timing::AtAck,
            discard: DiscardPolicy::Keep,
            seed: 77,
            ..base
        },
        // Reads taken on receipt and completed late.
        MemConfig {
            apply: Timing::AtAck,
            read: Timing::AtSubmit,
            discard: DiscardPolicy::Base,
            seed: 78,
            ..base
        },
        // Both on receipt.
        MemConfig {
            apply: Timing::AtSubmit,
            read: Timing::AtSubmit,
            seed: 79,
            ..base
        },
        // A store that learns its log has failed from the next append or
        // flush: until then it completes what was already done and reads
        // from memory, which rule R11 allows.
        MemConfig {
            failure: LogFailure::WhenNoticed,
            seed: 80,
            ..base
        },
        // A target that takes one request at a time.
        MemConfig {
            queue_depth: 1,
            ..base
        },
        // A larger device with a deep queue: read back by sampling.
        MemConfig {
            size_mib: 8,
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
            Some(Ok(Status::Passed)) => {}
            // Only the one-at-a-time target may leave a rule unexercised.
            Some(Ok(Status::NotExercised(_))) if config.queue_depth == 1 => {}
            Some(Ok(Status::NotExercised(why))) => {
                panic!("{name} was not exercised against {config:?}: {why}")
            }
            Some(Ok(Status::Weaker(what))) => {
                panic!("{name} ran in a weaker form against {config:?}: {what}")
            }
            Some(Err(failure)) => panic!("{name} failed against {config:?}:\n{failure}"),
            None => panic!("the suite has no case named {name}"),
        }
    }
}

/// The names of the cases that have a test below.
const TESTED: &[&str] = &[
    "parameters_are_as_the_contract_fixes_them",
    "fresh_device_reads_as_its_base",
    "rejects_misaligned",
    "rejects_out_of_bounds",
    "rejects_empty_and_oversize",
    "read_your_writes",
    "write_zeroes_hides_the_base",
    "discard_leaves_old_zeros_or_base",
    "discard_never_hits_later_writes",
    "flushed_operations_survive_power_cut",
    "flush_covers_what_was_acknowledged_before_it",
    "flush_covers_an_earlier_instance",
    "power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may",
    "recovered_state_is_durable",
    "first_request_after_power_cut_may_be_a_write",
    "queue_depth_requests_complete_in_any_order",
    "overlapping_operations_in_flight_apply_whole",
    "kill_keeps_acknowledged_operations",
    "reissue_after_kill_is_idempotent",
    "failed_volume_fails_every_request",
    "reopened_failed_volume_holds_what_was_flushed",
    "restart_after_the_log_failed_recovers_nothing",
    "read_only_target_refuses_mutations",
    "prop_invalid_requests_are_rejected_without_effect",
    "prop_concurrent_programs_match_model",
    "prop_failures_match_model",
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
    assert!(report.not_exercised().is_empty());
    assert!(report.weaker().is_empty(), "{report}");
}

/// Not a contract rule: a case whose rule the target cannot exercise is
/// reported as such and not as passed, so that a green run is not read as
/// coverage. A target that takes one request at a time cannot exercise
/// the two cases about requests in flight together.
#[test]
fn unexercised_cases_are_reported_as_such() {
    let config = MemConfig {
        queue_depth: 1,
        ..MemConfig::default()
    };
    let suite = Config {
        cases: 8,
        ..Config::default()
    };
    let report = run_all(&mut MemHarness::new(config), &suite);
    assert!(report.passed(), "{report}");
    assert_eq!(
        report.not_exercised(),
        [
            "overlapping_operations_in_flight_apply_whole",
            "reissue_after_kill_is_idempotent"
        ]
    );
    assert!(report.to_string().contains("SKIP"));
}

/// R1: the parameters are as the contract fixes them: a size in whole
/// MiB, the read-only flag as asked, unchanged by a daemon loss and a
/// power cut.
#[test]
fn parameters_are_as_the_contract_fixes_them() {
    passes("parameters_are_as_the_contract_fixes_them");
}

/// R3: a block never written reads as the base: zeros on an empty volume,
/// the commit's content on a clone.
#[test]
fn fresh_device_reads_as_its_base() {
    passes("fresh_device_reads_as_its_base");
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

/// R1, R2: an empty request and a read or write longer than 1 MiB are
/// rejected; 1 MiB itself is accepted, and a discard and a write-zeroes
/// may span the device.
#[test]
fn rejects_empty_and_oversize() {
    passes("rejects_empty_and_oversize");
}

/// R3, R4: once a write is acknowledged, reads return it, and an overwrite
/// replaces it.
#[test]
fn read_your_writes() {
    passes("read_your_writes");
}

/// R5: write-zeroes reads as zeros on a volume with a base (a recorded
/// zero extent, not an absence) and has the durability of a write.
#[test]
fn write_zeroes_hides_the_base() {
    passes("write_zeroes_hides_the_base");
}

/// R6: a discarded block reads as its content before the discard, zeros
/// or the base, also after a power cut, and its neighbours are untouched.
#[test]
fn discard_leaves_old_zeros_or_base() {
    passes("discard_leaves_old_zeros_or_base");
}

/// R6, R8: a discard is never applied to data written after it.
#[test]
fn discard_never_hits_later_writes() {
    passes("discard_never_hits_later_writes");
}

/// R7, R10: every operation acknowledged before a completed flush was
/// submitted survives a power cut that keeps nothing unflushed.
#[test]
fn flushed_operations_survive_power_cut() {
    passes("flushed_operations_survive_power_cut");
}

/// R7, the group-commit case: write A acknowledged, flush submitted, write
/// B acknowledged while the flush is pending, power cut right after the
/// flush completes: A is present, B may or may not be.
#[test]
fn flush_covers_what_was_acknowledged_before_it() {
    passes("flush_covers_what_was_acknowledged_before_it");
}

/// R7, R9: W acknowledged, daemon lost and restarted, flush, power cut
/// immediately after the flush completes: W is present. And recovery
/// makes the log durable even without the flush.
#[test]
fn flush_covers_an_earlier_instance() {
    passes("flush_covers_an_earlier_instance");
}

/// R10: a power cut at every record of what the target persists, not
/// only at flush boundaries, leaves what the volatile-cache disk may (a
/// prefix of the order is one such state).
#[test]
fn power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may() {
    passes("power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may");
}

/// R10: what recovery exposes is stable and is durable before the first
/// request is served: a second power cut does not take it back.
#[test]
fn recovered_state_is_durable() {
    passes("recovered_state_is_durable");
}

/// R10: the first request after a power cut may be a write, issued before
/// any read; it lands after everything recovery exposed.
#[test]
fn first_request_after_power_cut_may_be_a_write() {
    passes("first_request_after_power_cut_may_be_a_write");
}

/// R8: the target takes `queue_depth` requests in flight, completes each
/// exactly once in whatever order, and refuses an id already in flight.
#[test]
fn queue_depth_requests_complete_in_any_order() {
    passes("queue_depth_requests_complete_in_any_order");
}

/// R8: overlapping operations in flight together are applied whole in one
/// order: the same one wins on every block they share, in memory and
/// after a power cut.
#[test]
fn overlapping_operations_in_flight_apply_whole() {
    passes("overlapping_operations_in_flight_apply_whole");
}

/// R9: after a daemon loss every acknowledged operation is readable,
/// flushed or not.
#[test]
fn kill_keeps_acknowledged_operations() {
    passes("kill_keeps_acknowledged_operations");
}

/// R9: requests in flight when the daemon is lost are reissued under the
/// same ids, in either order, and complete as the contract describes
/// whether or not the lost daemon had applied them.
#[test]
fn reissue_after_kill_is_idempotent() {
    passes("reissue_after_kill_is_idempotent");
}

/// R11: once the log has failed for good the flush fails and so does
/// every later request, reads included, also after a daemon restart.
#[test]
fn failed_volume_fails_every_request() {
    passes("failed_volume_fails_every_request");
}

/// R11: a failed volume that is opened again after a repair holds at
/// least what every completed flush covered, and serves again.
#[test]
fn reopened_failed_volume_holds_what_was_flushed() {
    passes("reopened_failed_volume_holds_what_was_flushed");
}

/// R11 over R9: the log fails and the daemon restarts before any
/// failure was reported; opened again, the volume holds everything
/// flushed, and may have lost what was only acknowledged.
#[test]
fn restart_after_the_log_failed_recovers_nothing() {
    passes("restart_after_the_log_failed_recovers_nothing");
}

/// R12: on a read-only target write, write-zeroes and discard fail
/// without effect, and flush and read succeed.
#[test]
fn read_only_target_refuses_mutations() {
    passes("read_only_target_refuses_mutations");
}

/// R2, as a property: any invalid request is rejected and changes nothing.
#[test]
fn prop_invalid_requests_are_rejected_without_effect() {
    passes("prop_invalid_requests_are_rejected_without_effect");
}

/// R3 to R8, as a property: random overlapping requests on an empty or a
/// non-empty base, reaped at random moments with completions out of
/// order, read back as some order of whole operations allows.
#[test]
fn prop_concurrent_programs_match_model() {
    passes("prop_concurrent_programs_match_model");
}

/// R7, R9, R10, R11, as a property: random programs with daemon losses,
/// power cuts at random points of the log and log failure read back as
/// the reference model allows.
#[test]
fn prop_failures_match_model() {
    passes("prop_failures_match_model");
}
