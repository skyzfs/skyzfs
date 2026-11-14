"""Plant defects (and two experiments) in #74's head, one at a time, and run the tests named.
usage: python3 -I plants74.py WORKTREE TARGET_DIR"""
import os, subprocess, sys, pathlib
W, TARGET = pathlib.Path(sys.argv[1]), sys.argv[2]
LC = 'crates/skyzfs-ublk/src/lifecycle.rs'
LT = 'crates/skyzfs-ublk/src/lifecycle/tests.rs'
DL = 'bins/skyzfsd/src/delay.rs'
UBLK = ['cargo', 'test', '-p', 'skyzfs-ublk', '--lib']
DAEMON = ['cargo', 'test', '-p', 'skyzfsd', '--features', 'test-hooks', '--lib']
PLANTS = [
    ('L1 the failure note read before the lock', LC, UBLK, None,
     [("    let mut held = lock(records, volume, wanted.lock_retries)?;\n\n    // The failure note: under the lock, before the store is opened.\n    let late = mutants.has(Mutants::NOTE_READ_AFTER_OPEN);\n    let mut acknowledging = None;\n    if !late {\n        acknowledging = noted_failure(control, records, wanted, mutants)?;\n    }\n",
       "    let late = mutants.has(Mutants::NOTE_READ_AFTER_OPEN);\n    let mut acknowledging = None;\n    if !late {\n        acknowledging = noted_failure(control, records, wanted, mutants)?;\n    }\n    let mut held = lock(records, volume, wanted.lock_retries)?;\n")]),
    ('L2 an acknowledged start keeps the failure note', LC, UBLK, None,
     [("    if acknowledging.is_some() {\n        records\n            .remove_failure()",
       "    if acknowledging.is_some() && false {\n        records\n            .remove_failure()")]),
    ('L3 an acknowledging start opens without the acknowledgement', LC, UBLK, None,
     [("volume.open(held, acknowledging.is_some())", "volume.open(held, false)")]),
    ('D1 the stop of the delayed queue sends without giving on what waits', DL, DAEMON, None,
     [("    fn flush(&mut self) -> Result<(), QueueDown> {\n        self.release(true);\n        self.inner.flush()",
       "    fn flush(&mut self) -> Result<(), QueueDown> {\n        self.inner.flush()")]),
    ('E1 (experiment) the lock test with a lasting marker', LT, UBLK + ['a_start_that_fails_deletes_under_the_lock'], None,
     [("        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);\n        world.opens.push_back(OpenError::MarkedFailed);\n        world.intrude_at = Some(at);",
       "        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);\n        world.marked = true;\n        world.intrude_at = Some(at);")]),
    ('E2 (experiment) a lasting marker, and the intruder acknowledges', LT, UBLK + ['a_start_that_fails_deletes_under_the_lock'], None,
     [("        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);\n        world.opens.push_back(OpenError::MarkedFailed);\n        world.intrude_at = Some(at);",
       "        let mut world = World::new().attached(4, DeviceState::Waiting, EARLIER);\n        world.marked = true;\n        world.intrude_at = Some(at);"),
      ("let result = start_with(&mut c, &mut r, &mut v, &WANTED, Mutants(0));",
       "let result = start_with(&mut c, &mut r, &mut v, &Wanted { acknowledge_failure: true, ..WANTED }, Mutants(0));")]),
]
env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_INCREMENTAL='0')
def run(cmd):
    p = subprocess.run(cmd, cwd=W, capture_output=True, text=True, env=env, timeout=3000)
    out = p.stdout + p.stderr
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.rstrip().endswith('FAILED')})
    res = [l for l in out.splitlines() if l.startswith('test result:')]
    err = [l for l in out.splitlines() if l.startswith('error') and 'test failed' not in l][:2]
    return p.returncode, fails, res, err
for label, f, cmd, _, edits in PLANTS:
    path = W / f
    text = path.read_text()
    new = text
    ok = True
    for old, rep in edits:
        if new.count(old) != 1:
            print(f'{label}: NOT PLANTED ({new.count(old)} matches of an edit)'); ok = False; break
        new = new.replace(old, rep)
    if not ok:
        continue
    path.write_text(new)
    try:
        rc, fails, res, err = run(cmd)
        if err:
            print(f'{label}: BUILD ERROR: {err}')
        else:
            print(f"{label}: {'FAILS: ' + ', '.join(fails) if rc else 'nothing fails'} ({'; '.join(r[13:60] for r in res)})")
    finally:
        path.write_text(text)
st = subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout.strip()
print('worktree after:', st or 'clean')
