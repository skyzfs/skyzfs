#!/usr/bin/env python3
"""The plants of the next look at #179 (head 78c3997), written to
plants.json beside this script. Each is one textual mutant, planted
alone by ../plant.py and restored after its run.

A: the review's survivors again, as the answer says they now fare
   (the re-anchored ones with a prime).
B: the code of this round (c195ea4, 8653845, cdf6cb4), the reviewer's
   own plants, in the reviewer's own text.
"""
import json, os

LIB = ['cargo', 'test', '-p', 'skyzfs-agent', '--lib']
CLI = ['cargo', 'test', '-p', 'skyzfs-agent-cli', '--features', 'skyzfs-agent-cli/test-hooks']
UP = ['cargo', 'test', '-p', 'skyzfs-upload', '--test', 'retire']
RET = 'crates/skyzfs-agent/src/engine/retain.rs'
ENG = 'crates/skyzfs-agent/src/engine.rs'
REC = 'crates/skyzfs-agent/src/record.rs'
PRG = 'crates/skyzfs-agent/src/program.rs'
HOST = 'crates/skyzfs-agent/src/host.rs'
ZCLI = 'crates/skyzfs-agent/src/zfs/cli.rs'
MAIN = 'bins/skyzfs-agent/src/main.rs'

READ_FAIL = ('Report::NotRetired(format!("the members\' @{name} not read: {e}")),\n'
             '                );\n'
             '                return Ok(not_run_or_failed(&e));')
DESTROY_FAIL = ('Report::NotRetired(format!("zfs destroy {}@{name}: {e}", member.name)),\n'
                '                );\n'
                '                return Ok(not_run_or_failed(&e));')
ONE_REWRAP = ('        // As `destroy -r`: killed at its limit, the ioctl may finish in\n'
              '        // its own time.\n'
              '        let output = self\n'
              '            .exec("zfs", &args, &[], self.change_limit())\n'
              '            .map_err(|e| match e {\n'
              '                ZfsError::NotRun(why) => {\n'
              '                    ZfsError::NotRun(')
ONE_GONE = ('            .exec("zfs", &args, &[], self.change_limit())\n'
            '            .map_err(|e| match e {\n'
            '                ZfsError::NotRun(why) => {\n'
            '                    ZfsError::NotRun(format!("{snapshot} may not have been destroyed: {why}"))\n'
            '                }\n'
            '                e => e,\n'
            '            })?;\n'
            '        if output.code == Some(0) {\n'
            '            return Ok(());\n'
            '        }\n'
            '        match self.failed("zfs", &args, &output, true, pool) {\n'
            '            ZfsError::NoSnapshot => Ok(()),')

P = []


def plant(pid, what, edits, cmd=LIB):
    P.append({'id': pid, 'what': what, 'edits': edits, 'cmd': cmd})


# --- A: the review's survivors, again at the head -----------------------
plant("R12'", 'a members read that fails returns Destroyed::All (the name leaves, nothing destroyed)',
      [[RET, READ_FAIL, READ_FAIL.replace('not_run_or_failed(&e)', 'Destroyed::All')]])
plant('R15', 'a look answered unreadable is taken as absent',
      [[RET, 'Ok(Reply::Absent) => Ok(true),', 'Ok(Reply::Absent | Reply::Unreadable(_)) => Ok(true),'],
       [RET, 'Ok(Reply::Snap(_) | Reply::Unreadable(_)) => {', 'Ok(Reply::Snap(_)) => {']])
plant('R16', 'a look not answered is taken as absent',
      [[RET, 'self.say(name, Report::NotRetired(format!("look not answered: {e}")));\n                Ok(false)',
        'self.say(name, Report::NotRetired(format!("look not answered: {e}")));\n                Ok(true)']])
plant('R16b', 'a look answered otherwise is taken as absent',
      [[RET, 'Report::NotRetired(format!("look answered `{}`", other.line())),\n                );\n                Ok(false)',
        'Report::NotRetired(format!("look answered `{}`", other.line())),\n                );\n                Ok(true)']])
plant("R21'", 'given-up arm: retired told after the save',
      [[RET, '                            self.told(name.clone(), Outcome::Retired);\n'
             '                            self.commit(Effect::Retain, |r| {\n'
             '                                r.abandoned.retain(|a| a.name != name)\n'
             '                            })?;',
             '                            self.commit(Effect::Retain, |r| {\n'
             '                                r.abandoned.retain(|a| a.name != name)\n'
             '                            })?;\n'
             '                            self.told(name.clone(), Outcome::Retired);']])
