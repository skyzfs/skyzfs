//! The conformance suite: rules R1 to R12 of the [crate documentation](crate)
//! run against any [`BlockTarget`].
//!
//! Passing it is the entry condition for every transport and for the local
//! store (design §4.1).
//!
//! # Using it
//!
//! Implement [`Harness`] for the thing under test: how to create a device
//! on a given base, how to lose and restart its daemon, how to cut its
//! power at a chosen point of its log, and how to make its log fail. Then:
//!
//! ```
//! use skyzfs_blk::conformance::{run_all, Config};
//! use skyzfs_blk::mem::{MemConfig, MemHarness};
//!
//! let mut harness = MemHarness::new(MemConfig::default());
//! let report = run_all(&mut harness, &Config { cases: 8, ..Config::default() });
//! assert!(report.passed(), "{report}");
//! ```
//!
//! [`cases`] lists the cases with the rule each verifies, for a rig that
//! wants one test per case.
//!
//! # How it judges
//!
//! Every request the suite submits and every completion it receives goes
//! through [`Model`]. A case fails when the model reports a
//! [`Violation`](crate::model::Violation), when a request does not
//! complete within [`Config::timeout`], or when `submit` refuses a request
//! the contract obliges the target to take. The property cases run random
//! programs of requests, reaps, daemon losses and power cuts with
//! proptest, and report the shrunk program on failure.
//!
//! The two failures are kept apart (design §4.1): after [`Harness::kill`]
//! every acknowledged operation must be readable; after
//! [`Harness::power_cut`] the device must hold what a disk with a volatile
//! write cache may leave (rule R10): everything flushed, and of what was
//! written since, any version for each sector. After every power cut the
//! suite reads back each block an unflushed operation touched before it
//! does anything else, which is what lets the model check what
//! survived. The harness author does not have to arrange this.
//!
//! The suite works in a small window of blocks so that requests overlap,
//! but it reads back the whole device (a spread of it, above 4 MiB) at
//! the end of every case and after every power cut, so damage to a block
//! no request named is caught. The random programs place their window at
//! the start of the device, at its end, and across each boundary the
//! harness names ([`Harness::boundaries`]).
//!
//! A case whose rule the target or harness cannot exercise (one request
//! at a time; no cut point inside the log; a capability the harness
//! lacks) is reported as [`Status::NotExercised`], not as passed.
//!
//! # A harness that cannot do everything
//!
//! A rig that drives a real block device cannot cut the power at a
//! chosen record, or deliver a misaligned request past the kernel. It
//! says what it cannot do with [`Harness::capabilities`], and the suite
//! honours it, case by case:
//!
//! - a case whose rule needs a missing capability is **not exercised**:
//!   it does not run, not even in part, and is reported with the
//!   capability as the reason;
//! - a case that used the capability only to check more than its rule
//!   needs runs in a **weaker form**, and the report says what was left
//!   out ([`Status::Weaker`]);
//! - the random programs leave out the kinds of step the harness cannot
//!   do, and say which.
//!
//! The suite never calls a hook the harness has said it lacks. For a
//! harness that can do everything nothing changes, and a caller that
//! expects the whole suite asserts that [`Report::not_exercised`] and
//! [`Report::weaker`] are both empty.
//!
//! What each case does without a capability (P a power cut, B a base
//! that is not empty, R a read-only device, F a log that fails on
//! demand, I invalid requests, O reopening a failed volume). "Needs"
//! means not exercised without it; "weaker without" is in
//! [`Case::weaker`], with what is left out:
//!
//! | Case | Needs | Weaker without |
//! |---|---|---|
//! | `parameters_are_as_the_contract_fixes_them` | | P, R, B |
//! | `fresh_device_reads_as_its_base` | | B |
//! | `rejects_misaligned` | I | |
//! | `rejects_out_of_bounds` | | I |
//! | `rejects_empty_and_oversize` | | I |
//! | `read_your_writes` | | B |
//! | `write_zeroes_hides_the_base` | B | P |
//! | `discard_leaves_old_zeros_or_base` | | B, P |
//! | `discard_never_hits_later_writes` | | B, P |
//! | `flushed_operations_survive_power_cut` | P | B |
//! | `flush_covers_what_was_acknowledged_before_it` | P | |
//! | `flush_covers_an_earlier_instance` | P | |
//! | `power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may` | P | B |
//! | `recovered_state_is_durable` | P | |
//! | `first_request_after_power_cut_may_be_a_write` | P | B |
//! | `queue_depth_requests_complete_in_any_order` | | |
//! | `overlapping_operations_in_flight_apply_whole` | | P |
//! | `kill_keeps_acknowledged_operations` | | B |
//! | `reissue_after_kill_is_idempotent` | | P |
//! | `failed_volume_fails_every_request` | F | I |
//! | `reopened_failed_volume_holds_what_was_flushed` | F, O | B, P |
//! | `restart_after_the_log_failed_recovers_nothing` | F, O | B, P |
//! | `read_only_target_refuses_mutations` | R | B |
//! | `prop_invalid_requests_are_rejected_without_effect` | I | B |
//! | `prop_concurrent_programs_match_model` | | B, I, P |
//! | `prop_failures_match_model` | | P, F, O, B, I |
//!
//! What this costs is measured against the faulty targets of
//! [`mem`](crate::mem) in `tests/capabilities.rs`, in three columns for
//! each missing capability: the faults a remaining **fixed case**
//! notices, which is asserted and does not depend on a seed; the faults
//! that only **random programs** noticed, at some seeds and not at
//! others, which is reported and promised by nobody; and the faults
//! that are **invisible**, to the fixed cases and to the random
//! programs at three seeds.
//!
//! | Without | No fixed case notices | Of those, invisible at three seeds |
//! |---|---|---|
//! | a power cut | `FlushDoesNothing`, `FlushJoinsOldSync`, `FlushForgetsEarlierInstance`, `RecoveryNotDurable` | none: each was noticed by random programs at one to three of the seeds, through a log failure and a reopen |
//! | a base | `WriteZeroesShowsBase`, `ReopenLosesFlushed`, `StrayWrite` | `WriteZeroesShowsBase`, `StrayWrite` |
//! | a read-only device | `ReadOnlyAcceptsWrites` | the same |
//! | a log failure | `FailedServesReads`, `FlushSucceedsOnFailedLog`, `FailureForgottenOnKill`, `ReopenLosesFlushed` | the same |
//! | invalid requests | `AcceptMisaligned`, `AcceptOutOfBounds` | the same |
//! | a reopen | `ReopenLosesFlushed` | the same |
//! | all six | the faults above and `TearOnCut`: 15 of 24 | the same 15 |
//!
//! So a rig without a power cut must not count on the suite for the
//! faults of durability: no fixed case shows what was durable, and a
//! random program does only if it happens to fail the log and reopen.
//! A rig with none of the capabilities sees none of them at all. On an
//! empty volume, damage that zeroes a block nothing has written is not
//! seen either (`StrayWrite`).
//!
//! The suite assumes a fault-free environment apart from the faults it
//! injects: an I/O error on a valid request fails the case.
//!
//! # Limits
//!
//! - The random programs keep at most four writes, write-zeroes and
//!   discards in flight together (reads and flushes up to 16), because the
//!   model's cost grows with the number of overlapping operations whose
//!   order is still open.
//! - One queue. Ordering across several queues of one target is not
//!   tested.
//! - A failed volume fails every read, so what it holds can be looked at
//!   only once it is opened again ([`Harness::reopen_failed`]). Where a
//!   rig cannot do that ([`Capability::ReopenFailed`]), the suite cannot
//!   tell whether a flush that was
//!   in flight when the log failed, and succeeded, had made anything
//!   durable. Where it can, the suite asks what rule R11 promises:
//!   the volume as after a power cut (see
//!   [`Model::reopened`](crate::model::Model::reopened)). The suite does
//!   not cut the power on a failed volume before it is reopened.
//! - Above 4 MiB the read-back is a sample: the windows the suite works
//!   in, the start and the end of the device, and two blocks at each of
//!   63 evenly spaced places. Damage to a block that no request named and
//!   that the sample misses is not seen.
//! - [`Harness::power_cut`] names one of the outcomes the harness can
//!   produce (`keep`); which states a real disk leaves (a sector torn
//!   from its block, a later write kept and an earlier one lost) are
//!   the harness's to produce: the stores' harnesses in `skyzfs-local`
//!   do.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use proptest::prelude::*;
use proptest::strategy::Union;
use proptest::test_runner::{Config as ProptestConfig, RngAlgorithm, TestError, TestRng};
use proptest::test_runner::{TestCaseError, TestRunner};

use crate::model::{InFlight, Model};
use crate::{
    Base, BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId,
    SubmitErrorKind, BLOCK_SIZE, BLOCK_USIZE, MAX_IO_BYTES, SIZE_UNIT,
};

/// Something the suite asks of a harness that not every rig can do. A
/// harness says which of these it has with [`Harness::capabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    /// Cut the power under the device and bring it back
    /// ([`Harness::power_cut`]).
    PowerCut,
    /// Create a device on a base that is not empty ([`Spec::base`]).
    Base,
    /// Create a read-only device ([`Spec::read_only`]).
    ReadOnly,
    /// Make the write log fail on demand ([`Harness::fail_log`]).
    FailLog,
    /// Deliver an invalid request (misaligned, empty, beyond the end,
    /// too large) to the target as the suite issued it.
    InvalidRequests,
    /// Open a failed volume again ([`Harness::reopen_failed`]).
    ReopenFailed,
}

impl Capability {
    /// Every capability.
    pub const ALL: [Capability; 6] = [
        Capability::PowerCut,
        Capability::Base,
        Capability::ReadOnly,
        Capability::FailLog,
        Capability::InvalidRequests,
        Capability::ReopenFailed,
    ];

    /// A short name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            Capability::PowerCut => "a power cut",
            Capability::Base => "a base that is not empty",
            Capability::ReadOnly => "a read-only device",
            Capability::FailLog => "a log that fails on demand",
            Capability::InvalidRequests => "invalid requests",
            Capability::ReopenFailed => "reopening a failed volume",
        }
    }

    /// The reason a case that needs it is reported with when it was not
    /// exercised.
    pub fn missing(self) -> &'static str {
        match self {
            Capability::PowerCut => "the harness cannot cut the power",
            Capability::Base => "the harness cannot create a device on a base that is not empty",
            Capability::ReadOnly => "the harness cannot create a read-only device",
            Capability::FailLog => "the harness cannot make the log fail",
            Capability::InvalidRequests => {
                "the harness cannot deliver an invalid request to the target"
            }
            Capability::ReopenFailed => "the harness cannot open a failed volume again",
        }
    }
}

/// The set of capabilities a harness has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities(u8);

impl Capabilities {
    /// Everything.
    pub const ALL: Capabilities = Capabilities(0x3f);

    /// The same set without `capability`.
    #[must_use]
    pub const fn without(self, capability: Capability) -> Self {
        Capabilities(self.0 & !(1 << capability as u8))
    }

    /// Whether the set holds `capability`.
    pub const fn has(self, capability: Capability) -> bool {
        self.0 & (1 << capability as u8) != 0
    }
}

/// The harness could not do what the suite asked of it. This is a failure
/// of the rig, not a verdict on the target, but it fails the case.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("harness: {0}")]
pub struct HarnessError(pub String);

/// The device a case asks the harness for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    /// What the device holds before anything is written. For
    /// [`Base::Pattern`] the harness builds a volume whose every block is
    /// what [`Base::fill_block`] gives.
    pub base: Base,
    /// Whether the device is opened read-only.
    pub read_only: bool,
    /// For a harness whose target makes choices from a seed (completion
    /// order, what a discard leaves): derive them from this, and from
    /// nothing that changes between calls, so that a failing random
    /// program fails again when it is replayed and shrunk. A real rig may
    /// ignore it.
    pub seed: u64,
}

impl Spec {
    /// A writable device on an empty base.
    pub const EMPTY: Spec = Spec {
        base: Base::Empty,
        read_only: false,
        seed: 0,
    };

    /// A writable device on a non-empty base, as a clone has.
    pub const CLONE: Spec = Spec {
        base: Base::Pattern(0xBA5E),
        read_only: false,
        seed: 0,
    };
}

