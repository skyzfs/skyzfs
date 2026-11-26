#!/usr/bin/env python3
"""Look at #157 (M2.6c-5), head e35f9e9: plants on a scratch copy of the
head, each applied alone as text (one or more exact replacements, each
site found exactly once), then the tests, then restored in a finally.
One JSON line per plant: KILLED (the tests that failed, named; or the
build failed; or the 600 s limit), SURVIVED, or an error.

R*: the manifest's read (zfs/cli.rs: match_values, value_line,
output_lines, the value read alone, datasets). The line `oracle` says
whether `the_values_matched_are_the_values_set` was among the failures.
Q*, N*, U*: the items carried from #153 (T8, T11, T12, T13, N13) and the
Unreadable report. S*: the socket's path.

usage: python3 -I plants157.py WORKTREE [ID,ID...]
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
AG = "crates/skyzfs-agent/src"
ENG = f"{AG}/engine.rs"
ENGT = f"{AG}/engine/tests.rs"
CLI = f"{AG}/zfs/cli.rs"
PROG = f"{AG}/program.rs"
CTL = "crates/skyzfs-control/src/lib.rs"
MAIN = "bins/skyzfs-agent/src/main.rs"

ORACLE = "the_values_matched_are_the_values_set"

P = []


def plant(pid, what, *files):
    P.append((pid, what, list(files)))


# --- the read -------------------------------------------------------------
plant("R1", "a pair named twice not suspect", (CLI, [
    ("                    || named.get(&pair).copied().unwrap_or(0) != 1\n", ""),
]))
plant("R2", "a pair not listed not suspect", (CLI, [
    ("                !listed.contains(&pair)\n", "                false\n"),
]))
plant("R3", "a line that does not split not suspect", (CLI, [
    ("            None => true,\n", "            None => false,\n"),
]))
plant("R4", "no walk back to the line before a suspect one", (CLI, [
    ("        if let Some((name, _, _)) = lines[..i].iter().rev().flatten().next() {\n"
     "            again.insert(name.clone());\n        }\n", ""),
]))
plant("R5", "walk back to the line just before only (expected equivalent)", (CLI, [
    ("lines[..i].iter().rev().flatten().next()",
     "lines[..i].last().and_then(|l| l.as_ref())"),
]))
plant("R6", "a pair listed and named by no line not read again", (CLI, [
    ("        if !named.contains_key(&(name.as_str(), property.as_str())) {\n",
     "        if false && !named.contains_key(&(name.as_str(), property.as_str())) {\n"),
]))
plant("R7", "the order not compared", (CLI, [
    ("    loop {\n        let read_left = lines", "    while false {\n        let read_left = lines"),
]))
plant("R8", "at the first difference only the line's dataset read again", (CLI, [
    ("[a, b].into_iter().flatten()", "[a.or(b)].into_iter().flatten()"),
]))
plant("R8b", "at the first difference only the pair's dataset read again", (CLI, [
    ("[a, b].into_iter().flatten()", "[b.or(a)].into_iter().flatten()"),
]))
plant("R9", "the order compared by dataset only", (CLI, [
    ("                (a, b) if a == b => continue,\n",
     "                (a, b) if a.map(|x| x.0) == b.map(|x| x.0) => continue,\n"),
]))
plant("R10", "a line longer than a value can be not suspect", (CLI, [
    ("                    || value.len() > VALUE_MOST\n", ""),
]))
plant("R11", "a line split at every tab", (CLI, [
    ("    let mut parts = line.splitn(3, |&b| b == b'\\t');\n",
     "    let mut parts = line.split(|&b| b == b'\\t');\n"),
]))
plant("R12", "the value made UTF-8", (CLI, [
    ("    Some((name.to_string(), property.to_string(), value.to_vec()))\n",
     "    Some((name.to_string(), property.to_string(), String::from_utf8_lossy(value).into_owned().into_bytes()))\n"),
]))
plant("R13", "every empty line at the end dropped", (CLI, [
    ("    if lines.last().is_some_and(|line| line.is_empty()) {\n",
     "    while lines.last().is_some_and(|line| line.is_empty()) {\n"),
]))
plant("R14", "the empty line after the last newline kept", (CLI, [
    ("    if lines.last().is_some_and(|line| line.is_empty()) {\n",
     "    if false && lines.last().is_some_and(|line| line.is_empty()) {\n"),
]))
plant("R15", "values of the datasets read again taken from the one pass too", (CLI, [
    ("            if !suspect && !again.contains(&name) {\n", "            if !suspect {\n"),
]))
plant("R16", "a value read alone: every newline at its end taken off", (CLI, [
    ("        match out.strip_suffix(b\"\\n\") {\n",
     "        match Some(&out[..out.len() - out.iter().rev().take_while(|&&b| b == b'\\n').count()]).filter(|_| out.ends_with(b\"\\n\")) {\n"),
]))
plant("R17", "a value read alone: no newline required", (CLI, [
    ("        match out.strip_suffix(b\"\\n\") {\n",
     "        match Some(out.strip_suffix(b\"\\n\").unwrap_or(&out)) {\n"),
]))
plant("R18", "a value read alone: not bounded", (CLI, [
    ("            Some(value) if value.len() <= VALUE_MOST => Ok(value.to_vec()),\n",
     "            Some(value) => Ok(value.to_vec()),\n"),
]))
plant("R19", "a value read alone: bounded one short", (CLI, [
    ("            Some(value) if value.len() <= VALUE_MOST => Ok(value.to_vec()),\n",
     "            Some(value) if value.len() < VALUE_MOST => Ok(value.to_vec()),\n"),
]))
plant("R20", "the assist property kept", (CLI, [
    ("            if property == ASSIST_PROPERTY {\n", "            if false && property == ASSIST_PROPERTY {\n"),
]))
plant("R21", "received properties read too (-s local,received)", (CLI, [
    ("        \"local\",\n", "        \"local,received\",\n"),
]))
plant("R22", "VALUE_MOST one more (8192)", (CLI, [
    ("pub const VALUE_MOST: usize = 8191;", "pub const VALUE_MOST: usize = 8192;"),
]))
plant("R23", "a line of exactly VALUE_MOST bytes suspect (>=; conservative)", (CLI, [
    ("                    || value.len() > VALUE_MOST\n", "                    || value.len() >= VALUE_MOST\n"),
]))

# --- the items from #153 ----------------------------------------------------
plant("Q4", "no report at the give-up without a take", (ENG, [
    ("            self.tell(name, Report::NotWhole(Vec::new()));\n", ""),
]))
plant("Q7", "the last check's report after the give-up's record (with a take)", (ENG, [
    ("        self.tell(name, Report::NotWhole(names));\n        self.point(Effect::Destroy, false)?;\n",
     "        self.point(Effect::Destroy, false)?;\n"),
    ("        self.abandon(name, why)?;\n        Ok(Moved::Yes)\n    }\n\n    /// Destroy the take of",
     "        self.abandon(name, why)?;\n        self.tell(name, Report::NotWhole(names));\n"
     "        Ok(Moved::Yes)\n    }\n\n    /// Destroy the take of"),
]))
plant("Q7b", "the report of the give-up without a take after its record", (ENG, [
    ("            self.tell(name, Report::NotWhole(Vec::new()));\n            self.abandon(name, Why::NotWhole)?;\n",
     "            self.abandon(name, Why::NotWhole)?;\n            self.tell(name, Report::NotWhole(Vec::new()));\n"),
]))
plant("Q13", "the CLI's word for NotWhole is person", (PROG, [
    ('        Why::NotWhole => "not-whole",', '        Why::NotWhole => "person",'),
]))
plant("Q14", "no cap on the names", (PROG, [
    ("        .take(NOT_WHOLE_NAMED)\n", ""),
]))
plant("Q15", "the line with no names says datasets: and nothing", (PROG, [
    ('        return format!("not-whole: {given_up}; the datasets are not known");',
     '        return format!("not-whole: {given_up}; datasets: ");'),
]))
plant("Q16", "the give-up without a take not told to the sink", (ENG, [
    ("            self.tell(name, Report::NotWhole(Vec::new()));\n",
     "            self.reports.push((name.to_string(), Report::NotWhole(Vec::new())));\n"),
]))
plant("Q18", "Failed left out of Effect::ALL", (ENG, [
    ("    pub const ALL: [Effect; 28] = [", "    pub const ALL: [Effect; 27] = ["),
    ("        Effect::Take,\n        Effect::Failed,\n        Effect::Destroy,\n        Effect::Manifest,",
     "        Effect::Take,\n        Effect::Destroy,\n        Effect::Manifest,"),
]))
plant("Q19", "a new effect appended to the enum, named and placed in the test, not in ALL", (ENG, [
    ("    Recover,\n}\n\nimpl Effect {", "    Recover,\n    /// A plant.\n    Probe,\n}\n\nimpl Effect {"),
    ('            Effect::Recover => "recover",\n', '            Effect::Recover => "recover",\n            Effect::Probe => "probe",\n'),
]), (ENGT, [
    ("            Effect::Recover => 27,\n", "            Effect::Recover => 27,\n            Effect::Probe => 28,\n"),
]))
plant("N1", "the report of a take not as before names nothing", (ENG, [
    ("                let differ = differing(&program.members, before);",
     "                let differ: Vec<String> = Vec::new();"),
]))
plant("N2", "differing(): datasets read before and missing from the take not named", (ENG, [
    ("    names.extend(\n", "    names.truncate(names.len());\n    let _ = (\n"),
]))
plant("N3", "taken(): only the first dataset that breaks the take named", (f"{AG}/zfs.rs", [
    ("    if !breaking.is_empty() {\n", "    breaking.truncate(1);\n    if !breaking.is_empty() {\n"),
]))
plant("N13", "names joined by a space", (PROG, [
    ('        .join(", ");', '        .join(" ");'),
]))
plant("U1", "no Unreadable report before the take", (ENG, [
    ("        let before = match self.zfs.datasets(&self.pool, name) {\n            Ok(before) => before,\n"
     "            Err(e) => {\n                self.tell(name, Report::Unreadable(e.to_string()));\n",
     "        let before = match self.zfs.datasets(&self.pool, name) {\n            Ok(before) => before,\n"
     "            Err(e) => {\n                let _ = e;\n"),
]))
plant("U2", "no Unreadable report after the take", (ENG, [
    ("            Ok(datasets) => datasets,\n            Err(e) => {\n                self.tell(name, Report::Unreadable(e.to_string()));\n",
     "            Ok(datasets) => datasets,\n            Err(e) => {\n                let _ = e;\n"),
]))
plant("U3", "the error not carried in the report after the take", (ENG, [
    ("            Ok(datasets) => datasets,\n            Err(e) => {\n                self.tell(name, Report::Unreadable(e.to_string()));\n",
     "            Ok(datasets) => datasets,\n            Err(e) => {\n                let _ = e;\n                self.tell(name, Report::Unreadable(String::new()));\n"),
]))
plant("U4", "the publish given up when the read before the take fails", (ENG, [
    ("        let before = match self.zfs.datasets(&self.pool, name) {\n            Ok(before) => before,\n"
     "            Err(e) => {\n                self.tell(name, Report::Unreadable(e.to_string()));\n                return Ok(Moved::No);\n",
     "        let before = match self.zfs.datasets(&self.pool, name) {\n            Ok(before) => before,\n"
     "            Err(e) => {\n                self.tell(name, Report::Unreadable(e.to_string()));\n"
     "                self.abandon(name, Why::TooOld)?;\n                return Ok(Moved::Yes);\n"),
]))
plant("U5", "the report line keeps the error's line breaks", (PROG, [
    ("            one_line(why)\n", "            why\n"),
]))

# --- the socket -------------------------------------------------------------
plant("S1", "the agent's main opens RUN/control (the old path)", (MAIN, [
    ("    let socket = match skyzfs_control::socket_path(&run.run, &run.volume) {\n",
     "    let socket = match Ok::<_, std::io::Error>(run.run.join(skyzfs_control::SOCKET)) {\n"),
]))
plant("S2", "volume_run_dir does not follow a symbolic link", (CTL, [
    ("    let meta = std::fs::metadata(volume_dir)?;\n", "    let meta = std::fs::symlink_metadata(volume_dir)?;\n"),
]))
plant("S3", "volume_run_dir in decimal", (CTL, [
    ('    Ok(run_dir.join(format!("v-{:x}-{:x}", meta.dev(), meta.ino())))\n',
     '    Ok(run_dir.join(format!("v-{}-{}", meta.dev(), meta.ino())))\n'),
]))
plant("S4", "socket_path joins another name than SOCKET", (CTL, [
    ("    Ok(volume_run_dir(run_dir, volume_dir)?.join(SOCKET))\n",
     "    Ok(volume_run_dir(run_dir, volume_dir)?.join(\"control.sock\"))\n"),
]))

AGENT = ["cargo", "test", "-p", "skyzfs-agent", "-p", "skyzfs-agent-cli",
         "--features", "skyzfs-agent-cli/test-hooks", "--no-fail-fast",
         "--", "--skip", "review_", "--test-threads=4"]
SOCK = ["cargo", "test", "-p", "skyzfs-control", "-p", "skyzfs-ublk", "-p", "skyzfs-agent",
        "-p", "skyzfs-agent-cli", "-p", "skyzfsd", "--features", "skyzfs-agent-cli/test-hooks",
        "--no-fail-fast", "--", "--skip", "review_", "--test-threads=4"]


def run(p):
    pid, what, files = p
    texts = {}
    try:
        for rel, subs in files:
            path = ROOT / rel
            if rel not in texts:
                texts[rel] = path.read_text()
            new = path.read_text()
            for old, rep in subs:
                if new.count(old) != 1:
                    return {"id": pid, "what": what, "result": f"ERROR: site in {rel} found {new.count(old)} times"}
                new = new.replace(old, rep)
            path.write_text(new)
        cmd = SOCK if pid.startswith("S") else AGENT
        try:
            out = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            return {"id": pid, "what": what, "result": "KILLED (timeout 600 s)"}
        lines = (out.stdout + out.stderr).splitlines()
        failed = [l.split()[1] for l in lines if l.startswith("test ") and l.endswith("FAILED")]
        if "could not compile" in out.stderr:
            errs = [l for l in out.stderr.splitlines() if l.startswith("error")][:3]
            return {"id": pid, "what": what, "result": "KILLED (build)", "by": errs}
        if out.returncode != 0:
            return {"id": pid, "what": what, "result": "KILLED", "oracle": any(ORACLE in f for f in failed), "by": failed}
        passed = [l for l in lines if l.startswith("test result")]
        return {"id": pid, "what": what, "result": "SURVIVED", "tests": passed}
    finally:
        for rel, text in texts.items():
            (ROOT / rel).write_text(text)


def main():
    only = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else None
    for p in P:
        if only and p[0] not in only:
            continue
        print(json.dumps(run(p)), flush=True)
    st = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True)
    print(json.dumps({"worktree after": st.stdout.strip() or "clean"}))


main()
