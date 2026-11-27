#!/usr/bin/env python3
"""Write plants.json for the review of #179 (M2.7b-2) and check that each
plant's old text occurs exactly once in the tree given (a worktree of the
head). Usage: mk.py TREE OUT.json"""
import json, os, sys

AG = ["cargo", "test", "-p", "skyzfs-agent", "--lib"]
CLI = ["cargo", "test", "-p", "skyzfs-agent-cli", "--features", "skyzfs-agent-cli/test-hooks"]
D = ["cargo", "test", "-p", "skyzfsd", "--features", "test-hooks", "--lib"]
ST = ["cargo", "test", "-p", "skyzfs-ublk", "--lib"]
UP = ["cargo", "test", "-p", "skyzfs-upload"]

RET = "crates/skyzfs-agent/src/engine/retain.rs"
REC = "crates/skyzfs-agent/src/record.rs"
ENG = "crates/skyzfs-agent/src/engine.rs"
PLAN = "crates/skyzfs-agent/src/plan.rs"
PRG = "crates/skyzfs-agent/src/program.rs"
LOST = "crates/skyzfs-agent/src/engine/lost.rs"
FAKE = "crates/skyzfs-agent/src/engine/fake.rs"
CTL = "bins/skyzfsd/src/control.rs"
DREC = "bins/skyzfsd/src/record.rs"
HND = "bins/skyzfsd/src/control/handler.rs"
DUP = "bins/skyzfsd/src/upload.rs"
STAT = "crates/skyzfs-ublk/src/status.rs"
PUB = "crates/skyzfs-upload/src/publish.rs"
DRV = "crates/skyzfs-upload/src/driver.rs"
REM = "crates/skyzfs-upload/src/remote.rs"
MAIN = "bins/skyzfs-agent/src/main.rs"

P = []


def plant(pid, what, cmd, *edits):
    P.append({"id": pid, "what": what, "cmd": cmd, "edits": [list(e) for e in edits]})


# --- retain.rs: the step cap, RETIRING_MAX, the order of the saves.
plant("R1", "no RETIRING_MAX bound on the marks", AG,
      (RET, "let n = over.min(RETIRING_MAX.saturating_sub(retiring));", "let n = over;"))
plant("R2", "the newest past keep marked instead of the oldest", AG,
      (RET, "            .filter(|p| p.retire.is_none())\n            .take(n)",
       "            .filter(|p| p.retire.is_none())\n            .skip(kept - n)\n            .take(n)"))
plant("R3", "one more kept than --keep", AG,
      (RET, "let over = kept.saturating_sub(self.retention.keep);",
       "let over = kept.saturating_sub(self.retention.keep + 1);"))
plant("R4", "STEPS_MAX + 1 steps a round", AG,
      (RET, "_ if steps >= STEPS_MAX => {}", "_ if steps > STEPS_MAX => {}"))
plant("R5", "gone not taken as done", AG,
      (RET, "Ok(Reply::Retired | Reply::Gone) => {", "Ok(Reply::Retired) => {"))
plant("R6", "refused other destroys the members", AG,
      (RET, "self.commit(Effect::Retired, |r| set_retire(r, name, Retire::Other))?;\n                self.say_other(name, Some(&text));\n                Ok(())",
       "let _ = &text;\n                self.commit(Effect::Retired, |r| set_retire(r, name, Retire::Members))?;\n                self.retire_members(name)"))
plant("R8", "retired told after the name leaves the record", AG,
      (RET, "            self.told(name.to_string(), Outcome::Retired);\n            self.commit(Effect::Retain, |r| r.published.retain(|p| p.name != name))?;",
       "            self.commit(Effect::Retain, |r| r.published.retain(|p| p.name != name))?;\n            self.told(name.to_string(), Outcome::Retired);"))
plant("R9", "members destroyed by name alone (no GUID filter)", AG,
      (RET, ".filter(|m| ours.contains(&m.snapshot_guid))", ".filter(|m| !ours.is_empty() || m.snapshot_guid > 0)"))
plant("R10", "pool@t destroyed first", AG,
      (RET, "(Reverse(depth(a)), &b.name).cmp(&(Reverse(depth(b)), &a.name))",
       "(depth(a), &a.name).cmp(&(depth(b), &b.name)).then(Reverse(0).cmp(&Reverse(0)))"))
plant("R11", "a destroy that fails does not stop the name leaving", AG,
      (RET, "                return Ok(false);\n            }\n        }\n        Ok(true)",
       "                continue;\n            }\n        }\n        Ok(true)"))
plant("R12", "a members read that fails lets the name leave", AG,
      (RET, "Report::NotRetired(format!(\"the members' @{name} not read: {e}\")),\n                );\n                return Ok(false);",
       "Report::NotRetired(format!(\"the members' @{name} not read: {e}\")),\n                );\n                return Ok(true);"))
