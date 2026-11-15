#!/usr/bin/env python3
"""#106 at 21f6f83: the start that creates intent.map does not sync the directory.

A new volume's first start begins a map in a new file, writes it and syncs the file,
and serves. Cut 1 is committed, block 150 is written (its region's bit set and
synced). Then the power is cut, and the cut keeps every sector as it was last written
and loses only the creations no directory sync covered. The start after it finds no
map and counts the whole volume.

The script appends one test to inplace/tests/cuts.rs of CHECKOUT, runs it, and
restores the file. With `fix`, keep_intent also syncs the directory after it creates
the file, and the map is taken up.

    name_not_durable.py CHECKOUT TARGET_DIR [fix]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
FIX = sys.argv[3:] == ['fix']
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
EDIT = ("                let file = self.dir.create(intent::INTENT_NAME)?;\n",
        "                let file = self.dir.create(intent::INTENT_NAME)?;\n                self.dir.sync()?;\n")
TEST = r'''
/// A power cut that keeps what every sector was last written with and
/// every length, and loses the creations no directory sync covered.
struct NamesLost;

impl crate::sim::Cut for NamesLost {
    fn sector(&mut self, _: Option<&str>, _: u64, _: usize) -> crate::sim::Fate {
        crate::sim::Fate::Latest
    }
    fn name(&mut self, _: usize, _: usize) -> bool {
        false
    }
    fn length(&mut self, _: Option<&str>) -> bool {
        true
    }
}

#[test]
fn review_the_new_maps_name_survives_a_power_cut() {
    let disk = created(37, 0);
    let mut store = committed_one(&disk, 0, ON_CALLER);
    write(&mut store, 150, 0x66).unwrap();
    assert_eq!(intent_of(&store).named, 1);
    drop(store);
    disk.power_cut(NamesLost);
    let store = start_as(&disk, 0, &region(), ON_CALLER, AFTER_A_REBOOT);
    eprintln!("the start after the power cut: {:?}", store.intent_at_start());
    assert_eq!(
        store.intent_at_start(),
        Some(&IntentAtStart::TakenUp {
            named: 1,
            regions: 4
        }),
    );
}
'''


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    orig = {p: open(os.path.join(ROOT, p)).read() for p in (TESTS, INP)}
    try:
        with open(os.path.join(ROOT, TESTS), 'w') as f:
            f.write(orig[TESTS] + TEST)
        if FIX:
            assert orig[INP].count(EDIT[0]) == 1
            with open(os.path.join(ROOT, INP), 'w') as f:
                f.write(orig[INP].replace(*EDIT))
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks',
                            'review_the_new_maps_name', '--', '--nocapture'],
                           cwd=ROOT, capture_output=True, text=True, env=env)
        for ln in (p.stdout + p.stderr).splitlines():
            if ln.startswith(('test inplace', 'the start after', 'error', '  left', ' right', 'thread')) \
                    or ('test result' in ln and ' 0 passed; 0 failed' not in ln):
                print(ln[:300])
        print('with the directory synced after the creation' if FIX else 'as the head has it', '- exit', p.returncode)
    finally:
        for p_, t in orig.items():
            with open(os.path.join(ROOT, p_), 'w') as f:
                f.write(t)


if __name__ == '__main__':
    main()
