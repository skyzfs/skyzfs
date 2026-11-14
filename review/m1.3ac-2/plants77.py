import subprocess, os
W = '/home/user/skyzfs-pr73'
P = 'crates/skyzfs-ublk/src/pump.rs'
T = 'crates/skyzfs-ublk/src/pump/tests.rs'
PL = [
 ('P1 take_over lets an unknown tag through', P,
  "                // A tag the queue does not have: nothing can be\n                // committed for it.\n                return Err(Stopped::QueueDown);",
  "                continue;"),
 ('P2 the failed turn leaves the slot Waiting after its commit', P,
  "            if let Some(slot) = self.slots.get_mut(usize::from(tag)) {\n                *slot = Slot::Idle;\n            }\n            self.queue.commit(tag, -crate::translate::EIO, None);",
  "            self.queue.commit(tag, -crate::translate::EIO, None);"),
 ('P3 fail() commits a waiting request from its slot as well as from the backlog', P,
  "                Slot::Idle | Slot::Waiting | Slot::Finished => continue,",
  "                Slot::Idle | Slot::Finished => continue,\n                Slot::Waiting => None,"),
 ('P4 fail() leaves a waiting slot Waiting', P,
  "            let buf = match std::mem::replace(&mut self.slots[tag], Slot::Idle) {\n                // A waiting request",
  "            if matches!(self.slots[tag], Slot::Waiting) {\n                continue;\n            }\n            let buf = match std::mem::replace(&mut self.slots[tag], Slot::Idle) {\n                // A waiting request"),
 ('P5 the turn does not take over (the failed turn still does)', P,
  "        if !self.stopping {\n            if let Err(stopped) = self.take_over(&fetched) {\n                return Some(stopped);\n            }",
  "        if !self.stopping {"),
 ('P6 commit_finished leaves the slot Finished', P,
  "        for (tag, result, buf) in self.finished.drain(..) {\n            self.slots[usize::from(tag)] = Slot::Idle;",
  "        for (tag, result, buf) in self.finished.drain(..) {"),
 ('P7 the fake queue records no broken rule', T,
  "        if !self.out.remove(&tag) {",
  "        if !self.out.remove(&tag) && false {"),
 ('P8 the failed turn commits before it takes over', P,
  "        if let Err(stopped) = self.take_over(&fetched) {\n            return Some(stopped);\n        }\n        for Fetched { tag, .. } in fetched {",
  "        for Fetched { tag, .. } in fetched.clone() {\n            self.queue.commit(tag, -crate::translate::EIO, None);\n            self.failed_answers += 1;\n        }\n        if let Err(stopped) = self.take_over(&fetched) {\n            return Some(stopped);\n        }\n        for Fetched { tag, .. } in Vec::<Fetched>::new() {"),
]
env = dict(os.environ, CARGO_TARGET_DIR='/home/user/target-pr73', CARGO_INCREMENTAL='0')
for name, f, old, new in PL:
    path = f'{W}/{f}'
    s = open(path).read()
    n = s.count(old)
    if n != 1:
        print(f'{name}: PLANT NOT APPLIED ({n})', flush=True); continue
    open(path, 'w').write(s.replace(old, new))
    r = subprocess.run(['cargo', 'test', '-q', '-p', 'skyzfs-ublk', '--lib', 'pump'], cwd=W, env=env, capture_output=True, text=True)
    subprocess.run(['git', 'checkout', '--', f], cwd=W)
    out = r.stdout + r.stderr
    errs = [l for l in out.splitlines() if l.startswith('error') and 'test failed' not in l]
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith('---- ')})
    summ = [l for l in out.splitlines() if l.startswith('test result')]
    print(f'{name}: rc {r.returncode}; build errors {errs[:3]}; failed {fails}; {summ[-1:]}', flush=True)
print(subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout or 'clean')