/// What the suite needs from the thing under test.
///
/// The two fault hooks are separate on purpose: daemon loss and power loss
/// promise different things (rules R9 and R10), and a target that
/// acknowledges from memory must fail under [`Harness::kill`].
///
/// # What a harness cannot do
///
/// A rig that drives a real device through a kernel cannot do all of
/// this, and says so with [`Harness::capabilities`]. The suite then
/// never calls what the harness lacks. A case whose rule needs a missing
/// capability is reported as [`Status::NotExercised`], with the
/// capability named, before it runs anything. A case that used the
/// capability only to check more runs in a weaker form and is reported
/// as [`Status::Weaker`], with what was left out. Neither is a failure,
/// and neither is coverage of what was left out.
///
/// **A harness must not claim a capability it imitates loosely.** A
/// "power cut" that only restarts the daemon, a "base" written through
/// the device under test, a "log failure" that merely stops the daemon:
/// each makes cases pass that have checked nothing. If the rig cannot
/// do exactly what the hook's documentation says, it leaves the
/// capability out.
///
/// What a harness at the level of a kernel block device is expected to
/// answer:
///
/// | Capability | A kernel-level rig | Why |
/// |---|---|---|
/// | [`Capability::PowerCut`] | has it only if it can drop, under the daemon, exactly what the daemon's storage had not been told to make durable, and bring the device back; a process kill is not one | the page cache survives a kill |
/// | [`Capability::Base`] | has it only if it can build the volume's base outside the device under test | a base written through the device is a write, not a base |
/// | [`Capability::ReadOnly`] | has it if it can attach the volume read-only | |
/// | [`Capability::FailLog`] | has it only if it can make the daemon's log fail for good while a client holds the device. The device need not stay: a daemon may delete its device when its log fails, and the answers after that are the kernel's for a deleted device. The rig reports any I/O error from that point on as the failure | a failed volume gives up its attachment |
/// | [`Capability::InvalidRequests`] | does not have it | the kernel refuses or splits such a request and answers with its own error; the target never sees it |
/// | [`Capability::ReopenFailed`] | has it if it has `FailLog`, can repair the storage, and the daemon serves the volume again. A daemon that refuses a volume that failed while a client held it, until the next boot, cannot be reopened by the rig: such a rig does not claim it | |
pub trait Harness {
    /// The target type.
    type Target: BlockTarget;

    /// What this harness can do. There is no default: a harness for a
    /// real device that did not say would claim a power cut it can
    /// only imitate, and every case about flushes would pass having
    /// checked nothing. See the trait's documentation for what leaving
    /// a capability out means, and do not claim one that the rig only
    /// imitates. A harness that can do everything returns
    /// [`Capabilities::ALL`].
    fn capabilities(&self) -> Capabilities;

    /// A fresh device as `spec` describes. Each call must give a device
    /// independent of the earlier ones. It should be small: the suite
    /// works in its first [`Config::window_blocks`] blocks.
    ///
    /// Without [`Capability::Base`] the suite asks only for
    /// [`Base::Empty`], and without [`Capability::ReadOnly`] only for
    /// writable devices.
    fn create(&mut self, spec: &Spec) -> Result<Self::Target, HarnessError>;

    /// Daemon loss (rule R9): kill the daemon under `target` and start a
    /// new instance while the client stays attached. The returned target
    /// has nothing in flight; the suite reissues what was. `seed` is for a
    /// harness that can choose how far the requests in flight had got; a
    /// real rig may ignore it.
    ///
    /// Under a kernel transport the kernel reissues in-flight requests by
    /// itself, and user space cannot drop what the kernel reissues. An
    /// adapter that drives such a device must therefore swallow the
    /// suite's own resubmission of the same ids.
    ///
    /// After [`Harness::fail_log`] on a rig whose daemon gives up its
    /// device when its log fails, there is no daemon left to lose: a
    /// kill does nothing there.
    fn kill(&mut self, target: Self::Target, seed: u64) -> Result<Self::Target, HarnessError>;

    /// What the client can see, after [`Harness::kill`], of a request
    /// that was in flight at it: whether a request that the lost daemon
    /// applied and completed may have its completion reach the client
    /// only after the loss, and not be issued again.
    ///
    /// The default is [`InFlight::Reissued`]: the suite submits every
    /// request in flight again to the new daemon, so a correct target
    /// applies each one after the loss, and the model holds it to that.
    /// A harness whose transport can hold a completion past the loss
    /// returns [`InFlight::MayHaveCompleted`]: a kernel block device,
    /// where the kernel issues nothing again for a request the lost
    /// daemon completed, and a write's call can return after the
    /// restart. The model then also accepts, on the blocks such a
    /// request shares with a write acknowledged before the loss, what a
    /// target that dropped its reissue would leave, so do not claim it
    /// where the transport cannot do it. A harness that should claim it
    /// and does not sees a legal state refused, not a fault passed.
    fn in_flight_at_a_loss(&self) -> InFlight {
        InFlight::Reissued
    }

    /// After [`Harness::kill`]: the completions of requests that the lost
    /// daemon answered before the loss and that the client had not taken
    /// yet. The suite takes them as answered before the loss: the model
    /// holds the target to them after it, and they are not issued again.
    ///
    /// The default is none, for a harness whose kill holds no such
    /// answer. The device harness has them: at the old server's death it
    /// sorts the calls outstanding into those that had returned, whose
    /// requests the lost server completed, and those still waiting. A
    /// harness that has them and drops them makes the suite issue those
    /// requests again, and a target that lost one of them at the kill
    /// then applies it once more and passes.
    fn completed_before_loss(&mut self, _target: &mut Self::Target) -> Vec<Completion> {
        Vec::new()
    }

    /// How many outcomes of a power cut are worth trying right now. A
    /// store with a log counts the records, or whatever units it
    /// persists, that no flush covers yet, and a cut keeps that many; a
    /// store without one counts the ways it draws a cut. The suite cuts
    /// at every point from 0 to this number. A rig that can only cut in
    /// one way returns 0.
    fn cut_points(&mut self, target: &Self::Target) -> u64;

    /// Power loss (rule R10): cut the power under `target`, in the way
    /// `keep` chooses (for a log, the units beyond its durable part that
    /// survive; for a store without one, the seed of the cut), and
    /// return the device as it comes back. `keep` may exceed
    /// [`Harness::cut_points`]; the harness then picks any outcome,
    /// deterministically from `keep`.
    ///
    /// Not called without [`Capability::PowerCut`]; such a harness
    /// returns an error here and 0 from [`Harness::cut_points`].
    fn power_cut(&mut self, target: Self::Target, keep: u64) -> Result<Self::Target, HarnessError>;

    /// Make the target's write log fail for good (rule R11). Completions
    /// for work that finished before this call may still be delivered
    /// after it, successfully; a flush the target receives after it must
    /// fail if it has anything to make durable.
    ///
    /// Not called without [`Capability::FailLog`]; such a harness
    /// returns an error here.
    fn fail_log(&mut self, target: &mut Self::Target) -> Result<(), HarnessError>;

    /// A failed volume is repaired and opened again for a new client
    /// (rule R11): the fault that [`Harness::fail_log`] injected is gone,
    /// and whatever the daemon needs to be told to open a volume that
    /// failed, it is told. The result is the volume as it comes back,
    /// with nothing in flight; the suite then requires that it holds at
    /// least what every completed flush covered.
    ///
    /// Not called without [`Capability::ReopenFailed`]; such a harness
    /// returns an error here.
    fn reopen_failed(&mut self, target: Self::Target) -> Result<Self::Target, HarnessError>;

    /// Let the target do, now, whatever it does on its own between
    /// requests: a checkpoint, a compaction, an eviction. `seed` chooses
    /// among them. Nothing a client can observe may change. The random
    /// programs call this at arbitrary points, between requests in
    /// flight, daemon losses and power cuts. The default does nothing.
    fn maintenance(&mut self, target: &mut Self::Target, seed: u64) -> Result<(), HarnessError> {
        let _ = (target, seed);
        Ok(())
    }

    /// Block numbers at which the implementation changes something inside:
    /// the edge of a segment, a shard, an extent or a cache unit. The
    /// random programs also run in windows that straddle each of them. The
    /// default is none; the programs then run at the start and at the end
    /// of the device.
    fn boundaries(&mut self) -> Vec<u64> {
        Vec::new()
    }
}

/// How hard to run the suite.
#[derive(Debug, Clone)]
pub struct Config {
    /// Random programs per property case.
    pub cases: u32,
    /// Seed for the property cases. A run is reproducible from it.
    pub seed: u64,
    /// The longest random program, in steps.
    pub max_steps: usize,
    /// The suite works in the first `window_blocks` blocks of the device,
    /// so that requests overlap. Clamped to the device.
    pub window_blocks: u64,
    /// How many power cuts or restarts, each with its own seed, the
    /// deterministic durability cases repeat.
    pub rounds: u32,
    /// How long to wait for a completion before the case fails.
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cases: 256,
            seed: 0x5ca1_ab1e,
            max_steps: 48,
            window_blocks: 24,
            rounds: 16,
            timeout: Duration::from_secs(10),
        }
    }
}

/// Why a case failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub String);

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<crate::model::Violation> for Failure {
    fn from(v: crate::model::Violation) -> Self {
        Failure(v.to_string())
    }
}

impl From<HarnessError> for Failure {
    fn from(e: HarnessError) -> Self {
        Failure(e.to_string())
    }
}

type Outcome = Result<(), Failure>;

fn fail<T>(message: impl Into<String>) -> Result<T, Failure> {
    Err(Failure(message.into()))
}

/// What a case needs from the target and the harness to mean anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// Nothing special.
    Nothing,
    /// At least two requests in flight at once.
    Concurrency,
    /// A harness that can cut inside the unflushed part of the log.
    CutPoints,
}

/// One case of the suite.
pub struct Case<H: Harness> {
    /// A stable name.
    pub name: &'static str,
    /// The contract rule the case verifies.
    pub rule: &'static str,
    /// What the case needs to mean anything. Where the target or harness
    /// does not offer it, the case is reported as not exercised.
    pub needs: Needs,
    /// The capabilities the case's rule needs. Where the harness lacks
    /// one, the case is not run and is reported as not exercised.
    pub requires: &'static [Capability],
    /// The capabilities the case uses to check more than its rule needs,
    /// each with what is left out where the harness lacks it. The case
    /// then runs in that weaker form.
    pub weaker: &'static [(Capability, &'static str)],
    /// Whether the case runs [`Config::cases`] random programs.
    pub random: bool,
    /// Runs the case.
    pub run: fn(&mut H, &Config) -> Result<(), Failure>,
}

impl<H: Harness> Case<H> {
    /// Run the case and say whether it passed, failed, or could not
    /// exercise its rule on this target.
    pub fn run_on(&self, harness: &mut H, config: &Config) -> Result<Status, Failure> {
        self.run_checked(harness, config, true)
    }

    /// A defect of the suite, for the suite's own tests and for nothing
    /// else: the case runs although the harness lacks a capability its
    /// rule needs.
    #[doc(hidden)]
    pub fn defect_run_without_its_capabilities(
        &self,
        harness: &mut H,
        config: &Config,
    ) -> Result<Status, Failure> {
        self.run_checked(harness, config, false)
    }

    fn run_checked(
        &self,
        harness: &mut H,
        config: &Config,
        honour: bool,
    ) -> Result<Status, Failure> {
        let has = harness.capabilities();
        if honour {
            if let Some(lacking) = self.requires.iter().find(|c| !has.has(**c)) {
                return Ok(Status::NotExercised(lacking.missing()));
            }
        }
        (self.run)(harness, config)?;
        let missing = match self.needs {
            Needs::Nothing => None,
            Needs::Concurrency => {
                let d = Driver::new(harness, config, &Spec::EMPTY)?;
                (d.geometry.queue_depth() < 2).then_some("the target takes one request at a time")
            }
            Needs::CutPoints => {
                let mut d = Driver::new(harness, config, &Spec::EMPTY)?;
                d.write(0, 1)?;
                d.write(1, 1)?;
                (d.cut_points()? == 0)
                    .then_some("the harness offers no cut point inside the unflushed log")
            }
        };
        if let Some(why) = missing {
            return Ok(Status::NotExercised(why));
        }
        let mut left_out: Vec<String> = self
            .weaker
            .iter()
            .filter(|(capability, _)| !has.has(*capability))
            .map(|(capability, what)| format!("without {}: {what}", capability.name()))
            .collect();
        if left_out.is_empty() {
            return Ok(Status::Passed);
        }
        if self.random {
            left_out.push(format!("{} programs ran", config.cases));
        }
        Ok(Status::Weaker(left_out.join("; ")))
    }
}

