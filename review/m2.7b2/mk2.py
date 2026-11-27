#!/usr/bin/env python3
"""Write plants2.json for the review of #179 at its second head
(d68471f, the names stuck) and check that each plant's old text occurs
exactly once in the tree given (a worktree of d68471f).
Usage: mk2.py TREE OUT.json

N1-N18 remake the author's eighteen as the description lists them;
M1-M8 are the review's own (the cap, the forget order, the v2 read, the
stuck retry, a failed members read); P1-P8 are plants of plants.json
(adc391a) whose code d68471f changed or that bear on it, run again."""
import json, os, sys

AG = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]
CLI = ["cargo", "test", "-p", "skyzfs-agent-cli", "--features", "skyzfs-agent-cli/test-hooks"]
RET = "crates/skyzfs-agent/src/engine/retain.rs"
REC = "crates/skyzfs-agent/src/record.rs"
PRG = "crates/skyzfs-agent/src/program.rs"

P = []


def plant(pid, what, cmd, *edits):
    P.append({"id": pid, "what": what, "cmd": cmd, "edits": [list(e) for e in edits]})


RETRY_OK = ("            if self.destroy_members(&name, &members)? {\n"
            "                self.told(name.clone(), Outcome::Retired);\n"
            "                self.commit(Effect::Retain, |r| r.stuck.retain(|s| s.name != name))?;\n"
            "            }")
STICK_PUB = ("            self.commit(Effect::Retain, |r| r.published.retain(|p| p.name != name))?;\n"
             "        } else {\n"
             "            self.stick(name, members)?;\n"
             "        }")
STICK_GIVEN = ("                        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))?;\n"
               "                    } else {\n"
               "                        self.stick(&name, members)?;\n"
               "                    }")
TOLD_OLD = ("        for old in &forgotten {\n"
            "            self.told(old.clone(), Outcome::Forgotten(STUCK_FORGOTTEN.to_string()));\n"
            "        }\n")

# --- The author's eighteen, remade.
plant("N1", "no retry of the names stuck", AG,
      (RET, "        self.retry_stuck()?;\n        self.mark_retiring()?;", "        self.mark_retiring()?;"))
plant("N2", "no cap: none forgotten past STUCK_MAX", AG,
      (RET, "let over = (self.record.stuck.len() + 1).saturating_sub(STUCK_MAX);", "let over = 0;"))
plant("N3", "the newest stuck forgotten, not the oldest", AG,
      (RET, "            .stuck\n            .iter()\n            .take(over)", "            .stuck\n            .iter()\n            .rev()\n            .take(over)"))
plant("N4", "the oldest forgotten silently", AG,
      (RET, TOLD_OLD, ""))
plant("N5", "the forgotten line told after the save", AG,
      (RET, TOLD_OLD, ""),
      (RET, "                at_ms,\n            });\n        })\n    }",
            "                at_ms,\n            });\n        })?;\n"
            "        for old in &forgotten {\n"
            "            self.told(old.clone(), Outcome::Forgotten(STUCK_FORGOTTEN.to_string()));\n"
            "        }\n        Ok(())\n    }"))
plant("N6", "a name stuck left among the published too", AG,
      (RET, "            r.published.retain(|p| p.name != name);\n            r.abandoned.retain(|a| a.name != name);",
            "            r.abandoned.retain(|a| a.name != name);"))
plant("N7", "a name stuck left among the given up too", AG,
      (RET, "            r.published.retain(|p| p.name != name);\n            r.abandoned.retain(|a| a.name != name);",
            "            r.published.retain(|p| p.name != name);"))
plant("N8", "no stick for a name published", AG,
      (RET, STICK_PUB, "            self.commit(Effect::Retain, |r| r.published.retain(|p| p.name != name))?;\n        }"))
plant("N9", "no stick for a name given up", AG,
      (RET, STICK_GIVEN, "                        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))?;\n                    }"))
plant("N10", "a retry that succeeds stays stuck", AG,
      (RET, RETRY_OK, "            if self.destroy_members(&name, &members)? {\n"
                      "                self.told(name.clone(), Outcome::Retired);\n"
                      "            }"))
plant("N11", "a retry that succeeds tells nothing", AG,
      (RET, RETRY_OK, "            if self.destroy_members(&name, &members)? {\n"
                      "                self.commit(Effect::Retain, |r| r.stuck.retain(|s| s.name != name))?;\n"
                      "            }"))
plant("N12", "a retry that succeeds tells after the save", AG,
      (RET, RETRY_OK, "            if self.destroy_members(&name, &members)? {\n"
                      "                self.commit(Effect::Retain, |r| r.stuck.retain(|s| s.name != name))?;\n"
                      "                self.told(name.clone(), Outcome::Retired);\n"
                      "            }"))
plant("N13", "the names stuck counted for room", AG,
      (REC, "            && self.published.len() + ending <= PUBLISHED_MAX\n    }",
            "            && self.published.len() + self.stuck.len() + ending <= PUBLISHED_MAX\n    }"))
plant("N14", "names() does not find a name stuck", AG,
      (REC, "            || self.stuck.iter().any(|s| s.name == name)\n", ""))
plant("N15", "rule unchecked: more than STUCK_MAX", AG,
      (REC, "        if self.stuck.len() > STUCK_MAX {", "        if false && self.stuck.len() > STUCK_MAX {"))
