//! Tests of the suite itself: each deliberately broken `MemTarget` must
//! fail the conformance suite, and must fail the case written for the rule
//! it breaks. A suite that passes a mutant verifies nothing.
//!
//! Rule numbers refer to the crate documentation of `skyzfs-blk`.

use skyzfs_blk::conformance::{run_all, Config};
use skyzfs_blk::mem::{DiscardPolicy, Fault, MemConfig, MemHarness, Schedule};

fn mutant(fault: Fault) -> MemConfig {
    MemConfig {
        blocks: 64,
        max_io_blocks: 8,
        queue_depth: 16,
        fault: Some(fault),
        ..MemConfig::default()
    }
}

/// Run the whole suite against `config` and require that it fails, and
/// that every case in `must_fail` is among the failures.
fn caught(config: MemConfig, must_fail: &[&str]) {
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
            "{name} did not catch {:?}; failed cases: {failed:?}",
            config.fault
        );
    }
}

/// R5: a target whose flush makes nothing durable loses flushed writes at
/// a power cut. Caught by the flush durability case and the crash
/// property.
#[test]
fn catches_flush_that_does_nothing() {
    caught(
        mutant(Fault::FlushDoesNothing),
        &[
            "flushed_writes_survive_power_cut",
            "prop_power_cuts_and_restarts_match_model",
        ],
    );
}

/// R10: a target that leaves half a block of a lost write behind is caught
/// as torn.
#[test]
fn catches_torn_blocks() {
    caught(
        mutant(Fault::TearOnCrash),
        &[
            "unflushed_writes_are_old_or_new_never_torn",
            "prop_power_cuts_and_restarts_match_model",
        ],
    );
}

/// R2: a target that accepts misaligned requests is caught by the
/// alignment case and the rejection property.
#[test]
fn catches_accepting_misaligned_requests() {
    caught(
        mutant(Fault::AcceptMisaligned),
        &[
            "rejects_misaligned",
            "prop_invalid_requests_are_rejected_without_effect",
        ],
    );
}

/// R2: a target that accepts requests beyond the end of the device is
/// caught by the bounds case and the rejection property.
#[test]
fn catches_accepting_out_of_bounds_requests() {
    caught(
        mutant(Fault::AcceptOutOfBounds),
        &[
            "rejects_out_of_bounds",
            "prop_invalid_requests_are_rejected_without_effect",
        ],
    );
}

/// R3, R4: a target whose reads miss unflushed writes breaks
/// read-your-writes.
#[test]
fn catches_stale_reads() {
    caught(
        mutant(Fault::StaleReads),
        &["read_your_writes", "prop_concurrent_programs_match_model"],
    );
}

/// R8: a target that applies overlapping writes in submission order while
/// acknowledging them in another order is caught by the ack-order case.
#[test]
fn catches_writes_applied_out_of_ack_order() {
    caught(
        mutant(Fault::ApplyAtSubmit),
        &[
            "overlapping_writes_apply_in_ack_order",
            "prop_concurrent_programs_match_model",
        ],
    );
}

/// R8: when completions come in submission order, submission order is
/// acknowledgement order, so the same fault no longer breaks the rule for
/// writes among themselves and the ack-order case must pass: it judges by
/// the order the target delivers, not by one it assumes. The fault still
/// misorders a write against a write-zeroes or discard submitted before
/// it, which only the properties try.
#[test]
fn apply_at_submit_under_fifo_is_caught_only_by_the_properties() {
    let config = MemConfig {
        schedule: Schedule::Fifo,
        discard: DiscardPolicy::Zero,
        ..mutant(Fault::ApplyAtSubmit)
    };
    let suite = Config {
        cases: 256,
        ..Config::default()
    };
    let report = run_all(&mut MemHarness::new(config), &suite);
    assert_eq!(
        report.failed(),
        [
            "prop_concurrent_programs_match_model",
            "prop_power_cuts_and_restarts_match_model"
        ],
        "{report}"
    );
}

/// R3, R6: a target on which a discarded block reads as garbage is caught:
/// only the last write or zeros are allowed.
#[test]
fn catches_discard_that_corrupts() {
    caught(
        mutant(Fault::DiscardCorrupts),
        &[
            "discard_reads_old_or_zeros",
            "prop_concurrent_programs_match_model",
        ],
    );
}

/// R7: a target whose write-zeroes writes nothing is caught.
#[test]
fn catches_write_zeroes_that_does_nothing() {
    caught(
        mutant(Fault::WriteZeroesDoesNothing),
        &[
            "write_zeroes_reads_zeros_and_is_durable",
            "prop_concurrent_programs_match_model",
        ],
    );
}

/// R9: a target that loses acknowledged writes when its daemon restarts is
/// caught (open reading 5).
#[test]
fn catches_restart_that_loses_acknowledged_writes() {
    caught(
        mutant(Fault::RestartLosesWrites),
        &[
            "restart_keeps_acknowledged_writes",
            "prop_power_cuts_and_restarts_match_model",
        ],
    );
}

/// R9: a target that fails requests reissued after a restart is caught.
#[test]
fn catches_rejecting_reissued_requests() {
    caught(
        mutant(Fault::RejectReissued),
        &[
            "reissue_after_restart_is_idempotent",
            "prop_power_cuts_and_restarts_match_model",
        ],
    );
}

/// R4: a target that writes only the first block of a multi-block write is
/// caught by read-your-writes.
#[test]
fn catches_short_writes() {
    caught(
        mutant(Fault::ShortWrite),
        &["read_your_writes", "prop_concurrent_programs_match_model"],
    );
}
