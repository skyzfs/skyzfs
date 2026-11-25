#!/usr/bin/env python3
"""Fourth look at #147: scratch engine tests on the fake world, applied
to a scratch worktree of the head, run, and taken out again (finally).

- The fake daemon gains a reader of its own pending requests.
- `run_under`'s hooks (both agents) gain the strict check: at each of
  the agent's releases, no request of the publish may pend at the fake
  daemon (read from its state, not through the channel). `check` fails
  on it. So every matrix that runs through `run_under` is checked.
- `fl147_partial_release_tabulated`: the partial-release matrix (and,
  for comparison, DestroyChild, Destroy, Retake), with and without a
  destroyer of `pool@t` once nothing is held, tabulated, with the strict
  check and a stricter oracle.

usage: python3 -I fl147_engine.py WORKTREE [PLANT_FILE OLD NEW]
Runs `cargo test -p skyzfs-agent --lib engine::` (1200 s limit) and
prints the FL147 lines and the result lines.
"""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
HERE = pathlib.Path(__file__).resolve().parent
TESTS = ROOT / "crates/skyzfs-agent/src/engine/tests.rs"
FAKE = ROOT / "crates/skyzfs-agent/src/engine/fake.rs"

STRICT = r'''
thread_local! {
    static FL147_STRICT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Fourth look at #147 (scratch): at an agent's release, no request of the
/// publish may pend at the fake daemon.
fn fl147_strict(world: &Rc<RefCell<World>>, point: Point) {
    if point
        != (Point {
            effect: Effect::Release,
            after: false,
        })
    {
        return;
    }
    let w = world.borrow();
    let t = w.programs.keys().last().cloned().unwrap_or_default();
    if w.daemon.fl147_pends(&t, w.now_ms) {
        FL147_STRICT.with(|v| {
            v.borrow_mut()
                .push(format!("strict: released {t} while a request pends"))
        });
    }
}
'''

EDITS = [
    (TESTS, "/// One publish in `mode` with `fault`, then recovery to quiescence, on a\n",
     STRICT + "\n/// One publish in `mode` with `fault`, then recovery to quiescence, on a\n"),
    (TESTS, "fn run_under(rule: Rule, mode: Mode, fault: Fault) -> Ran {\n    let world = World::new(mode);\n",
     "fn run_under(rule: Rule, mode: Mode, fault: Fault) -> Ran {\n    FL147_STRICT.with(|v| v.borrow_mut().clear());\n    let world = World::new(mode);\n"),
    (TESTS, "            count.set(k + 1);\n            note_gone(&world, &gone, point);\n",
     "            count.set(k + 1);\n            note_gone(&world, &gone, point);\n            fl147_strict(&world, point);\n"),
    (TESTS, "                move |point: Point| {\n                    note_gone(&world, &gone, point);\n                    false\n",
     "                move |point: Point| {\n                    note_gone(&world, &gone, point);\n                    fl147_strict(&world, point);\n                    false\n"),
    (TESTS, "    let world = ran.world.borrow();\n    ran.settled.clone()?;\n",
     "    if let Some(v) = FL147_STRICT.with(|v| v.borrow().first().cloned()) {\n        return Err(v);\n    }\n    let world = ran.world.borrow();\n    ran.settled.clone()?;\n"),
]


def main():
    saved = {p: p.read_text() for p in (TESTS, FAKE)}
    plant = None
    if len(sys.argv) > 2:
        plant = (ROOT / sys.argv[2], sys.argv[3], sys.argv[4])
        saved.setdefault(plant[0], plant[0].read_text())
    try:
        for path, old, new in EDITS:
            text = path.read_text()
            assert text.count(old) == 1, (path, old)
            path.write_text(text.replace(old, new))
        TESTS.write_text(TESTS.read_text() + (HERE / "fl147_engine_add.rs").read_text())
        FAKE.write_text(FAKE.read_text() + (HERE / "fl147_fake_add.rs").read_text())
        if plant:
            text = plant[0].read_text()
            assert text.count(plant[1]) == 1, plant
            plant[0].write_text(text.replace(plant[1], plant[2]))
        out = subprocess.run(
            ["timeout", "1200", "cargo", "test", "-p", "skyzfs-agent", "--lib",
             "engine::", "--", "--nocapture", "--test-threads=4"],
            cwd=ROOT, capture_output=True, text=True)
        for line in (out.stdout + out.stderr).splitlines():
            if ("FL147" in line or "test result" in line or "FAILED" in line
                    or "panicked" in line or line.startswith("error")
                    or "strict:" in line):
                print(line[:600])
        print("exit", out.returncode)
    finally:
        for path, text in saved.items():
            path.write_text(text)


main()
