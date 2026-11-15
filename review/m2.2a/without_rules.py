#!/usr/bin/env python3
"""#98 at e905c2e: which defects the content of the held cuts shows, with the model of D7's rules
(check_rules) switched off. In a checkout that is restored after.

    without_rules.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
PATH = os.path.join(ROOT, 'crates/skyzfs-local/src/inplace/state/tests.rs')
OLD = "    fn check_rules(&self, step: Step) -> Result<(), String> {\n"
NEW = OLD + "        if std::env::var_os(\"REVIEW_NO_RULES\").is_some() {\n            let _ = step;\n            return Ok(());\n        }\n"
EXTRA = r'''

/// Review: what notices each defect when the rules are not compared.
#[test]
fn review_what_content_notices() {
    let fixed = |mutants: u64| -> Option<String> {
        for (name, steps, bound) in fixed_programs() {
            if let Err(e) = run(&steps, mutants, bound, None) {
                return Some(format!("FIXED {name}: {e}"));
            }
        }
        None
    };
    for (name, mutant) in [
        ("ABANDONED_MAP_DROPPED", Mutants::ABANDONED_MAP_DROPPED),
        ("REPLACED_MAP_DROPPED", Mutants::REPLACED_MAP_DROPPED),
        ("ABANDONED_TO_FLUSHING", Mutants::ABANDONED_TO_FLUSHING),
        ("ABANDONED_TO_OPEN", Mutants::ABANDONED_TO_OPEN),
        ("REPLACED_AT_INSTANT", Mutants::REPLACED_AT_INSTANT),
        ("FREEZE_SPLIT", Mutants::FREEZE_SPLIT),
    ] {
        let f = fixed(mutant);
        let r = caught(mutant, 400, 2, false).map(|e| format!("RANDOM {e}"));
        let r64 = caught(mutant, 400, 64, false).map(|e| format!("RANDOM(bound 64) {e}"));
        let d = caught(mutant, 20, 2, true).map(|e| format!("DEATHS {e}"));
        let cut = |s: Option<String>| s.map(|s| s.chars().take(260).collect::<String>()).unwrap_or_else(|| "-".into());
        eprintln!("CONTENT {name}\n   fixed:  {}\n   random: {}\n   random64: {}\n   deaths: {}", cut(f), cut(r), cut(r64), cut(d));
    }
}
'''


def main():
    orig = open(PATH).read()
    assert orig.count(OLD) == 1
    with open(PATH, 'w') as f:
        f.write(orig.replace(OLD, NEW) + EXTRA)
    try:
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET, REVIEW_NO_RULES='1')
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'inplace::state::tests::review_what_content',
                            '--nocapture'], cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        for ln in out.splitlines():
            if ln.startswith(('CONTENT', '   fixed', '   random', '   deaths', 'test ', 'error')) or 'panicked' in ln or 'test result' in ln:
                print(ln[:420])
    finally:
        with open(PATH, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