/// How a case that did not fail ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The rule was exercised and held.
    Passed,
    /// The rule was exercised and held, in a weaker form than the case
    /// has on a harness that can do everything: what was left out, and
    /// for the lack of which capability. This is not coverage of what
    /// was left out.
    Weaker(String),
    /// Nothing failed, but the target or harness cannot exercise the rule,
    /// for the reason given. This is not coverage: a green run with such
    /// cases says nothing about their rules.
    NotExercised(&'static str),
}

/// The result of one case.
#[derive(Debug, Clone)]
pub struct CaseOutcome {
    /// The case's name.
    pub name: &'static str,
    /// The rule it verifies.
    pub rule: &'static str,
    /// How it ended, or why it failed.
    pub result: Result<Status, Failure>,
}

/// The result of a run of the whole suite.
#[derive(Debug, Clone)]
pub struct Report {
    /// One entry per case, in the order of [`cases`].
    pub outcomes: Vec<CaseOutcome>,
}

impl Report {
    /// Whether no case failed. Cases that were not exercised do not count
    /// as failures; see [`Report::not_exercised`].
    pub fn passed(&self) -> bool {
        self.outcomes.iter().all(|o| o.result.is_ok())
    }

    /// The names of the cases that failed.
    pub fn failed(&self) -> Vec<&'static str> {
        self.outcomes
            .iter()
            .filter(|o| o.result.is_err())
            .map(|o| o.name)
            .collect()
    }

    /// The names of the cases whose rule this target or harness could not
    /// exercise.
    pub fn not_exercised(&self) -> Vec<&'static str> {
        self.unexercised()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// The cases whose rule this target or harness could not exercise,
    /// each with the reason.
    pub fn unexercised(&self) -> Vec<(&'static str, &'static str)> {
        self.outcomes
            .iter()
            .filter_map(|o| match o.result {
                Ok(Status::NotExercised(why)) => Some((o.name, why)),
                _ => None,
            })
            .collect()
    }

    /// The cases that ran in a weaker form, each with what was left out.
    /// A caller that expects the whole suite asserts that this and
    /// [`Report::not_exercised`] are empty.
    pub fn weaker(&self) -> Vec<(&'static str, &str)> {
        self.outcomes
            .iter()
            .filter_map(|o| match &o.result {
                Ok(Status::Weaker(what)) => Some((o.name, what.as_str())),
                _ => None,
            })
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for o in &self.outcomes {
            match &o.result {
                Ok(Status::Passed) => writeln!(f, "ok    {}", o.name)?,
                Ok(Status::Weaker(what)) => writeln!(
                    f,
                    "WEAK  {}
      a weaker form ran: {what}",
                    o.name
                )?,
                Ok(Status::NotExercised(why)) => writeln!(
                    f,
                    "SKIP  {}
      not exercised: {why}",
                    o.name
                )?,
                Err(e) => writeln!(
                    f,
                    "FAIL  {}
      rule: {}
      {}",
                    o.name, o.rule, e
                )?,
            }
        }
        Ok(())
    }
}

/// Run every case and collect the results.
pub fn run_all<H: Harness>(harness: &mut H, config: &Config) -> Report {
    let outcomes = cases::<H>()
        .into_iter()
        .map(|case| CaseOutcome {
            name: case.name,
            rule: case.rule,
            result: case.run_on(harness, config),
        })
        .collect();
    Report { outcomes }
}

/// Run the case called `name`. `None` if there is no such case.
pub fn run_case<H: Harness>(
    harness: &mut H,
    config: &Config,
    name: &str,
) -> Option<Result<Status, Failure>> {
    cases::<H>()
        .into_iter()
        .find(|case| case.name == name)
        .map(|case| case.run_on(harness, config))
}

/// The cases of the suite, each with the rule it verifies.
pub fn cases<H: Harness>() -> Vec<Case<H>> {
    vec![
        Case {
            name: "parameters_are_as_the_contract_fixes_them",
            rule: "R1: the size is a non-zero multiple of 1 MiB, the read-only flag is as \
                   asked, and the parameters survive a daemon loss and a power cut",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::PowerCut, "the parameters are not checked after a power cut"),
                (Capability::ReadOnly, "the read-only flag of a read-only device is not checked"),
                (Capability::Base, "the read-only device has an empty base"),
            ],
            random: false,
            run: parameters::<H>,
        },
        Case {
            name: "fresh_device_reads_as_its_base",
            rule: "R3: a block never written reads as the base: zeros on an empty volume, \
                   the commit's content on a clone",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::Base, "only an empty volume is read; a clone's base is not"),
            ],
            random: false,
            run: fresh_device_reads_as_its_base::<H>,
        },
        Case {
            name: "rejects_misaligned",
            rule: "R2: an offset or length that is not a multiple of 4 KiB is rejected, for all \
                   four ranged operations, without effect",
            needs: Needs::Nothing,
            requires: &[Capability::InvalidRequests],
            weaker: &[],
            random: false,
            run: rejects_misaligned::<H>,
        },
        Case {
            name: "rejects_out_of_bounds",
            rule: "R2: a request reaching past the end of the device is rejected without \
                   effect; a request ending exactly at the end is accepted",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::InvalidRequests, "no request beyond the end is issued; only that the last block can be written and read, which through a kernel exercises the target only as far as the block layer forwards the request unchanged"),
            ],
            random: false,
            run: rejects_out_of_bounds::<H>,
        },
        Case {
            name: "rejects_empty_and_oversize",
            rule: "R1, R2: an empty request and a read or write longer than 1 MiB are \
                   rejected; 1 MiB itself is accepted, and a discard and a write-zeroes may \
                   span the device",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::InvalidRequests, "no empty or oversize request is issued; only that 1 MiB is accepted and that a discard and a write-zeroes may span the device, which through a kernel exercises the target only if the device advertises a largest request of 1 MiB: the block layer splits to the advertised limits"),
            ],
            random: false,
            run: rejects_empty_and_oversize::<H>,
        },
        Case {
            name: "read_your_writes",
            rule: "R3, R4: once a write is acknowledged, reads return it, block for block, and \
                   an overwrite replaces it",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
            ],
            random: false,
            run: read_your_writes::<H>,
        },
        Case {
            name: "write_zeroes_hides_the_base",
            rule: "R5: write-zeroes reads as zeros on a volume with a base, and once flushed \
                   survives a power cut like a write",
            needs: Needs::Nothing,
            requires: &[Capability::Base],
            weaker: &[
                (Capability::PowerCut, "the result is not checked after a power cut"),
            ],
            random: false,
            run: write_zeroes_hides_the_base::<H>,
        },
        Case {
            name: "discard_leaves_old_zeros_or_base",
            rule: "R6: a discarded block reads as its content before the discard, zeros or the \
                   base, also after a power cut; blocks next to the discard are untouched",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::Base, "it runs on an empty volume, where the base is zeros"),
                (Capability::PowerCut, "the result is not checked after a power cut"),
            ],
            random: false,
            run: discard_leaves_old_zeros_or_base::<H>,
        },
        Case {
            name: "discard_never_hits_later_writes",
            rule: "R6, R8: a write acknowledged after a discard is what reads return, now, \
                   after a flush and after a power cut",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
                (Capability::PowerCut, "the result is not checked after a power cut"),
            ],
            random: false,
            run: discard_never_hits_later_writes::<H>,
        },
        Case {
            name: "flushed_operations_survive_power_cut",
            rule: "R7, R10: every operation acknowledged before a completed flush was \
                   submitted survives a power cut that keeps nothing unflushed",
            needs: Needs::Nothing,
            requires: &[Capability::PowerCut],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
            ],
            random: false,
            run: flushed_operations_survive_power_cut::<H>,
        },
        Case {
            name: "flush_covers_what_was_acknowledged_before_it",
            rule: "R7: write A acknowledged, flush submitted, write B submitted while the flush \
                   is pending, power cut once both completed: A is present (the group-commit \
                   bug)",
            needs: Needs::Nothing,
            requires: &[Capability::PowerCut],
            weaker: &[],
            random: false,
            run: flush_covers_what_was_acknowledged_before_it::<H>,
        },
        Case {
            name: "flush_covers_an_earlier_instance",
            rule: "R7, R9: a write acknowledged before a daemon loss is covered by a flush \
                   sent to the new instance, and is durable even without one",
            needs: Needs::Nothing,
            requires: &[Capability::PowerCut],
            weaker: &[],
            random: false,
            run: flush_covers_an_earlier_instance::<H>,
        },
        Case {
            name: "power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may",
            rule: "R10: cut at every point the harness offers: the device holds everything \
                   flushed and, of what was written since, any version for each sector",
            needs: Needs::CutPoints,
            requires: &[Capability::PowerCut],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
            ],
            random: false,
            run: power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may::<H>,
        },
        Case {
            name: "recovered_state_is_durable",
            rule: "R10: what is read after a power cut is stable, and a second power cut that \
                   keeps nothing unflushed does not take it back",
            needs: Needs::Nothing,
            requires: &[Capability::PowerCut],
            weaker: &[],
            random: false,
            run: recovered_state_is_durable::<H>,
        },
        Case {
            name: "first_request_after_power_cut_may_be_a_write",
            rule: "R10: a write issued straight after a power cut, before any read, lands \
                   after everything recovery exposed, and both survive the next cut",
            needs: Needs::Nothing,
            requires: &[Capability::PowerCut],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
            ],
            random: false,
            run: first_request_after_power_cut_may_be_a_write::<H>,
        },
        Case {
            name: "queue_depth_requests_complete_in_any_order",
            rule: "R8: the target takes queue_depth requests in flight, completes each exactly \
                   once in whatever order, and refuses an id that is already in flight",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[],
            random: false,
            run: queue_depth_requests_complete_in_any_order::<H>,
        },
        Case {
            name: "overlapping_operations_in_flight_apply_whole",
            rule: "R8: overlapping operations in flight together end up in one order: on \
                   every block they share the same one wins, in memory and after a power cut",
            needs: Needs::Concurrency,
            requires: &[],
            weaker: &[
                (Capability::PowerCut, "the order is checked in memory only, not in the log after a power cut"),
            ],
            random: false,
            run: overlapping_operations_in_flight_apply_whole::<H>,
        },
        Case {
            name: "kill_keeps_acknowledged_operations",
            rule: "R9: every operation acknowledged before the daemon is lost is read back \
                   after it returns, flushed or not",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
            ],
            random: false,
            run: kill_keeps_acknowledged_operations::<H>,
        },
        Case {
            name: "reissue_after_kill_is_idempotent",
            rule: "R9: requests in flight when the daemon is lost complete correctly when \
                   reissued under the same ids, in either order, whether or not they had \
                   already been applied",
            needs: Needs::Concurrency,
            requires: &[],
            weaker: &[
                (Capability::PowerCut, "the reissued operations are not checked after a flush and a power cut"),
            ],
            random: false,
            run: reissue_after_kill_is_idempotent::<H>,
        },
        Case {
            name: "failed_volume_fails_every_request",
            rule: "R11: once the log has failed for good the flush fails, and then every \
                   request does, reads included, also after a daemon restart",
            needs: Needs::Nothing,
            requires: &[Capability::FailLog],
            weaker: &[
                (Capability::InvalidRequests, "no invalid request is in flight at the failure or issued after it"),
            ],
            random: false,
            run: failed_volume_fails_every_request::<H>,
        },
        Case {
            name: "reopened_failed_volume_holds_what_was_flushed",
            rule: "R11: a completed flush stays a promise on a failed volume: opened again \
                   after a repair, it holds at least what every completed flush covered, \
                   and serves as a healthy volume does",
            needs: Needs::Nothing,
            requires: &[Capability::FailLog, Capability::ReopenFailed],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
                (Capability::PowerCut, "the write after the reopen is not checked after a power cut"),
            ],
            random: false,
            run: reopened_failed_volume_holds_what_was_flushed::<H>,
        },
        Case {
            name: "restart_after_the_log_failed_recovers_nothing",
            rule: "R11 over R9: the log fails, no request completes, the daemon restarts:                    opened again, the volume is as after a power cut, with everything flushed, and an                    acknowledged operation that no flush covered may be gone",
            needs: Needs::Nothing,
            requires: &[Capability::FailLog, Capability::ReopenFailed],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
                (Capability::PowerCut, "the write after the reopen is not checked after a power cut"),
            ],
            random: false,
            run: restart_after_the_log_failed_recovers_nothing::<H>,
        },
        Case {
            name: "read_only_target_refuses_mutations",
            rule: "R12: on a read-only target write, write-zeroes and discard fail without \
                   effect, and flush and read succeed",
            needs: Needs::Nothing,
            requires: &[Capability::ReadOnly],
            weaker: &[
                (Capability::Base, "the read-only device has an empty base"),
            ],
            random: false,
            run: read_only_target_refuses_mutations::<H>,
        },
        Case {
            name: "prop_invalid_requests_are_rejected_without_effect",
            rule: "R2: property: any invalid request is rejected and changes nothing",
            needs: Needs::Nothing,
            requires: &[Capability::InvalidRequests],
            weaker: &[
                (Capability::Base, "it runs on an empty volume only"),
            ],
            random: true,
            run: prop_invalid_requests::<H>,
        },
        Case {
            name: "prop_concurrent_programs_match_model",
            rule: "R3 to R8: property: random overlapping requests on an empty or a non-empty \
                   base, reaped at random moments, read back as some order of whole \
                   operations allows",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::Base, "every program runs on an empty volume"),
                (Capability::InvalidRequests, "no program has an invalid request"),
                (Capability::PowerCut, "no power cut before the last read-back"),
            ],
            random: true,
            run: prop_concurrent_programs::<H>,
        },
        Case {
            name: "prop_failures_match_model",
            rule: "R7, R9, R10, R11: property: random programs with daemon losses, power cuts \
                   at random points and log failure read back as the model allows",
            needs: Needs::Nothing,
            requires: &[],
            weaker: &[
                (Capability::PowerCut, "no program has a power cut, and none before the last read-back"),
                (Capability::FailLog, "no program has a log failure or a reopen"),
                (Capability::ReopenFailed, "no program reopens the volume; one whose log failed ends there"),
                (Capability::Base, "every program runs on an empty volume"),
                (Capability::InvalidRequests, "no program has an invalid request"),
            ],
            random: true,
            run: prop_failing_programs::<H>,
        },
    ]
}

