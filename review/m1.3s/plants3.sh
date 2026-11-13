#!/bin/bash
# Plants for the third look at M1.3s (skyzfs/skyzfs#46) at fe54d96.
# Run from a clean checkout of that commit: plants3.sh <dir of plant.py> <name>...
set -u
H=$1; shift
P="python3 $H/plant.py"
M=crates/skyzfs-blk/src/model.rs
I=crates/skyzfs-local/src/inplace.rs
F=crates/skyzfs-local/src/fs.rs
blk() { cargo test -p skyzfs-blk --lib 2>&1 | grep -E '^test result|FAILED|^error|panicked' | sort | uniq -c | head -20; }
inplace() { timeout 900 cargo test -p skyzfs-local --lib inplace:: 2>&1 | grep -E '^test result|FAILED|^error|^test .* (FAILED|has been running)' | sort | uniq -c | head -24; }
real() { timeout 600 cargo test -p skyzfs-local --test inplace_real -- --nocapture 2>&1 | grep -E '^test result|FAILED|^error|^test .* FAILED|skipped' | cut -c1-150 | sort | uniq -c | head -12; }
sys() { cargo test -p skyzfs-ublk-sys --lib cache -- --nocapture 2>&1 | grep -E '^test result|FAILED|^error|skipped' | cut -c1-150 | sort | uniq -c | head; }
restore() { git checkout -q -- crates; }

# s1: the start does not first show that the count sees the file.
s1() { $P $I 'if !self.mutants.has(Mutants::CACHE_SIGHT_UNCHECKED) {' 'if false {' && inplace; }
# s2: the look accepts a count of 0 after its read.
s2() { $P $I 'if self.cached_pages()? == 0 {' 'if false && self.cached_pages()? == 0 {' && inplace; }
# s3: one try of the drop, not two.
s3() { $P $I 'for _ in 0..2 {' 'for _ in 0..1 {' && inplace; }
# s4: three tries of the drop.
s4() { $P $I 'for _ in 0..2 {' 'for _ in 0..3 {' && inplace; }
# s5: a failed start releases the lock before the marker is written.
s5() { $P $I '        self.marker.begin();
        self.marker.finish();
        self.lock = None;' '        self.lock = None;
        self.marker.begin();
        self.marker.finish();' && inplace; }
# s6: the real count sees nothing, as through an overlay. What fails?
s6() { $P $F '            Some(call) => call(&self.file),' '            Some(call) => call(&self.file).map(|_| 0),' && real && inplace; }
# s6b: the system call's wrapper itself answers 0 always.
s6b() { $P crates/skyzfs-ublk-sys/src/cache.rs '        return Ok(stat.nr_cache);' '        return Ok(stat.nr_cache * 0);' && sys && real; }
# s7: the model does not narrow the worlds at a differing read (any world may explain each).
s7() { $P $M '                        let cx = &self.cx;
                        self.worlds.retain(|world| {' '                        let cx = &self.cx;
                        let all = self.worlds.clone();
                        self.worlds.retain(|world| {' && $P $M '                            });
                        }
                    }
                    Some(_) => {}' '                            });
                        }
                        self.worlds = all;
                    }
                    Some(_) => {}' && blk; }

# s8 (for 4e0d26d and later): the look comes before the drop again, as at fe54d96.
s8() { $P $I '        let mut left = 0;
        for _ in 0..2 {' '        if !self.mutants.has(Mutants::CACHE_SIGHT_UNCHECKED) {
            self.count_sees_the_file()?;
        }
        let mut left = 0;
        for _ in 0..2 {' && real && inplace; }
# s9: the look moved back before the drop, and none after it: the order of fe54d96.
# (s8 keeps the look after the drop too, and then every start finds block 0
# already in the cache, left there by the start before: no read-ahead, no refusal.)
s9() { $P $I '        let mut left = 0;
        for _ in 0..2 {' '        if !self.mutants.has(Mutants::CACHE_SIGHT_UNCHECKED) {
            self.count_sees_the_file()?;
        }
        let mut left = 0;
        for _ in 0..2 {' && $P $I '        if !self.mutants.has(Mutants::CACHE_SIGHT_UNCHECKED) {
            self.count_sees_the_file()?;
        }
        Ok(())' '        Ok(())' && real && inplace; }

for name in "$@"; do
    echo "=== $name"
    $name
    restore
done
