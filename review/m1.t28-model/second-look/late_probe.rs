//! Review probe: a harness that says completions can be held past a
//! daemon loss lets the skipped reissue pass. This holds that the driver
//! asks the harness, and shows the blind spot the documentation names.
use skyzfs_blk::conformance::{run_all, Capabilities, Config, Harness, HarnessError, Spec};
use skyzfs_blk::mem::{Fault, MemConfig, MemHarness, MemTarget};
use skyzfs_blk::model::InFlight;

struct Late(MemHarness);

impl Harness for Late {
    type Target = MemTarget;
    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }
    fn create(&mut self, spec: &Spec) -> Result<MemTarget, HarnessError> {
        self.0.create(spec)
    }
    fn kill(&mut self, target: MemTarget, seed: u64) -> Result<MemTarget, HarnessError> {
        self.0.kill(target, seed)
    }
    fn in_flight_at_a_loss(&self) -> InFlight {
        InFlight::MayHaveCompleted
    }
    fn cut_points(&mut self, target: &MemTarget) -> u64 {
        self.0.cut_points(target)
    }
    fn power_cut(&mut self, target: MemTarget, keep: u64) -> Result<MemTarget, HarnessError> {
        self.0.power_cut(target, keep)
    }
    fn fail_log(&mut self, target: &mut MemTarget) -> Result<(), HarnessError> {
        self.0.fail_log(target)
    }
    fn reopen_failed(&mut self, target: MemTarget) -> Result<MemTarget, HarnessError> {
        self.0.reopen_failed(target)
    }
}

fn run(fault: Option<Fault>) -> Vec<&'static str> {
    let config = MemConfig { queue_depth: 16, fault, ..MemConfig::default() };
    let suite = Config { cases: 64, ..Config::default() };
    let report = run_all(&mut Late(MemHarness::new(config)), &suite);
    report.failed()
}

#[test]
fn a_harness_that_claims_late_completions_lets_the_skipped_reissue_pass() {
    assert!(run(Some(Fault::SkipReissuedIfLogged)).is_empty());
}

#[test]
fn a_correct_target_passes_under_the_wider_reading() {
    assert!(run(None).is_empty());
}

#[test]
fn other_faults_at_a_daemon_loss_are_still_caught_under_the_wider_reading() {
    for fault in [Fault::KillLosesUnflushed, Fault::RejectReissued] {
        assert!(!run(Some(fault)).is_empty(), "{fault:?}");
    }
}
