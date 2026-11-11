//! Tests of the suite itself: each deliberately broken `MemTarget` must
//! fail the conformance suite, and must fail the case written for the rule
//! it breaks. A suite that passes a mutant verifies nothing.
//!
//! Rule numbers refer to the crate documentation of `skyzfs-blk`.

use skyzfs_blk::conformance::{run_all, Config};
use skyzfs_blk::mem::{Fault, MemConfig, MemHarness};

/// Run the whole suite against a target with `fault` and require that it
/// fails, and that every case in `must_fail` is among the failures.
fn caught(fault: Fault, must_fail: &[&str]) {
    let config = MemConfig {
        queue_depth: 16,
        fault: Some(fault),
        ..MemConfig::default()
    };
    let suite = Config {
        cases: 64,
        ..Config::default()
    };
    let report = run_all(&mut MemHarness::new(config), &suite);
    let failed = report.failed();
    assert!(!report.passed(), "the suite passed a broken target");
    for name in must_fail {
        assert!(
            failed.contains(name),
            "{name} did not catch {fault:?}; failed cases: {failed:?}"
        );
    }
}

const INVALID: &str = "prop_invalid_requests_are_rejected_without_effect";
const CONCURRENT: &str = "prop_concurrent_programs_match_model";
const FAILURES: &str = "prop_failures_match_model";

/// R2: a target that accepts misaligned requests is caught by the
/// alignment case and the rejection property.
#[test]
fn catches_accepting_misaligned_requests() {
    caught(Fault::AcceptMisaligned, &["rejects_misaligned", INVALID]);
}

/// R2: a target that accepts requests beyond the end of the device is
/// caught by the bounds case and the rejection property.
#[test]
fn catches_accepting_out_of_bounds_requests() {
    caught(
        Fault::AcceptOutOfBounds,
        &["rejects_out_of_bounds", INVALID],
    );
}

/// R3: a target whose reads miss acknowledged, unflushed operations breaks
/// "a read received after an operation was acknowledged observes it".
#[test]
fn catches_stale_reads() {
    caught(Fault::StaleReads, &["read_your_writes", CONCURRENT]);
}

/// R4: a target that writes only the first block of a multi-block write
/// is caught by read-your-writes.
#[test]
fn catches_short_writes() {
    caught(Fault::ShortWrite, &["read_your_writes", CONCURRENT]);
}

/// R5: a target whose write-zeroes writes nothing is caught.
#[test]
fn catches_write_zeroes_that_does_nothing() {
    caught(
        Fault::WriteZeroesDoesNothing,
        &["write_zeroes_hides_the_base", CONCURRENT],
    );
}

/// R5: a target that stores write-zeroes as an absence lets the base show
/// through on a clone. Only a case with a non-empty base can see it.
#[test]
fn catches_write_zeroes_that_shows_the_base() {
    caught(
        Fault::WriteZeroesShowsBase,
        &["write_zeroes_hides_the_base", CONCURRENT],
    );
}

/// R6: a target on which a discarded block reads as data that was never
/// there is caught: only the old content, zeros or the base are allowed.
#[test]
fn catches_discard_that_corrupts() {
    caught(
        Fault::DiscardCorrupts,
        &["discard_leaves_old_zeros_or_base", CONCURRENT],
    );
}

/// R6, R8: a target that applies a discard late, over data written after
/// it, is caught. This is one of the two ways the first draft of the
/// contract allowed a daemon to corrupt a pool.
#[test]
fn catches_discard_applied_to_later_writes() {
    caught(
        Fault::DiscardAppliedLate,
        &["discard_never_hits_later_writes", CONCURRENT],
    );
}

/// R7: a target whose flush makes nothing durable loses flushed
/// operations at a power cut.
#[test]
fn catches_flush_that_does_nothing() {
    caught(
        Fault::FlushDoesNothing,
        &["flushed_operations_survive_power_cut", FAILURES],
    );
}

