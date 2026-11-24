"""Textual plants against the head of #133 (d040d95), one at a time, each
file restored from its saved text after the run (try/finally). Each
suite runs in a process group of its own and is killed whole at its
limit. Plants on a race (U8, U9 and mine U8c, U9c) are also run again
REPEAT times on the race tests alone. Run from a clean worktree of the head:
WORKTREE=<path> CARGO_TARGET_DIR=<dir> python3 plants.py [names...]
Results: KILLED (the tests that failed), SURVIVED, NOT APPLIED, BUILD ERROR."""
import json, os, signal, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
LIMIT = int(os.environ.get("PLANT_LIMIT", "900"))
REPEAT = int(os.environ.get("REPEAT", "10"))
SUITES = {
    "r": ["cargo", "test", "-p", "skynbd-refs"],
    "d": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib"],
    "l": ["cargo", "test", "-p", "skyzfs-local", "--features", "test-hooks", "--lib", "--", "inplace::tests::cuts::"],
}
RACE = ["cargo", "test", "-p", "skynbd-refs", "--lib", "--", "renews_in_one", "take_committed"]
LIB = "crates/skynbd-refs/src/lib.rs"
RDB = "crates/skynbd-refs/src/redb_store.rs"
ACT = "crates/skynbd-refs/src/acting.rs"
CT = "bins/skyzfsd/src/control.rs"
CLI = "bins/skyzfsd/src/cli.rs"
SV = "bins/skyzfsd/src/serve.rs"
IP = "crates/skyzfs-local/src/inplace.rs"

MEM_RENEW = ("        let mut leases = self.leases.write().await;\n"
             "        let (write, outcome) = lease::renew(leases.get(&vol), holder, epoch, ttl_ms, now);\n")
RDB_RENEW_TAIL = """            let (write, outcome) = lease::renew(current.as_ref(), holder, epoch, ttl_ms, now);
            if let Some(l) = write {
                table
                    .insert(vol.0 .0, (l.holder.as_str(), l.epoch, l.expires_at_ms))
                    .map_err(backend)?;
            }
            outcome
        };
        txn.commit().map_err(backend)?;
        Ok(outcome)
"""

