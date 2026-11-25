// Review scratch tests for skyzfs/skyzfs#150 (M2.6c-4). Appended for a
// run to the end of bins/skyzfsd/src/kill/tests.rs at the head under
// review, then the file is restored (run150.sh). They use that module's
// helpers (log, noted, taken, placed, block_on, offered, reads, request,
// asked, Offers, STATE). Tests named r150_show_* pass when the behaviour
// they show is present: each is a demonstration, not a wish.

/// A store that holds whole cuts of `blocks` blocks of data each, read
/// one block a slice, and lets a cut go at its release. Blocks may be
/// zeros (`zero`): read as a run of zeros, no data.
struct Holds {
    held: Vec<(Uploadable, u64, bool)>,
}

impl Store for Holds {
    fn uploadable(&mut self) -> Option<Uploadable> {
        self.held.first().map(|(cut, _, _)| *cut)
    }
    fn read(&mut self, number: u64, from: u64, buf: &mut Buf) -> std::io::Result<CutSlice> {
        let Some(&(_, blocks, zero)) = self.held.iter().find(|(c, _, _)| c.number == number)
        else {
            return Err(std::io::Error::other("gone"));
        };
        let next = (from + 1 < blocks).then_some(from + 1);
        if zero {
            return Ok(CutSlice {
                blocks: Vec::new(),
                zeroes: vec![(from, 1)],
                next,
                copied: false,
            });
        }
        buf.set_blocks(1)
            .map_err(|_| std::io::Error::other("the buffer"))?;
        buf.fill((number + from + 1) as u8);
        Ok(CutSlice {
            blocks: vec![from],
            zeroes: Vec::new(),
            next,
            copied: false,
        })
    }
    fn begin_sync_copies(&mut self, _: u64) -> std::io::Result<()> {
        Ok(())
    }
    fn copies_synced(&mut self, _: bool) -> Option<std::io::Result<()>> {
        Some(Ok(()))
    }
    fn release(&mut self, number: u64) -> Released {
        self.held.retain(|(c, _, _)| c.number != number);
        Released::LetGo
    }
    fn name_committed(&mut self, _: u64, _: u64) -> bool {
        false
    }
    fn abandon_all(&mut self) -> Vec<u64> {
        Vec::new()
    }
    fn cuts(&mut self) -> CutsNow {
        CutsNow::default()
    }
}

type Rig = skyzfs_upload::Driver<
    KillLocal<Holds>,
    KillRemote<
        EngineRemote<
            KillStore<FaultStore<Log>>,
            Arc<skynbd_refs::ActingRefStore<Arc<ObjectRefStore<FaultStore<Log>>>>>,
        >,
    >,
>;

/// The real driver over `held`, with the remote the daemon's test-hooks
/// build makes (`kill::maker`) for `kill`, over a store in memory that
/// can fail a write after it has landed; the hook, the faults and the log.
fn rig(kill: KillAt, held: Vec<(Uploadable, u64, bool)>) -> (Rig, Arc<Carry>, FaultStore<Log>, Said) {
    let (store, said) = log();
    let (remote, _, epoch) = placed(&store);
    let faults = FaultStore::new(store);
    let refs = Arc::new(ObjectRefStore::new(faults.clone()));
    let die = noted(&said);
    let carry = Carry::new(Some(kill), Arc::clone(&die)).expect("a carry hook");
    let mut make = maker(
        faults.clone(),
        refs,
        "node-a".into(),
        remote.id,
        Some(kill),
        die,
        Some(Arc::clone(&carry)),
    );
    let driver = skyzfs_upload::Driver::with_slices(
        KillLocal::new(Holds { held }, Arc::clone(&carry)),
        make(epoch),
        1,
    )
    .with_publishing(skyzfs_upload::Publishing::new(Some([0xa1; 16]), Some(STATE)));
    let _ = taken(&said);
    (driver, carry, faults, said)
}

