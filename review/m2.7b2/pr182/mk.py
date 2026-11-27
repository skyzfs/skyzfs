#!/usr/bin/env python3
"""Write plants.json: the plants of the review of PR 182 at a034d39.
Each plant is one or more exact (file, old, new) edits; old occurs once."""
import json, sys
CLI = "crates/skyzfs-agent/src/zfs/cli.rs"
RET = "crates/skyzfs-agent/src/engine/retain.rs"
PRG = "crates/skyzfs-agent/src/program.rs"
LIB = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]
N2 = ("        // (N2 of the review of #179).\n"
      "        if output.code == Some(0) || says(&output.stderr, \"could not find any snapshots to destroy\")\n"
      "        {\n"
      "            return Ok(());\n"
      "        }\n"
      "        match self.failed(\"zfs\", &args, &output, true, pool) {\n"
      "            ZfsError::NoSnapshot => Ok(()),\n"
      "            e => Err(e),\n")
ONE_NOTRUN = ("        // As `destroy -r`: killed at its limit, the ioctl may finish in\n"
              "        // its own time.\n"
              "        let output = self\n"
              "            .exec(\"zfs\", &args, &[], self.change_limit())\n"
              "            .map_err(|e| match e {\n"
              "                ZfsError::NotRun(why) => {\n"
              "                    ZfsError::NotRun(")
GUARD = "if !skyzfs_control::valid_name(t) || !under || dataset.contains('@') {"
SKIP = "                Retire::Ref if self.retention.silent || self.retention.stopped => {}\n"
SET = ("                match code {\n"
       "                    Code::NoAnswer => self.retention.silent = true,\n"
       "                    Code::Stopped => self.retention.stopped = true,\n"
       "                    _ => {}\n"
       "                }\n")
RESET = "        self.retention.stopped = false;\n"
LOOKSKIP = "                Some(Due::Destroy { look: true, .. }) if self.retention.silent => {}\n"
STEPSKIP = "                Some(_) if *steps >= STEPS_MAX => {}\n"
NOTE = ("                    noted = Some(*note);\n"
        "                    reopens = 0;\n"
        "                    since_cap = 0;\n"
        "                    quick = 0;\n")
p = []
def add(pid, what, edits, cmd=LIB):
    p.append({"id": pid, "what": what, "edits": edits, "cmd": cmd})
# N2 / T1: destroy_one
add("C1-other", "destroy_one: a destroy not run rewrapped as Other (the review's B-cli-one-other)",
    [[CLI, ONE_NOTRUN, ONE_NOTRUN.replace("ZfsError::NotRun(\n", "x").replace("                    ZfsError::NotRun(", "                    ZfsError::Other(")]])
add("C2-gone", "destroy_one: `dataset does not exist` (NoSnapshot) taken as an error (B-cli-one-gone)",
    [[CLI, N2, N2.replace("ZfsError::NoSnapshot => Ok(()),", "ZfsError::NoSnapshot => Err(ZfsError::NoSnapshot),")]])
add("C3-guard", "destroy_one: the guard's || made && (B-cli-one-guard)",
    [[CLI, GUARD, "if !skyzfs_control::valid_name(t) && !under && dataset.contains('@') {"]])
add("C4-notfound", "destroy_one: `could not find any snapshots` read as an error (N2 undone)",
    [[CLI, N2, N2.replace(" || says(&output.stderr, \"could not find any snapshots to destroy\")", "")]])
add("C5-anyok", "destroy_one: any failure read as destroyed",
    [[CLI, N2, N2.replace("            e => Err(e),\n", "            _ => Ok(()),\n")]])
add("C6-nounder", "destroy_one: no check that the dataset is under the pool",
    [[CLI, GUARD, "if !skyzfs_control::valid_name(t) || dataset.contains('@') {"]])
add("C7-noat", "destroy_one: no check of an `@` in the dataset",
    [[CLI, GUARD, "if !skyzfs_control::valid_name(t) || !under {"]])
add("C8-novalid", "destroy_one: no check that t is a snapshot's name",
    [[CLI, GUARD, "if !under || dataset.contains('@') {"]])
add("C9-notfound-any", "destroy_one: the not-found line read as destroyed only on exit 0 path dropped, any exit 1 with no stderr read as destroyed",
    [[CLI, N2, N2.replace("says(&output.stderr, \"could not find any snapshots to destroy\")", "output.code == Some(1)")]])
# N1
add("N1-a-unset", "a retire refused stopped does not set the flag",
    [[RET, SET, SET.replace("Code::Stopped => self.retention.stopped = true,", "Code::Stopped => {}")]])
