#!/usr/bin/env python3
"""Plants for the review of #114 at 6c97063 (M2.4b-2).

usage: plants.py CHECKOUT OUT [ID ...]

Each plant is one textual change to the checkout, made, tested and taken
back (try/finally). The result is appended to OUT as it comes: KILLED
with the tests that fail, SURVIVED, or NOT APPLIED. A plant whose id is
in OUT already is skipped, so the script can be started again after an
interruption. It refuses a checkout with changes.

Some plants also run one of the review's scenarios while the plant is in
place ("with"): the scenario's verdict is on the same line.
"""
import os
import re
import resource
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CHECKOUT, OUT = sys.argv[1], sys.argv[2]
ONLY = set(sys.argv[3:])
ENV = dict(os.environ, CARGO_INCREMENTAL="0")

UP = "crates/skyzfs-upload/src/"
CHECK = UP + "check.rs"
DRIVER = UP + "driver.rs"
LOCAL = UP + "local.rs"
REMOTE = UP + "remote.rs"
ROW = UP + "row.rs"
CRASH = "crates/skyzfs-upload/tests/crash.rs"
INPLACE = "crates/skyzfs-local/src/inplace.rs"

# The tests a plant is run against.
T_UPLOAD = ["-p", "skyzfs-upload", "--lib", "--tests"]
T_CRASH = ["-p", "skyzfs-upload", "--test", "crash"]
T_STORE = ["-p", "skyzfs-local", "-p", "skyzfs-upload", "--features", "skyzfs-local/test-hooks", "--lib", "--tests"]

# The scenarios run with a plant in place.
def scn(filter_):
    return ("scenarios.rs " + filter_, ["python3", os.path.join(HERE, "scenarios.py"), CHECKOUT, "{out}", filter_])

T1 = ("scenarios_store.rs T1", ["python3", os.path.join(HERE, "scenarios_store.py"), CHECKOUT, "{out}"])

