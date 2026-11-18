#!/usr/bin/env python3
"""Plants for the review of #117 at 1415db1 (M2.4c-1).

usage: plants.py CHECKOUT OUT [ID ...]

Each plant is one textual change to the checkout, made, tested and taken
back (try/finally). The result is appended to OUT as it comes: KILLED
with the tests that fail, SURVIVED, or NOT APPLIED. A plant whose id is
in OUT already is skipped, so the script can be started again after an
interruption. It refuses a checkout with changes.

The tests are the head's own (cargo test -p skyzfs-upload --lib --tests).
Some plants also run one of the review's scenarios while the plant is in
place ("with"): the scenario's verdict is on the same line. A test or a
scenario that does not end is killed after its limit, which is a verdict
too. CARGO_TARGET_DIR is taken from the environment.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CHECKOUT, OUT = sys.argv[1], sys.argv[2]
ONLY = set(sys.argv[3:])
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
DETAIL = os.path.join(HERE, "plants_detail")

UP = "crates/skyzfs-upload/src/"
LEASE = UP + "lease.rs"
UPLOADER = UP + "uploader.rs"
BACKOFF = UP + "backoff.rs"
QUEUED = UP + "queued.rs"
LOCAL = UP + "local.rs"

TESTS = ["-p", "skyzfs-upload", "--lib", "--tests"]
TEST_LIMIT = 240
SCN_LIMIT = 600


def scn(filter_):
    return ("scenarios.rs " + filter_, ["python3", os.path.join(HERE, "scenarios.py"), CHECKOUT, "{out}", filter_])


PLANTS = [
    # --- the lease (lease.rs) -----------------------------------------
    ("K1", "a renewal takes the row as its own whatever the epoch", LEASE,
     "            Some(lease) if lease.holder == self.holder && lease.epoch == epoch => {}",
     "            Some(lease) if lease.holder == self.holder => {}", [scn("l1_")]),
    ("K2", "a renewal takes the row as its own whatever the holder", LEASE,
     "            Some(lease) if lease.holder == self.holder && lease.epoch == epoch => {}",
     "            Some(lease) if lease.epoch == epoch => {}", [scn("l1_")]),
    ("K3", "a renewal that finds no row takes the lease again", LEASE,
     "            None => {\n                return LeaseState::Lost {\n                    holder: String::new(),\n                    epoch: 0,\n                }\n            }\n",
     "            None => {}\n", [scn("l5_")]),
    ("K4", "a renewal that took the lease at a newer epoch does not give it back", LEASE,
     "                let _ = self.refs.lease_release(self.vol, &self.holder).await;\n",
     "", [scn("l4_")]),
    ("K4b", "a renewal that took the lease at a newer epoch holds it at that epoch", LEASE,
     "            Ok(LeaseOutcome::Acquired(lease)) if lease.epoch == epoch => LeaseState::Held {\n                epoch,",
     "            Ok(LeaseOutcome::Acquired(lease)) => LeaseState::Held {\n                epoch: lease.epoch,", [scn("l1_")]),
    ("K5", "a renewal's take that finds another holder leaves the lease unsure", LEASE,
     "            Ok(LeaseOutcome::Held(lease)) => LeaseState::Lost {\n                holder: lease.holder,\n                epoch: lease.epoch,\n            },\n            Err(e) => unsure(e),",
     "            Ok(LeaseOutcome::Held(_)) => LeaseState::Unsure {\n                epoch,\n                why: \"held\".into(),\n            },\n            Err(e) => unsure(e),", []),
    ("K6", "a first take that finds another holder is tried again (not lost)", LEASE,
     "            Ok(LeaseOutcome::Held(lease)) => LeaseState::Lost {\n                holder: lease.holder,\n                epoch: lease.epoch,\n            },\n            Err(e) => LeaseState::None(e.to_string()),",
     "            Ok(LeaseOutcome::Held(lease)) => LeaseState::None(lease.holder),\n            Err(e) => LeaseState::None(e.to_string()),", []),
    ("K7", "a lost lease is taken again at the next keep", LEASE,
     "            LeaseState::Lost { .. } => return &self.state,",
     "            LeaseState::Lost { .. } => self.take().await,", [scn("l1_")]),
    ("K8", "a commit the fence refused does not lose the lease", LEASE,
     "        self.state = LeaseState::Lost { holder, epoch };\n    }\n\n    /// Let the lease go",
     "        let _ = (holder, epoch);\n    }\n\n    /// Let the lease go", [scn("l1_")]),
    ("K9", "an unsure lease is not released", LEASE,
     "        if self.state.epoch().is_none() {\n            return Ok(());",
     "        if !matches!(self.state, LeaseState::Held { .. }) {\n            return Ok(());", []),
    ("K10", "an unsure lease has no epoch (LeaseState::epoch)", LEASE,
     "            LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. } => Some(*epoch),\n            LeaseState::None(_) | LeaseState::Lost { .. } => None,",
     "            LeaseState::Held { epoch, .. } => Some(*epoch),\n            LeaseState::Unsure { .. } | LeaseState::None(_) | LeaseState::Lost { .. } => None,", []),
    ("K11", "a renewal whose read fails loses the lease", LEASE,
     "            Err(e) => return unsure(e),",
     "            Err(_) => {\n                return LeaseState::Lost {\n                    holder: String::new(),\n                    epoch: 0,\n                }\n            }", []),
    ("K12", "a release leaves the keeper holding", LEASE,
     "        self.state = LeaseState::None(\"released\".into());\n",
     "", []),
    ("K13", "a renewal whose take fails loses the lease", LEASE,
     "            },\n            Err(e) => unsure(e),\n        }\n    }\n}",
     "            },\n            Err(_) => LeaseState::Lost {\n                holder: String::new(),\n                epoch: 0,\n            },\n        }\n    }\n}", []),
    # --- the uploader (uploader.rs) -----------------------------------
    ("P1", "no step while the lease is unsure", UPLOADER,
     "            LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. } => *epoch,",
     "            LeaseState::Unsure { .. } => return self.lease_at.saturating_duration_since(now),\n            LeaseState::Held { epoch, .. } => *epoch,", []),
    ("P2", "a cut found committed does not end the wait", UPLOADER,
     "\n                        | Step::FoundCommitted { .. }",
     "", [scn("b2_")]),
    ("P3", "a cut uploaded and not held does not end the wait", UPLOADER,
     "\n                        | Step::UploadedNotHeld { .. }",
     "", []),
    ("P3b", "a cut uploaded does not end the wait", UPLOADER,
     "                    Step::Uploaded { .. }\n                        | Step::UploadedNotHeld { .. }",
     "                    Step::UploadedNotHeld { .. }", []),
    ("P3c", "any step that goes on ends the wait (the reading the author sets aside)", UPLOADER,
     "                    self.backoff.succeeded();\n                }\n                self.state = State::Working(doing, under_way);",
     "                }\n                self.backoff.succeeded();\n                self.state = State::Working(doing, under_way);", [scn("b1_")]),
    ("P4", "no wait after a step that failed", UPLOADER,
     "            Err(e) => self.wait_after(now, e.to_string()),",
     "            Err(e) => {\n                let _ = self.wait_after(now, e.to_string());\n                self.step_at = now;\n                Duration::ZERO\n            }", []),
    ("P4b", "no wait after a commit given up", UPLOADER,
     "                self.wait_after(now, format!(\"commit of cut {number} refused: conflict\"))",
     "                let _ = self.wait_after(now, format!(\"commit of cut {number} refused: conflict\"));\n                self.step_at = now;\n                Duration::ZERO", []),
    ("P5", "a commit the fence refused is an error like another (wait and try again)", UPLOADER,
     "            Err(RemoteError::Fenced { holder, epoch }) => {",
     "            Err(RemoteError::Fenced { holder, epoch }) if false => {", [scn("l1_")]),
    ("P6", "a stopped driver is stepped all the same", UPLOADER,
     "        if driver.doing() == Doing::Stopped {",
     "        if false && driver.doing() == Doing::Stopped {", []),
    ("P7", "a step is taken during the wait", UPLOADER,
     "        if now < self.step_at {",
     "        if false && now < self.step_at {", []),
    ("P8", "the wait does not end for the lease's renewal", UPLOADER,
     "        until.min(self.lease_at).saturating_duration_since(now)",
     "        until.saturating_duration_since(now)", [scn("b3_")]),
    ("P9", "the lease is kept at every turn", UPLOADER,
     "        if now >= self.lease_at {",
     "        if true {", []),
    ("P10", "the lease is renewed at its whole time (not a third)", UPLOADER,
     "(self.lease.ttl_ms() / 3).max(1)",
     "self.lease.ttl_ms().max(1)", []),
    ("P11", "the driver is stepped though the lease is lost", UPLOADER,
     "            LeaseState::Lost { holder, epoch } => {\n                self.state = State::LeaseLost {\n                    holder: holder.clone(),\n                    epoch: *epoch,\n                };\n                return AT_REST;\n            }\n",
     "            LeaseState::Lost { epoch, .. } => *epoch,\n", [scn("l1_")]),
    ("P12", "the driver is stepped with no lease (at epoch 0)", UPLOADER,
     "            LeaseState::None(why) => {\n                self.state = State::NoLease(why.clone());\n                return self.lease_at.saturating_duration_since(now);\n            }\n",
     "            LeaseState::None(_) => 0,\n", []),
    ("P13", "an idle uploader asks again at once", UPLOADER,
     "                self.next_wait(now, now + IDLE)",
     "                Duration::ZERO", []),
    ("P14", "a failed take of the lease is tried again at once (no backoff)", UPLOADER,
     "            LeaseState::None(_) => after + self.lease_backoff.failed(),",
     "            LeaseState::None(_) => after,", []),
    ("P15", "the lease's backoff is never reset", UPLOADER,
     "                self.lease_backoff.succeeded();\n",
     "", []),
    ("R1", "run does not sleep between turns", UPLOADER,
     "                _ = tokio::time::sleep(wait) => {}",
     "                _ = tokio::time::sleep(Duration::ZERO) => {}", []),
    ("R2", "run does not stop during a turn", UPLOADER,
     "            let wait = tokio::select! {\n                _ = &mut stop => return,\n                wait = self.turn() => wait,\n            };",
     "            let wait = self.turn().await;", []),
    ("R3", "run does not publish", UPLOADER,
     "            publish(self.shown());\n",
     "", []),
    # --- the change of 1415db1: where the clock is read (uploader.rs) --
    ("X0", "all of 1415db1's change taken back: one clock read a turn", UPLOADER,
     ("            self.keep_lease(now).await;\n            now = Instant::now();\n",
      "        let step = driver.step().await;\n        let now = Instant::now();\n",
      "            LeaseState::None(_) => after + self.lease_backoff.failed(),",
      "            LeaseState::Lost { .. } => after + AT_REST,"),
     ("            self.keep_lease(now).await;\n",
      "        let step = driver.step().await;\n",
      "            LeaseState::None(_) => before + self.lease_backoff.failed(),",
      "            LeaseState::Lost { .. } => before + AT_REST,"), [scn("c1_")]),
    ("X1", "the clock is not read again after the lease is kept", UPLOADER,
     "            self.keep_lease(now).await;\n            now = Instant::now();\n",
     "            self.keep_lease(now).await;\n", []),
    ("X1s", "as X1 (the clock is not read again after the lease is kept), with the scenario of a slow take", UPLOADER,
     "            self.keep_lease(now).await;\n            now = Instant::now();\n",
     "            self.keep_lease(now).await;\n", [scn("c4_")]),
    ("X2", "the clock is not read again after the step", UPLOADER,
     "        let step = driver.step().await;\n        let now = Instant::now();\n",
     "        let step = driver.step().await;\n", [scn("c1_")]),
    ("X3", "a failed take's backoff is counted from before the take", UPLOADER,
     "            LeaseState::None(_) => after + self.lease_backoff.failed(),",
     "            LeaseState::None(_) => before + self.lease_backoff.failed(),", []),
    ("X3s", "as X3 (a failed take's backoff is counted from before the take), with the scenario of a slow take", UPLOADER,
     "            LeaseState::None(_) => after + self.lease_backoff.failed(),",
     "            LeaseState::None(_) => before + self.lease_backoff.failed(),", [scn("c4_")]),
    ("X4", "the rest after a lost lease is counted from before the take", UPLOADER,
     "            LeaseState::Lost { .. } => after + AT_REST,",
     "            LeaseState::Lost { .. } => before + AT_REST,", []),
    ("X5", "the next renewal is counted from when the take came back (not from when it was sent)", UPLOADER,
     "                before + renew\n",
     "                after + renew\n", []),
    # --- the backoff (backoff.rs) --------------------------------------
    ("B1", "the wait does not double", BACKOFF,
     "        self.next = Some(wait.saturating_mul(2).min(LAST));",
     "        self.next = Some(wait.min(LAST));", []),
    ("B2", "the wait has no upper bound", BACKOFF,
     "        self.next = Some(wait.saturating_mul(2).min(LAST));",
     "        self.next = Some(wait.saturating_mul(2));", []),
    ("B3", "a success does not reset the wait", BACKOFF,
     "    pub fn succeeded(&mut self) {\n        self.next = None;",
     "    pub fn succeeded(&mut self) {", []),
    # --- the desk (queued.rs) ------------------------------------------
    ("Q1", "close leaves the call waiting", QUEUED,
     "        slot.closed = true;\n        slot.call = None;",
     "        slot.closed = true;", []),
    ("Q2", "a call on a closed desk is left there and waited for", QUEUED,
     "            if slot.closed {\n                return None;\n            }\n",
     "", []),
    ("Q3", "a read on a closed desk answers a slice of nothing (not an error)", QUEUED,
     "            _ => (Buf::zeroed(0), Err(gone())),",
     "            _ => (Buf::zeroed(0), Ok(CutSlice { blocks: Vec::new(), next: None, copied: false })),", []),
    ("Q4", "a release on a closed desk answers that the cut was let go", QUEUED,
     "            _ => Released::Gone,",
     "            _ => Released::LetGo,", [scn("d4_")]),
    ("Q4b", "a naming on a closed desk answers that it named", QUEUED,
     "            _ => false,",
     "            _ => true,", [scn("d4_")]),
    ("Q5", "the sync of the copies on a closed desk answers done", QUEUED,
     "            Some(Answer::SyncCopies(synced)) => synced,\n            _ => Err(gone()),",
     "            Some(Answer::SyncCopies(synced)) => synced,\n            _ => Ok(()),", [scn("d4_")]),
    ("Q6", "a call does not wake the queue's thread", QUEUED,
     "        (self.wake)();\n        answer.await.ok()",
     "        answer.await.ok()", [scn("d3_")]),
    ("Q7", "the queue's thread is woken before the call is left", QUEUED,
     ("        let (reply, answer) = oneshot::channel();\n        {\n",
      "        (self.wake)();\n        answer.await.ok()"),
     ("        let (reply, answer) = oneshot::channel();\n        (self.wake)();\n        {\n",
      "        answer.await.ok()"), [scn("d3_")]),
    ("Q7b", "the queue's thread is woken before the call is left, on an open desk only", QUEUED,
     "        {\n            let mut slot = self.slot();\n            if slot.closed {\n                return None;\n            }\n"
     "            // One caller, which waits for each answer: the slot is empty.\n            slot.call = Some((call, reply));\n        }\n"
     "        (self.wake)();\n        answer.await.ok()",
     "        if self.slot().closed {\n            return None;\n        }\n        (self.wake)();\n"
     "        {\n            let mut slot = self.slot();\n            if slot.closed {\n                return None;\n            }\n"
     "            slot.call = Some((call, reply));\n        }\n"
     "        answer.await.ok()", [scn("d3_")]),
    ("Q8", "the desk answers the sync of the copies as done without asking the store", QUEUED,
     "        Call::SyncCopies(number) => Answer::SyncCopies(store.sync_copies(number)),",
     "        Call::SyncCopies(number) => {\n            let _ = number;\n            Answer::SyncCopies(Ok(()))\n        }", []),
    ("Q9", "the desk answers a release without asking the store", QUEUED,
     "        Call::Release(number) => Answer::Release(store.release(number)),",
     "        Call::Release(number) => {\n            let _ = number;\n            Answer::Release(Released::LetGo)\n        }", []),
    ("Q10", "the desk does not answer a call after it has closed, though it took it before", QUEUED,
     "        let answer = answer_from(store, call);\n        self.slot().answered += 1;",
     "        let answer = answer_from(store, call);\n        if self.slot().closed {\n            return false;\n        }\n        self.slot().answered += 1;", []),
]


def run(cmd, timeout, env=ENV):
    proc = subprocess.Popen(cmd, cwd=CHECKOUT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, start_new_session=True)
    try:
        out, _ = proc.communicate(timeout=timeout)
        return proc.returncode, out
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, _ = proc.communicate()
        return None, out


def status():
    return subprocess.run(["git", "status", "--short"], cwd=CHECKOUT, capture_output=True, text=True).stdout.strip()


def done():
    if not os.path.exists(OUT):
        return set()
    return {line.split()[0] for line in open(OUT) if line.strip() and not line.startswith("#")}


def record(line):
    with open(OUT, "a") as f:
        f.write(line + "\n")
    print(line, flush=True)


def one(pid, what, path, old, new, extras):
    full = os.path.join(CHECKOUT, path)
    text = open(full).read()
    olds = old if isinstance(old, tuple) else (old,)
    news = new if isinstance(new, tuple) else (new,)
    planted = text
    for o, n in zip(olds, news):
        if planted.count(o) != 1:
            return record(f"{pid}\tNOT APPLIED\t{what}\t(the text is there {planted.count(o)} times)")
        planted = planted.replace(o, n)
    os.makedirs(DETAIL, exist_ok=True)
    try:
        open(full, "w").write(planted)
        code, out = run(["cargo", "test", "--no-run"] + TESTS, 1800)
        if code != 0:
            open(os.path.join(DETAIL, pid + ".build"), "w").write(out[-8000:])
            return record(f"{pid}\tNOT APPLIED\t{what}\t(it does not build: plants_detail/{pid}.build)")
        code, out = run(["cargo", "test", "--no-fail-fast"] + TESTS, TEST_LIMIT)
        open(os.path.join(DETAIL, pid + ".tests"), "w").write(out[-30000:])
        failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))
        if code is None:
            running = sorted(set(re.findall(r"^test (\S+) has been running", out, re.M)))
            verdict = "KILLED\t" + what + f"\tby a test that does not end ({TEST_LIMIT} s)" + (
                ": " + ", ".join(running) if running else "") + ("; and " + ", ".join(failed) if failed else "")
        elif code != 0:
            verdict = "KILLED\t" + what + f"\tby {len(failed)}: " + ", ".join(failed[:6]) + (" ..." if len(failed) > 6 else "")
            if not failed:
                verdict += "(no test named: see plants_detail/" + pid + ".tests)"
        else:
            verdict = "SURVIVED\t" + what + "\t"
        for name, cmd in extras:
            eout = os.path.join(DETAIL, pid + ".with")
            if os.path.exists(eout):
                os.remove(eout)
            run([c.replace("{out}", eout) for c in cmd], SCN_LIMIT + 1800,
                env=dict(ENV, ALLOW_DIRTY="1", SCN_TIMEOUT=str(SCN_LIMIT)))
            got = open(eout).read() if os.path.exists(eout) else ""
            if "TIMEOUT" in got:
                how = f"DOES NOT END ({SCN_LIMIT} s; plants_detail/{pid}.with)"
            elif re.search(r"test result: ok", got) and not re.search(r"FAILED|panicked", got):
                how = "passes"
            elif not got or "build errors" in got:
                how = "DID NOT RUN (plants_detail/" + pid + ".with)"
            else:
                how = "FAILS (plants_detail/" + pid + ".with)"
            verdict += f"\twith {name}: " + how
        record(pid + "\t" + verdict)
    finally:
        open(full, "w").write(text)


def main():
    if status():
        sys.exit("the checkout has changes:\n" + status())
    seen = done()
    for plant in PLANTS:
        if ONLY and plant[0] not in ONLY:
            continue
        if plant[0] in seen:
            continue
        one(*plant)
        if status():
            sys.exit("the checkout has changes after " + plant[0] + ":\n" + status())
    print("done; the checkout is clean")


main()
