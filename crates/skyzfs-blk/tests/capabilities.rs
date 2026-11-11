//! Tests of the suite itself: what it does with a harness that cannot do
//! everything ([`Harness::capabilities`]).
//!
//! `Limited` is `MemHarness` without some capabilities. It records
//! every time the suite asks it for something it said it cannot do, and
//! every invalid request that reaches its target when it said none
//! would. Both must never happen.
//!
//! Rule numbers refer to the crate documentation of `skyzfs-blk`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use skyzfs_blk::conformance::{
    cases, run_all, run_case, Capabilities, Capability, Config, Harness, HarnessError, Report,
    Spec, Status,
};
use skyzfs_blk::mem::{Fault, MemConfig, MemHarness, MemTarget};
use skyzfs_blk::{Base, BlockTarget, Completion, Geometry, Request, SubmitError};

/// `MemHarness` without the capabilities in `lacks`.
struct Limited {
    inner: MemHarness,
    lacks: Vec<Capability>,
    /// What the suite asked for although the harness said it cannot.
    overstepped: Rc<Cell<Option<Capability>>>,
    /// How many devices the suite has asked for.
    created: u32,
}

/// The target of [`Limited`]: it notes an invalid request.
struct Watched {
    inner: MemTarget,
    no_invalid: bool,
    overstepped: Rc<Cell<Option<Capability>>>,
}

impl BlockTarget for Watched {
    fn geometry(&self) -> Geometry {
        self.inner.geometry()
    }

    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
        if self.no_invalid && self.inner.geometry().validate(&request.op).is_err() {
            self.overstepped.set(Some(Capability::InvalidRequests));
        }
        self.inner.submit(request)
    }

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        self.inner.poll(out)
    }

    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize {
        self.inner.wait(out, timeout)
    }
}

impl Limited {
    fn new(lacks: &[Capability], fault: Option<Fault>) -> Self {
        let config = MemConfig {
            queue_depth: 16,
            fault,
            ..MemConfig::default()
        };
        Limited {
            inner: MemHarness::new(config),
            lacks: lacks.to_vec(),
            overstepped: Rc::new(Cell::new(None)),
            created: 0,
        }
    }

    fn lacks(&self, capability: Capability) -> bool {
        self.lacks.contains(&capability)
    }

    /// The suite asked for `capability`: an error if the harness lacks
    /// it, and a note of it.
    fn asked(&self, capability: Capability) -> Result<(), HarnessError> {
        if self.lacks(capability) {
            self.overstepped.set(Some(capability));
            return Err(HarnessError(format!("cannot: {capability:?}")));
        }
        Ok(())
    }

    fn watch(&self, inner: MemTarget) -> Watched {
        Watched {
            inner,
            no_invalid: self.lacks(Capability::InvalidRequests),
            overstepped: self.overstepped.clone(),
        }
    }
}

impl Harness for Limited {
    type Target = Watched;

    fn capabilities(&self) -> Capabilities {
        self.lacks
            .iter()
            .fold(Capabilities::ALL, |set, c| set.without(*c))
    }

    fn create(&mut self, spec: &Spec) -> Result<Watched, HarnessError> {
        if spec.base != Base::Empty {
            self.asked(Capability::Base)?;
        }
        if spec.read_only {
            self.asked(Capability::ReadOnly)?;
        }
        self.created += 1;
        let inner = self.inner.create(spec)?;
        Ok(self.watch(inner))
    }

    fn kill(&mut self, target: Watched, seed: u64) -> Result<Watched, HarnessError> {
        let inner = self.inner.kill(target.inner, seed)?;
        Ok(self.watch(inner))
    }

    fn cut_points(&mut self, target: &Watched) -> u64 {
        if self.lacks(Capability::PowerCut) {
            0
        } else {
            self.inner.cut_points(&target.inner)
        }
    }

    fn power_cut(&mut self, target: Watched, keep: u64) -> Result<Watched, HarnessError> {
        self.asked(Capability::PowerCut)?;
        let inner = self.inner.power_cut(target.inner, keep)?;
        Ok(self.watch(inner))
    }

