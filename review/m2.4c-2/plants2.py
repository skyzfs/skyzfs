#!/usr/bin/env python3
"""Plants for the next look at #118, at da0b4c9 (M2.4c-2).

    plants2.py WORKTREE TARGET_DIR GROUP [ID ...]

GROUP is `daemon` (plants in the daemon, the transport and the store) or
`crate` (plants in the upload crate): two scratch worktrees can run one
group each. One change at a time is written into the worktree, the
head's tests are run, and the file is restored. A plant is

- KILLED       when a test of the head fails with it (or does not end),
- SURVIVED     when every test passes with it,
- NOT APPLIED  when its text is not found exactly once, or it does not
               build.

Each result is appended to plants2.out as it comes; a plant already
there is skipped on a rerun. The worktree must be clean at the start and
is checked clean at the end.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "plants2.out")

SERVE = "bins/skyzfsd/src/serve.rs"
UPLOAD = "bins/skyzfsd/src/upload.rs"
STATE = "crates/skyzfs-local/src/inplace/state.rs"
STATUS = "crates/skyzfs-ublk/src/status.rs"
UP = "crates/skyzfs-upload/src/"
LEASE = UP + "lease.rs"
UPLOADER = UP + "uploader.rs"
QUEUED = UP + "queued.rs"

# D: the daemon's tests in the build with the test hooks (a superset of
# the default build's). P: the daemon's, default build. U: the
# transport's. L: the store's cut state. C: the upload crate's.
D = ["cargo", "test", "-p", "skyzfsd", "--features", "skyzfsd/test-hooks", "--no-fail-fast"]
P = ["cargo", "test", "-p", "skyzfsd", "--no-fail-fast"]
U = ["cargo", "test", "-p", "skyzfs-ublk", "--lib", "--no-fail-fast"]
L = ["cargo", "test", "-p", "skyzfs-local", "--lib", "--no-fail-fast", "inplace::state"]
C = ["cargo", "test", "-p", "skyzfs-upload", "--no-fail-fast"]
LIMIT = 400

ANSWER = """                    if ended {
                        desk.close();
                    } else {
                        desk.answer(pump.target());
                    }
