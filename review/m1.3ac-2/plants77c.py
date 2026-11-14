"""#77 at b02f876: the old plants P1-P8 (P1 updated for BrokenRule), the reproducer with P2 and P4,
and new plants Q1-Q6 on the BrokenRule stop. One plant at a time; the worktree is restored after each."""
import subprocess, os
W = '/home/user/skyzfs-pr73'
P = 'crates/skyzfs-ublk/src/pump.rs'
T = 'crates/skyzfs-ublk/src/pump/tests.rs'
SV = 'bins/skyzfsd/src/serve.rs'
CL = 'bins/skyzfsd/src/cli.rs'
REPRO = '/home/user/skyzfs/review/m1.3ac-2/repro77.patch'
UBLK = ['cargo', 'test', '-q', '-p', 'skyzfs-ublk', '--lib', 'pump']
DAEMON = ['cargo', 'test', '-q', '-p', 'skyzfsd', '--features', 'skyzfsd/test-hooks', '--lib']
P2 = ('P2 the failed turn leaves the slot Waiting after its commit', P,
  "            if let Some(slot) = self.slots.get_mut(usize::from(tag)) {\n                *slot = Slot::Idle;\n            }\n            self.queue.commit(tag, -crate::translate::EIO, None);",
  "            self.queue.commit(tag, -crate::translate::EIO, None);", UBLK)
P4 = ('P4 fail() leaves a waiting slot Waiting', P,
  "            let buf = match std::mem::replace(&mut self.slots[tag], Slot::Idle) {\n                // A waiting request",
  "            if matches!(self.slots[tag], Slot::Waiting) {\n                continue;\n            }\n            let buf = match std::mem::replace(&mut self.slots[tag], Slot::Idle) {\n                // A waiting request", UBLK)
PL = [
 ('P1 take_over lets an unknown tag through', P,
  "                // A tag the queue does not have: nothing can be\n                // committed for it.\n                return Err(Stopped::BrokenRule { tag });",
  "                continue;", UBLK),
 P2,
 ('P3 fail() commits a waiting request from its slot as well as from the backlog', P,
  "                Slot::Idle | Slot::Waiting | Slot::Finished => continue,",
  "                Slot::Idle | Slot::Finished => continue,\n                Slot::Waiting => None,", UBLK),
 P4,
 ('P5 the turn does not take over (the failed turn still does)', P,
  "        if !self.stopping {\n            if let Err(stopped) = self.take_over(&fetched) {\n                return Some(stopped);\n            }",
  "        if !self.stopping {", UBLK),
 ('P6 commit_finished leaves the slot Finished', P,
  "        for (tag, result, buf) in self.finished.drain(..) {\n            self.slots[usize::from(tag)] = Slot::Idle;",
  "        for (tag, result, buf) in self.finished.drain(..) {", UBLK),
 ('P7 the fake queue records no broken rule', T,
  "        if !self.out.remove(&tag) {",
  "        if !self.out.remove(&tag) && false {", UBLK),
 ('P8 the failed turn commits before it takes over', P,
  "        if let Err(stopped) = self.take_over(&fetched) {\n            return Some(stopped);\n        }\n        for Fetched { tag, .. } in fetched {",
  "        for Fetched { tag, .. } in fetched.clone() {\n            self.queue.commit(tag, -crate::translate::EIO, None);\n            self.failed_answers += 1;\n        }\n        if let Err(stopped) = self.take_over(&fetched) {\n            return Some(stopped);\n        }\n        for Fetched { tag, .. } in Vec::<Fetched>::new() {", UBLK),
 ('Q1 a tag the queue does not have ends the pump with QueueDown again', P,
  "                // committed for it.\n                return Err(Stopped::BrokenRule { tag });",
  "                // committed for it.\n                return Err(Stopped::QueueDown);", UBLK),
 ('Q2 a tag still out ends the pump with QueueDown again', P,
  "                _ => return Err(Stopped::BrokenRule { tag }),",
  "                _ => return Err(Stopped::QueueDown),", UBLK),
 ('Q3 serve ends a broken rule as an error (status 1)', SV,
  "        Stopped::BrokenRule { tag } => {\n            say(broken_rule_said(number, tag));\n            Ok(Exit::BrokenRule)\n        }\n",
  "", DAEMON),
 ('Q4 serve says nothing of the device waiting', SV,
  "failed, and device {number} waits for the next server, as after a kill\"",
  "failed\"", DAEMON),
 ('Q5 the help has no row for 7', CL,
  "the device and a client still holds it open; 7 the kernel's driver broke its rule\n(it handed over a request on a tag whose request was still out):\nserve answered nothing more, the volume is not failed, and the device\nwaits for the next server, as after a kill.",
  "the device and a client still holds it open.", DAEMON),
 ('Q6 a broken rule marks the volume failed on the queue thread', SV,
  "    if !flags.failed.load(Ordering::SeqCst) {\n        return;\n    }\n    if pump.is_failed() {",
  "    if !flags.failed.load(Ordering::SeqCst) {\n        if last { flags.failed.store(true, Ordering::SeqCst); }\n        return;\n    }\n    if pump.is_failed() {", DAEMON),
]
env = dict(os.environ, CARGO_TARGET_DIR='/home/user/target-pr73', CARGO_INCREMENTAL='0')
def run(cmd):
    r = subprocess.run(cmd, cwd=W, env=env, capture_output=True, text=True)
    out = r.stdout + r.stderr
    errs = [l for l in out.splitlines() if l.startswith('error') and 'test failed' not in l]
    fails = sorted({l.split()[1] for l in out.splitlines() if l.startswith('---- ')})
    summ = [l for l in out.splitlines() if l.startswith('test result')]
    return f'rc {r.returncode}; build errors {errs[:3]}; failed {fails}; {summ[-1:]}'
def plant(name, f, old, new, cmd, extra=None):
    path = f'{W}/{f}'
    s = open(path).read()
    n = s.count(old)
    if n != 1:
        print(f'{name}: PLANT NOT APPLIED ({n})', flush=True); return
    open(path, 'w').write(s.replace(old, new))
    try:
        print(f'{name}: {run(cmd)}', flush=True)
    finally:
        subprocess.run(['git', 'checkout', '--', f], cwd=W)
for p in PL:
    if p[0].startswith('Q6'):
        continue  # needs `last` in scope: see below
    plant(*p)
# The reproducer from the last look, unplanted and with P2 and with P4.
subprocess.run(['git', 'apply', REPRO], cwd=W, check=True)
try:
    print('R  the reproducer, unplanted: ' + run(UBLK + ['review_']), flush=True)
    for p in (P2, P4):
        plant('R+' + p[0], p[1], p[2], p[3], UBLK + ['review_'])
finally:
    subprocess.run(['git', 'checkout', '--', T], cwd=W)
print(subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout or 'clean')
