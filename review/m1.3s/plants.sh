#!/bin/bash
# Plants for the review of M1.3s (skyzfs/skyzfs#46) at 4424460.
# Run from a clean checkout of that commit: plants.sh <dir of plant.py>
set -u
H=$1
P="python3 $H/plant.py"
M=crates/skyzfs-blk/src/model.rs
I=crates/skyzfs-local/src/inplace.rs
HA=crates/skyzfs-local/src/inplace/harness.rs
blk() { cargo test -p skyzfs-blk 2>&1 | grep -E '^test result|FAILED|^error' | sort | uniq -c | head -20; }
inplace() { cargo test -p skyzfs-local --lib inplace:: 2>&1 | grep -E '^test result|FAILED|^error' | sort | uniq -c | head -20; }
restore() { git checkout -q -- crates; }
run() { echo "=== $1"; shift; "$@"; restore; }

m1() { $P $M 'const SECTOR: usize = 512;' 'const SECTOR: usize = 256;' && blk && inplace; }
m2() { $P $M 'if floor.is_none_or(|f| i >= f) {' 'if true {' && $P $M 'let mut out = if floor.is_none() {
            cell.clone()
        } else {
            Vec::new()
        };' 'let mut out = cell.clone();' && blk; }
m3() { $P $M 'Some(World {
            durable,
            tail: Vec::new(),
            waiting: Vec::new(),
        })
    }
}' 'Some(World {
            durable: self.durable.clone(),
            tail: Vec::new(),
            waiting: Vec::new(),
        })
    }
}' && blk; }
m4() { $P $M 'Some(before) if &before[..] != bytes => {' 'Some(before) if &before[..] != bytes && false => {' && blk; }
m5() { $P $M 'if floor.is_none_or(|f| i >= f) {' 'if floor.is_none_or(|f| i > f) {' && blk; }
m6() { $P $M 'read.chunks(SECTOR).enumerate()' 'read.chunks(BLOCK_USIZE).enumerate()' && $P $M '&cx.bytes(*v)[s * SECTOR..(s + 1) * SECTOR] == sector' '&cx.bytes(*v)[s * BLOCK_USIZE..(s + 1) * BLOCK_USIZE] == sector' && blk && inplace; }
h1() { $P $HA 'disk.power_cut(SeededCut::new(keep ^ 0x5EED_C0DE));' 'let _ = keep; disk.kill();' && inplace; }
h2() { $P $HA 'disk.power_cut(SeededCut::new(0x0FA1_1ED0));' 'disk.kill();' && inplace; }
h3() { $P $HA '        7
    }' '        0
    }' && inplace; }
h4() { $P $HA 'disk.power_cut(SeededCut::new(keep ^ 0x5EED_C0DE));' 'disk.power_cut(SeededCut::with_garbage(keep ^ 0x5EED_C0DE));' && inplace; }
i1() { $P $I '        self.marker = match started {
            Ok(thread) => Marker::Writing(thread),' '        self.marker = match started {
            Ok(thread) => {
                while !thread.is_finished() {
                    std::thread::yield_now();
                }
                Marker::Writing(thread)
            }' && inplace; }
i2() { $P $I '        if let Marker::Writing(thread) = std::mem::replace(&mut self.marker, Marker::Done) {
            let _ = thread.join();
        }' '        let _ = std::mem::replace(&mut self.marker, Marker::Done);' && inplace; }
i3() { $P $I '                write.is_some_and(|write| write())
            });' '                drop(write);
                false
            });' && inplace; }
i4() { $P $I '        self.log.fail_without_marker();
        let Marker::Unwritten' '        self.log.fail_without_marker();
        self.marker = Marker::Done;
        let Marker::Unwritten' && inplace; }

for p in "${@:2}"; do run $p $p; done