/// Step until the hook kills or `n` steps: the steps, by their debug form.
fn steps(driver: &mut Rig, said: &Said, n: usize) -> Vec<String> {
    let mut seen = Vec::new();
    for _ in 0..n {
        let step = block_on(driver.step());
        seen.push(format!("{step:?}"));
        if said
            .lock()
            .expect("the log")
            .iter()
            .any(|s| s.starts_with("kill"))
        {
            break;
        }
    }
    seen
}

/// Accept publish `t` (C 6, G 7) in the driver's publishes and in the
/// hook, as the daemon's thread would.
fn accept(driver: &mut Rig, carry: &Carry) {
    assert_eq!(
        driver.publishing().ask(request("t", 6, 7)),
        Answer::Accepted
    );
    asked(carry, request("t", 6, 7), Answer::Accepted);
}

fn kill_line(said: &[String]) -> Option<String> {
    said.iter().find(|s| s.starts_with("kill")).cloned()
}

/// D: the carrier's commit lands but its call comes back failed (the refs
/// object's PUT took effect, then an error). The driver sees the row name
/// the cut at its next step and lets it go without the remote's commit
/// answering Ok, so `carry-swap` never sees it; the next cut is chosen,
/// and the hook kills after ITS commit, saying it "carries publish 1",
/// while publish 1 was made on the cut before.
#[test]
fn r150_show_d_carry_swap_falls_at_a_later_cut_when_the_carriers_commit_came_back_failed() {
    let held = vec![(offered(7, 7, true), 1, false), (offered(8, 8, true), 1, false)];
    let (mut driver, carry, faults, said) = rig(KillAt::CarrySwap(1), held);
    accept(&mut driver, &carry);
    // The lookup, the check of 7 and its read.
    let mut seen = steps(&mut driver, &said, 3);
    // Cut 7's commit: the swap lands, its answer is an error.
    block_on(faults.arm_after(FaultOp::Put, "/refs", 0));
    seen.extend(steps(&mut driver, &said, 1));
    block_on(faults.disarm());
    seen.extend(steps(&mut driver, &said, 12));
    let log = taken(&said);
    eprintln!("steps: {seen:#?}\nlog: {log:#?}");
    let published_on_7 = seen
        .iter()
        .any(|s| s.contains("Published {") && s.contains("number: 7"));
    let kill = kill_line(&log).unwrap_or_default();
    eprintln!("published on cut 7: {published_on_7}; kill line: {kill}");
    assert!(published_on_7, "publish 1 made on cut 7: {seen:?}");
    assert!(
        kill.contains("cut 8") && kill.contains("which carries publish 1"),
        "the kill fell at cut 8: {kill}"
    );
}

/// A: the carrier's reading began before publish 1 was accepted (the
/// uploader answers between its steps, so a publish is accepted while a
/// cut uploads). Publish 1 is made on that cut; the hook chooses the next
/// and kills after its commit, saying it "carries publish 1".
#[test]
fn r150_show_a_carry_swap_falls_at_the_next_cut_when_the_carrier_was_read_before() {
    let held = vec![(offered(7, 7, true), 2, false), (offered(8, 8, true), 1, false)];
    let (mut driver, carry, _faults, said) = rig(KillAt::CarrySwap(1), held);
    // The check of 7 and its first slice, then the publish.
    let mut seen = steps(&mut driver, &said, 2);
    accept(&mut driver, &carry);
    seen.extend(steps(&mut driver, &said, 14));
    let log = taken(&said);
    let published_on_7 = seen
        .iter()
        .any(|s| s.contains("Published {") && s.contains("number: 7"));
    let kill = kill_line(&log).unwrap_or_default();
    eprintln!("steps: {seen:#?}\npublished on cut 7: {published_on_7}; kill line: {kill}");
    assert!(published_on_7, "publish 1 made on cut 7: {seen:?}");
    assert!(kill.contains("cut 8"), "the kill fell at cut 8: {kill}");
}

