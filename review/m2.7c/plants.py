# Plants at 29f5538 (PR 173, M2.7c), each alone in a scratch worktree of
# the head, run against the lib tests of the affected crates (`cargo test
# -p skyzfs-agent -p skyzfs-control -p skyzfs-ublk --lib`, and skyzfs-ublk's
# integration test `failure_note` for the note's format), 600 s per
# plant, restored after each with `git checkout`. A.. are the author's
# mutants, re-made from the "Mutants:" lines of the new tests' docs (the
# description lists only their counts); R.. are the review's own.
# SCRATCH names the scratch directory, which holds the worktree pr173p
# at 29f5538. Usage: plants.py [TAG ...]
import subprocess, sys, os, re
S = os.environ['SCRATCH']
WT = S + '/pr173p'
L = 'crates/skyzfs-agent/src/engine/lost.rs'
P = 'crates/skyzfs-agent/src/program.rs'
E = 'crates/skyzfs-agent/src/engine.rs'
D = 'crates/skyzfs-agent/src/daemon.rs'
C = 'crates/skyzfs-agent/src/zfs/cli.rs'
F = 'crates/skyzfs-control/src/failure.rs'
env = dict(os.environ, CARGO_TARGET_DIR=S + '/t173p', CARGO_INCREMENTAL='0',
           CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')

REOPEN_ERR = "        let error = self.zfs.reopen(&self.pool).err().map(|e| e.to_string());\n"
DONE = "            if done || looked_ms >= most {"
PRE_NONE = "            None => return Ok(Step::NotImported),\n"
PRE_SUSP = "            Some(state) if state.suspended() => return Ok(Step::Suspended),\n"
LOSE_CALL = "                let step = match agent.lose() {"
LOST_RET = "                let _ = out.flush();\n                return exit::LOST;"
LOST_ROUND = "                let _ = out.write_all(round_line(&round).as_bytes());\n                let _ = out.flush();\n                return exit::LOST;"
LOST_LINE = "                let line = lost_line(agent.pool(), &agent.volume(), &note, step.as_ref());\n                let _ = out.write_all(line.as_bytes());\n"
UNHELD_ARM = """                    failure.reason
                );
            }
            Ok(Failed::Lost(note)) => {"""
MOVED = """        if let Some(why) = agent.moved() {
            let _ = writeln!(
                err,
                "skyzfs-agent: round {n}: {why}: run ends, to be started again on the device \\
                 as it is now"
            );
            return exit::MOVED;
        }
"""
ROUND_OUT = "        let _ = out.write_all(round_line(&round).as_bytes());\n        let _ = out.flush();\n        if let Some(code) = ended {"
ATT_CMP = "                Some(first) if first != now && self.moved.is_none() => {\n                    self.moved = Some(format!(\n                        \"the daemon's attachment"
DEV_CMP = "            Some(first) if first != now && self.moved.is_none() => {\n                self.moved = Some(format!(\n                    \"the volume's device"
RESTART_END = "        self.assists = 0;\n        self.at.clear();\n        Ok(())"
REFUSE = "    if matches!(task, Task::Publish | Task::Settle) {"
REF_UNHELD = """                failure.reason
            );
            Some(exit::ERROR)"""
REF_LOST = """                note.held_word()
            );
            Some(exit::LOST)"""
REOPEN_EXEC = "        let output = self.exec(\"zpool\", &args, &[], self.limits.reopen)?;\n        if output.code == Some(0) {"
NAME_CHECK = "        if pool.is_empty() || pool == \".\" || pool == \"..\" || pool.contains(['/', '\\0']) {"
STATE_NF = "            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),\n            Err(e) => return Err(ZfsError::Other(format!(\"{}: {e}\", path.display()))),"
WORD = "        let word = text.trim();\n        if word.is_empty() || word.contains(char::is_whitespace) {"
D_READ = """            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(stored) = decode_failure(&bytes) else {"""

PLANTS = [
 ('BASE', []),
 # ---- the author's, re-made from the tests' "Mutants:" lines
 ('A-D1 the note read elsewhere', [(D, "self.socket.with_file_name(FAILED)", "self.socket.with_file_name(\"failure\")")]),
 ('A-D2 the boot not compared', [(D, "Ok((stored.boot == boot).then_some(Noted::Failed(stored.failure)))",
                                  "let _ = boot;\n        Ok(Some(Noted::Failed(stored.failure)))")]),
 ('A-D3 a torn note taken for none', [(D, "return Ok(Some(Noted::NotWhole));", "return Ok(None);")]),
 ('A-C1 the reopen within a change limit', [(C, "self.exec(\"zpool\", &args, &[], self.limits.reopen)?", "self.exec(\"zpool\", &args, &[], self.change_limit())?")]),
 ('A-C2 the reopen error confirmed with zpool list', [(C, REOPEN_EXEC,
   "        self.zpool(&args, self.limits.reopen, pool)?;\n        let output = self.exec(\"true\", &[], &[], self.limits.reopen)?;\n        if output.code == Some(0) {")]),
 ('A-C3 a failed reopen read as done', [(C, "        if output.code == Some(0) {\n            return Ok(());\n        }\n        let said",
                                        "        if output.code.is_some() {\n            return Ok(());\n        }\n        let said")]),
 ('A-C4 a reopen past its limit read as done', [(C, REOPEN_EXEC,
   "        let Ok(output) = self.exec(\"zpool\", &args, &[], self.limits.reopen) else {\n            return Ok(());\n        };\n        if output.code == Some(0) {")]),
 ('A-C5 the wrong kstat path', [(C, ".join(pool).join(\"state\");", ".join(pool).join(\"status\");")]),
 ('A-C6a no file read as an error', [(C, STATE_NF, "            Err(e) => return Err(ZfsError::Other(format!(\"{}: {e}\", path.display()))),")]),
 ('A-C6b an error read as no file', [(C, STATE_NF, "            Err(_) => return Ok(None),")]),
 ('A-C7 the word not trimmed', [(C, WORD, "        let word: &str = &text;\n        if word.is_empty() || word.contains(char::is_whitespace) {")]),
 ('A-C8 a name that leaves the directory read', [(C, NAME_CHECK, "        if pool.is_empty() {")]),
 ('A-L1 Unknown taken as not held', [(L, "if failure.held == Held::No =>", "if failure.held != Held::Yes =>")]),
 ('A-L2 a torn note taken as none', [(L, "Some(Noted::NotWhole) => Failed::Lost(Note::Torn),", "Some(Noted::NotWhole) => Failed::No,")]),
 ('A-L3 a torn note taken as not held', [(L, "Some(Noted::NotWhole) => Failed::Lost(Note::Torn),",
   "Some(Noted::NotWhole) => Failed::Unheld(Failure { held: Held::No, reason: skyzfs_control::failure::Reason::Marked, identity: [0; 16] }),")]),
 ('A-L4 No taken as lost', [(L, "if failure.held == Held::No => Failed::Unheld(failure),", "if failure.held == Held::No => Failed::Lost(Note::Whole(failure)),")]),
 ('A-L5 no reopen', [(L, REOPEN_ERR, "        let error: Option<String> = None;\n")]),
 ('A-L6 the reopen taken for the signal (no look after a reopen that returned)', [(L, REOPEN_ERR,
   REOPEN_ERR + "        if error.is_none() {\n            return Ok(Step::Reopened { error, state: Some(PoolState(\"SUSPENDED\".into())), looked_ms: 0 });\n        }\n")]),
 ('A-L7 the look not repeated', [(L, DONE, "            if true || done || looked_ms >= most {")]),
 ('A-L8 the look not waited for between reads (half the wait)', [(L, "            self.clock.sleep(LOOK_EVERY);", "            self.clock.sleep(LOOK_EVERY / 2);")]),
 ('A-L9 the look bounded at twice its limit', [(L, DONE, "            if done || looked_ms >= most * 2 {")]),
 ('A-L10 the step ended at the reopen error', [(L, REOPEN_ERR,
   REOPEN_ERR + "        if error.is_some() {\n            return Ok(Step::Reopened { error, state: None, looked_ms: 0 });\n        }\n")]),
 ('A-L11a a pool not imported reopened', [(L, PRE_NONE, "            None => {}\n")]),
 ('A-L11b a suspended pool reopened', [(L, PRE_SUSP, "")]),
 ('A-E1 the attachment not compared', [(E, ATT_CMP, ATT_CMP.replace("if first != now", "if false && first != now"))]),
 ('A-E2 N3: restart clears moved only', [(E, RESTART_END, "        self.assists = 0;\n        self.moved = None;\n        self.at.clear();\n        Ok(())")]),
 ('A-E2b N3b: restart forgets the first attachment, device and the move', [(E, RESTART_END,
   "        self.assists = 0;\n        self.moved = None;\n        self.first_attachment = None;\n        self.first_device = None;\n        self.at.clear();\n        Ok(())")]),
 ('A-E3 the move forgotten at a later read that matches the first', [(E,
   "                Some(_) => {}\n            }\n        }\n        reply",
   "                Some(_) => self.moved = None,\n            }\n        }\n        reply")]),
 ('A-E4 the device not compared', [(E, DEV_CMP, DEV_CMP.replace("if first != now", "if false && first != now"))]),
 ('A-E5 the old 8-bit split', [(E, "    ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff)", "    (dev >> 8) & 0xff"),
                               (E, "    ((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff)", "    dev & 0xff")]),
 ('A-P1 run makes no reopen', [(P, LOSE_CALL, "                let step = match Ok::<Step, Error>(Step::NotImported) {")]),
 ('A-P2 run ends lost with 1', [(P, LOST_RET, "                let _ = out.flush();\n                return exit::ERROR;")]),
 ('A-P3 the lost round line left out', [(P, LOST_ROUND, "                let _ = round;\n                let _ = out.flush();\n                return exit::LOST;")]),
 ('A-P4 the lost line after the round line', [(P, LOST_LINE, ""),
   (P, LOST_ROUND, "                let _ = out.write_all(round_line(&round).as_bytes());\n                let line = lost_line(agent.pool(), &agent.volume(), &note, step.as_ref());\n                let _ = out.write_all(line.as_bytes());\n                let _ = out.flush();\n                return exit::LOST;")]),
 ('A-P5 a begin before the step', [(P, LOSE_CALL, "                let _ = agent.begin();\n" + LOSE_CALL)]),
 ('A-P6 Unheld: a reopen', [(P, UNHELD_ARM, UNHELD_ARM.replace("                );\n            }\n", "                );\n                let _ = agent.lose();\n            }\n", 1))]),
 ('A-P7 Unheld: an end with 9', [(P, UNHELD_ARM, UNHELD_ARM.replace("                );\n            }\n", "                );\n                return exit::LOST;\n            }\n", 1))]),
 ('A-P8 no refusal of the one-shots', [(P, REFUSE, "    if false && matches!(task, Task::Publish | Task::Settle) {")]),
 ('A-P9 one code for both refusals', [(P, REF_UNHELD, REF_UNHELD.replace("Some(exit::ERROR)", "Some(exit::LOST)"))]),
 ('A-P10 a refusal that reopens', [(P, REF_LOST, "                note.held_word()\n            );\n            let _ = agent.lose();\n            Some(exit::LOST)")]),
 ('A-P11 no end on a move', [(P, MOVED, "")]),
 ('A-P12 an end on a move before the round line', [(P, MOVED, ""),
   (P, ROUND_OUT, MOVED.replace("        if let", "        if let", 1) + ROUND_OUT)]),
 ('A-P13 an end at a take-over (any second cuts a move)', [(E, ATT_CMP, ATT_CMP.replace("if first != now", "if true"))]),
 ('A-P14 a lost line field misnamed', [(P, "         state={state} looked_ms={looked}\\n\",", "         state={state} looked={looked}\\n\",")]),
 ('A-F1 two reasons with one word', [(F, "            Reason::Short => \"short\",", "            Reason::Short => \"log-failed\",")]),
 # ---- the review's own
 ('R-L1 a pool gone during the look not taken as the end', [(L, "let done = state.as_ref().is_none_or(PoolState::suspended);", "let done = state.as_ref().is_some_and(PoolState::suspended);")]),
 ('R-L2 LOOK_MAX 60 s', [(L, "pub const LOOK_MAX: Duration = Duration::from_secs(10);", "pub const LOOK_MAX: Duration = Duration::from_secs(60);")]),
 ('R-L3 LOOK_EVERY 100 ms', [(L, "pub const LOOK_EVERY: Duration = Duration::from_millis(250);", "pub const LOOK_EVERY: Duration = Duration::from_millis(100);")]),
 ('R-L4 an error of a look swallowed (read as not imported)', [(L, "            let state = self.zfs.pool_state(&self.pool)?;", "            let state = self.zfs.pool_state(&self.pool).unwrap_or(None);")]),
 ('R-L5 the state before the reopen: an error read as not imported', [(L, "        match self.zfs.pool_state(&self.pool)? {", "        match self.zfs.pool_state(&self.pool).unwrap_or(None) {")]),
 ('R-P1 run: the step error dropped (round ok, nothing on stderr)', [(P, "                    Err(e) => {\n                        failed.push(e);\n                        None\n                    }",
                                                                      "                    Err(_) => None,")]),
 ('R-P2 run: a note that cannot be read ignored', [(P, "            Err(e) => failed.push(e),\n        }\n        if restart {", "            Err(_) => {}\n        }\n        if restart {")]),
 ('R-P3 one-shots: a note that cannot be read lets them through', [(P, "        Err(e) => Some(failed(&e, err)),", "        Err(_) => None,")]),
 ('R-P4 run: lost round line always failed', [(P, "                    failed: !failed.is_empty(),\n                    late_ms: began.saturating_sub(due),\n                    took_ms: now.saturating_sub(began),\n                    skipped: (now",
                                               "                    failed: true,\n                    late_ms: began.saturating_sub(due),\n                    took_ms: now.saturating_sub(began),\n                    skipped: (now")]),
 ('R-P5 run: Unheld said on no line', [(P, "                    \"skyzfs-agent: round {n}: the volume failed in this boot ({}), and no \\", "                    \"skyzfs-agent: round {n}: the volume failed ({}), and no \\")]),
 ('R-P6 run: the lost check after the syncs and the begin (as a later step of the round)', [
   (P, "        match agent.failure() {\n            Ok(Failed::No) => {}\n            Ok(Failed::Unheld(failure)) => {\n                let _ = writeln!(\n                    err,\n                    \"skyzfs-agent: round {n}:", "        match agent.failure().map(|f| { let _ = agent.begin(); f }) {\n            Ok(Failed::No) => {}\n            Ok(Failed::Unheld(failure)) => {\n                let _ = writeln!(\n                    err,\n                    \"skyzfs-agent: round {n}:")]),
 ('R-D1 the note: a read error other than none taken as no note', [(D, D_READ, D_READ.replace("Err(e) => return Err(e),", "Err(_) => return Ok(None),"))]),
 ('R-D2 the note: the boot compared inverted', [(D, "(stored.boot == boot)", "(stored.boot != boot)")]),
 ('R-D3 the note: a boot id that does not parse taken as this boot', [(D, "let boot = parse_boot_id(&String::from_utf8_lossy(&text)).ok_or_else(|| {", "let boot = parse_boot_id(&String::from_utf8_lossy(&text)).or(Some(stored.boot)).ok_or_else(|| {")]),
 ('R-C1 a state of two words taken', [(C, WORD, "        let word = text.trim();\n        if word.is_empty() {")]),
 ('R-C2 a reopen ended by a signal read as done', [(C, "        if output.code == Some(0) {\n            return Ok(());\n        }\n        let said",
                                                   "        if output.code == Some(0) || output.code.is_none() {\n            return Ok(());\n        }\n        let said")]),
 ('R-C3 Limits::reopen 60 s', [(C, "            reopen: Duration::from_secs(10),", "            reopen: Duration::from_secs(60),")]),
 ('R-F1 decode: a longer file taken', [(F, "    let whole = bytes.len() == FAILED_LEN", "    let whole = bytes.len() >= FAILED_LEN")]),
 ('R-F2 decode: holding 3 taken as unknown', [(F, "        && bytes[40] <= 2", "        && bytes[40] <= 3")]),
 ('R-F3 decode: the padding at 42 not checked', [(F, "        && bytes[42..48] == [0; 6]\n", "")]),
 ('R-F4 decode: the padding at 56 not checked', [(F, "        && bytes[56..] == [0; 8];", ";")]),
 ('R-F5 encode: Unknown written as yes', [(F, "        Held::Unknown => 2,\n    };", "        Held::Unknown => 1,\n    };")]),
 ('R-F6 decode: code 8 taken as image-failed', [(F, "    let (_, reason) = REASONS.iter().find(|(code, _)| *code == bytes[41])?;",
                                                 "    let (_, reason) = REASONS.iter().find(|(code, _)| *code == bytes[41].min(7).max(1))?;")]),
 ('R-F7 parse_boot_id: more than 32 digits taken', [(F, "    if hex.len() != 32 || !hex.iter().all(u8::is_ascii_hexdigit) {", "    if hex.len() < 32 || !hex.iter().all(u8::is_ascii_hexdigit) {")]),
 ('R-F8 read_failure: a torn note read as none', [(F, "    decode_failure(&bytes)\n        .map(Some)\n        .ok_or_else(|| not_whole(\"not a failure note\"))", "    Ok(decode_failure(&bytes))")]),
 ('R-F9 Held word unknown as no', [(F, "            Held::Unknown => \"unknown\",", "            Held::Unknown => \"no\",")]),
]

def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, cwd=WT, env=env, capture_output=True, text=True, timeout=timeout)
        return p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired as e:
        return 'TIMEOUT', (e.stdout or b'').decode(errors='replace') if isinstance(e.stdout, bytes) else (e.stdout or '')

