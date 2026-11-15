#!/usr/bin/env python3
"""#98 at e905c2e: experiments on the cut state's model test, in a checkout that is restored after.

    second_death.py CHECKOUT TARGET_DIR

It appends two tests to crates/skyzfs-local/src/inplace/state/tests.rs and gives World one field:

1. review_what_notices_each_defect: the mutant test's list again, printing what noticed each.
2. review_a_second_death_in_the_restart: after a death at a store, the next instance is itself
   lost at each store of its restart (CutState::restore's settle), and a third instance takes
   the state up. At the second death the region is read as it stands, as at the first.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
PATH = os.path.join(ROOT, 'crates/skyzfs-local/src/inplace/state/tests.rs')

FIELD_OLD = "    next_value: u64,\n    deaths: u64,\n}\n"
FIELD_NEW = "    next_value: u64,\n    deaths: u64,\n    /// Review: the store of the restart at which the next instance is lost too.\n    second: Option<u64>,\n    second_died: bool,\n}\n"
INIT_OLD = "            next_value: 1,\n            deaths: 0,\n        }\n"
INIT_NEW = "            next_value: 1,\n            deaths: 0,\n            second: None,\n            second_died: false,\n        }\n"
STEP_OLD = "                self.restart(step)?;\n                self.adopt();\n                // The kernel issues the request again"
STEP_NEW = """                if let Some(k) = self.second.take() {
                    self.region.die_at(k);
                    let lost = catch_unwind(AssertUnwindSafe(|| {
                        CutState::restore(self.region.clone(), SIZE, self.bound, self.mutants).map(|_| ())
                    }));
                    self.region.revive();
                    if let Err(payload) = lost {
                        if payload.downcast_ref::<Lost>().is_none() {
                            std::panic::resume_unwind(payload);
                        }
                        self.second_died = true;
                        let view = CutState::view(self.region.clone(), SIZE, self.bound, self.mutants)
                            .map_err(|e| e.to_string())?;
                        self.check_cuts(&view)
                            .map_err(|e| format!("read at the second death, store {k} of the restart after {step:?}: {e}"))?;
                    }
                }
                self.restart(step)?;
                self.adopt();
                // The kernel issues the request again"""

EXTRA = r'''

/// Review: what notices each defect.
#[test]
fn review_what_notices_each_defect() {
    let fixed = |mutants: u64| -> Option<String> {
        for (name, steps, bound) in fixed_programs() {
            if let Err(e) = run(&steps, mutants, bound, None) {
                return Some(format!("FIXED {name}: {e}"));
            }
        }
        None
    };
    for (name, mutant, deaths) in [
        ("COPY_UNRECORDED", Mutants::COPY_UNRECORDED, true),
        ("COPY_AFTER_OVERWRITE", Mutants::COPY_AFTER_OVERWRITE, false),
        ("ABANDONED_MAP_DROPPED", Mutants::ABANDONED_MAP_DROPPED, false),
        ("REPLACED_MAP_DROPPED", Mutants::REPLACED_MAP_DROPPED, false),
        ("ABANDONED_COPIES_FREED", Mutants::ABANDONED_COPIES_FREED, false),
        ("REPLACED_COPIES_FREED", Mutants::REPLACED_COPIES_FREED, false),
        ("ABANDONED_TO_FLUSHING", Mutants::ABANDONED_TO_FLUSHING, false),
        ("ABANDONED_TO_OPEN", Mutants::ABANDONED_TO_OPEN, false),
        ("REPLACED_AT_INSTANT", Mutants::REPLACED_AT_INSTANT, false),
        ("FREEZE_SPLIT", Mutants::FREEZE_SPLIT, true),
        ("SUMMARY_AFTER_BIT", Mutants::SUMMARY_AFTER_BIT, true),
        ("MOVE_CLEARS_FIRST", Mutants::MOVE_CLEARS_FIRST, true),
        ("MAP_NOT_CLEARED", Mutants::MAP_NOT_CLEARED, false),
    ] {
        let f = fixed(mutant);
        let r = caught(mutant, 200, 2, false).map(|e| format!("RANDOM {e}"));
        let d = if deaths { caught(mutant, 20, 2, true).map(|e| format!("DEATHS {e}")) } else { None };
        let cut = |s: Option<String>| s.map(|s| s.chars().take(230).collect::<String>()).unwrap_or_else(|| "-".into());
        eprintln!("NOTICED {name}\n   fixed:  {}\n   random: {}\n   deaths: {}", cut(f), cut(r), cut(d));
    }
}

/// Review: a second death, in the restart.
#[test]
fn review_a_second_death_in_the_restart() {
    let mut runs = 0u64;
    let mut seconds = 0u64;
    let mut programs: Vec<(String, Vec<Step>, u64)> = Vec::new();
    for bound in [2, 64] {
        for seed in 0..8 {
            programs.push((format!("seed {seed}"), program(seed, 50), bound));
        }
    }
    for (name, steps, bound) in fixed_programs() {
        programs.push((name.to_string(), steps, bound));
    }
    let mut worst = 0u64;
    for (name, steps, bound) in programs {
        let stores = stores_of(&steps, 0, bound);
        for death in 0..stores {
            for k in 0..200u64 {
                let mut world = World::new(0, bound);
                world.second = Some(k);
                world.region.die_at(death);
                let mut failed = None;
                for (i, step) in steps.iter().enumerate() {
                    if let Err(e) = world.step(*step) {
                        failed = Some(format!("step {i} ({step:?}): {e}"));
                        break;
                    }
                }
                runs += 1;
                if let Some(e) = failed {
                    panic!("SECOND {name}, bound {bound}, death at store {death}, then at store {k} of the restart: {e}");
                }
                if !world.second_died {
                    worst = worst.max(k);
                    break;
                }
                seconds += 1;
            }
        }
    }
    eprintln!("SECOND ok: {runs} runs, {seconds} with a second death, a restart makes at most {worst} stores");
}
'''


def main():
    orig = open(PATH).read()
    s = orig
    for old, new in ((FIELD_OLD, FIELD_NEW), (INIT_OLD, INIT_NEW), (STEP_OLD, STEP_NEW)):
        assert s.count(old) == 1, old
        s = s.replace(old, new)
    s += EXTRA
    with open(PATH, 'w') as f:
        f.write(s)
    try:
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'inplace::state::tests::review_',
                            '--nocapture', '--test-threads', '2'], cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        keep = False
        for ln in out.splitlines():
            if ln.startswith(('NOTICED', '   fixed', '   random', '   deaths', 'SECOND', 'test ', 'error')) or 'panicked' in ln or 'test result' in ln:
                print(ln[:400])
                keep = 'panicked' in ln
            elif keep:
                print(ln[:600])
                keep = False
    finally:
        with open(PATH, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
