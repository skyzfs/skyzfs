import subprocess, os
W = '/home/user/skyzfs-pr75'
S = 'crates/skyzfs-local/src/sim.rs'
P = [
 ('S2 a hole in a file of its own rewrites the disk block', S,
  '            .max(state.block_size_of(self.inode).unwrap_or(SECTOR));',
  '            .max(state.block_size.unwrap_or(SECTOR));'),
 ('R1 goes with the name: a rename onto the name takes the old file\'s', S,
  '        state.names.insert(to.to_owned(), inode);\n        state\n            .pending_names',
  '        if let Some(old) = state.names.insert(to.to_owned(), inode) { if let Some(b) = state.block_sizes_of.get(&old).copied() { state.block_sizes_of.insert(inode, b); } }\n        state\n            .pending_names'),
 ('R2 renamed, the file loses its own', S,
  '        state.names.insert(to.to_owned(), inode);\n        state\n            .pending_names',
  '        state.names.insert(to.to_owned(), inode);\n        state.block_sizes_of.remove(&inode);\n        state\n            .pending_names'),
 ('R3 the power cut drops the per-file sizes', S,
  '        let inodes = &self.inodes;\n        self.block_sizes_of\n            .retain(|inode, _| inodes.contains_key(inode));',
  '        let inodes = &self.inodes;\n        self.block_sizes_of\n            .retain(|inode, _| inodes.contains_key(inode) && names.values().any(|i| i == inode));'),
 ('R4 no housekeeping', S,
  '        let inodes = &self.inodes;\n        self.block_sizes_of\n            .retain(|inode, _| inodes.contains_key(inode));', ''),
]
env = dict(os.environ, CARGO_TARGET_DIR='/home/user/skyzfs-pr46b/target', CARGO_INCREMENTAL='0')
for name, f, old, new in P:
    path = f'{W}/{f}'
    s = open(path).read()
    n = s.count(old)
    if n != 1:
        print(f'{name}: PLANT NOT APPLIED ({n})'); continue
    open(path, 'w').write(s.replace(old, new))
    r = subprocess.run(['cargo', 'test', '-q', '-p', 'skyzfs-local', '--lib'], cwd=W, env=env, capture_output=True, text=True)
    subprocess.run(['git', 'checkout', '--', f], cwd=W)
    out = r.stdout + r.stderr
    errs = [l for l in out.splitlines() if l.startswith('error') and 'test failed' not in l]
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith('---- ')})
    summ = [l for l in out.splitlines() if l.startswith('test result')]
    print(f'{name}: rc {r.returncode}; build errors {errs[:2]}; failed {fails}; {summ[-1:] }')
print(subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout or 'clean')