add("N1-b-noreset", "the flag not cleared at the next round",
    [[RET, RESET, ""]])
add("N1-c-anyrefusal", "any refusal of a retire taken as stopped",
    [[RET, SET, SET.replace("                    _ => {}\n", "                    _ => self.retention.stopped = true,\n")]])
add("N1-d-assilent", "stopped taken as silent (ends the looks too)",
    [[RET, SET, SET.replace("Code::Stopped => self.retention.stopped = true,", "Code::Stopped => self.retention.silent = true,")]])
add("N1-e-skipignores", "the skip of the marked names ignores the flag",
    [[RET, SKIP, "                Retire::Ref if self.retention.silent => {}\n"]])
add("N1-f-members", "stopped also skips the members' destroys of names whose ref is retired",
    [[RET, SKIP, "                Retire::Ref if self.retention.silent => {}\n                _ if self.retention.stopped => {}\n"]])
add("N1-g-looks", "stopped also skips the give-ups' looks",
    [[RET, LOOKSKIP, "                Some(Due::Destroy { look: true, .. }) if self.retention.silent || self.retention.stopped => {}\n"]])
add("N1-h-giveups", "stopped also skips every step of the names given up",
    [[RET, STEPSKIP, STEPSKIP + "                Some(_) if self.retention.stopped => {}\n"]])
add("N1-i-stepcost", "a retire skipped for stopped still costs a step",
    [[RET, SKIP, "                Retire::Ref if self.retention.silent || self.retention.stopped => {\n                    steps += 1;\n                }\n"]])
add("N1-j-stoponly", "silent no longer skips the marked names, only stopped",
    [[RET, SKIP, "                Retire::Ref if self.retention.stopped => {}\n"]])
# The review's T2 to T4 of the next look, again
add("T2-silent-all", "after a retire not answered or refused stopped the members' steps stop too (B-silent-all)",
    [[RET, SKIP, "                _ if self.retention.silent || self.retention.stopped => {}\n"]])
add("T3-nr-turn", "a stuck name whose destroy was not run does not move the turn (B-nr-turn)",
    [[RET, "                Destroyed::Failed | Destroyed::NotRun => {\n                    self.retention.stuck_last = Some(name.clone())\n                }\n",
      "                Destroyed::Failed => {\n                    self.retention.stuck_last = Some(name.clone())\n                }\n                Destroyed::NotRun => {}\n"]])
add("T4-early-nofilter", "names already past their keep also counted as early (B-early-nofilter)",
    [[RET, "            .filter(|&i| {\n                self.record\n                    .abandoned\n                    .get(i)\n                    .is_some_and(|a| now.saturating_sub(a.at_ms) < bound)\n            })\n", ""]])
# G1, G3, G6, G12 and variants
add("G1", "cap: every Retry counted, not only a reopen's",
    [[PRG, "if verdict == Verdict::Retry && matches!(step, Some(Step::Reopened { .. })) {", "if verdict == Verdict::Retry {"]])
add("G1b", "cap: every reopen counted whatever its verdict",
    [[PRG, "if verdict == Verdict::Retry && matches!(step, Some(Step::Reopened { .. })) {", "if matches!(step, Some(Step::Reopened { .. })) {"]])
add("G3", "cap: only one more reopen after it",
    [[PRG, "since_cap.is_multiple_of(REOPEN_CAP)", "since_cap == REOPEN_CAP"]])
add("G3b", "cap: every round past the 10th after it reopens",
    [[PRG, "since_cap.is_multiple_of(REOPEN_CAP)", "since_cap >= REOPEN_CAP"]])
add("G6", "quick retries never renewed by another verdict",
    [[PRG, "        } else {\n            quick = 0;\n        }\n", "        }\n"]])
add("G12", "a new note does not start the counts again",
    [[PRG, NOTE, "                    noted = Some(*note);\n"]])
add("G12a", "a new note keeps the reopens only",
    [[PRG, NOTE, NOTE.replace("                    reopens = 0;\n", "")]])
add("G12b", "a new note keeps the rounds since the cap only",
    [[PRG, NOTE, NOTE.replace("                    since_cap = 0;\n", "")]])
add("G12c", "a new note keeps the quick retries only",
    [[PRG, NOTE, NOTE.replace("                    quick = 0;\n", "")]])
# --keep usage
add("K1", "the usage's N says N kept between rounds",
    [[PRG, "       so N+1 are kept between rounds\n", "       so N are kept between rounds\n"]])
json.dump(p, open(sys.argv[1], "w"), indent=1)
print(len(p), "plants")