plant('R22', 'forgotten told after the save',
      [[RET, '        self.told(name.to_string(), Outcome::Forgotten(why.to_string()));\n'
             '        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))',
             '        self.commit(Effect::Retain, |r| r.abandoned.retain(|a| a.name != name))?;\n'
             '        self.told(name.to_string(), Outcome::Forgotten(why.to_string()));\n'
             '        Ok(())']])
plant('E3', 'left_by at Intent with a snapshot seen gives Nothing',
      [[ENG, '(_, None) if snapshot_seen => Left::Unknown,', '(_, None) if snapshot_seen => Left::Nothing,']])
plant('E4', 'left_by: a take leaves Unknown, not its members',
      [[ENG, '(_, Some(taken)) => Left::Members(taken.members.clone()),', '(_, Some(_)) => Left::Unknown,']])
plant('E4b', 'left_by at Intent with no snapshot seen gives Unknown',
      [[ENG, '        (_, None) => Left::Nothing,\n    }', '        (_, None) => Left::Unknown,\n    }']])
plant('E8', 'begin checks only the bound on publishes under way, not room()',
      [[ENG, '        if !self.record.room() {\n            return Err(Error::Full);',
        '        if self.record.under_way.len() >= UNDER_WAY_MAX {\n            return Err(Error::Full);']])
plant("G11'", 'run never retains',
      [[PRG, 'if let Err(e) = agent.retain_while(go) {', 'if let Err(e) = Ok::<(), Error>(()) {']])
plant("G16-lib", 'schedule does not take --keep',
      [[PRG, '    agent.set_keep(kept.keep);\n', '']])
plant("G16-lib-ab", 'schedule does not take --abandoned-keep-s',
      [[PRG, '    agent.set_abandoned_keep(kept.abandoned_keep);\n', '']])
plant("G16'", 'the CLI does not pass what it parsed to schedule (Kept::default())',
      [[MAIN, 'program::schedule(&mut agent, every, kept, &mut go, &mut out, &mut err)',
        'program::schedule(&mut agent, every, { let _ = kept; program::Kept::default() }, &mut go, &mut out, &mut err)']],
      CLI)
plant('U1', 'the uploader counts gone as retired',
      [['crates/skyzfs-upload/src/publish.rs', 'Retired::Gone => self.figures.retire_gone += 1,',
        'Retired::Gone => self.figures.retired += 1,']], UP)
plant('U2', 'the driver counts no retire',
      [['crates/skyzfs-upload/src/driver.rs', '                self.publishing.count_retire(*retired);\n', '']], UP)
plant('M5', 'a retry that succeeds clears every stuck name',
      [[RET, 'self.commit(Effect::Retain, |r| r.stuck.retain(|s| s.name != name))?;',
        'self.commit(Effect::Retain, |r| r.stuck.clear())?;']])
plant('M7', 'a stuck name keeps only its first member',
      [[RET, '            r.stuck.push(Stuck {\n                name,\n                members,',
        '            r.stuck.push(Stuck {\n                name,\n                members: members.into_iter().take(1).collect(),']])
plant('Q8', 'the stuck names not sorted by name',
      [[RET, '        stuck.sort_by(|a, b| a.0.cmp(&b.0));\n', '']])
plant('Q9', 'the turn starts again at Agent::restart',
      [[ENG, '        self.retention.told.clear();\n        self.assists = 0;',
        '        self.retention.told.clear();\n        self.retention.stuck_last = None;\n        self.assists = 0;']])

# --- B: this round's code, the reviewer's own plants -------------------
# M1: the early leave.
plant('B-early-none', 'no name leaves early',
      [[RET, '.take(self.record.abandoned_over())', '.take(0)']])
plant('B-early-short', 'abandoned_over one short',
      [[REC, '(self.abandoned.len() + ending).saturating_sub(ABANDONED_MAX)',
        '(self.abandoned.len() + ending).saturating_sub(ABANDONED_MAX + 1)']])
plant('B-early-over', 'abandoned_over one too many',
      [[REC, '(self.abandoned.len() + ending).saturating_sub(ABANDONED_MAX)',
        '(self.abandoned.len() + ending + 1).saturating_sub(ABANDONED_MAX)']])
