// Review scratch tests for skyzfs/skyzfs#150 at d629834 (the next look).
// Appended for a run to the end of bins/skyzfsd/src/kill/tests.rs at that
// head, then the file is restored (run150n.sh). They use that module's
// helpers (log, noted, heard, taken, block_on, placed, offered, request,
// asked, Holds, Rig, rig, steps, accept, published_on, hooks, STATE).
// Tests named r150n_show_* pass when the behaviour they show is present:
// each is a demonstration, not a wish.

/// One step of the cut, or of the publishes, as the uploader's turns
/// choose them (`Driver::step` always gives a publish with work the step
/// first; the uploader does not: `uploader.rs`, `Turns::next`).
fn cut_step(driver: &mut Rig, seen: &mut Vec<String>) {
    let step = block_on(driver.step_cut());
    seen.push(format!("cut: {step:?}"));
}

fn publish_step(driver: &mut Rig, seen: &mut Vec<String>) {
    let step = block_on(driver.step_publish());
    seen.push(format!("publish: {step:?}"));
}

/// T: the window the module docs leave open, built in the order the
/// uploader's turns take after the carrier's commit when the publish is
/// ahead in the time the two sides have had since both became due (its
/// lookup took longer than the next cut's check and read; `Turns::next`
/// and its test `a_slow_step_of_a_publish_gives_the_cut_as_long`). Case A:
/// publish 1 is accepted while cut 7, its carrier, uploads; the lookup
/// takes a turn; once 7 has committed the publish is due, but the cut has
/// the next turns: cut 8 is checked, read and committed before the
/// publish's transaction. Each hook kills at cut 8, and the publish's
/// next step makes it on cut 7. No fault is needed.
#[test]
fn r150n_show_t_the_uploaders_turns_let_the_hook_kill_at_the_next_cut() {
    for kill in [KillAt::CarrySwap(1), KillAt::CarryPut(1)] {
        let held = vec![
            (offered(7, 7, true), 2, false),
            (offered(8, 8, true), 1, false),
        ];
        let (mut driver, carry, _faults, said) = rig(kill, held);
        let mut seen = Vec::new();
        cut_step(&mut driver, &mut seen); // Began 7
        cut_step(&mut driver, &mut seen); // Read 7, its first slice
        accept(&mut driver, &carry);
        publish_step(&mut driver, &mut seen); // the lookup
        cut_step(&mut driver, &mut seen); // Read 7, its last slice
        cut_step(&mut driver, &mut seen); // Uploaded 7
        let due = driver.publish_due();
        cut_step(&mut driver, &mut seen); // Began 8
        cut_step(&mut driver, &mut seen); // Read 8: chosen
        cut_step(&mut driver, &mut seen); // Uploaded 8
        let before = hooks(&taken(&said));
        publish_step(&mut driver, &mut seen); // the publish's transaction
        let after = hooks(&taken(&said));
        eprintln!(
            "{kill:?}\nsteps: {seen:#?}\npublish due after 7's commit: {due}\nthe hook before \
             the publish's step: {before:#?}\nafter it: {after:#?}"
        );
        assert!(due, "{kill:?}: the publish was due when cut 8 was checked");
        assert!(
            before
                .last()
                .is_some_and(|s| s.starts_with("kill") && s.contains("of cut 8 (group 8)")),
            "{kill:?}: the kill fell at cut 8: {before:?}"
        );
        assert!(published_on(&seen, 7), "{kill:?}: publish 1 made on cut 7: {seen:?}");
    }
}

/// T in case B: cut 7, the carrier, committed before publish 1 was
/// accepted. The publish's lookup takes a turn; its transaction is then
/// due at once, but where the lookup took longer than the cut's turns
/// since both became due, the cut has the next turns: cut 8 is checked,
/// read (chosen) and committed first. Each hook kills at cut 8, and the
/// publish's next step makes it on cut 7. No fault is needed.
#[test]
fn r150n_show_t_case_b_the_uploaders_turns_let_the_hook_kill_at_the_next_cut() {
    for kill in [KillAt::CarrySwap(1), KillAt::CarryPut(1)] {
        let held = vec![
            (offered(7, 7, true), 1, false),
            (offered(8, 8, true), 1, false),
        ];
        let (mut driver, carry, _faults, said) = rig(kill, held);
        let mut seen = Vec::new();
        cut_step(&mut driver, &mut seen); // Began 7
        cut_step(&mut driver, &mut seen); // Read 7
        cut_step(&mut driver, &mut seen); // Uploaded 7
        accept(&mut driver, &carry);
        publish_step(&mut driver, &mut seen); // the lookup
        let due = driver.publish_due();
        cut_step(&mut driver, &mut seen); // Began 8
        cut_step(&mut driver, &mut seen); // Read 8: chosen
        cut_step(&mut driver, &mut seen); // Uploaded 8
        let before = hooks(&taken(&said));
        publish_step(&mut driver, &mut seen); // the publish's transaction
        let after = hooks(&taken(&said));
        eprintln!(
            "{kill:?}\nsteps: {seen:#?}\npublish due after the lookup: {due}\nthe hook before \
             the publish's step: {before:#?}\nafter it: {after:#?}"
        );
        assert!(due, "{kill:?}: the publish was due when cut 8 was checked");
        assert!(
            before
                .last()
                .is_some_and(|s| s.starts_with("kill") && s.contains("of cut 8 (group 8)")),
            "{kill:?}: the kill fell at cut 8: {before:?}"
        );
        assert!(published_on(&seen, 7), "{kill:?}: publish 1 made on cut 7: {seen:?}");
    }
}

