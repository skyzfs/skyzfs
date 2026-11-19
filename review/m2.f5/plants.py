"""Textual plants against the head of #131 (c973611), one at a time, each
file restored from its saved text after the run (try/finally). Each
suite runs in a process group of its own and is killed whole at its
limit. Run from a clean worktree of the head:
WORKTREE=<path> python3 plants.py [names...]
Results: KILLED (the tests that failed), SURVIVED, NOT APPLIED, BUILD ERROR."""
import json, os, signal, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
LIMIT = int(os.environ.get("PLANT_LIMIT", "900"))
SUITES = {
    "r": ["cargo", "test", "-p", "skynbd-refs"],
    "u": ["cargo", "test", "-p", "skyzfs-upload", "--tests", "--lib"],
    "d": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib"],
}
LIB = "crates/skynbd-refs/src/lib.rs"
OBJ = "crates/skynbd-refs/src/object_store.rs"
RDB = "crates/skynbd-refs/src/redb_store.rs"
CAD = "crates/skynbd-refs/src/cadence.rs"
ACT = "crates/skynbd-refs/src/acting.rs"
LEA = "crates/skyzfs-upload/src/lease.rs"
UL = "crates/skyzfs-upload/src/uploader.rs"
QU = "crates/skyzfs-upload/src/queued.rs"
CT = "bins/skyzfsd/src/control.rs"
SV = "bins/skyzfsd/src/control/server.rs"

RENEW_ARM = "            Some(cur) if cur.holder == holder && cur.epoch == epoch => {\n"
UPDATE_INNER_RETRY = """                Err(StoreError::PreconditionFailed(_)) => continue,
                Err(e) => return Err(RefError::Backend(e.to_string())),
            }
        }
        Err(RefError::Backend(format!(
            "refs object for {vol} contended beyond {RACE_RETRIES} retries"
        )))
    }

    /// [`Self::update_inner`] fenced"""
OBJ_JUDGE = "            let (write, outcome) = lease::renew(current.as_ref(), &holder, epoch, ttl_ms, clock());\n"
KEEPER_ERR = """            Err(e) => LeaseState::Unsure {
                epoch,
                why: e.to_string(),
            },"""