PLANTS = [
    # --- the check ---------------------------------------------------
    ("U1", "a whole cut is uploaded whatever head's group", CHECK,
     "        return if head_is_the_rows {",
     "        return if head_is_the_rows || cut.whole {", T_UPLOAD, []),
    ("U2", "the row names the cut by its state and number alone (not its group)", CHECK,
     "    if named == Some(cut.row()) {",
     "    if named.map(|n| (n.state, n.number)) == Some((cut.state, cut.number)) {", T_UPLOAD, []),
    ("U3", "a delta is uploaded whatever cut the row names", CHECK,
     "        Some(named) if Some(named) != cut.base => Verdict::Mismatch(Mismatch::NotTheBase),\n",
     "", T_UPLOAD, []),
    ("U3b", "Q8 relaxed: a row of this state between the map's base and the cut is taken as the base", CHECK,
     "        Some(named) if Some(named) != cut.base => Verdict::Mismatch(Mismatch::NotTheBase),\n",
     "        Some(named)\n"
     "            if Some(named) != cut.base\n"
     "                && !(named.state == cut.state\n"
     "                    && named.number < cut.number\n"
     "                    && cut.base.is_some_and(|b| b.state != cut.state || b.number < named.number)) =>\n"
     "        {\n"
     "            Verdict::Mismatch(Mismatch::NotTheBase)\n"
     "        }\n", T_UPLOAD, []),
    ("U3c", "a delta is uploaded whatever head's group", CHECK,
     "        Some(_) if !head_is_the_rows => Verdict::Mismatch(Mismatch::HeadMoved),\n",
     "", T_UPLOAD, []),
    ("U3d", "a row that cannot be read is taken as no row", CHECK,
     "        Row::Unreadable(_) => return Verdict::Mismatch(Mismatch::Unreadable),\n        Row::Absent => None,",
     "        Row::Unreadable(_) | Row::Absent => None,", T_UPLOAD, []),
    # --- the row -----------------------------------------------------
    ("R1", "a row of another version is read as version 1", ROW,
     "    if version != ROW_VERSION {\n        return Err(RowError::Version(version));\n    }\n",
     "    let _ = version;\n", T_UPLOAD, []),
    ("R2", "a row of version 1 that is too long is read", ROW,
     "    if bytes.len() != ROW_LEN {\n        return Err(RowError::Length(bytes.len()));\n    }\n",
     "", T_UPLOAD, []),
    # --- the driver --------------------------------------------------
    ("U4", "no reset of the engine where the row names the cut after a failed commit", DRIVER,
     "            if Row::of(seen.row.as_deref()) == Row::Named(upload.cut.row()) {\n                self.remote.reset();\n",
     "            if Row::of(seen.row.as_deref()) == Row::Named(upload.cut.row()) {\n", T_UPLOAD, []),
    ("U5", "no reset of the engine where a commit is given up", DRIVER,
     "            if conflict || seen.row != upload.expected || !seen.current {\n                self.remote.reset();\n",
     "            if conflict || seen.row != upload.expected || !seen.current {\n", T_UPLOAD, []),
    ("U6", "a commit that conflicted is made again where the row and head read as before", DRIVER,
     "            if conflict || seen.row != upload.expected || !seen.current {",
     "            if seen.row != upload.expected || !seen.current {", T_UPLOAD, []),
    ("U7", "a failed commit is made again though head moved", DRIVER,
     "            if conflict || seen.row != upload.expected || !seen.current {",
     "            if conflict || seen.row != upload.expected {", T_UPLOAD, [scn("s6_")]),
    ("U7b", "a failed commit is made again though the row moved", DRIVER,
     "            if conflict || seen.row != upload.expected || !seen.current {",
     "            if conflict || !seen.current {", T_UPLOAD, []),
    ("U8", "only the last slice says whether a copy was read", DRIVER,
     "        upload.copied |= slice.copied;",
     "        upload.copied = slice.copied;", T_UPLOAD, []),
    ("U10", "a cut that went keeps its writes in the engine", DRIVER,
     "        self.remote.discard();\n        self.figures.lost += 1;",
     "        self.figures.lost += 1;", T_UPLOAD, []),
    ("U12", "a cut that went since its check is committed all the same", DRIVER,
     "        if held.map(|cut| cut.row()) != Some(upload.cut.row()) {",
     "        if held.map(|cut| cut.row()) != Some(upload.cut.row()) && false {", T_UPLOAD, []),
    ("U13", "a cut found committed and gone is not named", DRIVER,
     "            Released::Kept => self.kept(number),\n            Released::Gone => self.not_held(cut).await,",
     "            Released::Kept => self.kept(number),\n            Released::Gone => Step::FoundCommitted { number },", T_UPLOAD, []),
    ("U13b", "a cut uploaded and gone is not named", DRIVER,
     "            Released::Gone => self.not_held(&upload.cut).await,",
     "            Released::Gone => Step::UploadedNotHeld { number, named: false },", T_UPLOAD, []),
    ("U14", "a run of blocks is written as its first block is: zeros or data", DRIVER,
     " && zero(end) == zero(i) {",
     " {", T_UPLOAD, []),
    ("U15", "the commit expects no row", DRIVER,
     "                    expected: seen.row,",
     "                    expected: None,", T_UPLOAD, []),
    ("U17", "the driver goes on after a cut the store kept", DRIVER,
     "            Step::Kept { .. } => Stage::Stopped,",
     "            Step::Kept { .. } => Stage::Check,", T_UPLOAD, []),
    ("U18", "no PUT of sealed segments before a slice is read", DRIVER,
     "        if let Err(e) = self.remote.put_sealed().await {\n            return (Stage::Read(upload), Err(e));\n        }\n",
     "", T_UPLOAD, []),
    ("U19", "no PUT of sealed segments after a slice", DRIVER,
     "        let put = self.remote.put_sealed().await;",
     "        let put: Result<(), RemoteError> = Ok(());", T_UPLOAD, []),
    ("U20", "a cut with a copy read is committed without the sync of the copies", DRIVER,
     "            None if upload.copied => Stage::Sync(upload),\n",
     "", T_UPLOAD, []),
    ("U21", "a failed sync of the copies commits all the same", DRIVER,
     "            Err(e) => (Stage::Check, Ok(self.lost(number, e.to_string()))),\n        }\n    }\n\n    async fn commit(",
     "            Err(_) => (\n                Stage::Commit {\n                    upload,\n                    failed: None,\n                },\n                Ok(Step::CopiesSynced { number }),\n            ),\n        }\n    }\n\n    async fn commit(", T_UPLOAD, []),
    ("U5s", "as U5 (no reset where a commit is given up), with the scenario of a head moved after a failed commit", DRIVER,
     "            if conflict || seen.row != upload.expected || !seen.current {\n                self.remote.reset();\n",
     "            if conflict || seen.row != upload.expected || !seen.current {\n", T_UPLOAD, [scn("s6_")]),
    ("U18+19", "no PUT of sealed segments before the commit at all", DRIVER,
     ("        if let Err(e) = self.remote.put_sealed().await {\n            return (Stage::Read(upload), Err(e));\n        }\n",
      "        let put = self.remote.put_sealed().await;"),
     ("", "        let put: Result<(), RemoteError> = Ok(());"), T_UPLOAD, []),
    ("U24", "a commit that failed is made again without the row and head read again", DRIVER,
     "                        failed: Some(conflict),",
     "                        failed: None.or(Some(conflict)).filter(|_| false),", T_UPLOAD, []),
    # --- the store as the driver reaches it --------------------------
    ("U9", "a cut is offered whatever its phase (its flush under way)", LOCAL,
     "        if first.phase != CutPhase::Uploading {\n            return None;\n        }\n",
     "", T_UPLOAD, [scn("s7_")]),
    ("U31", "a cut the store kept is reported as let go", LOCAL,
     "        if self.held_cuts().iter().any(|cut| cut.number == number) {",
     "        if self.held_cuts().iter().any(|cut| cut.number == number) && false {", T_UPLOAD, []),
    ("U32", "a cut let go on a failed volume is reported as kept (the driver stops at any failure)", LOCAL,
     "        if self.held_cuts().iter().any(|cut| cut.number == number) {",
     "        if self.is_failed() || self.held_cuts().iter().any(|cut| cut.number == number) {", T_UPLOAD, [scn("s8_")]),
    # --- object storage as the driver reaches it ---------------------
    ("U26", "observe opens the engine again at a moved head though it holds writes", REMOTE,
     "            Some(engine) if !engine.is_clean() => {\n                return Ok(Observed {\n                    row,\n                    head_group: engine.head_group(),\n                    current: false,\n                });\n            }\n",
     "", T_UPLOAD, [scn("s6_")]),
    ("U27", "the commit carries no group", REMOTE,
     "        engine.set_group(Some(group));\n",
     "        let _ = group;\n", T_UPLOAD, []),
    ("U28", "the commit does not swap the row", REMOTE,
     "        engine.commit_with(CommitKind::Flush, ops).await?;",
     "        drop(ops);\n        engine.commit_with(CommitKind::Flush, Vec::new()).await?;", T_UPLOAD, []),
    ("U30", "observe keeps a clean engine though head moved", REMOTE,
     "            Some(engine) if at_head(engine) => {}",
     "            Some(engine) if at_head(engine) || engine.is_clean() => {}", T_UPLOAD, []),
    # --- the matrix (the test changed: does it notice a point skipped?)
    ("M1", "TEST: the object-storage matrix skips its first kill point", CRASH,
     "        for k in 0.. {",
     "        for k in 1.. {", T_CRASH, []),
    ("M1b", "TEST: the object-storage matrix stops one point early", CRASH,
     "            let seen = kill_at(Kill::Remote(when, k)).await;\n            if !seen.died {",
     "            let seen = kill_at(Kill::Remote(when, k)).await;\n            if !seen.died || k == 34 {", T_CRASH, []),
    ("M2", "TEST: the cut-state matrix skips its first kill point", CRASH,
     "    for n in 0.. {",
     "    for n in 1.. {", T_CRASH, []),
    ("M3", "TEST: the cut-state matrix stops after three kill points", CRASH,
     "        let seen = kill_at(Kill::Region(n)).await;\n        if !seen.died {",
     "        let seen = kill_at(Kill::Region(n)).await;\n        if !seen.died || n == 3 {", T_CRASH, []),
    ("M4", "release lets the cut go before the map names it", INPLACE,
     "        if !self.intent_committed(number, group) {\n"
     "            // The volume has failed, and the cut stays held: true all the\n"
     "            // same, since the map may name it from now on (see the doc\n"
     "            // for what finds it after).\n"
     "            return true;\n"
     "        }\n"
     "        self.state\n"
     "            .as_mut()\n"
     "            .is_some_and(|state| state.release(number))\n",
     "        let released = self\n"
     "            .state\n"
     "            .as_mut()\n"
     "            .is_some_and(|state| state.release(number));\n"
     "        if !self.intent_committed(number, group) {\n"
     "            return true;\n"
     "        }\n"
     "        released\n", T_CRASH, []),
    # --- the store: the naming ---------------------------------------
    ("L1", "naming a cut does not count as a commit of the map", INPLACE,
     "        intent.map.commit(named);\n        intent.commits += 1;\n",
     "        intent.map.commit(named);\n", T_STORE, [T1]),
    ("L2", "a release counts as a commit only where it moves the map's base (the alternative of question 3)", INPLACE,
     "        intent.map.commit(intent::Base {\n            state: id,\n            number,\n            group,\n        });\n        intent.commits += 1;\n",
     "        let base = intent::Base {\n            state: id,\n            number,\n            group,\n        };\n        if intent.map.base() != Some(base) {\n            intent.commits += 1;\n        }\n        intent.map.commit(base);\n", T_STORE, [T1]),
    ("L2b", "a release does not count as a commit of the map at all", INPLACE,
     "            group,\n        });\n        intent.commits += 1;\n        intent.map.clear(",
     "            group,\n        });\n        intent.map.clear(", T_STORE, [T1]),
    ("L3", "naming a cut clears every bit of the map", INPLACE,
     "        intent.map.commit(named);\n        intent.commits += 1;\n",
     "        intent.map.commit(named);\n        intent.commits += 1;\n        intent.map.clear(|_, _| false);\n", T_STORE, [scn("s5_")]),
    ("L3s", "as L3 (naming clears every bit), for what the random programs say of it", INPLACE,
     "        intent.map.commit(named);\n        intent.commits += 1;\n",
     "        intent.map.commit(named);\n        intent.commits += 1;\n        intent.map.clear(|_, _| false);\n", T_CRASH, [scn("s5_")]),
    ("L4", "a held cut can be named as committed", INPLACE,
     "        if held || number == 0 || number > state.count() {",
     "        if number == 0 || number > state.count() {\n            let _ = held;", T_STORE, []),
    ("L5", "naming moves the map's base back to an older cut of the state", INPLACE,
     "            Some(base) if base.state == id && base.number >= number => return false,\n",
     "", T_STORE, []),
    ("L6", "the state's newest cut cannot be named", INPLACE,
     "        if held || number == 0 || number > state.count() {",
     "        if held || number == 0 || number >= state.count() {", T_STORE, []),
    ("L6b", "a cut the state never made can be named", INPLACE,
     "        if held || number == 0 || number > state.count() {",
     "        if held || number == 0 {", T_STORE, []),
    ("L7", "a naming whose write of the map fails does not fail the volume", INPLACE,
     "        intent.map.commit(named);\n        intent.commits += 1;\n        if self.write_intent(false).is_err() {\n            self.fail();\n            self.settle_failed();\n            return false;\n        }\n        true\n",
     "        intent.map.commit(named);\n        intent.commits += 1;\n        let _ = self.write_intent(false);\n        true\n", T_STORE, [scn("s2_")]),
    ("L8", "a release whose write of the map fails says the cut was not the one (false)", INPLACE,
     "            // for what finds it after).\n            return true;",
     "            // for what finds it after).\n            return false;", T_STORE, []),
    ("L9", "naming does not write the map's pages", INPLACE,
     "        intent.map.commit(named);\n        intent.commits += 1;\n        if self.write_intent(false).is_err() {",
     "        intent.map.commit(named);\n        intent.commits += 1;\n        if false {", T_STORE, []),
]