/// R1: the publish's transaction fails before it lands, and the publish
/// waits its first second (`Publishing::wait_after`, which the uploader
/// calls after a failed publish step). The cut has the steps meanwhile:
/// `carry-swap` kills at cut 8, and the publish, once its wait is over,
/// is made on cut 7. This is the window the module docs name, built with
/// the driver through a failure.
#[test]
fn r150n_show_r1_a_failed_publish_step_lets_the_hook_kill_at_the_next_cut() {
    let held = vec![
        (offered(7, 7, true), 2, false),
        (offered(8, 8, true), 1, false),
    ];
    let (mut driver, carry, faults, said) = rig(KillAt::CarrySwap(1), held);
    let mut seen = steps(&mut driver, &said, 2); // Began 7, Read 7
    accept(&mut driver, &carry);
    seen.extend(steps(&mut driver, &said, 3)); // the lookup, Read 7, Uploaded 7
    block_on(faults.arm(FaultOp::Put, "/refs", 0));
    let failed = block_on(driver.step());
    block_on(faults.disarm());
    seen.push(format!("{failed:?}"));
    assert!(failed.is_err(), "the publish's transaction failed: {seen:?}");
    let wait = driver
        .publish_failed(tokio::time::Instant::now())
        .expect("the publish waits");
    seen.extend(steps(&mut driver, &said, 3)); // Began 8, Read 8, Uploaded 8
    let before = hooks(&taken(&said));
    assert!(wait <= std::time::Duration::from_secs(30), "{wait:?}");
    std::thread::sleep(wait + std::time::Duration::from_millis(50));
    seen.extend(steps(&mut driver, &said, 3));
    let after = hooks(&taken(&said));
    eprintln!("steps: {seen:#?}\nwait: {wait:?}\nthe hook: {before:#?}\nafter: {after:#?}");
    assert!(
        before.last().is_some_and(|s| s.starts_with(
            "kill carry-swap:1: after the commit of cut 8 (group 8), chosen to carry publish 1"
        )),
        "{before:?}"
    );
    assert!(published_on(&seen, 7), "publish 1 made on cut 7: {seen:?}");
}

/// R2: the publish's transaction lands and its call comes back failed
/// (the refs object's write takes effect, then an error). `KillRemote::
/// publish` returns at the error and does not tell the hook, so publish 1
/// is made on cut 7 and the hook still kills at cut 8, after it; once
/// the publish's wait is over it is found published on cut 7. The module
/// docs say the hook gives up "where publish N is made, on any cut, ...
/// before the hook has fired".
#[test]
fn r150n_show_r2_a_publish_landed_whose_call_failed_lets_the_hook_kill_after_it() {
    let held = vec![
        (offered(7, 7, true), 2, false),
        (offered(8, 8, true), 1, false),
    ];
    let (mut driver, carry, faults, said) = rig(KillAt::CarrySwap(1), held);
    let mut seen = steps(&mut driver, &said, 2);
    accept(&mut driver, &carry);
    seen.extend(steps(&mut driver, &said, 3));
    let _ = taken(&said);
    block_on(faults.arm_after(FaultOp::Put, "/refs", 0));
    let failed = block_on(driver.step());
    block_on(faults.disarm());
    seen.push(format!("{failed:?}"));
    assert!(failed.is_err(), "the publish's call failed: {seen:?}");
    let wait = driver
        .publish_failed(tokio::time::Instant::now())
        .expect("the publish waits");
    seen.extend(steps(&mut driver, &said, 3));
    let log = taken(&said);
    assert!(wait <= std::time::Duration::from_secs(30), "{wait:?}");
    std::thread::sleep(wait + std::time::Duration::from_millis(50));
    seen.extend(steps(&mut driver, &said, 3));
    eprintln!("steps: {seen:#?}\nlog: {log:#?}");
    let snap = log
        .iter()
        .position(|s| s.starts_with("put ") && s.contains("/snap/"))
        .expect("the publish's snap object");
    let swap = log
        .iter()
        .skip(snap)
        .position(|s| s.starts_with("put_if ") && s.ends_with("/refs"))
        .map(|i| i + snap)
        .expect("the publish's transaction landed");
    let kill = log
        .iter()
        .position(|s| s.starts_with("kill "))
        .expect("a kill");
    assert!(swap < kill, "the transaction landed before the kill: {log:?}");
    assert!(
        log[kill]
            .starts_with("kill carry-swap:1: after the commit of cut 8 (group 8), chosen to carry"),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|s| s.contains("no kill")),
        "the hook did not give up: {log:?}"
    );
    assert!(
        seen.iter()
            .any(|s| s.contains("Published {") && s.contains("number: 7") && s.contains("found: true")),
        "found published on cut 7: {seen:?}"
    );
}