// ---------------------------------------------------------------------------
// The driver: a client that keeps the model in step with the target.
// ---------------------------------------------------------------------------

struct Driver<'a, H: Harness> {
    harness: &'a mut H,
    target: Option<H::Target>,
    geometry: Geometry,
    model: Model,
    /// A copy of every request in flight, for reissue after a kill.
    inflight: BTreeMap<RequestId, Request>,
    next_id: u64,
    stamp: u64,
    window: u64,
    /// The first block of the window the random programs work in.
    origin: u64,
    timeout: Duration,
    /// What the harness can do.
    has: Capabilities,
}

/// The suite was about to ask the harness for something it says it
/// cannot do. The cases are written not to; this is the guard.
fn overstepped<T>(capability: Capability) -> Result<T, Failure> {
    fail(format!(
        "the case asked for {} although {}: it should not have run, or not in this form",
        capability.name(),
        capability.missing()
    ))
}

/// `spec`, on an empty base where the harness cannot build another.
fn on_base<H: Harness>(h: &H, spec: Spec) -> Spec {
    if h.capabilities().has(Capability::Base) {
        spec
    } else {
        Spec {
            base: Base::Empty,
            ..spec
        }
    }
}

/// The bases a case that runs on each of them runs on.
fn bases<H: Harness>(h: &H) -> Vec<Spec> {
    if h.capabilities().has(Capability::Base) {
        vec![Spec::EMPTY, Spec::CLONE]
    } else {
        vec![Spec::EMPTY]
    }
}