    fn fail_log(&mut self, target: &mut Watched) -> Result<(), HarnessError> {
        self.asked(Capability::FailLog)?;
        self.inner.fail_log(&mut target.inner)
    }

    fn reopen_failed(&mut self, target: Watched) -> Result<Watched, HarnessError> {
        self.asked(Capability::ReopenFailed)?;
        let inner = self.inner.reopen_failed(target.inner)?;
        Ok(self.watch(inner))
    }
}

fn suite() -> Config {
    Config {
        cases: 48,
        ..Config::default()
    }
}

fn run(lacks: &[Capability], config: &Config) -> (Report, Option<Capability>) {
    let mut harness = Limited::new(lacks, None);
    let report = run_all(&mut harness, config);
    (report, harness.overstepped.get())
}

const PARAMETERS: &str = "parameters_are_as_the_contract_fixes_them";
const FRESH: &str = "fresh_device_reads_as_its_base";
const MISALIGNED: &str = "rejects_misaligned";
const OUT_OF_BOUNDS: &str = "rejects_out_of_bounds";
const OVERSIZE: &str = "rejects_empty_and_oversize";
const READ_YOUR_WRITES: &str = "read_your_writes";
const WRITE_ZEROES: &str = "write_zeroes_hides_the_base";
const DISCARD_LEAVES: &str = "discard_leaves_old_zeros_or_base";
const DISCARD_LATER: &str = "discard_never_hits_later_writes";
const FLUSHED: &str = "flushed_operations_survive_power_cut";
const FLUSH_COVERS: &str = "flush_covers_what_was_acknowledged_before_it";
const FLUSH_EARLIER: &str = "flush_covers_an_earlier_instance";
const EVERY_POINT: &str = "power_cut_at_every_point_leaves_a_prefix";
const RECOVERED: &str = "recovered_state_is_durable";
const FIRST_REQUEST: &str = "first_request_after_power_cut_may_be_a_write";
const OVERLAPPING: &str = "overlapping_operations_in_flight_apply_whole";
const KILL_KEEPS: &str = "kill_keeps_acknowledged_operations";
const REISSUE: &str = "reissue_after_kill_is_idempotent";
const FAILED_VOLUME: &str = "failed_volume_fails_every_request";
const REOPENED: &str = "reopened_failed_volume_holds_what_was_flushed";
const RESTART_FAILED: &str = "restart_after_the_log_failed_recovers_nothing";
const READ_ONLY: &str = "read_only_target_refuses_mutations";
const PROP_INVALID: &str = "prop_invalid_requests_are_rejected_without_effect";
const PROP_CONCURRENT: &str = "prop_concurrent_programs_match_model";
const PROP_FAILURES: &str = "prop_failures_match_model";

/// What the suite does without one capability: the cases whose rule
/// needs it, which are not exercised, and the cases that run in a
/// weaker form.
struct Without {
    capability: Capability,
    not_exercised: &'static [&'static str],
    weaker: &'static [&'static str],
    /// The faults of `MemTarget` that no remaining case notices.
    invisible: &'static [Fault],
}