P = [
 # A sample of the author's seventeen, written again from the description.
 ("R1 (author) the renewal compares the epoch alone", LIB, RENEW_ARM,
  "            Some(cur) if cur.epoch == epoch => {\n", "ru"),
 ("R3 (author) an expired row of this holder and epoch is not renewed", LIB, RENEW_ARM,
  "            Some(cur) if cur.holder == holder && cur.epoch == epoch && cur.expires_at_ms > now_ms => {\n", "ru"),
 ("R6 (author) not held: the row read is written back", OBJ,
  "                // Not held: nothing to write, the answer is final.\n                None => Err(outcome),\n",
  "                None => Ok((outcome, vec![])),\n", "ru"),
 ("O1 (author) the object store renews without looking at the row", OBJ, OBJ_JUDGE,
  "            let me = Lease { holder: holder.clone(), epoch, expires_at_ms: 0 };\n"
  "            let _ = &current;\n"
  "            let (write, outcome) = lease::renew(Some(&me), &holder, epoch, ttl_ms, clock());\n", "ru"),
 ("K3 (author) the keeper takes a failed renewal as lost", LEA, KEEPER_ERR,
  "            Err(e) => LeaseState::Lost { holder: e.to_string(), epoch },", "u"),
 ("A1 (author) ask connects blocking", CT,
  "    let mut stream = connect(path, limit)?;\n",
  "    let mut stream = UnixStream::connect(path)?;\n", "d"),
 ("A2 (author) the connect's tries do not stop at the limit", CT,
  "                if left.is_zero() {\n", "                if left.is_zero() && limit.is_zero() {\n", "d"),
 ("A3 (author) a full backlog is an error at once", CT,
  "            Err(Errno::AGAIN | Errno::INTR) => {\n", "            Err(Errno::INTR) => {\n", "d"),
 ("A4 (author) the stream is left non-blocking", CT,
  "    stream.set_nonblocking(false)?;\n", "", "d"),
 ("N5 (author) every error of accept is tried again at once", SV,
  "fn gave_up(e: &io::Error) -> bool {\n    matches!(",
  "fn gave_up(e: &io::Error) -> bool {\n    true || matches!(", "d"),
 ("K13 (author) the turns' count starts again at a failure of the cut's step", UL,
  "            Err(e) => {\n                let wait = self.errors.failed();\n",
  "            Err(e) => {\n                self.turns.restart();\n                let wait = self.errors.failed();\n", "u"),
 ("N2 (author) an ask of the count does not wake the queue's thread", QU,
  "        (self.wake)();\n        answer.await.ok()\n    }\n\n    /// Leave `call`",
  "        answer.await.ok()\n    }\n\n    /// Leave `call`", "u"),

 # Mine: the conditional write and the rule.
 ("U1 update_inner retries blindly: on a refused write, the state judged before is written again under the new etag", OBJ,
  UPDATE_INNER_RETRY,
  UPDATE_INNER_RETRY.replace(
   "                Err(StoreError::PreconditionFailed(_)) => continue,\n",
   "                Err(StoreError::PreconditionFailed(_)) => {\n"
   "                    let (_, fresh) = self.load(vol).await?;\n"
   "                    return match self.store.put_if(&refs_key(vol), state.encode()?, fresh).await {\n"
   "                        Ok(_) => Ok(out),\n"
   "                        Err(e) => Err(RefError::Backend(e.to_string())),\n"
   "                    };\n"
   "                }\n"), "ru"),
 ("U2 the object store's renewal judges the rule once and keeps that decision on a retry", OBJ, OBJ_JUDGE,
  "            let (write, outcome) = first\n"
  "                .get_or_insert_with(|| lease::renew(current.as_ref(), &holder, epoch, ttl_ms, clock()))\n"
  "                .clone();\n", "ru"),
 ("U3 the object store's renewal is a take in one write (lease::acquire)", OBJ, OBJ_JUDGE,
  "            let (write, taken) = lease::acquire(current.as_ref(), &holder, ttl_ms, clock());\n"
  "            let _ = epoch;\n"
  "            let outcome = match taken {\n"
  "                LeaseOutcome::Acquired(l) => RenewOutcome::Renewed(l),\n"
  "                LeaseOutcome::Held(l) => RenewOutcome::NotHeld(Some(l)),\n"
  "            };\n", "ru"),
 ("U5 the keeper adopts a newer epoch of its own name", LEA,
  "            Ok(RenewOutcome::NotHeld(Some(lease))) => LeaseState::Lost {",
  "            Ok(RenewOutcome::NotHeld(Some(lease))) if lease.holder == self.holder => LeaseState::Held {\n"
  "                epoch: lease.epoch,\n"
  "                expires_at_ms: lease.expires_at_ms,\n"
  "            },\n"
  "            Ok(RenewOutcome::NotHeld(Some(lease))) => LeaseState::Lost {", "u"),
 ("U6 a released row is not renewed", LIB, RENEW_ARM,
  "            Some(cur) if cur.holder == holder && cur.epoch == epoch && cur.expires_at_ms != 0 => {\n", "ru"),
 ("U7 NotHeld answers no row in place of the row read", LIB,
  "            other => (None, RenewOutcome::NotHeld(other.cloned())),",
  "            other => {\n                let _ = other;\n                (None, RenewOutcome::NotHeld(None))\n            }", "ru"),
 ("U8 redb reads the row outside the write transaction", RDB,
  """        let now = (self.clock)();
        let txn = self.db.begin_write().map_err(backend)?;
        let outcome = {
            let mut table = txn.open_table(OWNER).map_err(backend)?;
            let current = table
                .get(vol.0 .0)
                .map_err(backend)?
                .map(|g| decode_lease(g.value()));
            let (write, outcome) = lease::renew(""",
  """        let now = (self.clock)();
        let current = self.lease_get(vol).await?;
        let txn = self.db.begin_write().map_err(backend)?;
        let outcome = {
            let mut table = txn.open_table(OWNER).map_err(backend)?;
            let (write, outcome) = lease::renew(""", "ru"),
 ("U9 the memory store reads the row under a read lock, then writes under another", LIB,
  "        let mut leases = self.leases.write().await;\n        let (write, outcome) = lease::renew(leases.get(&vol), holder, epoch, ttl_ms, now);",
  "        let current = self.leases.read().await.get(&vol).cloned();\n        let mut leases = self.leases.write().await;\n        let (write, outcome) = lease::renew(current.as_ref(), holder, epoch, ttl_ms, now);", "ru"),
 ("U12 an unsure keeper takes (lease_acquire) instead of renewing", LEA,
  "            LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. } => {\n",
  "            LeaseState::Unsure { .. } => self.take().await,\n            LeaseState::Held { epoch, .. } => {\n", "u"),
 ("U13 the renewal renews a newer epoch of its holder, writing its own older epoch", LIB, RENEW_ARM,
  "            Some(cur) if cur.holder == holder && cur.epoch >= epoch => {\n", "ru"),
 ("U15 the renewal pushes the row's old expiry out, not the time now", LIB,
  "                    expires_at_ms: now_ms.saturating_add(ttl_ms),\n                };\n                (Some(renewed.clone()), RenewOutcome::Renewed(renewed))",
  "                    expires_at_ms: cur.expires_at_ms.saturating_add(ttl_ms),\n                };\n                (Some(renewed.clone()), RenewOutcome::Renewed(renewed))", "ru"),
 ("U16 the cadenced store renews in its cache, not the authority", CAD,
  "        self.authority.lease_renew(vol, holder, epoch, ttl_ms).await",
  "        self.cache.lease_renew(vol, holder, epoch, ttl_ms).await", "ru"),
 ("U17 the acting store renews at its own epoch, not the one asked", ACT,
  "        self.inner.lease_renew(vol, holder, epoch, ttl_ms).await",
  "        let _ = epoch;\n        self.inner.lease_renew(vol, holder, self.epoch, ttl_ms).await", "rud"),
 ("U18 the keeper's renewal reads the row and then renews (two calls; the read decides)", LEA,
  "        match self\n            .refs\n            .lease_renew(self.vol, &self.holder, epoch, self.ttl_ms)\n            .await\n        {",
  "        match self.refs.lease_get(self.vol).await {\n"
  "            Ok(Some(l)) if l.holder == self.holder && l.epoch == epoch => {}\n"
  "            Ok(other) => return LeaseState::Lost { holder: other.as_ref().map(|l| l.holder.clone()).unwrap_or_default(), epoch: other.map(|l| l.epoch).unwrap_or(0) },\n"
  "            Err(e) => return LeaseState::Unsure { epoch, why: e.to_string() },\n"
  "        }\n"
  "        match self\n            .refs\n            .lease_acquire(self.vol, &self.holder, self.ttl_ms)\n            .await\n            .map(|o| match o {\n"
  "                LeaseOutcome::Acquired(l) => RenewOutcome::Renewed(l),\n"
  "                LeaseOutcome::Held(l) => RenewOutcome::NotHeld(Some(l)),\n"
  "            })\n        {", "u"),

 # Mine: the connect.
 ("CA a refused or missing socket is tried again until the limit", CT,
  "            Err(Errno::AGAIN | Errno::INTR) => {\n",
  "            Err(Errno::AGAIN | Errno::INTR | Errno::CONNREFUSED | Errno::NOENT) => {\n", "d"),
 ("CB no pause between tries (a busy loop)", CT,
  "                std::thread::sleep(pause.min(left));\n", "                let _ = pause.min(left);\n", "d"),
 ("CC the limit's refusal is not TimedOut", CT,
  "                        io::ErrorKind::TimedOut,\n", "                        io::ErrorKind::WouldBlock,\n", "d"),
 ("CD the pause does not grow", CT,
  "                pause = (pause * 2).min(CONNECT_PAUSE);\n", "", "d"),

 # Mine: the follow-ups' tests.
 ("QW a call does not wake the queue's thread (the sync's test, given a limit)", QU,
  "            slot.call = Some((call, reply));\n        }\n        (self.wake)();\n",
  "            slot.call = Some((call, reply));\n        }\n", "u"),
]

