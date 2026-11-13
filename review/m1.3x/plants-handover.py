#!/usr/bin/env python3
"""plants754.py WORKTREE [name...]: plant defects in #55's hand-over at
754a15a (crates/skyzfs-local/src/store.rs), one at a time, run the
in-place store's tests and say which fail. The file is put back after
each plant."""
import re
import subprocess
import sys

tree = sys.argv[1]
STORE = 'crates/skyzfs-local/src/store.rs'
TEST = ['-p', 'skyzfs-local', '--lib', 'inplace']
READ = '''        let what = if self.core.shared.read().index.is_empty() {
            StartFailed::Image
        } else {
            StartFailed::Log
        };
'''
PLANTS = [
    ('h1 the hand-over always says Log (the state before this commit)',
     READ, '        let what = StartFailed::Log;\n'),
    ('h2 the hand-over always says Image',
     READ, '        let what = StartFailed::Image;\n'),
    ('h3 the test is inverted',
     'if self.core.shared.read().index.is_empty() {',
     'if !self.core.shared.read().index.is_empty() {'),
    ('h4 a store that had failed before the hand-over says Image',
     'return Err(failed(&self.core, StartFailed::Log));',
     'return Err(failed(&self.core, StartFailed::Image));'),
    ('h5 the index is read after the hand-over, not before',
     '            return Err(failed(&self.core, what));\n        }\n        let (start, end)',
     '            let what = if self.core.shared.read().index.is_empty() {\n'
     '                StartFailed::Image\n            } else {\n                StartFailed::Log\n            };\n'
     '            return Err(failed(&self.core, what));\n        }\n        let _ = what;\n        let (start, end)'),
    ('h6 "the log holds no record" (floor == tail) in place of the index',
     'if self.core.shared.read().index.is_empty() {',
     'if self.core.log.floor() == self.core.log.tail() {'),
]
only = sys.argv[2:]
path = f'{tree}/{STORE}'
for name, old, new in PLANTS:
    if only and name.split()[0] not in only:
        continue
    orig = open(path).read()
    assert orig.count(old) == 1, (name, orig.count(old))
    open(path, 'w').write(orig.replace(old, new))
    try:
        out = subprocess.run(['cargo', 'test', *TEST], cwd=tree, capture_output=True, text=True)
    finally:
        open(path, 'w').write(orig)
    text = out.stdout + out.stderr
    failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', text, re.M)))
    results = re.findall(r'^test result: .*$', text, re.M)
    built = 'error' not in text.split('Running')[0] if 'Running' in text else False
    print(f'{name}\n    built: {built}; {results}\n    failed: {failed or "NOTHING"}', flush=True)
