#!/usr/bin/env python3
"""#106 at 21f6f83: how often the store-level power-cut test takes a map up, and how
many of the four regions the map then names. Counters are added to the test in
CHECKOUT, the test is run, and the file is restored.

    narrowed_count.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
EDITS = [
    ("        Some(IntentAtStart::TakenUp { named, regions }) => named < regions,\n        Some(IntentAtStart::Begun(_)) => {\n            // The whole volume is the next cut, which holds everything.\n",
     "        Some(IntentAtStart::TakenUp { named, regions }) => {\n            REVIEW[*named as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n            named < regions\n        }\n"
     "        Some(IntentAtStart::Begun(why)) => {\n            if why.contains(\"no intent map\") {\n                REVIEW[6].fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n            }\n            REVIEW[5].fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n"),
    ("    assert!(\n        narrowed > 0,\n        \"no start after a power cut narrowed the new cut\"\n    );\n",
     "    let c: Vec<u64> = REVIEW.iter().map(|a| a.load(std::sync::atomic::Ordering::Relaxed)).collect();\n"
     "    eprintln!(\"runs with a map taken up, by the regions it named (0 to 4): {:?}; runs with a new map begun: {} (of them with no map found: {}); narrowed {narrowed}\", &c[..5], c[5], c[6]);\n"
     "    assert!(\n        narrowed > 0,\n        \"no start after a power cut narrowed the new cut\"\n    );\n"),
    ("/// A step of the store-level programs of the intent map.\n",
     "static REVIEW: [std::sync::atomic::AtomicU64; 7] = [const { std::sync::atomic::AtomicU64::new(0) }; 7];\n\n/// A step of the store-level programs of the intent map.\n"),
]


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    full = os.path.join(ROOT, TESTS)
    orig = open(full).read()
    text = orig
    for old, new in EDITS:
        assert text.count(old) == 1, old[:60]
        text = text.replace(old, new)
    try:
        with open(full, 'w') as f:
            f.write(text)
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks',
                            'after_a_power_cut_the_next_cut_holds_every_block_changed_since_the_newest_commit',
                            '--', '--nocapture'], cwd=ROOT, capture_output=True, text=True, env=env)
        for ln in (p.stdout + p.stderr).splitlines():
            if ln.startswith(('runs with', 'error', 'test inplace')):
                print(ln[:300])
        print('exit', p.returncode)
    finally:
        with open(full, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()