/// The same with carry-put: the first segment PUT of cut 8 kills, after
/// publish 1 was made on cut 7.
#[test]
fn r150_show_a_carry_put_falls_at_the_next_cut_when_the_carrier_was_read_before() {
    let held = vec![(offered(7, 7, true), 2, false), (offered(8, 8, true), 1, false)];
    let (mut driver, carry, _faults, said) = rig(KillAt::CarryPut(1), held);
    let mut seen = steps(&mut driver, &said, 2);
    accept(&mut driver, &carry);
    seen.extend(steps(&mut driver, &said, 14));
    let log = taken(&said);
    let published_on_7 = seen
        .iter()
        .any(|s| s.contains("Published {") && s.contains("number: 7"));
    let kill = kill_line(&log).unwrap_or_default();
    eprintln!("steps: {seen:#?}\npublished on cut 7: {published_on_7}; kill line: {kill}");
    assert!(published_on_7, "publish 1 made on cut 7: {seen:?}");
    assert!(kill.contains("cut 8"), "the kill fell at cut 8: {kill}");
}

/// B: the carrier was committed before publish 1 was accepted: the
/// lookup gives it to the publish at once, and the hook kills after the
/// commit of the next cut, saying it "carries publish 1".
#[test]
fn r150_show_b_carry_swap_falls_at_a_later_cut_when_the_carrier_was_committed_before() {
    let held = vec![(offered(7, 7, true), 1, false), (offered(8, 8, true), 1, false)];
    let (mut driver, carry, _faults, said) = rig(KillAt::CarrySwap(1), held);
    let mut seen = steps(&mut driver, &said, 3);
    accept(&mut driver, &carry);
    seen.extend(steps(&mut driver, &said, 14));
    let log = taken(&said);
    let published_on_7 = seen
        .iter()
        .any(|s| s.contains("Published {") && s.contains("number: 7"));
    let kill = kill_line(&log).unwrap_or_default();
    eprintln!("steps: {seen:#?}\npublished on cut 7: {published_on_7}; kill line: {kill}");
    assert!(published_on_7, "publish 1 made on cut 7: {seen:?}");
    assert!(
        kill.contains("cut 8") && kill.contains("which carries publish 1"),
        "the kill fell at cut 8: {kill}"
    );
}

/// Z: the carrier puts no segment (its blocks read as zeros), so
/// carry-put passes it over (as documented) and kills at the next cut's
/// first segment, after publish 1 was made on the carrier.
#[test]
fn r150_show_z_carry_put_passes_over_a_carrier_with_no_segment() {
    let held = vec![(offered(7, 7, true), 1, true), (offered(8, 8, true), 1, false)];
    let (mut driver, carry, _faults, said) = rig(KillAt::CarryPut(1), held);
    accept(&mut driver, &carry);
    let seen = steps(&mut driver, &said, 16);
    let log = taken(&said);
    let published_on_7 = seen
        .iter()
        .any(|s| s.contains("Published {") && s.contains("number: 7"));
    let kill = kill_line(&log).unwrap_or_default();
    eprintln!("steps: {seen:#?}\nlog: {log:#?}\npublished on cut 7: {published_on_7}; kill line: {kill}");
    assert!(published_on_7, "publish 1 made on cut 7: {seen:?}");
    assert!(kill.contains("cut 8"), "the kill fell at cut 8: {kill}");
}

/// The control: the carrier is read after the acceptance and commits as
/// it should; carry-swap falls after ITS commit, before the publish.
#[test]
fn r150_control_carry_swap_falls_at_the_carrier_read_after_the_acceptance() {
    let held = vec![(offered(7, 7, true), 1, false), (offered(8, 8, true), 1, false)];
    let (mut driver, carry, _faults, said) = rig(KillAt::CarrySwap(1), held);
    accept(&mut driver, &carry);
    let seen = steps(&mut driver, &said, 16);
    let log = taken(&said);
    let kill = kill_line(&log).unwrap_or_default();
    eprintln!("steps: {seen:#?}\nkill line: {kill}");
    assert!(kill.contains("cut 7"), "{kill}");
    assert!(
        !seen.iter().any(|s| s.contains("Published {")),
        "killed before the publish's transaction: {seen:?}"
    );
}