"""
AS = "        && commit.{}\n"

DAEMON = [
    # --- the wiring in serve.rs's run, which passed at e42ea6a ---------
    ("W01", SERVE, ANSWER,
     "                    if ended {\n                        desk.close();\n                    }\n",
     "the queue's thread never answers the desk", [D]),
    ("W02", SERVE, ANSWER,
     "                    if !ended {\n                        desk.answer(pump.target());\n                    }\n",
     "the queue's thread does not close the desk at its end", [D]),
    ("W03", SERVE, "Desk::new(move || wake.wake())", "Desk::new(move || { let _ = &wake; })",
     "the desk's wake does nothing", [D]),
    ("W04", SERVE, "let release = exit == Exit::Done;", "let release = true;",
     "the lease is let go at every exit that reaches the end", [D]),
    ("W05", SERVE, "let release = exit == Exit::Done;", "let release = false;",
     "the lease is never let go", [D]),
    ("W07", SERVE, "volume.lock + volume.position + volume.recovery + lease_time",
     "volume.lock + volume.position + volume.recovery",
     "the lease's time is counted in the start's `devices` time", [D]),
    ("W08", SERVE, "            return Ok(Exit::LeaseHeld);", "            return Ok(Exit::RefusedHeld);",
     "a start refused for the lease exits 8, not 10", [D]),
    ("W12", SERVE, "        .uploading(uploads.is_some());", "        .uploading(false);",
     "the hook command `release` is taken in a daemon that uploads", [D]),
    ("W13", SERVE, "        .unwrap_or(crate::upload::DEFAULT_LEASE_TTL_S);", "        .unwrap_or(3600);",
     "the lease's default time is an hour", [D]),
    # --- the default holder --------------------------------------------
    ("H01", SERVE, '    let holder = format!("{name}:{id}");', '    let holder = format!("{name}");',
     "the default holder is the host's name alone", [D]),
    ("H02", SERVE, "    if id.len() != 32 || !id.bytes()", "    if !id.bytes()",
     "a machine id of another length is taken", [D]),
    ("H03", SERVE, "!id.bytes().all(|b| b.is_ascii_hexdigit()) || ", "",
     "a machine id that is not hexadecimal is taken", [D]),
    ("H04", SERVE, " || id.bytes().all(|b| b == b'0')", "",
     "a machine id of all zeros is taken", [D]),
    ("H05", SERVE, "        None => default_holder(Path::new(HOSTNAME), Path::new(MACHINE_ID)),",
     "        None => default_holder(Path::new(HOSTNAME), Path::new(HOSTNAME)),",
     "the machine id is read from another file", [D]),
    # --- the status's cuts replaced ------------------------------------
    ("T02", SERVE, "        replaced: Some(figures.replaced),", "        replaced: Some(figures.abandoned),",
     "the status's cuts replaced are the cuts abandoned", [D]),
    ("T03", STATUS, "    bytes[968..976].copy_from_slice(&held.replaced.unwrap_or(0).to_le_bytes());\n", "",
     "the status file does not carry the cuts replaced", [U]),
    ("T04", STATUS, "            replaced: (version >= 15).then(|| field(968)),",
     "            replaced: Some(field(968)),",
     "a status file of the fourteenth version is read as having cuts replaced", [U]),
    ("T05", STATUS, '                lines.push(("cuts_replaced", replaced.to_string()));\n', "",
     "the report does not print the cuts replaced", [U, D]),
    ("S01", STATE, "            self.give(flushing, Heir::Open);\n            self.free_unneeded(None);\n            self.replaced += 1;",
     "            self.give(flushing, Heir::Open);\n            self.free_unneeded(None);",
     "a cut still flushing at the next cut's instant is not counted replaced", [L]),
    ("S02", STATE, "                self.free_unneeded(Some(waiting.cut.number));\n                self.replaced += 1;",
     "                self.free_unneeded(Some(waiting.cut.number));",
     "a waiting cut replaced at a freeze is not counted", [L]),
    ("S03", STATE, "                        .then_some(waiting.cut.number),\n                );\n                self.replaced += 1;",
     "                        .then_some(waiting.cut.number),\n                );",
     "a waiting cut replaced when a newer cut's flush completes is not counted", [L]),
    # --- create ---------------------------------------------------------
    ("A00", UPLOAD, "    if clone {\n", "    if false {\n",
     "create takes a clone", [D]),
    ("A01", UPLOAD, "    commit.vol_id == vol\n        && commit.parent.is_none()", "    commit.parent.is_none()",
     "create takes a head commit of another volume", [D]),
    ("A02", UPLOAD, AS.format("parent.is_none()"), "",
     "create takes a head commit that has a parent", [D]),
    ("A03", UPLOAD, AS.format("segments.is_empty()"), "",
     "create takes a head commit that has segments", [D]),
    ("A04", UPLOAD, AS.format("group.is_none()"), "",
     "create takes a head commit that names a group", [D]),
    ("A05", UPLOAD, AS.format("flush_seq == 0"), "",
     "create takes a head commit that counts flushes", [D]),
    ("A06", UPLOAD, AS.format("epoch == 0"), "",
     "create takes a head commit written under a lease", [D]),
    ("A07", UPLOAD, AS.format("kind == CommitKind::Checkpoint"), "",
     "create takes a head commit of another kind", [D]),
    ("A08", UPLOAD, "        && commit.shard_index == *empty\n", "",
     "create takes a head commit whose index is not the empty one", [D]),
    ("A09", UPLOAD, "    let shards = usize::try_from(size.div_ceil(SHARD_BYTES)).unwrap_or(usize::MAX);",
     "    let shards = usize::try_from(size / SHARD_BYTES).unwrap_or(usize::MAX);",
     "the empty index has no shard for a last part shard", [D]),
    ("V01", UPLOAD,
     "        .put_if(&key, superblock.into(), Precondition::IfNoneMatch)\n        .await\n    {\n        Ok(_) => Ok(None),",
     "        .put(&key, superblock.into())\n        .await\n    {\n        Ok(()) => Ok(None),",
     "the create's superblock is written whether one is there or not", [D]),
    ("V02", UPLOAD, "            if (superblock.size, superblock.shard_bytes) != (size, SHARD_BYTES) {",
     "            if (superblock.size, superblock.shard_bytes) != (superblock.size, superblock.shard_bytes) {",
     "a create goes on over a superblock of another size or shards", [D]),
    ("V03", UPLOAD, "            if (superblock.size, superblock.shard_bytes) != (size, SHARD_BYTES) {",
     "            if superblock.size != size {",
     "a create goes on over a superblock of other shards", [D]),
    ("V04", UPLOAD, "            if (superblock.size, superblock.shard_bytes) != (size, SHARD_BYTES) {",
     "            if superblock.shard_bytes != SHARD_BYTES {",
     "a create goes on over a superblock of another size", [D]),
    ("V05", UPLOAD, "        let there = reserve(&store, vol, size)\n            .await\n            .map_err(|e| format!(\"making {named}: {e}\"))?;",
     "        let there = reserve(&store, vol, size).await.unwrap_or(None);",
     "a superblock that cannot be written does not refuse the create", [D]),
    ("V06", UPLOAD, "                keys::superblock(remote.id, 0),\n                remote.bucket",
     "                \"the superblock\",\n                remote.bucket",
     "the refusal over a superblock with no head does not name the object", [D]),
    ("V07", UPLOAD, "        Ok(Some(_)) => format!(\n            \"{named} holds {what}, not {asked}: another create",
     "        Ok(Some(_)) if false => format!(\n            \"{named} holds {what}, not {asked}: another create",
     "the refusal tells to delete the superblock though the volume's head is made", [D]),
    ("V08", UPLOAD, "            let same = if there.shard_bytes == SHARD_BYTES {", "            let same = if true {",
     "the refusal says a create of that size goes on, over a superblock of other shards", [D]),
    # --- the keys --------------------------------------------------------
    ("Y01", UPLOAD, '    if env("AWS_SESSION_TOKEN").is_some_and(|token| !token.is_empty()) {', "    if false {",
     "keys that come with a session token are signed with as fixed keys", [D]),
    ("Y02", UPLOAD, "        region: region(&env),", "        region: REGION.to_owned(),",
     "fixed keys sign in us-east-1 whatever AWS_REGION says", [D]),
    ("Y03", UPLOAD, "                &keys.region,", "                REGION,",
     "connect names us-east-1 to the client of fixed keys, whatever the keys' region", [D]),
    ("Y04", UPLOAD, "        .filter(|region| !region.is_empty())\n", "",
     "an empty AWS_REGION is the region", [D]),
    # --- the thread -------------------------------------------------------
    ("N01", UPLOAD, "                runtime.shutdown_background();\n", "",
     "the thread's end waits for the runtime's blocking work", [D]),
    ("N02", UPLOAD, "                            if !posted.closed() {", "                            if true {",
     "the thread posts after the desk is closed", [D]),
    ("N03", UPLOAD, "            out.expires_unix_ms = *expires_at_ms;\n            out.holder = Text::new(holder);",
     "            out.expires_unix_ms = *expires_at_ms;",
     "the status names no holder while the lease is held", [D]),
    ("N04", UPLOAD, "{why}\"));\n            out.holder = Text::new(holder);", "{why}\"));",
     "the status names no holder while the lease is unsure", [D]),
    ("N05", UPLOAD, "        Some(shown_of(&shown, self.keeper.holder()))", "        Some(shown_of(&shown, \"\"))",
     "the start's status names no holder", [D]),
    ("U12", UPLOAD, "                        Ok(Ok(())) if held => Ended::Released,",
     "                        Ok(Ok(())) if held || !held => Ended::Released,",
     "an end with no lease held says it was let go", [D]),
    ("U20p", UPLOAD, "ActingRefStore::new(refs.clone(), holder.clone(), epoch);",
     "ActingRefStore::new(refs.clone(), holder.clone(), epoch + 1);",
     "the uploader's commits are fenced by another epoch than the one held, default build", [P]),
    ("U21", UPLOAD, "EngineRemote::new(objects.clone(), Arc::new(acting), vol, engine_config(epoch))",
     "EngineRemote::new(objects.clone(), Arc::new(acting), vol, engine_config(0))",
     "the uploader's commits carry epoch 0, default build", [P]),
    ("U26", UPLOAD, "        let acting = ActingRefStore::new(refs.clone(), holder.clone(), epoch);",
     "        let acting = ActingRefStore::new(refs.clone(), String::from(\"somebody\"), epoch);",
     "the uploader's commits are made as another holder, default build", [P]),
]

CRATE = [
    # --- the two waits (F3 of #117's review) ---------------------------
    ("P2", UPLOADER, "                        | Step::FoundCommitted { .. }\n", "",
     "a cut found committed does not end the refusals' wait", [C]),
    ("P3", UPLOADER, "                        | Step::UploadedNotHeld { .. }\n", "",
     "a cut committed and gone does not end the refusals' wait", [C]),
    ("P3b", UPLOADER, "                    Step::Uploaded { .. }\n                        | Step::UploadedNotHeld { .. }",
     "                    Step::UploadedNotHeld { .. }",
     "a cut uploaded does not end the refusals' wait", [C]),
    ("P3c", UPLOADER, "                    self.refused.succeeded();\n                }",
     "                    self.refused.succeeded();\n                }\n                self.refused.succeeded();",
     "any step that goes on ends the refusals' wait", [C]),
    ("E1", UPLOADER, "        if step.is_ok() {\n            self.errors.succeeded();\n        }\n", "",
     "the errors' wait is never reset", [C]),
    ("E2", UPLOADER, "                let wait = self.refused.failed();", "                let wait = self.errors.failed();",
     "a give-up waits in the errors' backoff", [C]),
    ("E3", UPLOADER, "            Err(e) => {\n                let wait = self.errors.failed();",
     "            Err(e) => {\n                let wait = self.refused.failed();",
     "an error waits in the refusals' backoff", [C]),
    ("E4", UPLOADER, "if doing == Doing::Committing && !rereading => {", "if doing == Doing::Committing => {",
     "a conflict right after a refused commit is not waited after", [C]),
    ("E5", UPLOADER, "if doing == Doing::Committing && !rereading => {", "if !rereading => {",
     "a conflict anywhere but at the commit is followed at once by another step", [C]),
    ("E6", UPLOADER, "if doing == Doing::Committing && !rereading => {", "if false => {",
     "a refused commit waits as an error does, and again at its give-up", [C]),
    ("P4b", UPLOADER,
     "                self.wait_after(\n                    wait,\n                    ended,\n                    now,\n                    format!(\"commit of cut {number} refused: conflict\"),\n                )",
     "                self.wait_after(\n                    wait,\n                    ended,\n                    now,\n                    format!(\"commit of cut {number} refused: conflict\"),\n                );\n                Duration::ZERO",
     "a give-up announces its wait and returns none", [C]),
    ("P1", UPLOADER, "            LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. } => *epoch,",
     "            LeaseState::Unsure { .. } => {\n                return self.lease_at.saturating_duration_since(now);\n            }\n            LeaseState::Held { epoch, .. } => *epoch,",
     "no step while the lease is unsure", [C]),
    ("P6", UPLOADER, "        if driver.doing() == Doing::Stopped {", "        if false {",
     "a stopped driver is stepped all the same", [C]),
    ("P15", UPLOADER, "                lease_backoff.succeeded();\n", "",
     "the lease's backoff is never reset", [C]),
    # --- the renewal beside a step (F4 of #117's review) ---------------
    ("B1", UPLOADER, "                () = tokio::time::sleep_until(*lease_at) => {}",
     "                () = std::future::pending::<()>() => {}",
     "no renewal is made beside a step", [C]),
    ("B2", UPLOADER, "                    let ended = Instant::now();\n                    *lease_at = keep.await;\n                    return (step, ended);",
     "                    *lease_at = keep.await;\n                    let ended = Instant::now();\n                    return (step, ended);",
     "a step that ends during a renewal counts its wait from the renewal's end", [C]),
    ("B3", UPLOADER, "                    let ended = Instant::now();\n                    *lease_at = keep.await;\n                    return (step, ended);",
     "                    let ended = Instant::now();\n                    return (step, ended);",
     "a renewal under way when the step ends is dropped half done", [C]),
    ("B4", UPLOADER, "                self.next_wait(now, ended + IDLE)", "                self.next_wait(now, now + IDLE)",
     "the idle wait is counted from the turn's end, not the step's", [C]),
    ("B5", UPLOADER, "        self.step_at = ended + wait;", "        self.step_at = now + wait;",
     "a backoff is counted from the turn's end, not the step's", [C]),
    ("X1", UPLOADER, "now).await;\n            now = Instant::now();", "now).await;",
     "the clock is not read again after the lease is kept", [C]),
    ("X3", UPLOADER, "            LeaseState::None(_) => after + lease_backoff.failed(),",
     "            LeaseState::None(_) => before + lease_backoff.failed(),",
     "a failed take's backoff is counted from before the take", [C]),
    ("X4", UPLOADER, "            LeaseState::Lost { .. } => after + AT_REST,", "            LeaseState::Lost { .. } => before + AT_REST,",
     "the rest after a lost lease is counted from before the take", [C]),
    ("X5", UPLOADER, "                before + renew", "                after + renew",
     "the next renewal is counted from when the take came back", [C]),
    # --- run (F2 and G2 of #117's review) -------------------------------
    ("R1", UPLOADER, "                () = tokio::time::sleep(wait) => {}", "                () = tokio::time::sleep(Duration::ZERO) => {}",
     "run does not sleep between turns", [C]),
    ("R2", UPLOADER,
     "            let wait = tokio::select! {\n                biased;\n                () = &mut stop => break,\n                wait = self.turn() => wait,\n            };",
     "            let wait = self.turn().await;",
     "run does not stop during a turn", [C]),
    ("R3", UPLOADER, "            publish(self.shown());\n", "",
     "run does not publish", [C]),
    ("R4", UPLOADER, "            tokio::select! {\n                biased;\n                () = &mut stop => break,\n                () = tokio::time::sleep(wait) => {}\n            }",
     "            tokio::time::sleep(wait).await;",
     "run does not stop during the sleep between turns", [C]),
    # --- the desk (F1 and G3, G4 of #117's review) ----------------------
    ("Q8", QUEUED,
     "                match store\n                    .begin_sync_copies(number)\n                    .map(|()| store.copies_synced(false))\n                {",
     "                match {\n                    let _ = number;\n                    Ok::<Option<io::Result<()>>, io::Error>(Some(Ok(())))\n                } {",
     "the desk answers the sync of the copies as done without asking the store", [C]),
    ("Q11", QUEUED, "                    .map(|()| store.copies_synced(false))", "                    .map(|()| store.copies_synced(true))",
     "the desk waits for the sync of the copies on the queue's thread", [C]),
    ("Q12", QUEUED, "                    Ok(None) => self.park(reply),", "                    Ok(None) => drop(reply),",
     "the reply to a sync under way is dropped", [C]),
    ("Q13", QUEUED, "            let Some(synced) = store.copies_synced(false) else {\n                self.park(reply);\n                return false;\n            };",
     "            let Some(synced) = store.copies_synced(false) else {\n                drop(reply);\n                return false;\n            };",
     "the reply to a sync still under way at the next turn is dropped", [C]),
    ("Q14", QUEUED, "        slot.call = None;\n        slot.syncing = None;", "        slot.call = None;",
     "the close keeps the reply to a sync under way", [C]),
    ("Q15", QUEUED, "        let mut slot = self.slot();\n        if !slot.closed {\n            slot.syncing = Some(reply);\n        }",
     "        let mut slot = self.slot();\n        slot.syncing = Some(reply);",
     "a reply is parked on a closed desk", [C]),
    ("Q16", QUEUED, "            self.reply(reply, Answer::SyncCopies(synced));\n            done = true;",
     "            self.reply(reply, Answer::SyncCopies(Ok(())));\n            let _ = synced;\n            done = true;",
     "a sync that ended at a later turn is answered as done whatever its outcome", [C]),
    ("Q7b", QUEUED,
     "        {\n            let mut slot = self.slot();\n            if slot.closed {\n                return None;\n            }\n",
     "        if self.slot().closed {\n            return None;\n        }\n        (self.wake)();\n        {\n            let mut slot = self.slot();\n            if slot.closed {\n                return None;\n            }\n",
     "the queue's thread is woken before the call is left as well, on an open desk", [C]),
    ("Q7c", QUEUED, "        (self.wake)();\n        answer.await.ok()", "        answer.await.ok()",
     "a call does not wake the queue's thread", [C]),
    # --- the lease (G5 and G6 of #117's review) -------------------------
    ("K3", LEASE,
     "            None => {\n                return LeaseState::Lost {\n                    holder: String::new(),\n                    epoch: 0,\n                }\n            }\n",
     "            None => {}\n",
     "a renewal that finds no row takes the lease again", [C]),
    ("K5", LEASE,
     "            Ok(LeaseOutcome::Held(lease)) => LeaseState::Lost {\n                holder: lease.holder,\n                epoch: lease.epoch,\n            },\n            Err(e) => unsure(e),",
     "            Ok(LeaseOutcome::Held(_)) => LeaseState::Unsure {\n                epoch,\n                why: \"held\".into(),\n            },\n            Err(e) => unsure(e),",
     "a renewal's take that finds another holder leaves the lease unsure", [C]),
    ("K8", LEASE, "        self.state = LeaseState::Lost { holder, epoch };\n    }\n\n    /// Let the lease go",
     "        let _ = (holder, epoch);\n    }\n\n    /// Let the lease go",
     "a commit the fence refused does not lose the lease", [C]),
    ("K9", LEASE, "        if self.state.epoch().is_none() {\n            return Ok(());",
     "        if !matches!(self.state, LeaseState::Held { .. }) {\n            return Ok(());",
     "an unsure lease is not released", [C]),
    ("K13", LEASE, "            },\n            Err(e) => unsure(e),\n        }\n    }\n}",
     "            },\n            Err(_) => LeaseState::Lost {\n                holder: String::new(),\n                epoch: 0,\n            },\n        }\n    }\n}",
     "a renewal whose take fails loses the lease", [C]),
]


def status(tree):
    return subprocess.run(["git", "-C", tree, "status", "--short"],
                          capture_output=True, text=True).stdout.strip()


def run(cmd, tree, target):
    """(verdict, detail): 'pass', 'fail' with the failing tests, 'build', 'timeout'."""
    env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_TARGET_DIR=target, RUST_BACKTRACE="0",
               CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_TEST_DEBUG="0")
    # A plant can make a test of the head wait for ever: the run is ended,
    # with its test processes, after LIMIT seconds.
    proc = subprocess.Popen(cmd, cwd=tree, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, start_new_session=True)
    try:
        text, _ = proc.communicate(timeout=LIMIT)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        text, _ = proc.communicate()
        running = sorted(set(re.findall(r"^test (\S+) has been running for over", text, re.M)))
        names = ", ".join(name.split("::")[-1] for name in running[:3])
        return "timeout", f"no end within {LIMIT} s" + (f"; still running: {names}" if names else "")
    if "could not compile" in text:
        errors = re.findall(r"^error(?:\[E\d+\])?: (.*)$", text, re.M)
        return "build", "; ".join(errors[:2])
    failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", text, re.M)))
    if proc.returncode == 0 and not failed:
        return "pass", ""
    return "fail", failed


def main():
    tree, target, group, only = sys.argv[1], sys.argv[2], sys.argv[3], set(sys.argv[4:])
    plants = {"daemon": DAEMON, "crate": CRATE}[group]
    if status(tree):
        sys.exit("the worktree is not clean:\n" + status(tree))
    done = set()
    if os.path.exists(OUT):
        with open(OUT) as f:
            done = {line.split(" | ")[0] for line in f if " | " in line}
    for ident, path, old, new, what, cmds in plants:
        if ident in done or (only and ident not in only):
            continue
        full = os.path.join(tree, path)
        with open(full) as f:
            source = f.read()
        if source.count(old) != 1:
            result = f"NOT APPLIED (its text is found {source.count(old)} times)"
        else:
            try:
                with open(full, "w") as f:
                    f.write(source.replace(old, new))
                result = None
                for cmd in cmds:
                    verdict, detail = run(cmd, tree, target)
                    if verdict == "build":
                        result = f"NOT APPLIED (does not build: {detail})"
                        break
                    if verdict == "timeout":
                        result = f"KILLED: the head's tests have {detail}"
                        break
                    if verdict == "fail":
                        names = [name.split("::")[-1] for name in detail] or ["a run that failed and named no test"]
                        more = f" and {len(names) - 3} more" if len(names) > 3 else ""
                        result = f"KILLED by {', '.join(names[:3])}{more}"
                        break
                if result is None:
                    result = "SURVIVED"
            finally:
                with open(full, "w") as f:
                    f.write(source)
        line = f"{ident} | {path} | {what} | {result}\n"
        with open(OUT, "a") as f:
            f.write(line)
        print(line, end="", flush=True)
    left = status(tree)
    print("worktree after:", left or "clean")


if __name__ == "__main__":
    main()
