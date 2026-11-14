"""Plant defects in #75's sim and store, one at a time, and run skyzfs-local's library tests.
usage: python3 -I plants75.py WORKTREE TARGET_DIR"""
import os, subprocess, sys, pathlib
W, TARGET = pathlib.Path(sys.argv[1]), sys.argv[2]
SIM = 'crates/skyzfs-local/src/sim.rs'
START = 'crates/skyzfs-local/src/inplace/start.rs'
LOCAL = ['cargo', 'test', '-p', 'skyzfs-local', '--lib']
WRITE_REACH = ("            _ => state\n                .write_unit\n                .max(state.block_size_of(self.inode).unwrap_or(SECTOR)),",
               "            _ => state\n                .write_unit\n                .max(state.block_size.unwrap_or(SECTOR)),")
DEALLOC_REACH = ("        let unit = state\n            .write_unit\n            .max(state.block_size_of(self.inode).unwrap_or(SECTOR));",
                 "        let unit = state\n            .write_unit\n            .max(state.block_size.unwrap_or(SECTOR));")
PLANTS = [
    ('S1 a write to a file on its own file system rewrites the disk\'s block', SIM, [WRITE_REACH]),
    ('S2 a deallocation in such a file rewrites the disk\'s block', SIM, [DEALLOC_REACH]),
    ('S3 the file says the disk\'s block size', SIM,
     [("            .block_size_of(self.inode)\n", "            .block_size\n")]),
    ('P1 the home image is checked with the directory\'s block size', START,
     [("within_unit(Under::Home, home.block_size(), unit)?;", "within_unit(Under::Home, dir.block_size(), unit)?;")]),
    ('P2 the home image\'s refusal names the directory', START,
     [("within_unit(Under::Home, home.block_size(), unit)?;", "within_unit(Under::Directory, home.block_size(), unit)?;")]),
]
env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_INCREMENTAL='0')
def run(cmd):
    p = subprocess.run(cmd, cwd=W, capture_output=True, text=True, env=env, timeout=3000)
    out = p.stdout + p.stderr
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.rstrip().endswith('FAILED')})
    res = [l[13:60] for l in out.splitlines() if l.startswith('test result:')]
    err = [l for l in out.splitlines() if l.startswith('error') and 'test failed' not in l][:2]
    return p.returncode, fails, res, err
for label, f, edits in PLANTS:
    path = W / f
    text = path.read_text()
    new = text
    bad = [old for old, _ in edits if new.count(old) != 1]
    if bad:
        print(f'{label}: NOT PLANTED ({[new.count(o) for o in bad]} matches)'); continue
    for old, rep in edits:
        new = new.replace(old, rep)
    path.write_text(new)
    try:
        rc, fails, res, err = run(LOCAL)
        if err:
            print(f'{label}: BUILD ERROR: {err}')
        else:
            print(f"{label}: {'FAILS: ' + ', '.join(fails) if rc else 'nothing fails'} ({'; '.join(res)})")
    finally:
        path.write_text(text)
st = subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout.strip()
print('worktree after:', st or 'clean')
