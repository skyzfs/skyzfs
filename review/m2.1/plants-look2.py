import subprocess, sys, re, os
ROOT = '/home/user/skyzfs-pr92p'
F = ROOT + '/crates/skyzfs-local/src/inplace.rs'
C = ROOT + '/crates/skyzfs-local/src/inplace/cuts.rs'
PLANTS = {
 # P1: the barrier told when a sync begins, not when the flush executes
 'P1_barrier_at_sync_begin': [
   (F, "        self.cuts.barrier(id);\n        match &mut self.syncing {", "        match &mut self.syncing {"),
   (F, "                self.counts.clean += 1;\n", "                self.counts.clean += 1;\n                self.cuts.barrier(id);\n"),
   (F, "    fn begin_sync(&mut self, flushes: Vec<RequestId>) {\n", "    fn begin_sync(&mut self, flushes: Vec<RequestId>) {\n        for &id in &flushes {\n            self.cuts.barrier(id);\n        }\n"),
 ],
 # P2: the release loop passes a front that must wait and releases what is behind it
 'P2_release_skips_blocked_front': [
   (F, """        while let Some(front) = self.deferred.front() {
            if self.must_wait(&front.op) {
                break;
            }
            let Some(Request { id, op }) = self.deferred.pop_front() else {
                break;
            };
            self.execute(id, op);
        }""", """        let mut i = 0;
        while i < self.deferred.len() {
            if self.must_wait(&self.deferred[i].op) {
                i += 1;
                continue;
            }
            let Some(Request { id, op }) = self.deferred.remove(i) else {
                break;
            };
            self.execute(id, op);
        }"""),
 ],
 # P3: no applied on the caller's thread
 'P3_no_applied_inline': [
   (F, """                let result = self.mutated(written);
                if result.is_ok() {
                    self.cuts.applied(id);
                }""", """                let result = self.mutated(written);"""),
 ],
 # P4: the barrier told when the flush's sync has ended
 'P4_barrier_at_sync_end': [
   (F, "        self.cuts.barrier(id);\n        match &mut self.syncing {", "        match &mut self.syncing {"),
   (F, "                self.counts.clean += 1;\n", "                self.counts.clean += 1;\n                self.cuts.barrier(id);\n"),
   (F, """            Ok(()) => {
                for id in self.syncing.take().unwrap_or_default() {
                    self.complete(id, Ok(()), None);""", """            Ok(()) => {
                for id in self.syncing.take().unwrap_or_default() {
                    self.cuts.barrier(id);
                    self.complete(id, Ok(()), None);"""),
 ],
 # P5: the mark is kept after a flush takes it (every flush after cuts again until a new uberblock)
 'P5_mark_not_taken': [
   (C, "                let Some(state) = self.mark.take() else {", "                let Some(state) = self.mark else {"),
 ],
 # P6: a repeat compared with the mark only, not the newest cut
 'P6_repeat_mark_only': [
   (C, "        !is_repeat(self.mark.as_ref(), self.newest.as_ref(), state)", "        !is_repeat(self.mark.as_ref(), None, state)"),
 ],
 # P7: a mark kept as the highest group seen, not the latest applied
 'P7_mark_is_max': [
   (C, """            if self.marks(&state) {
                self.mark = Some(state);
            }""", """            if self.marks(&state) && self.mark.is_none_or(|m| m.txg <= state.txg) {
                self.mark = Some(state);
            }"""),
 ],
 # P8: only writes, not reads, may pass a waiting flush
 'P8_writes_pass_waiting_flush': [
   (F, "        let mut behind = !self.deferred.is_empty();", "        let mut behind = !self.deferred.is_empty();\n        if matches!(request.op, Op::Write { .. }) && self.deferred.iter().all(|r| matches!(r.op, Op::Flush)) {\n            behind = false;\n        }"),
 ],
 # P9: a flush that joins a sync under way reaches no barrier
 'P9_joined_flush_no_barrier': [
   (F, "        self.cuts.barrier(id);\n        match &mut self.syncing {\n            Some(covered) => {", "        let joins = self.syncing.is_some() && !self.unsynced;\n        if !joins {\n            self.cuts.barrier(id);\n        }\n        match &mut self.syncing {\n            Some(covered) => {"),
 ],
 'C1_carried_only_first': [
   (C, "                .values()\n                .any(|carried| is_repeat(Some(carried), None, state));", "                .values()\n                .take(0)\n                .any(|carried| is_repeat(Some(carried), None, state));"),
 ],
 'C2_carried_by_group': [
   (C, "                .any(|carried| is_repeat(Some(carried), None, state));", "                .any(|carried| carried.txg == state.txg);"),
 ],
}
def run(feats):
    cmd = ['cargo', 'test', '-q', '-p', 'skyzfs-local'] + feats
    env = dict(os.environ, CARGO_TARGET_DIR='/home/user/target-pr92p', CARGO_INCREMENTAL='0')
    p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=3000)
    out = p.stdout + p.stderr
    failed = sorted(set(re.findall(r'^    (\S+)$', out, re.M)))
    res = re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed', out)
    err = 'error[' in out or 'error:' in out and not res
    return p.returncode, failed, res, (out[-1500:] if err else '')
names = sys.argv[1:] or list(PLANTS)
for name in names:
    saved = {}
    ok = True
    for path, old, new in PLANTS[name]:
        if path not in saved:
            saved[path] = open(path).read()
        text = open(path).read()
        if text.count(old) != 1:
            print(name, 'PATCH FAILED', repr(old[:60]), text.count(old)); ok = False; break
        open(path, 'w').write(text.replace(old, new))
    try:
        if ok:
            for feats in ([],):
                rc, failed, res, err = run(feats)
                print(name, feats, 'rc', rc, 'failed:', failed or 'nothing', res, err, flush=True)
    finally:
        for path, text in saved.items():
            open(path, 'w').write(text)
print('restored', subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True).stdout or 'clean')