# The object store's judge-once plant needs its cache declared before
# the closure.
FIRST_DECL = ("        let clock = std::sync::Arc::clone(&self.clock);\n        // Unfenced as the other lease operations",
              "        let clock = std::sync::Arc::clone(&self.clock);\n        let mut first: Option<(Option<Lease>, RenewOutcome)> = None;\n        // Unfenced as the other lease operations")


def run_suite(cmd):
    proc = subprocess.Popen(cmd, cwd=W, env=ENV, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, start_new_session=True)
    try:
        out, _ = proc.communicate(timeout=LIMIT)
        return proc.returncode, out, False
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, _ = proc.communicate()
        return None, out, True


only = sys.argv[1:]
res = []
dirty = subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout
if dirty.strip():
    sys.exit("the worktree has changes:\n" + dirty)
for name, f, old, new, suites in P:
    if only and not any(name.startswith(o + " ") for o in only):
        continue
    path = os.path.join(W, f)
    text = open(path).read()
    n = text.count(old)
    extra = None
    if name.startswith("U2 "):
        extra = FIRST_DECL
        if text.count(extra[0]) != 1:
            n = 0
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})"))
        print(f"{name}: NOT APPLIED ({n})", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        planted = text.replace(old, new)
        if extra:
            planted = planted.replace(extra[0], extra[1])
        open(path, "w").write(planted)
        for s in suites:
            code, out, timed_out = run_suite(SUITES[s])
            if timed_out:
                verdict = f"KILLED ({s}: no end within {LIMIT} s)"
                break
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
                break
            if code != 0:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = f"KILLED ({s}): " + ", ".join(failed[:4]) + (f" and {len(failed) - 4} more" if len(failed) > 4 else "")
                break
    finally:
        open(path, "w").write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
left = subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout
print("worktree after:", left.strip() or "clean")
json.dump(res, open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "plants-%d.json" % os.getpid()), "w"), indent=1)
print("done")