plant("R13", "a name given up kept one ms past its bound", AG,
      (RET, "let past = now.saturating_sub(a.at_ms) >= bound;", "let past = now.saturating_sub(a.at_ms) > bound;"))
plant("R14", "a person's give-up not looked up", AG,
      (RET, "look: a.why == Why::Person,", "look: a.why == Why::Person && false,"))
plant("R15", "a look answered unreadable taken as absent", AG,
      (RET, "Ok(Reply::Absent) => Ok(true),", "Ok(Reply::Absent | Reply::Unreadable(_)) => Ok(true),"),
      (RET, "Ok(Reply::Snap(_) | Reply::Unreadable(_)) => {", "Ok(Reply::Snap(_)) => {"))
plant("R16", "a look not answered taken as absent", AG,
      (RET, "self.say(name, Report::NotRetired(format!(\"look not answered: {e}\")));\n                Ok(false)",
       "self.say(name, Report::NotRetired(format!(\"look not answered: {e}\")));\n                Ok(true)"))
plant("R16b", "a look answered otherwise (a refusal) taken as absent", AG,
      (RET, "Report::NotRetired(format!(\"look answered `{}`\", other.line())),\n                );\n                Ok(false)",
       "Report::NotRetired(format!(\"look answered `{}`\", other.line())),\n                );\n                Ok(true)"))
plant("R17", "no GUIDs never forgotten", AG,
      (RET, "                Some(Due::Forget) => {\n                    *steps += 1;\n                    self.forget_given_up(&name, \"no member guids; destroy by hand\")?;\n                }",
       "                Some(Due::Forget) => {\n                    *steps += 1;\n                }"))
plant("R18", "a report told at each round", AG,
      (RET, "        if self.retention.told.get(name) == Some(&key) {\n            return;\n        }\n", ""))
plant("R19", "a stop `other` told at each round", AG,
      (RET, "        if self.retention.told.get(name).map(String::as_str) == Some(KEY) {\n            return;\n        }\n", ""))
plant("R20", "names given up take no step", AG,
      (RET, "                    *steps += 1;\n                    if look", "                    if look"))
plant("R21", "a name given up: retired told after it leaves", AG,
      (RET, "                        self.told(name.clone(), Outcome::Retired);\n                        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))?;",
       "                        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))?;\n                        self.told(name.clone(), Outcome::Retired);"))
plant("R22", "forgotten told after the name leaves", AG,
      (RET, "        self.told(name.to_string(), Outcome::Forgotten(why.to_string()));\n        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))",
       "        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))?;\n        self.told(name.to_string(), Outcome::Forgotten(why.to_string()));\n        Ok(())"))
plant("R23", "names that left nothing never leave", AG,
      (RET, "        if !dropped.is_empty() {", "        if false && !dropped.is_empty() {"))
plant("R24", "set_keep unclamped and keep 0 allowed by retention", AG,
      (RET, "self.retention.keep = keep.min(KEEP_MAX);", "self.retention.keep = keep.min(KEEP_MAX).saturating_sub(1);"))

# --- record.rs: room, the rules, version 2.
plant("C1", "room: the names given up not counted with the ones under way", AG,
      (REC, "&& self.abandoned.len() + ending <= ABANDONED_MAX", "&& self.abandoned.len() <= ABANDONED_MAX"))
plant("C2", "room: no bound on the names published", AG,
      (REC, "&& self.published.len() + ending <= PUBLISHED_MAX", "&& true"))
plant("C3", "check: no bound on the names being retired", AG,
      (REC, "if retiring > RETIRING_MAX {", "if false && retiring > RETIRING_MAX {"))
plant("C5", "v2: a phase of v3 accepted", AG,
      (REC, "if version == VERSION_2\n            && record", "if false\n            && version == VERSION_2\n            && record"))
plant("C6", "a name published starts marked", AG,
      (REC, "            at_ms,\n            retire: None,\n        };", "            at_ms,\n            retire: Some(Retire::Ref),\n        };"))
plant("C7", "the names given up dropped past 64 (v2's rule)", AG,
      (REC, "            at_ms,\n            left,\n        });\n    }",
       "            at_ms,\n            left,\n        });\n        let over = self.abandoned.len().saturating_sub(64);\n        self.abandoned.drain(..over);\n    }"))
plant("C8", "check: published and under way allowed", AG,
      (REC, "            if self.publish(name).is_some() {\n                return rule(format!(\"`{name}` is published and under way\"));",
       "            if false && self.publish(name).is_some() {\n                return rule(format!(\"`{name}` is published and under way\"));"))
plant("C9", "check: a give-up leaving no members allowed", AG,
      (REC, "if matches!(&abandoned.left, Left::Members(m) if m.is_empty()) {",
       "if false && matches!(&abandoned.left, Left::Members(m) if m.is_empty()) {"))