/// Fill `buf` with a pattern that names `stamp` and the block, differs
/// from any other stamp's, from every base and from zeros.
fn fill(buf: &mut [u8], stamp: u64, first_block: u64) {
    for (i, block) in buf.chunks_mut(BLOCK_USIZE).enumerate() {
        let mut z = stamp
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((first_block + i as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        z ^= z >> 29;
        let word = ((z | 0x0101_0101_0101_0101) & 0x7F7F_7F7F_7F7F_7F7F).to_le_bytes();
        for chunk in block.chunks_mut(8) {
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}

fn mutates(op: &Op) -> bool {
    matches!(
        op,
        Op::Write { .. } | Op::WriteZeroes { .. } | Op::Discard { .. }
    )
}

impl<'a, H: Harness> Driver<'a, H> {
    fn new(harness: &'a mut H, config: &Config, spec: &Spec) -> Result<Self, Failure> {
        let has = harness.capabilities();
        if spec.base != Base::Empty && !has.has(Capability::Base) {
            return overstepped(Capability::Base);
        }
        if spec.read_only && !has.has(Capability::ReadOnly) {
            return overstepped(Capability::ReadOnly);
        }
        let target = harness.create(spec)?;
        let geometry = target.geometry();
        Ok(Self {
            harness,
            target: Some(target),
            geometry,
            model: Model::with_base(geometry, spec.base),
            inflight: BTreeMap::new(),
            next_id: 0,
            stamp: 0,
            window: config.window_blocks.clamp(16, 256),
            origin: 0,
            timeout: config.timeout,
            has,
        })
    }

    fn target(&mut self) -> Result<&mut H::Target, Failure> {
        match self.target.as_mut() {
            Some(t) => Ok(t),
            None => fail("no target: an earlier harness call failed"),
        }
    }

    /// Requests the suite keeps in flight at most.
    fn depth(&self) -> usize {
        (self.geometry.queue_depth() as usize).min(16)
    }

    /// Writes, write-zeroes and discards the random programs keep in
    /// flight together at most. The model's cost grows with it.
    fn mutation_depth(&self) -> usize {
        self.depth().min(4)
    }

    fn can(&self, capability: Capability) -> bool {
        self.has.has(capability)
    }

    fn submit(&mut self, op: Op) -> Result<RequestId, Failure> {
        if !self.can(Capability::InvalidRequests) && self.geometry.validate(&op).is_err() {
            return overstepped(Capability::InvalidRequests);
        }
        self.next_id += 1;
        let request = Request {
            id: RequestId(self.next_id),
            op,
        };
        self.model.submit(&request)?;
        self.inflight.insert(request.id, request.clone());
        let id = request.id;
        if let Err(e) = self.target()?.submit(request) {
            return fail(format!(
                "R8: submit refused with {} requests in flight: {e}",
                self.inflight.len() - 1
            ));
        }
        Ok(id)
    }

    fn account(&mut self, done: &[Completion]) -> Outcome {
        for completion in done {
            self.model.complete(completion)?;
            let Some(request) = self.inflight.remove(&completion.id) else {
                continue;
            };
            // Buffers make a round trip, whatever the result.
            let lent = match &request.op {
                Op::Read { buf, .. } => Some(buf.len()),
                Op::Write { data, .. } => Some(data.len()),
                _ => None,
            };
            let back = completion.buf.as_ref().map(|b| b.len());
            if lent.is_some() && back != lent {
                return fail(format!(
                    "the buffer of {} {} did not come back: lent {lent:?} bytes, got {back:?}",
                    request.op.name(),
                    request.id
                ));
            }
        }
        Ok(())
    }

    /// Wait for at least one completion, if anything is in flight.
    fn reap(&mut self) -> Result<Vec<Completion>, Failure> {
        let mut done = Vec::new();
        if self.inflight.is_empty() {
            return Ok(done);
        }
        let timeout = self.timeout;
        if self.target()?.wait(&mut done, timeout) == 0 || done.is_empty() {
            return fail(format!(
                "R8: {} requests in flight and none completed within {timeout:?}",
                self.inflight.len()
            ));
        }
        self.account(&done)?;
        Ok(done)
    }

    /// Take what is ready without waiting.
    fn poll(&mut self) -> Outcome {
        let mut done = Vec::new();
        self.target()?.poll(&mut done);
        self.account(&done)
    }

    /// Reap until nothing is in flight.
    fn drain(&mut self) -> Result<Vec<Completion>, Failure> {
        let mut all = Vec::new();
        while !self.inflight.is_empty() {
            all.append(&mut self.reap()?);
        }
        Ok(all)
    }

    /// Submit one request and reap until it completes.
    fn run(&mut self, op: Op) -> Result<Completion, Failure> {
        let id = self.submit(op)?;
        loop {
            if let Some(c) = self.reap()?.into_iter().find(|c| c.id == id) {
                return Ok(c);
            }
            if !self.inflight.contains_key(&id) {
                return fail(format!("{id} left the queue without a completion"));
            }
        }
    }

    /// Submit, making room first if the queue is at the suite's depth, or
    /// if `op` mutates and enough mutations are in flight already.
    fn submit_with_room(&mut self, op: Op) -> Result<RequestId, Failure> {
        loop {
            let mutations = self.inflight.values().filter(|r| mutates(&r.op)).count();
            let full = self.inflight.len() >= self.depth()
                || (mutates(&op) && mutations >= self.mutation_depth());
            if !full {
                return self.submit(op);
            }
            self.reap()?;
        }
    }

    fn data(&mut self, block: u64, count: u64) -> Vec<u8> {
        self.stamp += 1;
        let mut data = vec![0; (count * BLOCK_SIZE) as usize];
        fill(&mut data, self.stamp, block);
        data
    }

    fn write_op(&mut self, block: u64, count: u64) -> Op {
        let data = self.data(block, count);
        write_of(block, &data)
    }

    /// Write fresh data and wait for the acknowledgement. Returns the
    /// data.
    fn write(&mut self, block: u64, count: u64) -> Result<Vec<u8>, Failure> {
        let data = self.data(block, count);
        self.run(write_of(block, &data))?;
        Ok(data)
    }

    /// Read and return the bytes. The model has checked them.
    fn read(&mut self, block: u64, count: u64) -> Result<Vec<u8>, Failure> {
        match self.run(read_op(block, count))?.buf {
            Some(buf) => Ok(buf.to_vec()),
            None => fail("read completed without a buffer"),
        }
    }

    fn expect_read(&mut self, block: u64, expected: &[u8], what: &str) -> Outcome {
        let got = self.read(block, expected.len() as u64 / BLOCK_SIZE)?;
        if got == expected {
            Ok(())
        } else {
            fail(format!(
                "{what}: block {block} does not read back as written"
            ))
        }
    }

    fn zeroes(&mut self, block: u64, count: u64) -> Outcome {
        self.run(Op::WriteZeroes {
            offset: block * BLOCK_SIZE,
            len: count * BLOCK_SIZE,
        })
        .map(|_| ())
    }

    fn discard(&mut self, block: u64, count: u64) -> Outcome {
        self.run(Op::Discard {
            offset: block * BLOCK_SIZE,
            len: count * BLOCK_SIZE,
        })
        .map(|_| ())
    }

    fn flush(&mut self) -> Outcome {
        self.run(Op::Flush).map(|_| ())
    }

    /// Read the device back through the model: all of it when it is small,
    /// otherwise the windows the suite works in, the end of the device and
    /// a spread of blocks in between. A target that damages blocks no
    /// request named is caught here.
    fn verify(&mut self) -> Outcome {
        let blocks = self.geometry.blocks();
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        if blocks <= 1024 {
            ranges.push((0, blocks));
        } else {
            ranges.push((0, self.window.max(256)));
            ranges.push((self.origin, self.window));
            ranges.push((blocks - 16, 16));
            ranges.extend((1..64).map(|i| (blocks / 64 * i, 2)));
        }
        for (first, count) in ranges {
            let mut block = first;
            while block < first + count {
                let n = 64.min(first + count - block);
                self.submit_with_room(read_op(block, n))?;
                block += n;
            }
        }
        self.drain().map(|_| ())
    }

    /// If the model is carrying many candidate orders, settle them: read
    /// everything back and flush.
    fn settle(&mut self) -> Outcome {
        if self.model.worlds() > 48 {
            self.drain()?;
            self.verify()?;
            self.flush()?;
        }
        Ok(())
    }

    fn check_geometry(&mut self) -> Outcome {
        let now = self.target()?.geometry();
        if now == self.geometry {
            Ok(())
        } else {
            fail(format!(
                "R1: parameters changed from {:?} to {now:?}",
                self.geometry
            ))
        }
    }

    fn cut_points(&mut self) -> Result<u64, Failure> {
        match self.target.as_ref() {
            Some(target) => Ok(self.harness.cut_points(target)),
            None => fail("no target: an earlier harness call failed"),
        }
    }

    /// Cut the power so that `keep` unflushed units survive, bring the
    /// device back, and read back every block the cut may have changed.
    fn power_cut(&mut self, keep: u64) -> Outcome {
        if !self.can(Capability::PowerCut) {
            return overstepped(Capability::PowerCut);
        }
        let Some(target) = self.target.take() else {
            return fail("no target: an earlier harness call failed");
        };
        self.target = Some(self.harness.power_cut(target, keep)?);
        self.model.power_cut();
        self.inflight.clear();
        self.check_geometry()?;
        self.read_back()
    }

    /// Where a case cuts the power only to check more than its rule
    /// needs: cut it if the harness can, and go on if it cannot.
    fn power_cut_if_able(&mut self, keep: u64) -> Outcome {
        if self.can(Capability::PowerCut) {
            self.power_cut(keep)
        } else {
            Ok(())
        }
    }

    /// Open the failed volume again and read back every block whose
    /// content the failure left open.
    fn reopen_failed(&mut self) -> Outcome {
        if !self.can(Capability::ReopenFailed) {
            return overstepped(Capability::ReopenFailed);
        }
        let Some(target) = self.target.take() else {
            return fail("no target: an earlier harness call failed");
        };
        self.target = Some(self.harness.reopen_failed(target)?);
        self.model.reopened();
        self.inflight.clear();
        self.check_geometry()?;
        self.read_back()
    }

    /// Let the target do its own work between requests.
    fn maintain(&mut self, seed: u64) -> Outcome {
        let Some(target) = self.target.as_mut() else {
            return fail("no target: an earlier harness call failed");
        };
        self.harness.maintenance(target, seed)?;
        Ok(())
    }

    /// After a power cut or a reopen: read what the model needs to learn
    /// what the device holds.
    fn read_back(&mut self) -> Outcome {
        let blocks = self.model.readback();
        let mut i = 0;
        while i < blocks.len() {
            // One read per run of consecutive blocks, up to 1 MiB.
            let mut j = i + 1;
            while j < blocks.len() && blocks[j] == blocks[j - 1] + 1 && j - i < 256 {
                j += 1;
            }
            self.run(read_op(blocks[i], (j - i) as u64))?;
            i = j;
        }
        Ok(())
    }

    /// Lose the daemon, bring it back and reissue what was in flight.
    /// What the harness says was answered before the loss is accounted
    /// first, as answered before it.
    fn kill(&mut self, seed: u64) -> Outcome {
        let Some(target) = self.target.take() else {
            return fail("no target: an earlier harness call failed");
        };
        let mut target = self.harness.kill(target, seed)?;
        let before = self.harness.completed_before_loss(&mut target);
        self.target = Some(target);
        self.account(&before)?;
        self.model.kill(self.harness.in_flight_at_a_loss());
        self.check_geometry()?;
        let mut reissue: Vec<Request> = self.inflight.values().cloned().collect();
        if seed & 1 == 1 {
            reissue.reverse();
        }
        for request in reissue {
            let id = request.id;
            if let Err(e) = self.target()?.submit(request) {
                return fail(format!(
                    "R9: reissue of {id} after daemon loss refused: {e}"
                ));
            }
        }
        Ok(())
    }

    fn fail_log(&mut self) -> Outcome {
        if !self.can(Capability::FailLog) {
            return overstepped(Capability::FailLog);
        }
        let Some(target) = self.target.as_mut() else {
            return fail("no target: an earlier harness call failed");
        };
        self.harness.fail_log(target)?;
        self.model.log_failed();
        Ok(())
    }

    /// Submit an invalid request and check how it is rejected. A read or
    /// write that is also longer than the largest request may be rejected
    /// for that instead: the contract does not rank the reasons.
    fn expect_rejection(&mut self, op: Op, want: fn(&BlkError) -> bool, what: &str) -> Outcome {
        let name = op.name();
        let oversize = op.carries_data() && op.range().is_some_and(|(_, len)| len > MAX_IO_BYTES);
        let done = self.run(op)?;
        match done.result {
            Err(e) if want(&e) || (oversize && is_too_large(&e)) => Ok(()),
            other => fail(format!(
                "R2: {name} {what}: expected a rejection, got {other:?}"
            )),
        }
    }

    /// Submit a request that must fail with `want`.
    fn expect_error(&mut self, op: Op, want: BlkError, rule: &str) -> Outcome {
        let name = op.name();
        let done = self.run(op)?;
        if done.result == Err(want) {
            Ok(())
        } else {
            fail(format!(
                "{rule}: {name} must fail with {want:?}, got {:?}",
                done.result
            ))
        }
    }
}

fn read_op(block: u64, count: u64) -> Op {
    Op::Read {
        offset: block * BLOCK_SIZE,
        buf: Buf::filled(count as usize, 0xEE),
    }
}

/// A write of `data`, which is whole blocks, at `block`.
fn write_of(block: u64, data: &[u8]) -> Op {
    let mut buf = Buf::zeroed(data.len() / BLOCK_USIZE);
    buf.copy_from_slice(data);
    Op::Write {
        offset: block * BLOCK_SIZE,
        data: buf,
    }
}

/// The ranged operations over `offset, len`: discard and write-zeroes,
/// and, where `len` is whole blocks, read and write. (A buffer cannot
/// hold a partial block, so a read or write of a misaligned length cannot
/// be expressed.)
fn ranged_ops(offset: u64, len: u64) -> Vec<Op> {
    let mut ops = vec![Op::Discard { offset, len }, Op::WriteZeroes { offset, len }];
    if len.is_multiple_of(BLOCK_SIZE) {
        let blocks = (len / BLOCK_SIZE) as usize;
        ops.push(Op::Read {
            offset,
            buf: Buf::zeroed(blocks),
        });
        ops.push(Op::Write {
            offset,
            data: Buf::filled(blocks, 0x5A),
        });
    }
    ops
}

/// A request that the geometry rejects, one of several kinds by `pick`,
/// in or near the window at `origin`.
fn invalid_op(geometry: Geometry, origin: u64, pick: u64) -> Op {
    let at = (origin + pick / 8 % 8) * BLOCK_SIZE;
    let end = geometry.size_bytes();
    match pick % 8 {
        0 => Op::Write {
            offset: at + 512,
            data: Buf::filled(1, 0x5A),
        },
        1 => Op::Discard { offset: at, len: 0 },
        2 => Op::WriteZeroes {
            offset: at,
            len: BLOCK_SIZE + 1,
        },
        3 => Op::Read {
            offset: end,
            buf: Buf::zeroed(1),
        },
        4 => Op::Write {
            offset: end - BLOCK_SIZE,
            data: Buf::filled(2, 0x5A),
        },
        5 => Op::Discard {
            offset: at,
            len: (u64::MAX - at) / BLOCK_SIZE * BLOCK_SIZE,
        },
        6 => Op::WriteZeroes {
            offset: at + 1,
            len: BLOCK_SIZE,
        },
        _ => Op::Read {
            offset: at,
            buf: Buf::zeroed((MAX_IO_BYTES / BLOCK_SIZE) as usize + 1),
        },
    }
}

fn is_misaligned(e: &BlkError) -> bool {
    matches!(e, BlkError::Misaligned { .. })
}

fn is_out_of_bounds(e: &BlkError) -> bool {
    matches!(e, BlkError::OutOfBounds { .. })
}

fn is_zero_length(e: &BlkError) -> bool {
    matches!(e, BlkError::ZeroLength)
}

fn is_too_large(e: &BlkError) -> bool {
    matches!(e, BlkError::TooLarge { .. })
}

// ---------------------------------------------------------------------------
// Deterministic cases.
// ---------------------------------------------------------------------------

fn base_bytes(base: Base, block: u64, count: u64) -> Vec<u8> {
    let mut out = vec![0; (count * BLOCK_SIZE) as usize];
    for (i, chunk) in out.chunks_mut(BLOCK_USIZE).enumerate() {
        base.fill_block(block + i as u64, chunk);
    }
    out
}

fn parameters<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    let g = d.geometry;
    if g.size_bytes() == 0 || g.size_bytes() % SIZE_UNIT != 0 {
        return fail(format!(
            "R1: device size {} is not a multiple of 1 MiB",
            g.size_bytes()
        ));
    }
    if g.queue_depth() == 0 {
        return fail("R1: queue depth is zero");
    }
    if g.read_only() {
        return fail("R1: a writable device reports read-only");
    }
    d.kill(1)?;
    d.power_cut_if_able(0)?;
    if !d.can(Capability::ReadOnly) {
        return Ok(());
    }
    let read_only = Spec {
        read_only: true,
        ..on_base(h, Spec::CLONE)
    };
    let d = Driver::new(h, cfg, &read_only)?;
    if !d.geometry.read_only() {
        return fail("R1: a read-only device does not report read-only");
    }
    Ok(())
}

fn fresh_device_reads_as_its_base<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    for spec in bases(h) {
        let mut d = Driver::new(h, cfg, &spec)?;
        d.verify()?;
        let last = d.geometry.blocks() - 1;
        for (block, count) in [(0, 4), (last, 1)] {
            let expected = base_bytes(spec.base, block, count);
            d.expect_read(block, &expected, "R3: a fresh device")?;
        }
    }
    Ok(())
}

fn rejects_misaligned<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    let before = d.write(0, 4)?;
    for (offset, len) in [
        (1, BLOCK_SIZE),
        (512, BLOCK_SIZE),
        (BLOCK_SIZE - 1, BLOCK_SIZE),
        (BLOCK_SIZE + 2048, 2 * BLOCK_SIZE),
        (0, 1),
        (0, 512),
        (BLOCK_SIZE, BLOCK_SIZE + 512),
        (BLOCK_SIZE, 2 * BLOCK_SIZE - 1),
    ] {
        for op in ranged_ops(offset, len) {
            d.expect_rejection(op, is_misaligned, &format!("at {offset}+{len}"))?;
        }
    }
    d.expect_read(0, &before, "R2: after rejected misaligned requests")
}

fn rejects_out_of_bounds<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    let size = d.geometry.size_bytes();
    let last = d.geometry.blocks() - 1;
    let before = d.write(last - 1, 2)?;
    let far = u64::MAX - u64::MAX % BLOCK_SIZE;
    let rejections = d.can(Capability::InvalidRequests);
    let beyond: &[(u64, u64)] = &[
        (size, BLOCK_SIZE),
        (size - BLOCK_SIZE, 2 * BLOCK_SIZE),
        (size + 16 * BLOCK_SIZE, BLOCK_SIZE),
        (far - BLOCK_SIZE, BLOCK_SIZE),
        (far, BLOCK_SIZE),
    ];
    for &(offset, len) in beyond.iter().filter(|_| rejections) {
        for op in ranged_ops(offset, len) {
            d.expect_rejection(op, is_out_of_bounds, &format!("at {offset}+{len}"))?;
        }
    }
    let longer = [far, far - size + BLOCK_SIZE, size + BLOCK_SIZE];
    for len in longer.into_iter().filter(|_| rejections) {
        d.expect_rejection(
            Op::Discard { offset: 0, len },
            is_out_of_bounds,
            "longer than the device",
        )?;
        d.expect_rejection(
            Op::WriteZeroes {
                offset: BLOCK_SIZE,
                len,
            },
            is_out_of_bounds,
            "longer than the device",
        )?;
    }
    d.expect_read(
        last - 1,
        &before,
        "R2: after rejected out-of-bounds requests",
    )?;
    // The last block itself is in bounds.
    let data = d.write(last, 1)?;
    d.expect_read(last, &data, "R2: the last block")
}

fn rejects_empty_and_oversize<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    let rejections = d.can(Capability::InvalidRequests);
    for op in ranged_ops(BLOCK_SIZE, 0).into_iter().filter(|_| rejections) {
        d.expect_rejection(op, is_zero_length, "of zero length")?;
    }
    // Exactly 1 MiB is accepted.
    let max_blocks = MAX_IO_BYTES / BLOCK_SIZE;
    let data = d.write(0, max_blocks)?;
    d.expect_read(0, &data, "R1: a request of exactly 1 MiB")?;
    // One block more is not, even where it fits the device.
    let over = (max_blocks + 1) as usize;
    let fits = d.geometry.blocks() > max_blocks;
    let want: fn(&BlkError) -> bool = if fits {
        is_too_large
    } else {
        |e| is_too_large(e) || is_out_of_bounds(e)
    };
    if rejections {
        d.expect_rejection(
            Op::Read {
                offset: 0,
                buf: Buf::zeroed(over),
            },
            want,
            "longer than 1 MiB",
        )?;
        d.expect_rejection(
            Op::Write {
                offset: 0,
                data: Buf::filled(over, 1),
            },
            want,
            "longer than 1 MiB",
        )?;
        d.expect_read(0, &data, "R2: after rejected oversize requests")?;
    }
    // Discard and write-zeroes carry no data and may span the device.
    // (Capped at 64 MiB so that the model stays small on a large rig.)
    let span = d.geometry.size_bytes().min(64 * SIZE_UNIT);
    d.run(Op::Discard {
        offset: 0,
        len: span,
    })?;
    d.verify()?;
    d.run(Op::WriteZeroes {
        offset: 0,
        len: span,
    })?;
    let zeros = vec![0; 8 * BLOCK_USIZE];
    d.expect_read(0, &zeros, "R5: after write-zeroes over the device")
}