plant('B-early-newest', 'the newest given up leave early, not the oldest',
      [[RET, '        oldest.sort_unstable();\n', '        oldest.sort_unstable_by(|a, b| b.cmp(a));\n']])
plant('B-early-nolook', 'an early leave is not looked up',
      [[RET, 'look: early || a.why == Why::Person,', 'look: a.why == Why::Person,']])
plant('B-early-person-text', 'every snap/T-there line says given up by a person',
      [[RET, 'person: a.why == Why::Person,', 'person: true,']])
plant('B-early-nofilter', 'names already past their keep also counted as early (and looked up)',
      [[RET, '            .filter(|&i| {\n                self.record\n                    .abandoned\n                    .get(i)\n'
             '                    .is_some_and(|a| now.saturating_sub(a.at_ms) < bound)\n            })\n',
        '']]),
plant('B-over-ending1', 'abandoned_over counts only one ending',
      [[REC, '    pub fn abandoned_over(&self) -> usize {\n        let ending = self.under_way.len() + 1;',
        '    pub fn abandoned_over(&self) -> usize {\n        let ending = 1;']])
plant('B-over-noguard-uw', 'abandoned_over ignores the bound on publishes under way',
      [[REC, '        if self.under_way.len() >= UNDER_WAY_MAX || self.published.len() + ending > PUBLISHED_MAX {',
        '        if self.published.len() + ending > PUBLISHED_MAX {']])
plant('B-over-noguard-pub', 'abandoned_over ignores the names published',
      [[REC, '        if self.under_way.len() >= UNDER_WAY_MAX || self.published.len() + ending > PUBLISHED_MAX {',
        '        if self.under_way.len() >= UNDER_WAY_MAX {']])
plant('B-slack0', 'PUBLISHED_SLACK 0',
      [[REC, 'pub const PUBLISHED_SLACK: usize = 64;', 'pub const PUBLISHED_SLACK: usize = 0;']])
# M2: the halt.
plant('B-halt-nonote', 'halt asks go only, not the note',
      [[RET, 'if !self.retention.halted && (!go() || self.note_came()) {',
        'if !self.retention.halted && !go() {']])
plant('B-halt-nogo', 'halt asks the note only, not go',
      [[RET, 'if !self.retention.halted && (!go() || self.note_came()) {',
        'if !self.retention.halted && self.note_came() {']])
plant('B-halt-noreset', 'halted not reset at a new round',
      [[RET, '        self.retention.silent = false;\n        self.retention.halted = false;\n',
        '        self.retention.silent = false;\n']])
plant('B-silent-noreset', 'silent not reset at a new round',
      [[RET, '        self.retention.silent = false;\n        self.retention.halted = false;\n',
        '        self.retention.halted = false;\n']])
plant('B-halt-step', 'no halt before a step of the names published',
      [[RET, '                _ if self.halt(go) => return Ok(()),\n', '']])
plant('B-halt-givenup', 'no halt before a step of the names given up',
      [[RET, '                Some(_) if self.halt(go) => return Ok(()),\n', '']])
plant('B-halt-read', 'no halt before the members read',
      [[RET, '        let pool = self.pool.clone();\n        if self.halt(go) {\n            return Ok(Destroyed::Halted);\n        }\n',
        '        let pool = self.pool.clone();\n']])
plant('B-halt-destroy', 'no halt before each member destroy',
      [[RET, '        for member in doomed {\n            if self.halt(go) {\n                return Ok(Destroyed::Halted);\n            }\n',
        '        for member in doomed {\n']])
plant('B-halt-stuck-goes-on', 'a halt in the stuck retries moves the turn on and goes on',
      [[RET, '                Destroyed::Halted => break,', '                Destroyed::Halted => self.retention.stuck_last = Some(name.clone()),']])
plant('B-halt-program', 'run does not retain once go said no before retention (old guard back, go only)',
      [[PRG, '            if !restart {\n                if let Err(e) = agent.retain_while(go) {',
        '            if !restart && go() {\n                if let Err(e) = agent.retain_while(go) {']])
# L2: the silences.
plant('B-silent-ref', 'a retire not answered does not stop the round\'s retires',
      [[RET, '                Retire::Ref if self.retention.silent => {}\n', '']])
plant('B-silent-all', 'after a retire not answered the members steps stop too',
      [[RET, '                Retire::Ref if self.retention.silent => {}\n', '                _ if self.retention.silent => {}\n']])