def limit():
    # A plant can make the driver loop for ever while it gathers steps:
    # bound the memory of the tests, not of the build.
    resource.setrlimit(resource.RLIMIT_AS, (8 << 30, 8 << 30))


def run(cmd, timeout, bounded=False, env=ENV):
    proc = subprocess.Popen(cmd, cwd=CHECKOUT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, start_new_session=True, preexec_fn=limit if bounded else None)
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


def one(pid, what, path, old, new, tests, extras):
    full = os.path.join(CHECKOUT, path)
    text = open(full).read()
    olds = old if isinstance(old, tuple) else (old,)
    news = new if isinstance(new, tuple) else (new,)
    planted = text
    for o, n in zip(olds, news):
        if planted.count(o) != 1:
            return record(f"{pid}\tNOT APPLIED\t{what}\t(the text is there {planted.count(o)} times)")
        planted = planted.replace(o, n)
    detail = os.path.join(HERE, "plants_detail")
    os.makedirs(detail, exist_ok=True)
    try:
        open(full, "w").write(planted)
        code, out = run(["cargo", "test", "--no-run"] + tests, 2400)
        if code != 0:
            open(os.path.join(detail, pid + ".build"), "w").write(out[-8000:])
            return record(f"{pid}\tNOT APPLIED\t{what}\t(it does not build: plants_detail/{pid}.build)")
        code, out = run(["cargo", "test", "--no-fail-fast"] + tests, 1200, bounded=True)
        open(os.path.join(detail, pid + ".tests"), "w").write(out[-60000:])
        failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))
        if code is None:
            verdict = "KILLED\t" + what + "\tby a test that does not end (20 minutes)" + (
                "; and " + ", ".join(failed) if failed else "")
        elif code != 0:
            verdict = "KILLED\t" + what + f"\tby {len(failed)}: " + ", ".join(failed[:6]) + (" ..." if len(failed) > 6 else "")
            if not failed:
                verdict += "(no test named: see plants_detail/" + pid + ".tests)"
        else:
            verdict = "SURVIVED\t" + what + "\t"
        for name, cmd in extras:
            eout = os.path.join(detail, pid + ".with")
            if os.path.exists(eout):
                os.remove(eout)
            ecode, etext = run([c.replace("{out}", eout) for c in cmd], 2400, env=dict(ENV, ALLOW_DIRTY="1"))
            got = open(eout).read() if os.path.exists(eout) else etext
            ok = re.search(r"test result: ok", got) and not re.search(r"FAILED|panicked", got)
            verdict += f"\twith {name}: " + ("passes" if ok else "FAILS (plants_detail/" + pid + ".with)")
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
