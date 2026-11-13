#!/bin/bash
# Plants for the second look at M1.3s (skyzfs/skyzfs#46) at 1280579.
# Run from a clean checkout of that commit: plants2.sh <dir of plant.py> <name>...
set -u
H=$1; shift
P="python3 $H/plant.py"
M=crates/skyzfs-blk/src/model.rs
I=crates/skyzfs-local/src/inplace.rs
F=crates/skyzfs-local/src/fs.rs
blk() { cargo test -p skyzfs-blk 2>&1 | grep -E '^test result|FAILED|^error|panicked' | sort | uniq -c | head -20; }
inplace() { timeout 900 cargo test -p skyzfs-local --lib inplace:: 2>&1 | grep -E '^test result|FAILED|^error|^test .* (FAILED|has been running)' | sort | uniq -c | head -24; }
real() { timeout 600 cargo test -p skyzfs-local --test inplace_real 2>&1 | grep -E '^test result|FAILED|^error|^test .* FAILED' | sort | uniq -c | head -12; }
sys() { cargo test -p skyzfs-ublk-sys --lib cache 2>&1 | grep -E '^test result|FAILED|^error' | sort | uniq -c | head; }
restore() { git checkout -q -- crates; }

# c1: the start does not drop the cache; the check is as it is.
c1() { $P $I 'if !self.mutants.has(Mutants::CACHE_NOT_DROPPED) {' 'if false {' && inplace; }
# c2: the check always passes.
c2() { $P $I '                Ok(0) => return Ok(()),
                Ok(pages) => pages,' '                Ok(_) => return Ok(()),' && inplace; }
# c3: the drop and its check come before the start's sync, not after.
c3() { $P $I '        store.counts.syncs += 1;
        if store.home.sync().is_err() {' '        store.empty_cache().map_err(IntoInPlaceError::CacheKept)?;
        store.counts.syncs += 1;
        if store.home.sync().is_err() {' && $P $I '        }
        store.empty_cache().map_err(IntoInPlaceError::CacheKept)?;
        Ok(store)' '        }
        Ok(store)' && inplace && real; }
# c4: one try, not two.
c4() { $P $I 'for _ in 0..2 {' 'for _ in 0..1 {' && inplace; }
# c5: the real drop does nothing.
c5() { $P $F '            rustix::fs::fadvise(&self.file, 0, None, rustix::fs::Advice::DontNeed)
                .map_err(io::Error::from)' '            Ok(())' && real; }
# c6: the real count sees nothing, as through an overlay.
c6() { $P $F '            Some(call) => call(&self.file),' '            Some(call) => call(&self.file).map(|_| 0),' && real && inplace; }

# f1: the failure waits for the marker before it answers.
f1() { $P $I '        self.log.fail_without_marker();
        self.marker.begin();
    }' '        self.log.fail_without_marker();
        self.marker.begin();
        self.marker.finish();
    }' && inplace; }
# f2: a failed start releases the lock before the marker is written.
f2() { $P $I '        self.marker.begin();
        self.marker.finish();
        self.lock = None;' '        self.lock = None;
        self.marker.begin();
        self.marker.finish();' && inplace; }
# f3: a failed start writes its marker before it returns.
f3() { $P $I '            let marker = std::mem::replace(&mut store.marker, Marker::Done);' '            store.marker.begin();
            store.marker.finish();
            let marker = std::mem::replace(&mut store.marker, Marker::Done);' && inplace; }
# m1: the model's sector is 256 bytes.
m1() { $P $M 'const SECTOR: usize = 512;' 'const SECTOR: usize = 256;' && blk; }
# m4: a second, different answer after a cut is never refused.
m4() { $P $M '                        if !free {' '                        if false && !free {' && blk; }
# m7: the discard's freedom after a cut is not kept.
m7() { $P $M '                Some(cell) if cell.contains(&id) => cell,' '                Some(cell) if false && cell.contains(&id) => cell,' && blk; }

for name in "$@"; do echo "=== $name"; $name; restore; done