fn read_your_writes<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    for spec in bases(h) {
        let mut d = Driver::new(h, cfg, &spec)?;
        let a = d.write(2, 1)?;
        d.expect_read(2, &a, "R3: a single block")?;
        let b = d.write(2, 1)?;
        d.expect_read(2, &b, "R3: an overwrite")?;
        let c = d.write(4, 8)?;
        d.expect_read(4, &c, "R3: a multi-block write")?;
        // Partial overwrite: the rest of the earlier write must remain.
        let mid = d.write(5, 1)?;
        let got = d.read(4, 8)?;
        let n = BLOCK_USIZE;
        if got[..n] != c[..n] || got[n..2 * n] != mid[..] || got[2 * n..] != c[2 * n..] {
            return fail("R3: a partial overwrite disturbed its neighbours");
        }
        d.expect_read(4, &c[..n], "R3: the first block of a multi-block write")?;
        d.verify()?;
    }
    Ok(())
}

fn write_zeroes_hides_the_base<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let base = Spec::CLONE.base;
    let mut d = Driver::new(h, cfg, &Spec::CLONE)?;
    for round in 0..u64::from(cfg.rounds) {
        let data = d.write(0, 4)?;
        d.flush()?;
        // Over written blocks, and over blocks only the base has.
        d.zeroes(1, 2)?;
        d.zeroes(6, 2)?;
        let n = BLOCK_USIZE;
        let mut expected = data.clone();
        expected[n..3 * n].fill(0);
        expected.extend(base_bytes(base, 4, 2));
        expected.extend(vec![0; 2 * n]);
        d.expect_read(0, &expected, "R5: after write-zeroes")?;
        d.flush()?;
        d.power_cut_if_able(0)?;
        d.expect_read(0, &expected, "R5: after write-zeroes, flush and power cut")?;
        // Give blocks 6 and 7 data again for the next round.
        d.write(6, 2)?;
        let _ = round;
    }
    Ok(())
}

fn discard_leaves_old_zeros_or_base<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &on_base(h, Spec::CLONE))?;
    for round in 0..u64::from(cfg.rounds) {
        let data = d.write(0, 6)?;
        // Written blocks, and blocks only the base has.
        d.discard(2, 2)?;
        d.discard(8, 2)?;
        // The model accepts old, zeros or base for the discarded blocks.
        // The neighbours are checked here by hand as well.
        for _ in 0..3 {
            let got = d.read(0, 6)?;
            let n = BLOCK_USIZE;
            if got[..2 * n] != data[..2 * n] || got[4 * n..] != data[4 * n..] {
                return fail("R6: a discard changed blocks outside its range");
            }
            d.read(8, 2)?;
        }
        d.flush()?;
        d.power_cut_if_able(round)?;
        d.verify()?;
        d.verify()?;
    }
    Ok(())
}

fn discard_never_hits_later_writes<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &on_base(h, Spec::CLONE))?;
    for _ in 0..cfg.rounds {
        d.write(0, 4)?;
        d.discard(0, 4)?;
        let data = d.write(1, 2)?;
        d.expect_read(1, &data, "R6: a write after a discard")?;
        d.flush()?;
        d.expect_read(1, &data, "R6: a write after a discard, after a flush")?;
        d.power_cut_if_able(0)?;
        d.expect_read(1, &data, "R6: a write after a discard, after a power cut")?;
    }
    Ok(())
}

fn flushed_operations_survive_power_cut<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &on_base(h, Spec::CLONE))?;
    for _ in 0..cfg.rounds {
        let single = d.write(0, 1)?;
        let multi = d.write(2, 8)?;
        d.zeroes(11, 1)?;
        d.flush()?;
        // Unflushed operations elsewhere must not cost the flushed ones.
        d.write(12, 2)?;
        d.power_cut(0)?;
        d.expect_read(0, &single, "R7: a flushed block after a power cut")?;
        d.expect_read(2, &multi, "R7: a flushed write after a cut")?;
        d.expect_read(11, &vec![0; BLOCK_USIZE], "R7: a flushed write-zeroes")?;
        d.verify()?;
    }
    Ok(())
}

fn flush_covers_what_was_acknowledged_before_it<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    for _ in 0..cfg.rounds {
        // An earlier flush, so that a sync "already under way" exists.
        d.flush()?;
        let a = d.write(0, 2)?;
        // The flush, and a write that is acknowledged while it is pending.
        d.submit(Op::Flush)?;
        if d.depth() >= 2 {
            let b = d.write_op(4, 2);
            d.submit(b)?;
        }
        d.drain()?;
        d.power_cut(0)?;
        d.expect_read(0, &a, "R7: a write acknowledged before the flush arrived")?;
        d.verify()?;
    }
    Ok(())
}

fn flush_covers_an_earlier_instance<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    for round in 0..u64::from(cfg.rounds) {
        let w = d.write(1, 3)?;
        d.kill(round)?;
        d.flush()?;
        d.power_cut(0)?;
        d.expect_read(1, &w, "R7: acknowledged, daemon lost, flushed, power cut")?;
        // Recovery itself makes the log durable: no flush needed.
        let w = d.write(5, 2)?;
        d.kill(round)?;
        d.power_cut(0)?;
        d.expect_read(5, &w, "R9: acknowledged, daemon lost, power cut")?;
    }
    Ok(())
}

fn power_cut_at_every_point_leaves_what_a_volatile_cache_disk_may<H: Harness>(
    h: &mut H,
    cfg: &Config,
) -> Outcome {
    let mut point = 0;
    loop {
        let mut d = Driver::new(h, cfg, &on_base(h, Spec::CLONE))?;
        d.write(0, 4)?;
        d.zeroes(6, 2)?;
        d.flush()?;
        // Unflushed: overlapping multi-block writes, a write-zeroes, a
        // discard and writes over it.
        d.write(0, 2)?;
        d.write(1, 4)?;
        d.zeroes(3, 1)?;
        d.discard(8, 2)?;
        d.write(8, 1)?;
        d.write(10, 3)?;
        d.write(11, 1)?;
        let points = d.cut_points()?;
        d.power_cut(point)?;
        d.verify()?;
        d.verify()?;
        if point >= points.min(64) {
            return Ok(());
        }
        point += 1;
    }
}

fn recovered_state_is_durable<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    for round in 0..u64::from(cfg.rounds) {
        d.write(0, 4)?;
        d.flush()?;
        d.write(0, 2)?;
        d.write(4, 2)?;
        d.write(1, 1)?;
        let points = d.cut_points()?;
        d.power_cut(round % (points + 1))?;
        // Reading twice checks stability; the second cut, which keeps
        // nothing that is not durable, checks that what was read is.
        d.verify()?;
        d.verify()?;
        d.power_cut(0)?;
        d.verify()?;
    }
    Ok(())
}

fn first_request_after_power_cut_may_be_a_write<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &on_base(h, Spec::CLONE))?;
    for _ in 0..cfg.rounds {
        let old = d.write(0, 4)?;
        d.zeroes(6, 1)?;
        d.flush()?;
        // Everything is flushed, so nothing needs reading back and the
        // first request after the cut can be a write: where does the
        // first append after recovery go?
        d.power_cut(0)?;
        let new = d.write(2, 4)?;
        d.flush()?;
        d.power_cut(0)?;
        d.expect_read(
            0,
            &old[..2 * BLOCK_USIZE],
            "R10: recovered, then written past",
        )?;
        d.expect_read(2, &new, "R10: the first write after a power cut")?;
        d.expect_read(6, &vec![0; BLOCK_USIZE], "R10: recovered, then left alone")?;
        d.verify()?;
    }
    Ok(())
}

fn queue_depth_requests_complete_in_any_order<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    let depth = (d.geometry.queue_depth() as u64).min(d.window) as usize;
    for _ in 0..4 {
        // queue_depth independent writes, none reaped until all are in.
        for block in 0..depth as u64 {
            let op = d.write_op(block, 1);
            let id = d.submit(op)?;
            if block == 0 && d.geometry.queue_depth() >= 2 {
                // One request in flight and room for more: the same id
                // again must be refused as a duplicate, not as "busy". A
                // target that has completed the request internally still
                // holds its id until the completion is reaped.
                let again = Request { id, op: Op::Flush };
                match d.target()?.submit(again) {
                    Err(e) if e.kind == SubmitErrorKind::DuplicateId => {}
                    other => {
                        return fail(format!(
                            "R8: an id in flight was submitted again and got {other:?}"
                        ))
                    }
                }
            }
        }
        let done = d.drain()?;
        if done.len() != depth {
            return fail(format!(
                "R8: {depth} requests submitted, {} completions",
                done.len()
            ));
        }
        // And as many reads in flight together.
        for block in 0..depth as u64 {
            d.submit(read_op(block, 1))?;
        }
        d.drain()?;
    }
    Ok(())
}

fn overlapping_operations_in_flight_apply_whole<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    let k = d.depth().min(3) as u64;
    if k < 2 {
        return Ok(());
    }
    for round in 0..u64::from(cfg.rounds) {
        // Staggered multi-block writes, each overlapping the next.
        for i in 0..k {
            let op = d.write_op(2 + 2 * i, 4);
            d.submit(op)?;
        }
        d.drain()?;
        d.verify()?;
        // Writes over exactly the same blocks, and a write-zeroes inside.
        for _ in 0..k - 1 {
            let op = d.write_op(12, 4);
            d.submit(op)?;
        }
        d.submit(Op::WriteZeroes {
            offset: 13 * BLOCK_SIZE,
            len: 2 * BLOCK_SIZE,
        })?;
        d.drain()?;
        d.verify()?;
        // The same order must hold in the log.
        let points = d.cut_points()?;
        d.power_cut_if_able(round % (points + 1))?;
        d.verify()?;
        d.flush()?;
    }
    Ok(())
}

fn kill_keeps_acknowledged_operations<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &on_base(h, Spec::CLONE))?;
    for round in 0..u64::from(cfg.rounds) {
        let data = d.write(1, 8)?;
        d.write(12, 1)?;
        d.zeroes(12, 1)?;
        d.kill(round)?;
        d.expect_read(1, &data, "R9: an acknowledged write after a daemon loss")?;
        d.expect_read(
            12,
            &vec![0; BLOCK_USIZE],
            "R9: an acknowledged write-zeroes",
        )?;
        let more = d.write(10, 1)?;
        d.kill(round)?;
        d.expect_read(10, &more, "R9: an acknowledged write after a daemon loss")?;
        d.verify()?;
    }
    Ok(())
}

fn reissue_after_kill_is_idempotent<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let mut d = Driver::new(h, cfg, &Spec::EMPTY)?;
    if d.depth() < 2 {
        return Ok(());
    }
    for round in 0..u64::from(cfg.rounds) {
        let before = d.write(15, 1)?;
        // In flight when the daemon goes: a write, two overlapping
        // writes, a write-zeroes, a flush and a read.
        let lone = d.write_op(0, 1);
        let Op::Write { data: expected, .. } = &lone else {
            return fail("write_op did not build a write");
        };
        let expected = expected.to_vec();
        for op in [
            lone,
            d.write_op(4, 3),
            d.write_op(5, 3),
            Op::WriteZeroes {
                offset: 11 * BLOCK_SIZE,
                len: BLOCK_SIZE,
            },
            Op::Flush,
            read_op(15, 1),
        ] {
            d.submit_with_room(op)?;
        }
        if round % 2 == 1 {
            // Let the target get part of the way.
            d.poll()?;
        }
        d.kill(round)?;
        d.drain()?;
        d.expect_read(0, &expected, "R9: a reissued write")?;
        d.expect_read(15, &before, "R9: a block no reissued request wrote")?;
        d.verify()?;
        // Everything is acknowledged now: a flush makes it durable.
        d.flush()?;
        d.power_cut_if_able(0)?;
        d.expect_read(
            0,
            &expected,
            "R9, R7: a reissued write, flushed, after a cut",
        )?;
        d.verify()?;
    }
    Ok(())
}

