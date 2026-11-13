#!/usr/bin/env python3
"""plants.py WORKTREE [name...]: for #58 (M1.3y). Plants one defect at a
time in the in-place store's ordering (inplace.rs) or its thread pool
(inplace/pool.rs), runs the in-place store's unit tests under a limit,
and says which tests fail, or that the run hung. Each file is put back
after its plant.

With SKYZFS_PLANTS_SCOPE=whole in the environment it runs every test of
skyzfs-local and skyzfsd instead, with no filter, so that "caught by
nothing" can be said of the crates and not of one module."""
import os
import re
import subprocess
import sys
import time

tree = sys.argv[1]
INPLACE = 'crates/skyzfs-local/src/inplace.rs'
POOL = 'crates/skyzfs-local/src/inplace/pool.rs'
WHOLE = os.environ.get('SKYZFS_PLANTS_SCOPE') == 'whole'
# The crates' tests, or the in-place store's alone.
TESTS = ['-p', 'skyzfs-local', '-p', 'skyzfsd'] if WHOLE else ['-p', 'skyzfs-local', '--lib', 'inplace']
BUILD = ['-p', 'skyzfs-local', '-p', 'skyzfsd'] if WHOLE else ['-p', 'skyzfs-local', '--lib']
LIMIT = 600 if WHOLE else 330

OVERLAP = '.any(|&(from, to, busy_write)| (write || busy_write) && from < end && offset < to)'
PLANTS = [
    ('m1 only two writes that overlap wait: a read neither waits nor is waited for', INPLACE,
     OVERLAP, OVERLAP.replace('(write || busy_write)', '(write && busy_write)')),
    ('m2 a write does not wait for a read it overlaps', INPLACE,
     OVERLAP, OVERLAP.replace('(write || busy_write)', 'busy_write')),
    ('m3 a read does not wait for a write it overlaps', INPLACE,
     OVERLAP, OVERLAP.replace('(write || busy_write)', 'write')),
    ('m4 a flush waits only when everything on the threads is a write', INPLACE,
     'Op::Flush => return self.busy.values().any(|&(_, _, write)| write),',
     'Op::Flush => return self.busy.values().all(|&(_, _, write)| write),'),
    ('m5 a request passes those that wait, unless it must wait itself', INPLACE,
     'if !self.deferred.is_empty() || self.must_wait(&request.op) {',
     'if self.must_wait(&request.op) {'),
    ('m6 a write-zeroes never waits', INPLACE,
     'Op::WriteZeroes { offset, len } => (*offset, *len, true),',
     'Op::WriteZeroes { .. } => return false,'),
    ('m7 two requests overlap only where they begin at the same byte', INPLACE,
     OVERLAP, OVERLAP.replace('from < end && offset < to', 'from == offset && end > 0 && to > 0')),
    ('p1 a thread that panics does not hand its job back', POOL,
     '''        if let Some((id, write, buf)) = self.job.take() {
            state.done.push(Done {
                id,
                write,
                result: Err(io::Error::other("the I/O thread panicked")),
                buf,
            });
        }
        drop(state);''', '        drop(state);'),
    ('p2 after a failure the jobs no thread has begun are not taken back', INPLACE,
     '''        if self.failed {
            for job in self.pool.as_ref().map(Pool::take_back).unwrap_or_default() {''',
     '''        if self.failed && self.busy.len() > usize::MAX - 1 {
            for job in self.pool.as_ref().map(Pool::take_back).unwrap_or_default() {'''),
    ('p3 a thread\'s death is not counted', POOL,
     '        state.died += 1;\n', '        state.died += 0;\n'),
    ('p4 a failed write on a thread does not fail the volume', INPLACE,
     '''            let result = if write {
                let result = self.mutated(result);''',
     '''            let result = if write {
                let result = self.mutated(result.or(Ok(())));'''),
    ('p5 a write\'s end on a thread does not count for the next flush', INPLACE,
     '''            let result = if write {
                let result = self.mutated(result);''',
     '''            let result = if write {
                let was = self.unsynced;
                let result = self.mutated(result);
                self.unsynced = was;'''),
    ('s1 a direct-I/O unit that does not divide a block is taken', INPLACE,
     '                || !block.is_multiple_of(offset)\n', ''),
    ('s2 the threads read and write through the cache', INPLACE,
     'let pool = Pool::start(files, self.io.direct, self.wake.clone())',
     'let pool = Pool::start(files, false, self.wake.clone())'),
    ('c1 when something ends, what waits is started past a head that must still wait', INPLACE,
     """        while let Some(front) = self.deferred.front() {
            if self.must_wait(&front.op) {
                break;
            }
            let Some(Request { id, op }) = self.deferred.pop_front() else {
                break;
            };
            self.execute(id, op);
        }""",
     """        let mut at = 0;
        while at < self.deferred.len() {
            if self.must_wait(&self.deferred[at].op) {
                at += 1;
                continue;
            }
            let Some(Request { id, op }) = self.deferred.remove(at) else {
                break;
            };
            self.execute(id, op);
        }"""),
    ('d1 the store\'s drop does not wait for what is on the threads or behind them', INPLACE,
     'while self.pool.is_some() && !(self.busy.is_empty() && self.deferred.is_empty()) {',
     'while self.pool.is_some() && self.busy.len() > usize::MAX - 1 {'),
]

only = sys.argv[2:]
for name, rel, old, new in PLANTS:
    if only and name.split()[0] not in only:
        continue
    path = f'{tree}/{rel}'
    orig = open(path).read()
    assert orig.count(old) == 1, (name, orig.count(old))
    open(path, 'w').write(orig.replace(old, new))
    began = time.time()
    try:
        subprocess.run(['cargo', 'test', *BUILD, '--no-run'], cwd=tree, capture_output=True, text=True)
        built = time.time() - began
        try:
            out = subprocess.run(['cargo', 'test', *TESTS], cwd=tree,
                                 capture_output=True, text=True, timeout=LIMIT)
            text = out.stdout + out.stderr
            hung = False
        except subprocess.TimeoutExpired as e:
            text = (e.stdout or b'').decode(errors='replace') + (e.stderr or b'').decode(errors='replace')
            hung = True
            for binary in ('deps/skyzfs_local-', 'deps/skyzfsd-', 'deps/inplace_real-', 'deps/store_real-'):
                subprocess.run(['pkill', '-f', binary], capture_output=True)
    finally:
        open(path, 'w').write(orig)
    failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', text, re.M)))
    results = re.findall(r'^test result: FAILED.*$|^test result: ok.*$', text, re.M)
    if 'error[' in text or 'could not compile' in text:
        verdict = 'DID NOT BUILD: ' + text[-300:]
    elif hung:
        running = sorted(set(re.findall(r'^test (\S+) has been running', text, re.M)))
        started = set(re.findall(r'^test (\S+) \.\.\. ', text, re.M))
        verdict = f'HUNG: no end after {LIMIT} s; failed before that: {failed or "nothing"}; said to be running long: {running or "?"}'
    else:
        kinds = [r for r in results if 'FAILED' in r]
        verdict = f'failed: {failed or "NOTHING"}; {len(results)} binaries, {len(kinds)} with a failure'
    print(f'{name}\n    {verdict} ({time.time() - began - built:.0f} s of tests)', flush=True)