# --- engine.rs: the give-up's saves, what it left, sync_past, begin.
plant("E2", "the give-up told after it moves (v2's order)", AG,
      (ENG, "        self.leave(name.to_string(), Outcome::Abandoned(why));\n        let now = self.clock.now_ms();\n        self.commit(Effect::Abandon, |r| r.abandon(name, why, now, left))?;",
       "        let now = self.clock.now_ms();\n        self.commit(Effect::Abandon, |r| r.abandon(name, why, now, left))?;\n        self.leave(name.to_string(), Outcome::Abandoned(why));"))
plant("E3", "left_by: an Intent with a snapshot seen leaves Nothing", AG,
      (ENG, "(_, None) if snapshot_seen => Left::Unknown,", "(_, None) if snapshot_seen => Left::Nothing,"))
plant("E4", "left_by: a take leaves Unknown", AG,
      (ENG, "(_, Some(taken)) => Left::Members(taken.members.clone()),", "(_, Some(_)) => Left::Unknown,"))
plant("E4b", "left_by: an Intent with no snapshot seen leaves Unknown", AG,
      (ENG, "        (_, None) => Left::Nothing,\n    }", "        (_, None) => Left::Unknown,\n    }"))
plant("E6", "sync_past: clear at the mark (count == rewound)", AG,
      (ENG, "Some((group, true)) if cuts.count <= cuts.rewound => group,", "Some((group, true)) if cuts.count < cuts.rewound => group,"))
plant("E7", "sync_past: the mark not read", AG,
      (ENG, "Some((group, true)) if cuts.count <= cuts.rewound => group,", "Some((group, true)) if false => group,"))
plant("E8", "begin: only the under-way bound (no room for the ends)", AG,
      (ENG, "        if !self.record.room() {\n            return Err(Error::Full);", "        if self.record.under_way.len() >= UNDER_WAY_MAX {\n            return Err(Error::Full);"))
plant("E9", "the Abandoning save does not clear held", AG,
      (ENG, "                        p.phase = phase;\n                        p.held = false;", "                        p.phase = phase;"))
plant("E10", "a publish done not kept among the names published", AG,
      (ENG, "self.commit(Effect::Forget, |r| r.leave_published(&name, now))?;",
       "self.commit(Effect::Forget, |r| { let _ = now; r.under_way.retain(|p| p.name != name) })?;"))

# --- plan.rs
plant("PL1", "plan: Abandoning not first (a person's choice first)", AG,
      (PLAN, "    if let Phase::Abandoning { why, .. } = &publish.phase {\n        return Action::Abandon(*why);\n    }\n", ""))

# --- program.rs: the cap, the quick retry, retention's place.
plant("G1", "cap: every Retry counted, not only a reopen's", AG,
      (PRG, "if verdict == Verdict::Retry && matches!(step, Some(Step::Reopened { .. })) {", "if verdict == Verdict::Retry {"))
plant("G2", "cap: one reopen more before it", AG,
      (PRG, "let reopen = if reopens >= REOPEN_CAP {", "let reopen = if reopens > REOPEN_CAP {"))
plant("G3", "cap: only one more reopen after it", AG,
      (PRG, "since_cap.is_multiple_of(REOPEN_CAP)", "since_cap == REOPEN_CAP"))
plant("G4", "quick retry: one more than QUICK_RETRIES", AG,
      (PRG, "if quick < QUICK_RETRIES && reopens < REOPEN_CAP {", "if quick <= QUICK_RETRIES && reopens < REOPEN_CAP {"))
plant("G5", "quick retry past the cap", AG,
      (PRG, "if quick < QUICK_RETRIES && reopens < REOPEN_CAP {", "if quick < QUICK_RETRIES {"))
plant("G6", "quick retries never renewed by another verdict", AG,
      (PRG, "        } else {\n            quick = 0;\n        }\n", "        }\n"))
plant("G7", "an early round uses up its tick (D1 back)", AG,
      (PRG, "tick = tick.saturating_add(if early { passed } else { passed.max(1) });", "tick = tick.saturating_add(passed.max(1)); let _ = early;"))
plant("G8", "early_ms never said", AG,
      (PRG, "early_ms: (began < due).then(|| due - began),", "early_ms: None,"))
plant("G9", "retention after news on the note", AG,
      (PRG, "if !restart && go() && !agent.note_came() {\n                if let Err(e) = agent.retain() {", "if !restart && go() {\n                if let Err(e) = agent.retain() {"))
plant("G10", "retention after an end was asked", AG,
      (PRG, "if !restart && go() && !agent.note_came() {\n                if let Err(e) = agent.retain() {", "if !restart && !agent.note_came() {\n                if let Err(e) = agent.retain() {"))