/// The table: written down here, not derived from the suite, so that a
/// change of what a case needs shows as a change of this file.
const TABLE: [Without; 6] = [
    Without {
        capability: Capability::PowerCut,
        not_exercised: &[
            FLUSHED,
            FLUSH_COVERS,
            FLUSH_EARLIER,
            EVERY_POINT,
            RECOVERED,
            FIRST_REQUEST,
        ],
        weaker: &[
            PARAMETERS,
            WRITE_ZEROES,
            DISCARD_LEAVES,
            DISCARD_LATER,
            OVERLAPPING,
            REISSUE,
            REOPENED,
            RESTART_FAILED,
            PROP_CONCURRENT,
            PROP_FAILURES,
        ],
        // The other durability faults are still seen, through a log
        // failure and a reopen, which also shows what was durable.
        invisible: &[Fault::CutDropsMiddle],
    },
    Without {
        capability: Capability::Base,
        not_exercised: &[WRITE_ZEROES],
        weaker: &[
            PARAMETERS,
            FRESH,
            READ_YOUR_WRITES,
            DISCARD_LEAVES,
            DISCARD_LATER,
            FLUSHED,
            EVERY_POINT,
            FIRST_REQUEST,
            KILL_KEEPS,
            REOPENED,
            RESTART_FAILED,
            READ_ONLY,
            PROP_INVALID,
            PROP_CONCURRENT,
            PROP_FAILURES,
        ],
        invisible: &[Fault::WriteZeroesShowsBase, Fault::StrayWrite],
    },
    Without {
        capability: Capability::ReadOnly,
        not_exercised: &[READ_ONLY],
        weaker: &[PARAMETERS],
        invisible: &[Fault::ReadOnlyAcceptsWrites],
    },
    Without {
        capability: Capability::FailLog,
        not_exercised: &[FAILED_VOLUME, REOPENED, RESTART_FAILED],
        weaker: &[PROP_FAILURES],
        invisible: &[
            Fault::FailedServesReads,
            Fault::FlushSucceedsOnFailedLog,
            Fault::FailureForgottenOnKill,
            Fault::ReopenLosesFlushed,
        ],
    },
    Without {
        capability: Capability::InvalidRequests,
        not_exercised: &[MISALIGNED, PROP_INVALID],
        weaker: &[
            OUT_OF_BOUNDS,
            OVERSIZE,
            FAILED_VOLUME,
            PROP_CONCURRENT,
            PROP_FAILURES,
        ],
        invisible: &[Fault::AcceptMisaligned, Fault::AcceptOutOfBounds],
    },
    Without {
        capability: Capability::ReopenFailed,
        not_exercised: &[REOPENED, RESTART_FAILED],
        weaker: &[PROP_FAILURES],
        invisible: &[Fault::ReopenLosesFlushed],
    },
];

/// With every capability missing, as for a rig that can only create an
/// empty writable device and lose the daemon: the faults no remaining
/// case notices. More than the sum of the rows above: without a power
/// cut and without a reopen nothing shows what was durable.
const INVISIBLE_WITH_NOTHING: &[Fault] = &[
    Fault::AcceptMisaligned,
    Fault::AcceptOutOfBounds,
    Fault::WriteZeroesShowsBase,
    Fault::FlushDoesNothing,
    Fault::FlushJoinsOldSync,
    Fault::FlushForgetsEarlierInstance,
    Fault::TearOnCut,
    Fault::CutDropsMiddle,
    Fault::RecoveryNotDurable,
    Fault::FailedServesReads,
    Fault::FlushSucceedsOnFailedLog,
    Fault::FailureForgottenOnKill,
    Fault::ReopenLosesFlushed,
    Fault::ReadOnlyAcceptsWrites,
    Fault::StrayWrite,
];

const FAULTS: [Fault; 24] = [
    Fault::AcceptMisaligned,
    Fault::AcceptOutOfBounds,
    Fault::StaleReads,
    Fault::ShortWrite,
    Fault::WriteZeroesDoesNothing,
    Fault::WriteZeroesShowsBase,
    Fault::DiscardCorrupts,
    Fault::DiscardAppliedLate,
    Fault::FlushDoesNothing,
    Fault::FlushJoinsOldSync,
    Fault::FlushForgetsEarlierInstance,
    Fault::SplitWrites,
    Fault::KillLosesUnflushed,
    Fault::RejectReissued,
    Fault::TearOnCut,
    Fault::CutDropsMiddle,
    Fault::RecoveryNotDurable,
    Fault::FailedServesReads,
    Fault::FlushSucceedsOnFailedLog,
    Fault::FailureForgottenOnKill,
    Fault::ReopenLosesFlushed,
    Fault::ReadOnlyAcceptsWrites,
    Fault::StrayWrite,
    Fault::DropWriteBuffer,
];