plant("N16", "rule unchecked: a name stuck twice", AG,
      (REC, "            if self.stuck[..i].iter().any(|s| s.name == *name) {", "            if false && self.stuck[..i].iter().any(|s| s.name == *name) {"))
plant("N17", "rule unchecked: stuck and kept otherwise too", AG,
      (REC, "            if self.publish(name).is_some()\n                || self.abandoned.iter().any(|a| a.name == *name)\n                || self.published.iter().any(|p| p.name == *name)\n            {\n                return rule(format!(\"`{name}` is stuck and kept otherwise too\"));",
            "            if false\n            {\n                return rule(format!(\"`{name}` is stuck and kept otherwise too\"));"))
plant("N18", "rule unchecked: stuck with no members", AG,
      (REC, "            if stuck.members.is_empty() {", "            if false && stuck.members.is_empty() {"))

# --- The review's own.
plant("M1", "the cap one high (65 kept)", AG,
      (RET, "let over = (self.record.stuck.len() + 1).saturating_sub(STUCK_MAX);",
            "let over = self.record.stuck.len().saturating_sub(STUCK_MAX);"))
plant("M2", "the cap one low (63 kept)", AG,
      (RET, "let over = (self.record.stuck.len() + 1).saturating_sub(STUCK_MAX);",
            "let over = (self.record.stuck.len() + 2).saturating_sub(STUCK_MAX);"))
plant("M3", "the retry within the round's steps (none once they are spent, and it spends them)", AG,
      (RET, "        self.retry_stuck()?;\n        self.mark_retiring()?;\n        let mut steps = 0;",
            "        self.mark_retiring()?;\n        let mut steps = 0;"),
      (RET, "        self.retain_given_up(&mut steps)?;\n",
            "        self.retain_given_up(&mut steps)?;\n        if steps < STEPS_MAX {\n            self.retry_stuck()?;\n        }\n"))
plant("M4", "only the oldest name stuck retried each round", AG,
      (RET, "        for (name, members) in stuck {\n            self.at.clone_from(&name);\n            if self.destroy_members(&name, &members)? {",
            "        for (name, members) in stuck.into_iter().take(1) {\n            self.at.clone_from(&name);\n            if self.destroy_members(&name, &members)? {"))
plant("M5", "a retry that succeeds removes every name stuck", AG,
      (RET, "                self.commit(Effect::Retain, |r| r.stuck.retain(|s| s.name != name))?;\n            }\n        }\n        Ok(())",
            "                self.commit(Effect::Retain, |r| r.stuck.clear())?;\n            }\n        }\n        Ok(())"))
plant("M6", "v2: a record of version 2 refused", AG,
      (REC, "        if version != VERSION && version != VERSION_2 {", "        if version != VERSION {"))
plant("M7", "the record keeps the members of the first only (stuck with one member)", AG,
      (RET, "            r.stuck.push(Stuck {\n                name,\n                members,",
            "            r.stuck.push(Stuck {\n                name,\n                members: members.into_iter().take(1).collect(),"))
plant("M8", "the dump leaves the names stuck out", AG,
      (PRG, "            \"stuck {} members={} at_ms={}\\n\",", "            \"\","))
# M8 does not compile (format! with arguments left over); M8b is its compiling form.
plant("M8b", "the dump leaves the names stuck out (compiling form of M8)", AG,
      (PRG, "    for s in &record.stuck {", "    for s in record.stuck.iter().take(0) {"))
plant("M9", "the oldest stuck forgotten from the newest end of the list (stuck pushed to the front)", AG,
      (RET, "            r.stuck.push(Stuck {\n                name,", "            r.stuck.insert(0, Stuck {\n                name,"))

# --- plants.json (adc391a) again, where d68471f changed the code or the ruling bears on it.
plant("P1", "R11 again: a destroy that fails does not stop the name leaving (no stick)", AG,
      (RET, "            if let Err(e) = destroyed {\n                self.say(\n                    name,\n                    Report::NotRetired(format!(\"zfs destroy {}@{name}: {e}\", member.name)),\n                );\n                return Ok(false);",
            "            if let Err(e) = destroyed {\n                self.say(\n                    name,\n                    Report::NotRetired(format!(\"zfs destroy {}@{name}: {e}\", member.name)),\n                );\n                return Ok(true);"))
plant("P2", "R12 again: a members read that fails lets the name leave", AG,
      (RET, "Report::NotRetired(format!(\"the members' @{name} not read: {e}\")),\n                );\n                return Ok(false);",
            "Report::NotRetired(format!(\"the members' @{name} not read: {e}\")),\n                );\n                return Ok(true);"))
plant("P3", "R18 again: a report told at each round (the dedupe off)", AG,
      (RET, "        if self.retention.told.get(name) == Some(&key) {\n            return;\n        }", ""))
plant("P4", "C1 again: room counts the names given up with those under way wrongly (no ending)", AG,
      (REC, "            && self.abandoned.len() + ending <= ABANDONED_MAX", "            && self.abandoned.len() < ABANDONED_MAX"))

tree, out = sys.argv[1], sys.argv[2]
bad = 0
for p in P:
    texts = {}
    for f, old, new in p["edits"]:
        t = texts.setdefault(f, open(os.path.join(tree, f)).read())
        n = t.count(old)
        if n != 1:
            print(f"{p['id']}: {f}: {n} matches", file=sys.stderr)
            bad += 1
        texts[f] = t.replace(old, new)
json.dump(P, open(out, "w"), indent=1)
print(f"{len(P)} plants, {bad} bad")
sys.exit(1 if bad else 0)
