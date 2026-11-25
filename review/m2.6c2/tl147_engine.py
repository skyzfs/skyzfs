#!/usr/bin/env python3
"""Third look at #147: scratch engine tests of a partial hold, on the
fake world. Each is appended to the agent's engine tests in a scratch
worktree, run, and taken out again (try/finally). The fake's `hold` is
given, behind a thread-local switch the tests set, the refusal ZFS gives
(recorded on both images): `zfs hold -r` of a tag that some members have
fails, and the adapter then reports the members that lack it; where every
member has it, the adapter answers Ok.

usage: python3 -I tl147_engine.py WORKTREE
Prints the test output; exit 0 whatever the tests say.
"""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
TESTS = ROOT / "crates/skyzfs-agent/src/engine/tests.rs"
FAKE = ROOT / "crates/skyzfs-agent/src/engine/fake.rs"

FAKE_SITE = """        if !p.snaps.contains_key(&format!("{POOL}@{t}")) {
            return Err(ZfsError::NoSnapshot);
        }
        let suffix = format!("@{t}");
        let txg = p.next_txg.unwrap_or(p.txg + 1);"""

FAKE_NEW = """        if !p.snaps.contains_key(&format!("{POOL}@{t}")) {
            return Err(ZfsError::NoSnapshot);
        }
        if ZFS_HOLD.with(|z| z.get()) {
            let suffix = format!("@{t}");
            let has: Vec<bool> = p
                .snaps
                .iter()
                .filter(|(name, _)| name.ends_with(&suffix))
                .map(|(_, s)| s.holds.contains(tag))
                .collect();
            if has.iter().any(|h| *h) {
                return if has.iter().all(|h| *h) {
                    Ok(())
                } else {
                    Err(ZfsError::Other("tag already exists; some members lack it".into()))
                };
            }
        }
        let suffix = format!("@{t}");
        let txg = p.next_txg.unwrap_or(p.txg + 1);"""

FAKE_DECL = """
thread_local! {
    /// Third look at #147: `hold` refuses as ZFS does where some members
    /// have the tag.
    pub static ZFS_HOLD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
"""

TESTS_ADD = r'''

/// Third look at #147: what the agent leaves of a partial hold.
/// `keep_root`: the root's `@t` keeps the tag and the members lose it
/// (NL-F1's case), or the other way.
fn tl147_unhold(world: &Rc<RefCell<World>>, keep_root: bool) {
    let mut w = world.borrow_mut();
    let t = w.programs.keys().last().cloned().unwrap_or_default();
    let root = format!("{POOL}@{t}");
    let suffix = format!("@{t}");
    if let Some(p) = w.pool.as_mut() {
        for (name, snap) in p.snaps.iter_mut() {
            if name.ends_with(&suffix) && ((*name == root) != keep_root) {
                snap.holds.remove(HOLD_TAG);
            }
        }
    }
}

/// At `Done` (before it is recorded) a person releases the tag on some
/// members: the agent should release what is left as it forgets.
#[test]
fn tl147_a_partial_hold_at_done() {
    for keep_root in [true, false] {
        let world = World::new(Mode::Idle);
        let keep = MemKeep::default();
        let armed = Rc::new(Cell::new(true));
        let hook = {
            let world = world.clone();
            let armed = armed.clone();
            move |point: Point| {
                if point == DONE && armed.replace(false) {
                    tl147_unhold(&world, keep_root);
                }
                false
            }
        };
        let mut agent = agent(&world, &keep).with_points(Box::new(hook));
        agent.begin().expect("begun");
        agent.settle(LIMIT).expect("settled");
        let w = world.borrow();
        println!(
            "TL147 done keep_root={keep_root}: outcomes {:?}; holds left {:?}",
            agent.outcomes(),
            holds(&w)
        );
    }
}

/// Just after the hold, a person releases the tag on some members; the
/// fake's hold refuses as ZFS does. The publish is given up as too old:
/// the agent should release what is left first.
#[test]
fn tl147_a_partial_hold_after_the_hold() {
    super::fake::ZFS_HOLD.with(|z| z.set(true));
    for keep_root in [true, false] {
        let world = World::new(Mode::Idle);
        let armed = Rc::new(Cell::new(true));
        let hook = {
            let world = world.clone();
            let armed = armed.clone();
            move |point: Point| {
                if point
                    == (Point {
                        effect: Effect::Hold,
                        after: true,
                    })
                    && armed.replace(false)
                {
                    tl147_unhold(&world, keep_root);
                }
                false
            }
        };
        let (agent, _t) = aged_run(&world, Duration::from_secs(600), hook);
        let w = world.borrow();
        println!(
            "TL147 hold keep_root={keep_root}: outcomes {:?}; holds left {:?}; published refs {}",
            agent.outcomes(),
            holds(&w),
            w.refs.len()
        );
    }
}
'''


def main():
    tests = TESTS.read_text()
    fake = FAKE.read_text()
    if fake.count(FAKE_SITE) != 1:
        print(f"fake site found {fake.count(FAKE_SITE)} times")
        return
    try:
        FAKE.write_text(fake.replace(FAKE_SITE, FAKE_NEW) + FAKE_DECL)
        TESTS.write_text(tests + TESTS_ADD)
        p = subprocess.run(
            ["cargo", "test", "-p", "skyzfs-agent", "--lib", "tl147_", "--",
             "--nocapture", "--test-threads=1"],
            cwd=ROOT, capture_output=True, text=True, timeout=600,
        )
        out = p.stdout + p.stderr
        for line in out.splitlines():
            if line.startswith(("TL147", "test ", "error", "test result")) or "panicked" in line:
                print(line)
        print("exit", p.returncode)
    finally:
        TESTS.write_text(tests)
        FAKE.write_text(fake)


main()