def main():
    tags = sys.argv[1:]
    plants = [p for p in PLANTS if not tags or p[0].split(' ')[0] in tags]
    for name, edits in plants:
        subprocess.run(['git', 'checkout', '-q', '--', '.'], cwd=WT, check=True)
        ok = True
        for path, old, new in edits:
            full = os.path.join(WT, path)
            text = open(full).read()
            n = text.count(old)
            if n != 1:
                print(f'{name}: PLANT DOES NOT APPLY ({path}: {n} matches)', flush=True)
                ok = False
                break
            open(full, 'w').write(text.replace(old, new))
        if not ok:
            continue
        rc, out = run(['cargo', 'test', '-p', 'skyzfs-agent', '-p', 'skyzfs-control', '-p', 'skyzfs-ublk', '--lib'], 600)
        if rc not in ('TIMEOUT',) and re.search(r'error(\[E\d+\])?: ', out) and 'test result' not in out:
            first = [l for l in out.splitlines() if l.startswith('error')][:3]
            print(f'{name}: DOES NOT BUILD: {first}', flush=True)
            continue
        rc2, out2 = (0, '')
        if path_touches(edits, F):
            rc2, out2 = run(['cargo', 'test', '-p', 'skyzfs-ublk', '--test', 'failure_note'], 600)
        failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', out + out2, re.M)))
        passed = sum(int(x) for x in re.findall(r'test result: \w+\. (\d+) passed', out + out2))
        if rc == 'TIMEOUT' or rc2 == 'TIMEOUT':
            verdict = 'TIMEOUT (600 s)'
        elif failed:
            verdict = 'KILLED by ' + ', '.join(failed)
        elif rc != 0 or rc2 != 0:
            verdict = f'FAILED rc={rc}/{rc2} with no failing test named'
        else:
            verdict = f'SURVIVED ({passed} passed)'
        print(f'{name}: {verdict}', flush=True)
    subprocess.run(['git', 'checkout', '-q', '--', '.'], cwd=WT, check=True)

def path_touches(edits, path):
    return any(e[0] == path for e in edits)

main()