plant("G11", "run never retains", AG,
      (PRG, "if let Err(e) = agent.retain() {", "if let Err(e) = Ok::<(), Error>(()) {"))
plant("G12", "a new note does not start the counts again", AG,
      (PRG, "                    noted = Some(*note);\n                    reopens = 0;\n                    since_cap = 0;\n                    quick = 0;", "                    noted = Some(*note);"))
plant("G13", "reopen=held never said", AG,
      (PRG, "reopen_held = matches!(step, Some(Step::Held { .. }));", "reopen_held = false;"))
plant("G14", "the cap said again at each reopen past it", AG,
      (PRG, "if reopens == REOPEN_CAP {", "if reopens >= REOPEN_CAP {"))
plant("G15", "--keep 0 accepted", AG,
      (PRG, "Ok(n) if (1..=max).contains(&n) => kept.keep = n,", "Ok(n) if (0..=max).contains(&n) => kept.keep = n,"))
plant("G16", "the CLI does not pass --keep to the agent", CLI,
      (MAIN, "            agent.set_keep(kept.keep);\n", ""))

# --- lost.rs
plant("L1", "a held step's verdict failed (run goes on as if nothing could be done)", AG,
      (LOST, "Some(Step::Held { .. }) => Verdict::Retry,", "Some(Step::Held { .. }) => Verdict::Failed,"))
plant("L2", "look_lost reopens", AG,
      (LOST, "Some(state) if !reopen => return Ok(Step::Held { state }),", "Some(state) if false && !reopen => return Ok(Step::Held { state }),"))

# --- the fake daemon (c7790db)
plant("F1", "fake: the outcome kept after a retire", AG,
      (FAKE, "                self.daemon.ended.remove(&r.name);\n                reply", "                reply"))
plant("F2", "fake: a retire not refused once stopped", AG,
      (FAKE, "                if self.daemon.stopped {\n                    return Ok(Reply::Refused(\n                        Code::Stopped,",
       "                if false && self.daemon.stopped {\n                    return Ok(Reply::Refused(\n                        Code::Stopped,"))

# --- skyzfsd: found= and commit= (adc391a), the ended advice (c7790db), status (39ef4eb)
plant("X1", "the refusal's text written without the shared prefix", D,
      (CTL, "\"{OTHER_NAMES}{}, not the request's; it is left as it is\",", "\"snap/T names the commit {}, not the request's; it is left as it is\","))
plant("X2", "found= never written", D,
      (DREC, "if let Some(found) = crate::control::other_commit(reply) {", "if let Some(found) = None::<[u8; 32]> {\n                let _ = reply;"))
plant("X3", "found= read one digit short", D,
      (CTL, ".get(..64)?)", ".get(..63)?)"))
plant("X4", "commit= of the reply, not the request (the found one)", D,
      (DREC, "answer.push_str(&format!(\" commit={}\", hex(&commit)));", "answer.push_str(&format!(\" commit={}\", hex(&crate::control::other_commit(reply).unwrap_or(commit))));"))
plant("X5", "a retire dropped at the end given the no-answer advice", D,
      (HND, "ended: \"the retire may have landed: look tells (absent once it has)\",", "ended: \"the retire may still be made: ask again, which answers gone once it is\","))
plant("X6", "status: the daemon shows gone as retired", D,
      (DUP, "            retired: f.retired,\n            gone: f.retire_gone,", "            retired: f.retired,\n            gone: f.retired,"))
plant("S1", "status: retires written over lost_whole", ST,
      (STAT, "bytes[1416 + 8 * i..1424 + 8 * i]", "bytes[1408 + 8 * i..1416 + 8 * i]"))
plant("S2", "status: v20's retires not read", ST,
      (STAT, "retires: (version >= 20).then(|| RetiresShown {", "retires: (version >= 21).then(|| RetiresShown {"))
plant("S3", "status: v19 refused", ST,
      (STAT, "    } else if is(NINETEENTH_LEN, NINETEENTH_MAGIC) {\n        19", "    } else if false && is(NINETEENTH_LEN, NINETEENTH_MAGIC) {\n        19"))
plant("S4", "status: gone reported as other", ST,
      (STAT, "lines.push((\"publish_retire_gone\", r.gone.to_string()));", "lines.push((\"publish_retire_gone\", r.other.to_string()));"))
plant("U1", "upload: gone counted as retired", UP,
      (PUB, "Retired::Gone => self.figures.retire_gone += 1,", "Retired::Gone => self.figures.retired += 1,"))
plant("U2", "upload: retires not counted", UP,
      (DRV, "                self.publishing.count_retire(*retired);\n", ""))
plant("U3", "upload: no lease row worded as snap/T moving (L5 undone)", UP,
      (REM, "RemoteError::Conflict => RemoteError::Volume(NO_LEASE_ROW.into()),", "RemoteError::Conflict => RemoteError::Conflict,"))

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