fn failed_volume_fails_every_request<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    // What the requests in flight at the failure get depends on the order
    // the target completes them in: try several.
    for seed in 0..16 {
        failed_volume_round(h, cfg, seed)?;
    }
    Ok(())
}

fn failed_volume_round<H: Harness>(h: &mut H, cfg: &Config, seed: u64) -> Outcome {
    let spec = Spec {
        seed,
        ..Spec::EMPTY
    };
    let mut d = Driver::new(h, cfg, &spec)?;
    d.write(0, 2)?;
    d.flush()?;
    d.write(2, 1)?;
    // In flight when the log fails: each may complete either way, and a
    // read that succeeds shows the model's content.
    let invalid = d.can(Capability::InvalidRequests);
    if invalid {
        d.submit_with_room(Op::Discard {
            offset: 1,
            len: BLOCK_SIZE,
        })?;
    }
    let write = d.write_op(6, 1);
    d.submit_with_room(write)?;
    d.submit_with_room(read_op(6, 1))?;
    d.submit_with_room(Op::Flush)?;
    d.submit_with_room(read_op(0, 8))?;
    d.fail_log()?;
    d.drain()?;
    // A flush with nothing to make durable need not notice. A write
    // makes the target meet its log: it fails, or it is acknowledged
    // and then the flush has something to make durable and must fail.
    let meet = d.write_op(5, 1);
    d.run(meet)?;
    let every_request = |d: &mut Driver<'_, H>| -> Outcome {
        d.expect_error(Op::Flush, BlkError::Failed, "R11")?;
        let write = d.write_op(4, 1);
        for op in [
            read_op(0, 1),
            write,
            Op::WriteZeroes {
                offset: 0,
                len: BLOCK_SIZE,
            },
            Op::Discard {
                offset: 0,
                len: BLOCK_SIZE,
            },
            Op::Flush,
        ] {
            d.expect_error(op, BlkError::Failed, "R11")?;
        }
        // An invalid request fails too; which error it gets is not
        // specified, and the model accepts either.
        if invalid {
            d.run(Op::Discard {
                offset: 1,
                len: BLOCK_SIZE,
            })?;
        }
        Ok(())
    };
    every_request(&mut d)?;
    // A new instance must not serve a shorter log to a client that was
    // told more.
    d.kill(1)?;
    every_request(&mut d)
}

fn reopened_failed_volume_holds_what_was_flushed<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    // What is in flight at the failure completes either way, in an order
    // the target chooses: try several.
    for seed in 0..16 {
        let spec = Spec {
            seed,
            ..if seed % 2 == 0 {
                Spec::EMPTY
            } else {
                on_base(h, Spec::CLONE)
            }
        };
        let mut d = Driver::new(h, cfg, &spec)?;
        // Flushed before the failure.
        let flushed = d.write(0, 4)?;
        d.zeroes(6, 1)?;
        d.flush()?;
        // Acknowledged and not flushed: may or may not be there.
        d.write(2, 4)?;
        // In flight at the failure, a flush among them. If that flush
        // succeeds, the model holds the target to what it covered.
        let depth = d.depth();
        if depth >= 3 {
            let write = d.write_op(8, 2);
            d.submit_with_room(write)?;
            d.submit_with_room(Op::Flush)?;
        }
        d.fail_log()?;
        d.drain()?;
        // A write makes the target meet its failed log; the flush
        // behind it must fail whichever way the write went.
        let meet = d.write_op(12, 1);
        d.run(meet)?;
        d.expect_error(Op::Flush, BlkError::Failed, "R11")?;
        if seed % 4 >= 2 {
            // The daemon restarts in between, and still serves nothing.
            d.kill(seed)?;
            d.expect_error(read_op(0, 1), BlkError::Failed, "R11")?;
        }
        d.reopen_failed()?;
        // The model has read back what the failure left open and found a
        // state that includes everything flushed. Blocks nothing touched
        // after the flush are as they were flushed.
        d.expect_read(
            0,
            &flushed[..2 * BLOCK_USIZE],
            "R11: flushed before the failure",
        )?;
        d.verify()?;
        // And it is a healthy volume again.
        let again = d.write(1, 3)?;
        d.flush()?;
        d.power_cut_if_able(0)?;
        d.expect_read(1, &again, "R11: a write after the reopen, flushed")?;
        d.verify()?;
    }
    Ok(())
}

fn restart_after_the_log_failed_recovers_nothing<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    for seed in 0..8 {
        let spec = Spec {
            seed,
            ..if seed % 2 == 0 {
                Spec::EMPTY
            } else {
                on_base(h, Spec::CLONE)
            }
        };
        let mut d = Driver::new(h, cfg, &spec)?;
        let flushed = d.write(0, 4)?;
        d.flush()?;
        // Acknowledged, and no flush covers it.
        d.write(2, 4)?;
        // The log fails and the daemon is lost before any request
        // completes, so before any failure is reported. The new
        // instance's recovery cannot make its prefix durable.
        d.fail_log()?;
        d.kill(seed)?;
        if seed % 4 >= 2 {
            d.kill(seed + 1)?;
        }
        d.reopen_failed()?;
        // The model has read back blocks 2 to 5 and takes either
        // content; what was flushed and not touched since is there.
        d.expect_read(
            0,
            &flushed[..2 * BLOCK_USIZE],
            "R11: flushed before the failure",
        )?;
        d.verify()?;
        let again = d.write(1, 3)?;
        d.flush()?;
        d.power_cut_if_able(0)?;
        d.expect_read(1, &again, "R11: a write after the reopen, flushed")?;
        d.verify()?;
    }
    Ok(())
}

fn read_only_target_refuses_mutations<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let spec = Spec {
        read_only: true,
        ..on_base(h, Spec::CLONE)
    };
    let mut d = Driver::new(h, cfg, &spec)?;
    let base = base_bytes(spec.base, 0, 8);
    for round in 0..2 {
        d.expect_read(0, &base, "R12: a read-only target reads as its base")?;
        let write = d.write_op(0, 2);
        for op in [
            write,
            Op::WriteZeroes {
                offset: 0,
                len: 2 * BLOCK_SIZE,
            },
            Op::Discard {
                offset: 0,
                len: 2 * BLOCK_SIZE,
            },
        ] {
            d.expect_error(op, BlkError::ReadOnly, "R12")?;
        }
        d.flush()?;
        d.expect_read(0, &base, "R12: after refused mutations")?;
        d.kill(round)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Property cases.
// ---------------------------------------------------------------------------

/// One step of a random program. Spans are `(first block, blocks)`.
#[derive(Debug, Clone)]
enum Step {
    Write(u64, u64),
    Read(u64, u64),
    WriteZeroes(u64, u64),
    Discard(u64, u64),
    Flush,
    /// Wait for at least one completion.
    Reap,
    /// Take what is ready without waiting.
    Poll,
    /// Reap until nothing is in flight.
    Drain,
    /// Lose the daemon and reissue what was in flight.
    Kill(u64),
    /// Cut the power, keeping this many unflushed units (wrapping).
    PowerCut(u64),
    /// Make the write log fail for good.
    FailLog,
    /// Open the volume again, if it has failed and the harness can.
    Reopen,
    /// Let the target do its own work between requests.
    Maintain(u64),
    /// Submit a request that must be rejected, chosen by the number.
    Invalid(u64),
}

/// The steps of the random programs. Kinds of step the harness cannot
/// do are left out.
fn step_strategy(window: u64, failures: bool, has: Capabilities) -> impl Strategy<Value = Step> {
    let span = move || (0..window, 1..=8u64).prop_map(move |(b, l)| (b, l.min(window - b)));
    let mut choices: Vec<(u32, BoxedStrategy<Step>)> = vec![
        (16, span().prop_map(|(b, l)| Step::Write(b, l)).boxed()),
        (12, span().prop_map(|(b, l)| Step::Read(b, l)).boxed()),
        (4, span().prop_map(|(b, l)| Step::WriteZeroes(b, l)).boxed()),
        (4, span().prop_map(|(b, l)| Step::Discard(b, l)).boxed()),
        (6, Just(Step::Flush).boxed()),
        (8, Just(Step::Reap).boxed()),
        (4, Just(Step::Poll).boxed()),
        (2, Just(Step::Drain).boxed()),
        (3, any::<u64>().prop_map(Step::Maintain).boxed()),
    ];
    if has.has(Capability::InvalidRequests) {
        choices.push((2, any::<u64>().prop_map(Step::Invalid).boxed()));
    }
    if failures {
        choices.push((4, any::<u64>().prop_map(Step::Kill).boxed()));
        if has.has(Capability::PowerCut) {
            choices.push((4, (0..12u64).prop_map(Step::PowerCut).boxed()));
        }
        if has.has(Capability::FailLog) {
            choices.push((1, Just(Step::FailLog).boxed()));
            choices.push((2, Just(Step::Reopen).boxed()));
        }
    }
    Union::new_weighted(choices)
}

/// Where a random program runs: on which base, and in which window.
#[derive(Debug, Clone)]
struct Place {
    on_clone: bool,
    /// Picks one of the windows of [`placements`].
    window: usize,
    /// The seed handed to the harness.
    seed: u64,
}

/// The first blocks of the windows the random programs run in: the start
/// of the device, the end, and one straddling each boundary the harness
/// names.
fn placements<H: Harness>(h: &mut H, blocks: u64, window: u64) -> Vec<u64> {
    let last = blocks.saturating_sub(window);
    let mut origins = vec![0, last];
    for boundary in h.boundaries() {
        origins.push(boundary.saturating_sub(window / 2).min(last));
    }
    origins.sort_unstable();
    origins.dedup();
    origins
}

fn run_program<H: Harness>(h: &mut H, cfg: &Config, place: &Place, program: &[Step]) -> Outcome {
    let spec = Spec {
        seed: place.seed,
        ..if place.on_clone {
            on_base(h, Spec::CLONE)
        } else {
            Spec::EMPTY
        }
    };
    let mut d = Driver::new(h, cfg, &spec)?;
    let origins = placements(d.harness, d.geometry.blocks(), d.window);
    let origin = origins[place.window % origins.len()];
    d.origin = origin;
    let mut failed = false;
    for step in program {
        // Spans are relative to the window.
        let step = match *step {
            Step::Write(b, l) => Step::Write(origin + b, l),
            Step::Read(b, l) => Step::Read(origin + b, l),
            Step::WriteZeroes(b, l) => Step::WriteZeroes(origin + b, l),
            Step::Discard(b, l) => Step::Discard(origin + b, l),
            ref other => other.clone(),
        };
        match step {
            Step::Write(b, l) => {
                let op = d.write_op(b, l);
                d.submit_with_room(op)?;
            }
            Step::Read(b, l) => {
                d.submit_with_room(read_op(b, l))?;
            }
            Step::WriteZeroes(b, l) => {
                d.submit_with_room(Op::WriteZeroes {
                    offset: b * BLOCK_SIZE,
                    len: l * BLOCK_SIZE,
                })?;
            }
            Step::Discard(b, l) => {
                d.submit_with_room(Op::Discard {
                    offset: b * BLOCK_SIZE,
                    len: l * BLOCK_SIZE,
                })?;
            }
            Step::Flush => {
                d.submit_with_room(Op::Flush)?;
            }
            Step::Reap => {
                d.reap()?;
            }
            Step::Poll => d.poll()?,
            Step::Drain => {
                d.drain()?;
            }
            Step::Kill(seed) => d.kill(seed)?,
            // A failed volume cannot be read, so what it kept cannot be
            // checked.
            Step::PowerCut(_) if failed => {}
            Step::PowerCut(keep) => {
                d.power_cut(keep)?;
                d.verify()?;
            }
            Step::FailLog => {
                d.fail_log()?;
                failed = true;
            }
            Step::Reopen if failed && d.can(Capability::ReopenFailed) => {
                // What was in flight across the failure is gone with it.
                d.reopen_failed()?;
                failed = false;
                d.verify()?;
            }
            Step::Reopen => {}
            Step::Maintain(seed) => d.maintain(seed)?,
            Step::Invalid(pick) => {
                let op = invalid_op(d.geometry, origin, pick);
                d.submit_with_room(op)?;
            }
        }
        d.settle()?;
    }
    d.drain()?;
    d.verify()?;
    if failed {
        if !d.can(Capability::ReopenFailed) {
            return Ok(());
        }
        d.reopen_failed()?;
    }
    // Whatever the program did: the device reads as the model allows now,
    // after a flush and a power cut, and again after that.
    d.flush()?;
    d.power_cut_if_able(0)?;
    d.verify()?;
    d.verify()
}

/// Run `test` on values of `strategy`, reproducibly from the config's
/// seed, and turn a failure into the shrunk input and its reason.
fn check<S: Strategy>(
    cfg: &Config,
    salt: u8,
    strategy: S,
    test: impl Fn(S::Value) -> Outcome,
) -> Outcome {
    let mut seed = [salt; 32];
    seed[..8].copy_from_slice(&cfg.seed.to_le_bytes());
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig {
            cases: cfg.cases,
            failure_persistence: None,
            max_shrink_iters: 8192,
            ..ProptestConfig::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &seed),
    );
    match runner.run(&strategy, |value| {
        test(value).map_err(|f| TestCaseError::fail(f.0))
    }) {
        Ok(()) => Ok(()),
        Err(TestError::Fail(reason, value)) => fail(format!(
            "{reason}\n      minimal failing input (seed {:#x}): {value:?}",
            cfg.seed
        )),
        Err(TestError::Abort(reason)) => fail(format!("property test aborted: {reason}")),
    }
}