/// R7: the group-commit bug, a flush that joins a sync begun before it
/// arrived, is caught by the case named for it.
#[test]
fn catches_the_group_commit_bug() {
    caught(
        Fault::FlushJoinsOldSync,
        &["flush_covers_what_was_acknowledged_before_it", FAILURES],
    );
}

/// R7, R9: a flush that covers only what the current instance
/// acknowledged loses what the earlier instance did.
#[test]
fn catches_flush_that_forgets_the_earlier_instance() {
    caught(
        Fault::FlushForgetsEarlierInstance,
        &["flush_covers_an_earlier_instance", FAILURES],
    );
}

/// R8: a target that applies a multi-block write in two parts lets two
/// overlapping writes each win some shared blocks, which no order of
/// whole operations produces.
#[test]
fn catches_operations_not_applied_whole() {
    caught(
        Fault::SplitWrites,
        &["overlapping_operations_in_flight_apply_whole", CONCURRENT],
    );
}

/// R9: a target that acknowledges from memory loses acknowledged
/// operations when its daemon is killed. This is the other way the first
/// draft of the contract allowed a daemon to corrupt a pool: treating
/// daemon loss as a power cut.
#[test]
fn catches_acknowledging_from_memory() {
    caught(
        Fault::KillLosesUnflushed,
        &["kill_keeps_acknowledged_operations", FAILURES],
    );
}

/// R9: a target that fails requests reissued after a daemon loss is
/// caught.
#[test]
fn catches_rejecting_reissued_requests() {
    caught(
        Fault::RejectReissued,
        &["reissue_after_kill_is_idempotent", FAILURES],
    );
}

/// R10: a target that keeps part of an operation across a power cut is
/// caught by the cut-at-every-point case.
#[test]
fn catches_torn_operations() {
    caught(
        Fault::TearOnCut,
        &["power_cut_at_every_point_leaves_a_prefix", FAILURES],
    );
}

/// R10: a target that loses an operation and keeps later ones does not
/// leave a prefix.
#[test]
fn catches_a_cut_that_is_not_a_prefix() {
    caught(
        Fault::CutDropsMiddle,
        &["power_cut_at_every_point_leaves_a_prefix", FAILURES],
    );
}

/// R10: a target whose recovery does not make what it exposes durable
/// loses it at a second power cut.
#[test]
fn catches_recovery_that_is_not_durable() {
    caught(
        Fault::RecoveryNotDurable,
        &["recovered_state_is_durable", FAILURES],
    );
}

/// R11: a failed volume that still serves reads is caught.
#[test]
fn catches_a_failed_volume_that_serves_reads() {
    caught(
        Fault::FailedServesReads,
        &["failed_volume_fails_every_request", FAILURES],
    );
}

/// R11: a daemon restart that forgets the volume has failed is caught.
#[test]
fn catches_a_failure_forgotten_on_restart() {
    caught(
        Fault::FailureForgottenOnKill,
        &["failed_volume_fails_every_request", FAILURES],
    );
}

/// R11: a failed volume that comes back without something a flush had
/// covered is caught, by the case that opens it again and by the random
/// programs.
#[test]
fn catches_a_reopened_volume_that_lost_what_was_flushed() {
    caught(
        Fault::ReopenLosesFlushed,
        &["reopened_failed_volume_holds_what_was_flushed", FAILURES],
    );
}

/// R12: a read-only target that accepts writes is caught.
#[test]
fn catches_a_read_only_target_that_writes() {
    caught(
        Fault::ReadOnlyAcceptsWrites,
        &["read_only_target_refuses_mutations"],
    );
}

/// R4: a target whose writes also damage a block far outside their range
/// is caught, because the suite reads the whole device back and not only
/// the blocks it wrote. (Found in review; B3.)
#[test]
fn catches_a_write_that_strays_outside_its_range() {
    caught(
        Fault::StrayWrite,
        &["read_your_writes", CONCURRENT, FAILURES],
    );
}

/// The trait's promise that buffers make a round trip: a target that
/// keeps the buffer of a write is caught.
#[test]
fn catches_a_write_buffer_that_does_not_come_back() {
    caught(Fault::DropWriteBuffer, &["read_your_writes", CONCURRENT]);
}