fn names(report: &Report, weaker: bool) -> Vec<&'static str> {
    if weaker {
        report.weaker().into_iter().map(|(name, _)| name).collect()
    } else {
        report.not_exercised()
    }
}

/// The names of `TABLE`'s lists for the capabilities in `lacks`, in the
/// suite's order.
fn expected(lacks: &[Capability], weaker: bool) -> Vec<&'static str> {
    let rows = || TABLE.iter().filter(|row| lacks.contains(&row.capability));
    let left_out = |name: &str| rows().any(|row| row.not_exercised.contains(&name));
    cases::<Limited>()
        .iter()
        .map(|case| case.name)
        .filter(|name| {
            if weaker {
                !left_out(name) && rows().any(|row| row.weaker.contains(name))
            } else {
                left_out(name)
            }
        })
        .collect()
}

/// Not a contract rule: the suite honours what a harness cannot do. On
/// a correct target, without each capability in turn and without all of
/// them: no case fails; the suite never asks the harness for what it
/// lacks, and no invalid request reaches a target whose harness cannot
/// deliver one; exactly the cases whose rule needs the capability are
/// not exercised, each with the capability as the reason; and the cases
/// that only used it to check more are reported as having run in a
/// weaker form, with the capability named. With every capability,
/// nothing is left out.
#[test]
fn a_missing_capability_leaves_out_exactly_the_cases_that_need_it() {
    let mut sets: Vec<Vec<Capability>> = Capability::ALL.iter().map(|c| vec![*c]).collect();
    sets.push(Capability::ALL.to_vec());
    sets.push(Vec::new());
    for lacks in &sets {
        let (report, overstepped) = run(lacks, &suite());
        assert!(report.passed(), "without {lacks:?}:\n{report}");
        assert_eq!(
            overstepped, None,
            "without {lacks:?} the suite asked for it"
        );
        assert_eq!(
            names(&report, false),
            expected(lacks, false),
            "not exercised without {lacks:?}"
        );
        assert_eq!(
            names(&report, true),
            expected(lacks, true),
            "weaker without {lacks:?}"
        );
        for (name, why) in report.unexercised() {
            assert!(
                lacks.iter().any(|c| c.missing() == why),
                "{name} without {lacks:?}: {why}"
            );
        }
        for (name, what) in report.weaker() {
            assert!(
                lacks.iter().any(|c| what.contains(c.name())),
                "{name} without {lacks:?}: {what}"
            );
        }
    }
    // Each list of the table names cases of the suite.
    let all: Vec<&str> = cases::<Limited>().iter().map(|c| c.name).collect();
    for row in &TABLE {
        for name in row.not_exercised.iter().chain(row.weaker) {
            assert!(all.contains(name), "{name} is no case");
        }
    }
}

/// Not a contract rule: a case whose rule needs a capability the
/// harness lacks does not run at all, not even in part: the harness is
/// not asked for a device.
#[test]
fn a_case_that_is_not_exercised_runs_nothing() {
    for row in &TABLE {
        for name in row.not_exercised {
            let mut harness = Limited::new(&[row.capability], None);
            let status = run_case(&mut harness, &suite(), name);
            assert!(
                matches!(status, Some(Ok(Status::NotExercised(why))) if why == row.capability.missing()),
                "{name} without {:?}: {status:?}",
                row.capability
            );
            assert_eq!(harness.created, 0, "{name} without {:?}", row.capability);
        }
    }
}

/// Not a contract rule: the report of a random-program case that ran in
/// a weaker form says which kinds of step were left out and how many
/// programs ran.
#[test]
fn the_random_programs_say_what_they_left_out() {
    let lacks = [Capability::PowerCut, Capability::FailLog];
    let config = Config {
        cases: 8,
        ..Config::default()
    };
    let mut harness = Limited::new(&lacks, None);
    let report = run_all(&mut harness, &config);
    let weaker = report.weaker();
    let Some((_, what)) = weaker.iter().find(|(name, _)| *name == PROP_FAILURES) else {
        panic!("{PROP_FAILURES} did not run in a weaker form:\n{report}");
    };
    for part in [
        "no program has a power cut",
        "no program has a log failure",
        "8 programs ran",
    ] {
        assert!(what.contains(part), "{what}");
    }
    assert!(format!("{report}").contains("WEAK  prop_failures_match_model"));
    // The outcome is the status the case's own entry point gives.
    let case = cases::<Limited>()
        .into_iter()
        .find(|case| case.name == PROP_FAILURES)
        .unwrap();
    assert!(matches!(
        case.run_on(&mut harness, &config),
        Ok(Status::Weaker(_))
    ));
}