fn prop_programs<H: Harness>(h: &mut H, cfg: &Config, failures: bool) -> Outcome {
    let window = cfg.window_blocks.clamp(16, 256);
    let place = (any::<bool>(), 0..8usize, 0..64u64).prop_map(|(on_clone, window, seed)| Place {
        on_clone,
        window,
        seed,
    });
    let strategy = (
        place,
        proptest::collection::vec(
            step_strategy(window, failures, h.capabilities()),
            1..=cfg.max_steps.max(1),
        ),
    );
    let h = RefCell::new(h);
    check(cfg, u8::from(failures), strategy, |(place, program)| {
        run_program(&mut **h.borrow_mut(), cfg, &place, &program)
    })
}

fn prop_concurrent_programs<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    prop_programs(h, cfg, false)
}

fn prop_failing_programs<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    prop_programs(h, cfg, true)
}

fn prop_invalid_requests<H: Harness>(h: &mut H, cfg: &Config) -> Outcome {
    let geometry = Driver::new(h, cfg, &Spec::EMPTY)?.geometry;
    let blocks = geometry.blocks();
    let max_blocks = MAX_IO_BYTES / BLOCK_SIZE;
    // Offsets and lengths around every limit: aligned or not, inside,
    // at and beyond the end, and near u64::MAX.
    let jitter = || prop_oneof![2 => Just(0u64), 1 => 1..BLOCK_SIZE];
    let offset = (
        prop_oneof![
            4 => 0..=8u64,
            4 => blocks.saturating_sub(4)..=blocks + 2,
            1 => Just(u64::MAX / BLOCK_SIZE - 1)
        ],
        jitter(),
    )
        .prop_map(|(b, j)| b * BLOCK_SIZE + j);
    let len = (
        prop_oneof![6 => 0..=6u64, 1 => max_blocks - 1..=max_blocks + 2],
        jitter(),
    )
        .prop_map(|(b, j)| b * BLOCK_SIZE + j);
    let op = (0..4usize, offset, len)
        .prop_map(|(kind, offset, len)| {
            let mut ops = ranged_ops(offset, len);
            let pick = kind % ops.len();
            ops.swap_remove(pick)
        })
        .prop_filter("must be invalid", move |op| geometry.validate(op).is_err());
    let strategy = proptest::collection::vec(op, 1..12);
    let h = RefCell::new(h);
    check(cfg, 2, strategy, |ops| {
        let mut guard = h.borrow_mut();
        let spec = on_base(&**guard, Spec::CLONE);
        let mut d = Driver::new(&mut **guard, cfg, &spec)?;
        let last = d.geometry.blocks() - 1;
        let head = d.write(0, 8)?;
        let tail = d.write(last, 1)?;
        for op in ops {
            d.submit_with_room(op)?;
        }
        d.drain()?;
        d.expect_read(0, &head, "R2: after rejected requests")?;
        d.expect_read(last, &tail, "R2: after rejected requests")?;
        d.verify()
    })
}

#[cfg(test)]
mod tests {
    //! The suite's own parts that cannot be seen from outside.

    use proptest::strategy::ValueTree;

    use super::*;
    use crate::mem::{Fault, MemConfig, MemHarness, MemTarget, Schedule};

    /// The in-memory harness, whose kill first takes what the target
    /// has completed by then, as the device harness takes the calls
    /// that returned before the old server's death, and hands it to the
    /// suite as answered before the loss. With `drops` it drops them,
    /// as the device harness did before, and the suite issues them
    /// again.
    struct HandsOver {
        inner: MemHarness,
        before: Vec<Completion>,
        drops: bool,
    }

    impl Harness for HandsOver {
        type Target = MemTarget;

        fn capabilities(&self) -> Capabilities {
            self.inner.capabilities()
        }

        fn create(&mut self, spec: &Spec) -> Result<MemTarget, HarnessError> {
            self.inner.create(spec)
        }

        fn kill(&mut self, mut target: MemTarget, seed: u64) -> Result<MemTarget, HarnessError> {
            let mut done = Vec::new();
            target.poll(&mut done);
            if !self.drops {
                self.before = done;
            }
            self.inner.kill(target, seed)
        }

        fn completed_before_loss(&mut self, _target: &mut MemTarget) -> Vec<Completion> {
            std::mem::take(&mut self.before)
        }

        fn cut_points(&mut self, target: &MemTarget) -> u64 {
            self.inner.cut_points(target)
        }

        fn power_cut(&mut self, target: MemTarget, keep: u64) -> Result<MemTarget, HarnessError> {
            self.inner.power_cut(target, keep)
        }

        fn fail_log(&mut self, target: &mut MemTarget) -> Result<(), HarnessError> {
            self.inner.fail_log(target)
        }

        fn reopen_failed(&mut self, target: MemTarget) -> Result<MemTarget, HarnessError> {
            self.inner.reopen_failed(target)
        }
    }

    /// R9: what the harness hands over at a loss as answered before it
    /// counts from before the loss. A write that the lost daemon
    /// answered, and whose answer the client had not taken, must read
    /// back after the loss; the suite does not issue it again. A target
    /// that acknowledged it from memory and lost it at the kill is
    /// caught. Were the answer dropped and the write issued again, that
    /// target would apply it once more after the loss and pass.
    #[test]
    fn an_answer_handed_over_at_a_loss_counts_from_before_it() {
        let run = |fault: Option<Fault>, drops: bool| -> Result<bool, Failure> {
            let config = MemConfig {
                schedule: Schedule::Fifo,
                fault,
                ..MemConfig::default()
            };
            let mut harness = HandsOver {
                inner: MemHarness::new(config),
                before: Vec::new(),
                drops,
            };
            let suite = Config::default();
            let mut d = Driver::new(&mut harness, &suite, &Spec::EMPTY)?;
            let op = d.write_op(3, 2);
            let id = d.submit(op)?;
            d.kill(1)?;
            let issued_again = d.inflight.contains_key(&id);
            d.drain()?;
            d.read(3, 2)?;
            Ok(issued_again)
        };
        assert_eq!(run(None, false), Ok(false), "answered before the loss");
        assert_eq!(run(None, true), Ok(true), "dropped: issued again");
        let caught = run(Some(Fault::KillLosesUnflushed), false);
        assert!(caught.is_err(), "the lost write is not caught: {caught:?}");
        assert_eq!(
            run(Some(Fault::KillLosesUnflushed), true),
            Ok(true),
            "with the answer dropped the write is applied again and the loss is masked"
        );
    }

    /// R9, the order: what the harness hands over at a loss is accounted
    /// before the model hears of the loss. Two writes of one block are
    /// in flight; the lost daemon answers one of them before the loss
    /// and the harness hands it over, and the other is issued again, so
    /// the block must hold the one issued again. A target that skips the
    /// reissue of a write its lost instance had logged leaves the
    /// answered one there. Told of the loss first, the model takes both
    /// as issued again, in either order, and accepts that. Some of the
    /// seeded schedules reach the case. (From the review of the pull
    /// request: caught at 29 of 200 seeds in this order, at none with
    /// the two steps of `kill` swapped.)
    #[test]
    fn what_is_handed_over_is_accounted_before_the_loss() {
        let run = |fault: Option<Fault>, seed: u64| -> Result<(), Failure> {
            let config = MemConfig {
                schedule: Schedule::Seeded,
                apply: crate::mem::Timing::AtSubmit,
                fault,
                seed,
                ..MemConfig::default()
            };
            let mut harness = HandsOver {
                inner: MemHarness::new(config),
                before: Vec::new(),
                drops: false,
            };
            let suite = Config::default();
            let mut d = Driver::new(&mut harness, &suite, &Spec::EMPTY)?;
            let a = d.write_op(3, 1);
            let b = d.write_op(3, 1);
            d.submit(a)?;
            d.submit(b)?;
            d.kill(seed)?;
            d.drain()?;
            d.read(3, 1).map(drop)
        };
        for seed in 0..200 {
            assert_eq!(run(None, seed), Ok(()), "a correct target, seed {seed}");
        }
        let caught = (0..200)
            .filter(|&seed| run(Some(Fault::SkipReissuedIfLogged), seed).is_err())
            .count();
        assert!(caught > 0, "a skipped reissue is never caught");
    }

    /// The kinds of step in `samples` programs' worth of steps.
    fn kinds(failures: bool, has: Capabilities) -> Vec<&'static str> {
        let strategy = step_strategy(24, failures, has);
        let mut runner = TestRunner::deterministic();
        let mut seen = Vec::new();
        for _ in 0..4000 {
            let step = strategy.new_tree(&mut runner).unwrap().current();
            let kind = match step {
                Step::Write(..) => "write",
                Step::Read(..) => "read",
                Step::WriteZeroes(..) => "write_zeroes",
                Step::Discard(..) => "discard",
                Step::Flush => "flush",
                Step::Reap => "reap",
                Step::Poll => "poll",
                Step::Drain => "drain",
                Step::Kill(_) => "kill",
                Step::PowerCut(_) => "power_cut",
                Step::FailLog => "fail_log",
                Step::Reopen => "reopen",
                Step::Maintain(_) => "maintain",
                Step::Invalid(_) => "invalid",
            };
            if !seen.contains(&kind) {
                seen.push(kind);
            }
        }
        seen.sort_unstable();
        seen
    }

    /// Not a contract rule: the random programs of a harness that can
    /// do everything contain every kind of step, and those of a harness
    /// that lacks a capability leave out exactly the steps that need
    /// it. A strategy that dropped a kind of step for everyone would
    /// otherwise pass every test.
    #[test]
    fn the_random_programs_have_every_kind_of_step_the_harness_can_do() {
        let plain = [
            "discard",
            "drain",
            "flush",
            "invalid",
            "maintain",
            "poll",
            "read",
            "reap",
            "write",
            "write_zeroes",
        ];
        let with = |more: &[&'static str]| {
            let mut all: Vec<&str> = plain.iter().chain(more).copied().collect();
            all.sort_unstable();
            all
        };
        let without = |all: Vec<&'static str>, gone: &[&str]| -> Vec<&'static str> {
            all.into_iter().filter(|k| !gone.contains(k)).collect()
        };
        let failing = with(&["kill", "power_cut", "fail_log", "reopen"]);
        assert_eq!(kinds(false, Capabilities::ALL), with(&[]));
        assert_eq!(kinds(true, Capabilities::ALL), failing);
        for (capability, gone) in [
            (Capability::PowerCut, &["power_cut"][..]),
            (Capability::FailLog, &["fail_log", "reopen"][..]),
            (Capability::InvalidRequests, &["invalid"][..]),
            (Capability::Base, &[][..]),
            (Capability::ReadOnly, &[][..]),
            (Capability::ReopenFailed, &[][..]),
        ] {
            let has = Capabilities::ALL.without(capability);
            assert_eq!(
                kinds(true, has),
                without(failing.clone(), gone),
                "without {capability:?}"
            );
        }
    }
}
