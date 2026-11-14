#!/usr/bin/env python3
"""Plants for #87 at a8c8c86: one at a time, each with the tests named, then restored."""
import os
import subprocess
import sys

TREE = "/home/user/skyzfs-pr87"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
PUMP = "crates/skyzfs-ublk/src/pump.rs"
SERVE = "bins/skyzfsd/src/serve.rs"
LIFE = "crates/skyzfs-ublk/src/lifecycle.rs"
QUEUE = "crates/skyzfs-ublk-sys/src/queue.rs"

T_PUMP = ["cargo", "test", "-q", "-p", "skyzfs-ublk", "--lib", "pump::tests"]
T_SERVE = ["cargo", "test", "-q", "-p", "skyzfsd", "--features", "test-hooks", "--lib", "serve::tests"]
T_SERVE_ALL = ["cargo", "test", "-q", "-p", "skyzfsd", "--features", "test-hooks", "--lib"]
T_SYS = ["cargo", "test", "-q", "-p", "skyzfs-ublk-sys", "--features", "control-test-program"]

PLANTS = [
    ("Q0 none", [], [T_PUMP, T_SERVE]),
    ("Q1 the stop's last flush does not read a refused fetch", [(PUMP,
      "        if let Some(refused) = self.driver_refused(&unfetched) {\n            return Some(refused);\n        }\n        Some(Stopped::Orderly)",
      "        if let Some(refused) = self\n            .driver_refused(&unfetched)\n            .filter(|s| !matches!(s, Stopped::FetchRefused { .. }))\n        {\n            return Some(refused);\n        }\n        Some(Stopped::Orderly)")], [T_PUMP]),
    ("Q2 the last refusal of a wait is named, not the first", [(PUMP,
      "            .find_map(|command| {", "            .filter_map(|command| {"),
     (PUMP, "                    _ => None,\n                }\n            })\n    }",
      "                    _ => None,\n                }\n            })\n            .last()\n    }")], [T_PUMP]),
    ("Q3 an aborted fetch is read as refused", [(PUMP,
      "            .filter(|command| command.result != ABORTED)\n", "")], [T_PUMP]),
    ("Q4 a failed volume's pump ends at a refused fetch", [(PUMP,
      "        let waited = self.queue.wait(&mut fetched, &mut unfetched);\n        self.count_unfetched(&unfetched);\n        let woken = match waited {",
      "        let waited = self.queue.wait(&mut fetched, &mut unfetched);\n        self.count_unfetched(&unfetched);\n        if let Some(r @ Stopped::FetchRefused { .. }) = self.driver_refused(&unfetched) {\n            return Some(r);\n        }\n        let woken = match waited {")], [T_PUMP]),
    ("Q5 a start thread that ended with no answer is waited for to the limit", [(SERVE,
      "            Err(mpsc::TryRecvError::Disconnected) => {\n                return StartWait::Answered(Err(io::Error::other(\n                    \"the start's thread ended with no answer\",\n                )))\n            }\n",
      "            Err(mpsc::TryRecvError::Disconnected) => {}\n")], [T_SERVE]),
    ("Q6 the failure and the queue's end looked at before an answer that came", [(SERVE,
      "    loop {\n        match answer.try_recv() {",
      "    loop {\n        if failed() {\n            return StartWait::Failed;\n        }\n        if ended() {\n            return StartWait::QueueEnded;\n        }\n        match answer.try_recv() {")], [T_SERVE]),
    ("Q7 a start given up deletes nothing", [(SERVE,
      "    (!took_over).then(|| lifecycle::delete_unstarted(control, records, number))",
      "    let _ = (control, records, number, took_over);\n    None")], [T_SERVE]),
    ("Q8 delete_unstarted deletes before it removes the note", [(LIFE,
      "    number: u32,\n) -> Result<(), Undeleted> {\n    delete(control, records, number, Mutants::default())\n}\n\npub(crate) fn fail_serving_with",
      "    number: u32,\n) -> Result<(), Undeleted> {\n    delete(control, records, number, Mutants(Mutants::DELETE_BEFORE_NOTE))\n}\n\npub(crate) fn fail_serving_with")], [T_SERVE]),
    ("Q9 a refused fetch at a take-over names the device's start", [(SERVE,
      "            if took_over {\n                \"the end of its recovery\"\n            } else {\n                \"the device's start\"\n            }",
      "            if !took_over {\n                \"the end of its recovery\"\n            } else {\n                \"the device's start\"\n            }")], [T_SERVE]),
    ("Q10 a refused fetch after the start says no advice", [(SERVE,
      "         as it is; {NOT_STARTED_ADVICE}\",", "         as it is\",")], [T_SERVE]),
    ("Q11 the queue's end not looked at again for a failure after the join", [(SERVE,
      "            let (stopped, _store) = joined(serving, number);\n            finish(&board);\n            if flags.failed.load(Ordering::SeqCst) {\n                return Ok(failed_at_end(&mut kernel, &mut records, number, identity));\n            }\n            return match stopped? {",
      "            let (stopped, _store) = joined(serving, number);\n            finish(&board);\n            return match stopped? {")], [T_SERVE_ALL]),
    ("Q12 Device::start makes no control ring", [(QUEUE,
      "        own_control_ring()?;\n        match self.control.start_dev",
      "        match self.control.start_dev")], [T_SYS]),
]


def run(cmd):
    p = subprocess.run(cmd, cwd=TREE, env=ENV, capture_output=True, text=True)
    out = p.stdout + p.stderr
    fails = [l.strip() for l in out.splitlines() if l.strip().startswith("test ") and l.strip().endswith("FAILED")]
    fails += [l.strip() for l in out.splitlines() if l.startswith("    ") and "::" in l and "FAILED" not in l and l.strip().count(" ") == 0]
    oks = [l for l in out.splitlines() if l.startswith("test result:")]
    err = "" if p.returncode == 0 or fails else " | " + " / ".join(l for l in out.splitlines() if l.startswith("error"))[:400]
    return p.returncode, sorted(set(fails)), oks, err


def main():
    for name, edits, tests in PLANTS:
        ok = True
        for path, old, new in edits:
            full = os.path.join(TREE, path)
            text = open(full).read()
            if text.count(old) != 1:
                print(f"{name}: ANCHOR NOT FOUND ONCE in {path}", flush=True)
                ok = False
                break
            open(full, "w").write(text.replace(old, new))
        if ok:
            for t in tests:
                rc, fails, oks, err = run(t)
                print(f"{name} [{' '.join(t[2:])}]: rc {rc}; failed: {fails if fails else 'nothing'}; {[o[13:60] for o in oks]}{err}", flush=True)
        subprocess.run(["git", "checkout", "--"] + sorted({e[0] for e in edits}), cwd=TREE) if edits else None
    st = subprocess.run(["git", "status", "--short"], cwd=TREE, capture_output=True, text=True).stdout
    print("worktree clean:", st.strip() == "", st)


if __name__ == "__main__":
    sys.exit(main())