/// A store that holds whole cuts, as `Holds`, and writes its releases down
/// in the log, so that the log shows whether a kill came before the cut
/// was let go.
struct HoldsSaid {
    holds: Holds,
    said: Said,
}

impl Store for HoldsSaid {
    fn uploadable(&mut self) -> Option<Uploadable> {
        Store::uploadable(&mut self.holds)
    }
    fn read(&mut self, number: u64, from: u64, buf: &mut Buf) -> std::io::Result<CutSlice> {
        Store::read(&mut self.holds, number, from, buf)
    }
    fn begin_sync_copies(&mut self, number: u64) -> std::io::Result<()> {
        Store::begin_sync_copies(&mut self.holds, number)
    }
    fn copies_synced(&mut self, wait: bool) -> Option<std::io::Result<()>> {
        Store::copies_synced(&mut self.holds, wait)
    }
    fn release(&mut self, number: u64) -> Released {
        self.said
            .lock()
            .expect("the log")
            .push(format!("release {number}"));
        Store::release(&mut self.holds, number)
    }
    fn name_committed(&mut self, number: u64, group: u64) -> bool {
        Store::name_committed(&mut self.holds, number, group)
    }
    fn abandon_all(&mut self) -> Vec<u64> {
        Store::abandon_all(&mut self.holds)
    }
    fn cuts(&mut self) -> CutsNow {
        Store::cuts(&mut self.holds)
    }
}

/// F2's fix: `carry-swap` fires at the chosen cut's release where its
/// commit's call failed, and before the cut is let go, as its line says.
/// No test of the pull request orders the kill against the release.
#[test]
fn r150n_carry_swap_at_the_release_fires_before_the_cut_is_let_go() {
    let (store, said) = log();
    let (remote, _, epoch) = placed(&store);
    let faults = FaultStore::new(store);
    let refs = Arc::new(ObjectRefStore::new(faults.clone()));
    let die = noted(&said);
    let kill = KillAt::CarrySwap(1);
    let carry = Carry::new(Some(kill), Arc::clone(&die), heard(&said)).expect("a carry hook");
    let mut make = maker(
        faults.clone(),
        refs,
        "node-a".into(),
        remote.id,
        Some(kill),
        die,
        Some(Arc::clone(&carry)),
    );
    let holds = HoldsSaid {
        holds: Holds {
            held: vec![
                (offered(7, 7, true), 1, false),
                (offered(8, 8, true), 1, false),
            ],
        },
        said: Arc::clone(&said),
    };
    let mut driver = skyzfs_upload::Driver::with_slices(
        KillLocal::new(holds, Arc::clone(&carry)),
        make(epoch),
        1,
    )
    .with_publishing(skyzfs_upload::Publishing::new(
        Some([0xa1; 16]),
        Some(STATE),
    ));
    let _ = taken(&said);
    assert_eq!(
        driver.publishing().ask(request("t", 6, 7)),
        Answer::Accepted
    );
    asked(&carry, request("t", 6, 7), Answer::Accepted);
    for _ in 0..3 {
        let _ = block_on(driver.step());
    }
    block_on(faults.arm_after(FaultOp::Put, "/refs", 0));
    let _ = block_on(driver.step());
    block_on(faults.disarm());
    let _ = block_on(driver.step());
    let log = taken(&said);
    let kill = log.iter().position(|s| s.starts_with("kill ")).expect("a kill");
    let release = log
        .iter()
        .position(|s| s == "release 7")
        .expect("cut 7 let go");
    eprintln!("{log:#?}");
    assert!(kill < release, "killed before the cut was let go: {log:?}");
}