plant('B-silent-look', 'a look not answered does not stop the round\'s looks',
      [[RET, '                Some(Due::Destroy { look: true, .. }) if self.retention.silent => {}\n', '']])
plant('B-silent-noanswer-code', 'a retire refused stopped silences the round, not no-answer',
      [[RET, '                if code == Code::NoAnswer {\n                    self.retention.silent = true;',
        '                if code == Code::Stopped {\n                    self.retention.silent = true;']])
# Decision 4: NotRun.
plant('B-nr-read', 'a members read not run is taken as failed',
      [[RET, READ_FAIL, READ_FAIL.replace('not_run_or_failed(&e)', 'Destroyed::Failed')]])
plant('B-nr-destroy', 'a destroy not run is taken as failed',
      [[RET, DESTROY_FAIL, DESTROY_FAIL.replace('not_run_or_failed(&e)', 'Destroyed::Failed')]])
plant('B-nr-destroy-all', 'a destroy that fails or is not run is taken as done',
      [[RET, DESTROY_FAIL, DESTROY_FAIL.replace('not_run_or_failed(&e)', 'Destroyed::All')]])
plant('B-nr-published', 'a published name whose destroy was not run is stuck',
      [[RET, '            Destroyed::NotRun | Destroyed::Halted => {}\n',
        '            Destroyed::NotRun => self.stick(name, members)?,\n            Destroyed::Halted => {}\n']])
plant('B-nr-givenup', 'a given-up name whose destroy was not run is stuck',
      [[RET, '                        Destroyed::NotRun => {}\n',
        '                        Destroyed::NotRun => self.stick(&name, members)?,\n']])
plant('B-nr-turn', 'a stuck name whose destroy was not run does not move the turn',
      [[RET, '                Destroyed::Failed | Destroyed::NotRun => {\n                    self.retention.stuck_last = Some(name.clone())\n                }\n',
        '                Destroyed::Failed => {\n                    self.retention.stuck_last = Some(name.clone())\n                }\n                Destroyed::NotRun => {}\n']])
plant('B-cli-exec-other', 'exec gives Other',
      [[ZCLI, '.map_err(|e| ZfsError::NotRun(format!("{}: {e}", command(program, args))))',
        '.map_err(|e| ZfsError::Other(format!("{}: {e}", command(program, args))))']])
plant('B-cli-one-other', 'destroy_one rewraps a destroy not run as Other',
      [[ZCLI, ONE_REWRAP, ONE_REWRAP[:-len('ZfsError::NotRun(')] + 'ZfsError::Other(']])
plant('B-cli-one-gone', 'destroy_one: a snapshot already gone is an error',
      [[ZCLI, ONE_GONE, ONE_GONE[:-len('ZfsError::NoSnapshot => Ok(()),')] + 'ZfsError::NoSnapshot => Err(ZfsError::NoSnapshot),']])
plant('B-cli-one-guard', 'destroy_one destroys any dataset@t (no guard on the name)',
      [[ZCLI, "        if !skyzfs_control::valid_name(t) || !under || dataset.contains('@') {\n            return Err(ZfsError::Other(format!(\n                \"{snapshot} was not destroyed: not a snapshot of pool {pool} that the agent names\"",
        "        if !skyzfs_control::valid_name(t) && !under && dataset.contains('@') {\n            return Err(ZfsError::Other(format!(\n                \"{snapshot} was not destroyed: not a snapshot of pool {pool} that the agent names\""]])
plant('B-host-shared', 'the step\'s commands counted with the others',
      [[HOST, 'run_counted(&STEP_STUCK_NOW, program, args, stdin, limit)', 'run_counted(&STUCK_NOW, program, args, stdin, limit)']])
plant('B-reopen-run', 'the reopen run as any other command',
      [[ZCLI, '            .run_step("zpool", &args, &[], self.limits.reopen)', '            .run("zpool", &args, &[], self.limits.reopen)']])
plant('B-L5', 'a take found destroyed records left=unknown',
      [[ENG, '(Why::NotWhole, Left::Nothing)', '(Why::NotWhole, Left::Unknown)']])

here = os.path.dirname(os.path.abspath(__file__))
json.dump(P, open(os.path.join(here, 'plants.json'), 'w'), indent=1)
print(len(P), 'plants')