/// Open point 3: the same publish asked again while it pends is accepted
/// again and counted again, so carry-put:2 arms on publish 1's request.
#[test]
fn r150_show_3_a_publish_asked_again_counts_as_publish_2() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry = Carry::new(Some(KillAt::CarryPut(2)), Arc::clone(&die)).expect("a carry hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    assert_eq!(carry.accepted(), 2);
    reads(&mut local, offered(6, 10, true));
    block_on(store.put("vols/a/seg/1", Bytes::from_static(b"x"))).expect("landed");
    let log = taken(&said);
    eprintln!("{log:#?}");
    assert!(
        log.iter().any(|s| s.contains("chosen to carry publish 2")),
        "{log:?}"
    );
}

/// The hook never armed (publish N never accepted) says nothing at all:
/// a run that ends without the kill has no line of the hook's to read.
#[test]
fn r150_show_n_an_unreached_hook_says_nothing() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry = Carry::new(Some(KillAt::CarryPut(2)), Arc::clone(&die)).expect("a carry hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    for n in 6..20 {
        reads(&mut local, offered(n, n + 5, true));
        block_on(store.put(&format!("vols/a/seg/{n}"), Bytes::from_static(b"x"))).expect("landed");
    }
    let log = taken(&said);
    assert!(log.iter().all(|s| s.starts_with("put ")), "{log:?}");
}

/// C and G at their bounds: the first cut counted after C (C + 1) of
/// group exactly G is chosen and fires. No test of the pull request has a
/// cut at either bound.
#[test]
fn r150_bounds_a_cut_numbered_c_plus_1_of_group_g_fires() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry = Carry::new(Some(KillAt::CarryPut(1)), Arc::clone(&die)).expect("a carry hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    // Cut 5 (= C) of group 12: not counted after C.
    reads(&mut local, offered(5, 12, true));
    block_on(store.put("vols/a/seg/1", Bytes::from_static(b"x"))).expect("landed");
    // Cut 6 (= C + 1) of group 9 (< G): not chosen.
    reads(&mut local, offered(6, 9, true));
    block_on(store.put("vols/a/seg/2", Bytes::from_static(b"x"))).expect("landed");
    // Cut 7 of group 10 (= G): chosen.
    reads(&mut local, offered(7, 10, true));
    block_on(store.put("vols/a/seg/3", Bytes::from_static(b"x"))).expect("landed");
    let log = taken(&said);
    assert_eq!(
        log,
        [
            "put vols/a/seg/1",
            "put vols/a/seg/2",
            "put vols/a/seg/3",
            "kill carry-put:1: after the first segment PUT of cut 7 (group 10), chosen to carry \
             publish 1, landed (vols/a/seg/3)",
        ]
    );
}

/// The cut under way at the acceptance, read on from a later block after
/// it, is not chosen; the next cut is. This pins the rule as built (a
/// continued read is not a new reading), which the pull request's tests do
/// not: plant M7 survives them.
#[test]
fn r150_a_cut_under_way_read_on_after_the_acceptance_is_not_chosen() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry = Carry::new(Some(KillAt::CarryPut(1)), Arc::clone(&die)).expect("a carry hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    reads(&mut local, offered(6, 10, true));
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    let _ = block_on(local.read(6, 512, Buf::zeroed(0)));
    block_on(store.put("vols/a/seg/1", Bytes::from_static(b"x"))).expect("landed");
    reads(&mut local, offered(7, 10, true));
    block_on(store.put("vols/a/seg/2", Bytes::from_static(b"x"))).expect("landed");
    assert_eq!(
        taken(&said),
        [
            "put vols/a/seg/1",
            "put vols/a/seg/2",
            "kill carry-put:1: after the first segment PUT of cut 7 (group 10), chosen to carry \
             publish 1, landed (vols/a/seg/2)",
        ]
    );
}