P = [
 # The author's twelve, written again from the description's table.
 ("Q1 (author) the header sync is off unless asked for, as before Q7", CLI,
  "            let intent_header_sync = !no_header_sync;\n",
  "            let intent_header_sync = false && !no_header_sync;\n", "d"),
 ("Q2 (author) --no-intent-header-sync is taken without --hold-cuts", CLI,
  "            if no_header_sync && !hold_cuts {\n", "            if false && no_header_sync && !hold_cuts {\n", "d"),
 ("Q3 (author) the option that turned the sync on is still taken (and ignored)", CLI,
  "    opt(\"--no-intent-header-sync\", false, false),\n",
  "    opt(\"--no-intent-header-sync\", false, false),\n    opt(\"--intent-header-sync\", false, false),\n", "d"),
 ("Q4 (author) the daemon does not tell the store to sync the header", SV,
  "        intent_header_sync: serve.intent_header_sync,\n", "        intent_header_sync: false,\n", "d"),
 ("N6 (author) a failed store's drop waits for a header's sync under way", IP,
  "                !intent.waiting.is_empty() || (intent.syncing.is_some() && !failed)\n",
  "                !intent.waiting.is_empty()\n                    || (intent.syncing.is_some() && (!failed || intent.syncing_header))\n", "l"),
 ("FA (author) a release on a failed store begins no header's sync", IP,
  "        let failed = self.failed;\n        self.begin_intent_sync();\n        failed || !self.failed\n",
  "        let failed = self.failed;\n        if failed {\n            return true;\n        }\n        self.begin_intent_sync();\n        failed || !self.failed\n", "l"),
 ("CA (author) a refused or missing socket is tried again until the limit", CT,
  "            Err(Errno::AGAIN | Errno::INTR) => {\n",
  "            Err(Errno::AGAIN | Errno::INTR | Errno::CONNREFUSED | Errno::NOENT) => {\n", "d"),
 ("CB (author) no pause between tries", CT,
  "                std::thread::sleep(pause.min(left));\n", "                let _ = pause.min(left);\n", "d"),
 ("CD (author) the pause does not grow", CT,
  "                pause = (pause * 2).min(CONNECT_PAUSE);\n", "", "d"),
 ("U9 (author) the memory store reads under a read lock, then writes under another", LIB, MEM_RENEW,
  "        let current = self.leases.read().await.get(&vol).cloned();\n"
  "        let mut leases = self.leases.write().await;\n"
  "        let (write, outcome) = lease::renew(current.as_ref(), holder, epoch, ttl_ms, now);\n", "r"),
 ("U8 (author) redb reads the row outside its write transaction", RDB,
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
            let (write, outcome) = lease::renew(""", "r"),
 ("U17 (author) the acting store renews at its own epoch, not the one asked", ACT,
  "        self.inner.lease_renew(vol, holder, epoch, ttl_ms).await",
  "        let _ = epoch;\n        self.inner.lease_renew(vol, holder, self.epoch, ttl_ms).await", "r"),

 # Mine.
 ("Q6 a volume that uploads never syncs the header", SV,
  "        intent_header_sync: serve.intent_header_sync,\n",
  "        intent_header_sync: serve.intent_header_sync && !uploads,\n", "d"),
 ("Q8 --no-intent-header-sync is ignored where --hold-cuts is given", CLI,
  "            let intent_header_sync = !no_header_sync;\n",
  "            let intent_header_sync = !no_header_sync || hold_cuts;\n", "d"),
 ("Q9 the help's paragraph names the old option", CLI,
  "      the volume. With --no-intent-header-sync (only with --hold-cuts)\n",
  "      the volume. With --intent-header-sync (only with --hold-cuts)\n", "d"),
 ("CE the pause is not capped at 50 ms (it keeps doubling)", CT,
  "                pause = (pause * 2).min(CONNECT_PAUSE);\n",
  "                pause = (pause * 2).min(CONNECT_PAUSE * 1000);\n", "d"),
 ("CF the pause starts at the cap, 50 ms", CT,
  "    let mut pause = Duration::from_millis(1);\n", "    let mut pause = CONNECT_PAUSE;\n", "d"),
 ("CI the last pause is not cut to what is left of the limit", CT,
  "                std::thread::sleep(pause.min(left));\n", "                let _ = left;\n                std::thread::sleep(pause);\n", "d"),
 ("U9c the memory store judges under one write lock and writes under a second", LIB,
  MEM_RENEW + "        if let Some(l) = write {\n",
  "        let leases = self.leases.write().await;\n"
  "        let (write, outcome) = lease::renew(leases.get(&vol), holder, epoch, ttl_ms, now);\n"
  "        drop(leases);\n        let mut leases = self.leases.write().await;\n"
  "        if let Some(l) = write {\n", "r"),
 ("U8c redb judges in one write transaction and writes in a second", RDB, RDB_RENEW_TAIL,
  """            lease::renew(current.as_ref(), holder, epoch, ttl_ms, now)
        };
        txn.commit().map_err(backend)?;
        let (write, outcome) = outcome;
        if let Some(l) = write {
            let txn = self.db.begin_write().map_err(backend)?;
            {
                let mut table = txn.open_table(OWNER).map_err(backend)?;
                table
                    .insert(vol.0 .0, (l.holder.as_str(), l.epoch, l.expires_at_ms))
                    .map_err(backend)?;
            }
            txn.commit().map_err(backend)?;
        }
        Ok(outcome)
""", "r"),
]
RACY = ("U9 ", "U8 ", "U9c ", "U8c ")


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


def failed_of(out):
    return [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]


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
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})"))
        print(f"{name}: NOT APPLIED ({n})", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    extra = ""
    try:
        open(path, "w").write(text.replace(old, new))
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
                failed = failed_of(out)
                verdict = f"KILLED ({s}): " + ", ".join(failed[:6]) + (f" and {len(failed) - 6} more" if len(failed) > 6 else "")
                break
        if name.startswith(RACY) and verdict != "BUILD ERROR":
            per = {}
            for i in range(REPEAT):
                code, out, timed_out = run_suite(RACE)
                for test in failed_of(out) if not timed_out else ["TIMEOUT"]:
                    per[test] = per.get(test, 0) + 1
            extra = f"; race tests alone, {REPEAT} runs: " + (", ".join(f"{k} failed {v}/{REPEAT}" for k, v in sorted(per.items())) or "none failed")
    finally:
        open(path, "w").write(text)
    res.append((name, verdict + extra))
    print(f"{name}: {verdict}{extra} ({time.time() - t:.0f}s)", flush=True)
left = subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout
print("worktree after:", left.strip() or "clean")
json.dump(res, open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "plants-result.json"), "w"), indent=1)
print("done")