/// Whether any case notices `fault` on a harness that lacks `lacks`.
fn noticed(lacks: &[Capability], fault: Fault) -> bool {
    let mut harness = Limited::new(lacks, Some(fault));
    let config = suite();
    let noticed = cases::<Limited>()
        .iter()
        .any(|case| case.run_on(&mut harness, &config).is_err());
    assert_eq!(
        harness.overstepped.get(),
        None,
        "{fault:?} without {lacks:?}"
    );
    noticed
}

/// Every fault outside `invisible` is caught without `lacks`, and every
/// fault in it is not: the list is exact.
fn invisible_are_exactly(lacks: &[Capability], invisible: &[Fault]) {
    for fault in FAULTS {
        assert_eq!(
            !noticed(lacks, fault),
            invisible.contains(&fault),
            "{fault:?} without {lacks:?}"
        );
    }
}

/// Not a contract rule: what running without a capability costs. A
/// fault of `MemTarget` that the remaining cases can see is still
/// caught, and the faults that become invisible are exactly those the
/// table lists. One test per capability, so that they run side by side.
#[test]
fn without_a_power_cut_the_other_faults_are_still_caught() {
    invisible_are_exactly(&[TABLE[0].capability], TABLE[0].invisible);
}

/// As above, without a base that is not empty.
#[test]
fn without_a_base_the_other_faults_are_still_caught() {
    invisible_are_exactly(&[TABLE[1].capability], TABLE[1].invisible);
}

/// As above, without a read-only device.
#[test]
fn without_a_read_only_device_the_other_faults_are_still_caught() {
    invisible_are_exactly(&[TABLE[2].capability], TABLE[2].invisible);
}

/// As above, without a log that fails on demand.
#[test]
fn without_a_log_failure_the_other_faults_are_still_caught() {
    invisible_are_exactly(&[TABLE[3].capability], TABLE[3].invisible);
}

/// As above, without invalid requests.
#[test]
fn without_invalid_requests_the_other_faults_are_still_caught() {
    invisible_are_exactly(&[TABLE[4].capability], TABLE[4].invisible);
}

/// As above, without reopening a failed volume.
#[test]
fn without_a_reopen_the_other_faults_are_still_caught() {
    invisible_are_exactly(&[TABLE[5].capability], TABLE[5].invisible);
}

/// As above, with every capability missing: the nine faults that a rig
/// which can only lose the daemon still catches, and the fifteen it
/// cannot see.
#[test]
fn with_no_capability_the_faults_a_daemon_loss_shows_are_still_caught() {
    invisible_are_exactly(&Capability::ALL, INVISIBLE_WITH_NOTHING);
}

/// Not a contract rule: a defect of the suite itself. A suite whose
/// cases run although the harness lacks a capability their rule needs
/// (`Config::run_without_capability`) is noticed: exactly those cases
/// fail, because they ask for what the harness cannot do, where the
/// suite as it is reports them as not exercised.
#[test]
fn a_suite_that_runs_a_case_without_its_capability_is_caught() {
    for row in &TABLE {
        let lacks = [row.capability];
        let config = Config {
            run_without_capability: true,
            ..suite()
        };
        let (report, _) = run(&lacks, &config);
        assert_eq!(
            report.failed(),
            expected(&lacks, false),
            "without {lacks:?}:\n{report}"
        );
        assert!(report.not_exercised().is_empty(), "{report}");
    }
}
