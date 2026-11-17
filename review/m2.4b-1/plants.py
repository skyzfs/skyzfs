#!/usr/bin/env python3
"""#112 at 88e4edc (M2.4b-1): plants in skyzfs-local, one at a time, against the
crate's tests built with its test hooks (the superset: the plain build has the same
261 unit tests and three fewer tests on real files). The file is restored after each;
a run that does not end in 12 minutes is stopped and reported so.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
S = 'crates/skyzfs-local/src/inplace/state.rs'
M = 'crates/skyzfs-local/src/inplace/state/maps.rs'
I = 'crates/skyzfs-local/src/inplace.rs'
N = 'crates/skyzfs-local/src/inplace/intent.rs'

ABANDON_ALL = ("        let mut abandoned = Vec::new();\n"
               "        for _ in 0..HELD {\n"
               "            match self.abandon() {\n"
               "                Some(number) => abandoned.push(number),\n"
               "                None => break,\n"
               "            }\n"
               "        }\n"
               "        if self.held().is_empty() {\n"
               "            self.store(at::WHOLE, self.epoch());\n"
               "        }\n"
               "        abandoned\n")
RELEASE = ("        self.intent_committed(number, group);\n"
           "        self.state\n"
           "            .as_mut()\n"
           "            .is_some_and(|state| state.release(number))\n")
FOLLOWS = ("            (Place::Image(a), Place::Image(b)) | (Place::Copy(a), Place::Copy(b)) => b == a + 1,\n"
           "            _ => false,\n")
SYNC_FAILED = ("        if result.is_err() && self.uploading(number) {\n"
               "            if let Some(state) = self.state.as_mut() {\n"
               "                state.abandon_uploading(number);\n"
               "            }\n"
               "        }\n"
               "        Some(result)\n")
VERSION_FIRST = ("        let version = u32_at(8);\n"
                 "        if version != VERSION {\n"
                 "            return Err(Distrust::Version(version));\n"
                 "        }\n"
                 "        if !whole(header) {\n"
                 "            return Err(Distrust::Torn(0));\n"
                 "        }\n")

PLANTS = [
    # Question 1: abandon_all
    ('A1 a mismatch marks the open epoch first, before any cut is abandoned', S,
     [(ABANDON_ALL,
       "        let mut abandoned = Vec::new();\n"
       "        self.store(at::WHOLE, self.epoch());\n"
       "        for _ in 0..HELD {\n"
       "            match self.abandon() {\n"
       "                Some(number) => abandoned.push(number),\n"
       "                None => break,\n"
       "            }\n"
       "        }\n"
       "        abandoned\n")]),
    ('A2 a mismatch abandons the cuts and does not mark the open epoch', S,
     [("        if self.held().is_empty() {\n            self.store(at::WHOLE, self.epoch());\n        }\n        abandoned\n",
       "        abandoned\n")]),
    ('A3 a mismatch marks the epoch of the newest cut, not the open one', S,
     [("        if self.held().is_empty() {\n            self.store(at::WHOLE, self.epoch());\n",
       "        if self.held().is_empty() {\n            self.store(at::WHOLE, self.count());\n")]),
    ('A4 a mismatch abandons the oldest cut only', S,
     [("        let mut abandoned = Vec::new();\n        for _ in 0..HELD {\n",
       "        let mut abandoned = Vec::new();\n        for _ in 0..1 {\n")]),
    ('A5 a mismatch does not count the cuts it abandons', S,
     [("            match self.abandon() {\n                Some(number) => abandoned.push(number),\n                None => break,\n            }\n        }\n        if self.held().is_empty() {\n",
       "            let before = self.abandoned();\n            match self.abandon() {\n                Some(number) => {\n                    abandoned.push(number);\n                    self.store(at::ABANDONED, before);\n                }\n                None => break,\n            }\n        }\n        if self.held().is_empty() {\n")]),
    ('A6 a mismatch stops at a cut whose flush is under way', S,
     [("        for _ in 0..HELD {\n            match self.abandon() {\n                Some(number) => abandoned.push(number),\n",
       "        for _ in 0..HELD {\n            if self.held().first().is_some_and(|c| c.phase == Phase::Flushing) {\n                break;\n            }\n            match self.abandon() {\n                Some(number) => abandoned.push(number),\n")]),
    ('A7 a mismatch marks the open epoch though a cut is still held (with A6: stops at the flushing cut and marks)', S,
     [("        for _ in 0..HELD {\n            match self.abandon() {\n                Some(number) => abandoned.push(number),\n",
       "        for _ in 0..HELD {\n            if self.held().first().is_some_and(|c| c.phase == Phase::Flushing) {\n                break;\n            }\n            match self.abandon() {\n                Some(number) => abandoned.push(number),\n"),
      ("        if self.held().is_empty() {\n            self.store(at::WHOLE, self.epoch());\n",
       "        {\n            self.store(at::WHOLE, self.epoch());\n")]),
    ('A8 a start counts a cut whose abandoning a death interrupted though it is still held', S,
     [("            self.store(at::ABANDONED, self.abandoned() + u64::from(gone));\n",
       "            let _ = gone;\n            self.store(at::ABANDONED, self.abandoned() + 1);\n")]),
    ('A9 a start counts a cut that waits when a newer cut replaces it there (the count with a flush under way)', S,
     [("        for cut in self.held() {\n            if cut.phase == Phase::Flushing {\n                self.flushed(cut.number);\n            }\n        }\n",
       "        for cut in self.held() {\n            if cut.phase == Phase::Flushing {\n                let before = self.held().len();\n                self.flushed(cut.number);\n                if self.held().len() < before {\n                    self.store(at::ABANDONED, self.abandoned() + 1);\n                }\n            }\n        }\n")]),
    ('A10 an abandoned cut being uploaded does not make the cut that waits the one being uploaded', S,
     [("        if oldest.cut.phase == Phase::Uploading {\n            if let Some(waiting) = waiting {\n                self.set_phase(waiting.cut.number, Phase::Uploading);\n            }\n        }\n        self.store(at::ABANDONED, abandoned + 1);\n",
       "        let _ = waiting;\n        self.store(at::ABANDONED, abandoned + 1);\n")]),
    ('A11 is_whole_cut says no for every cut', S,
     [("        self.hold(number).is_some_and(|h| self.is_whole(&h.cut))\n",
       "        self.hold(number).is_some_and(|h| self.is_whole(&h.cut) && false)\n")]),
    ('A12 is_whole_cut says yes for every held cut', S,
     [("        self.hold(number).is_some_and(|h| self.is_whole(&h.cut))\n",
       "        self.hold(number).is_some()\n")]),
    # abandon_uploading, the copies' sync
    ('B1 a failed read for the cut being uploaded does not abandon it', I,
     [("        if let Err(e) = self.read_places(&reads, buf) {\n            if self.uploading(number) {\n",
       "        if let Err(e) = self.read_places(&reads, buf) {\n            if self.uploading(number) && self.state.is_none() {\n")]),
    ('B2 a failed sync of the copies does not abandon its cut', I,
     [(SYNC_FAILED, "        let _ = number;\n        Some(result)\n")]),
    ('B3 a sync of the copies that succeeds abandons its cut too', I,
     [("        if result.is_err() && self.uploading(number) {\n",
       "        if self.uploading(number) {\n")]),
    ('B4 a failed sync of the copies abandons the cut being uploaded, whichever it is', I,
     [(SYNC_FAILED,
       "        if result.is_err() {\n"
       "            let _ = number;\n"
       "            let first = self.held_cuts().first().map(|cut| cut.number);\n"
       "            if let (Some(first), Some(state)) = (first, self.state.as_mut()) {\n"
       "                state.abandon_uploading(first);\n"
       "            }\n"
       "        }\n"
       "        Some(result)\n")]),
    ('B5 sync_cut_copies syncs nothing and says the sync succeeded', I,
     [("        let begun = file.sync_begin(Box::new(move || {\n            if let Some(wake) = wake {\n                wake();\n            }\n        }));\n",
       "        drop(wake);\n        let begun: io::Result<()> = Err(io::Error::other(\"planted\"));\n"),
      ("            Err(e) => CopiesSync::Ended(number, Err(e)),\n",
       "            Err(_) => CopiesSync::Ended(number, Ok(())),\n")]),
    ('B6 a second sync of the copies is begun while one is under way', I,
     [("        if self.copies_sync.is_some() {\n            return Err(io::Error::new(\n                io::ErrorKind::InvalidInput,\n                \"a sync of the copies for an upload is under way\",\n",
       "        if self.copies_sync.is_some() && self.state.is_none() {\n            return Err(io::Error::new(\n                io::ErrorKind::InvalidInput,\n                \"a sync of the copies for an upload is under way\",\n")]),
    ('B7 a sync of the copies is begun for a cut that is not the one being uploaded', I,
     [("        if !self.uploading(number) {\n            return Err(io::Error::new(\n                io::ErrorKind::InvalidInput,\n                \"the cut is not the one being uploaded\",\n",
       "        if !self.uploading(number) && self.state.is_none() {\n            return Err(io::Error::new(\n                io::ErrorKind::InvalidInput,\n                \"the cut is not the one being uploaded\",\n")]),
    ('B8 a slice never says it read a copy', I,
     [("        let copied = reads.iter().any(|read| matches!(read, Place::Copy(_)));\n",
       "        let copied = reads.iter().any(|read| matches!(read, Place::Copy(_))) && self.state.is_none();\n")]),
    ('B9 a slice always says it read a copy', I,
     [("        let copied = reads.iter().any(|read| matches!(read, Place::Copy(_)));\n",
       "        let copied = !reads.is_empty();\n")]),
    # Question 2: release_cut, the state's id
    ('R1 release_cut lets the cut go before the intent map names it (the order before this pull request)', I,
     [(RELEASE,
       "        let released = self\n"
       "            .state\n"
       "            .as_mut()\n"
       "            .is_some_and(|state| state.release(number));\n"
       "        if released {\n"
       "            self.intent_committed(number, group);\n"
       "        }\n"
       "        released\n")]),
    ('R2 release_cut names a cut that waits as committed', I,
     [("            .find(|cut| cut.number == number && cut.phase == CutPhase::Uploading)\n            .map(|cut| cut.state.txg)\n",
       "            .find(|cut| cut.number == number)\n            .map(|cut| cut.state.txg)\n")]),
    ('R3 the header of the intent map does not hold the state of its base', N,
     [("            image[88..104].copy_from_slice(&base.state);\n", "")]),
    ('R4 the base names no state (zeros)', I,
     [("        intent.map.commit(intent::Base {\n            state: id,\n",
       "        let _ = id;\n        intent.map.commit(intent::Base {\n            state: [0; 16],\n")]),
    ('R5 a start that takes a state up names it anew', S,
     [("        for (i, word) in id_words(start).into_iter().enumerate() {\n            self.store(at::START + i, word);\n        }\n        self.settle();\n",
       "        for (i, word) in id_words(start).into_iter().enumerate() {\n            self.store(at::START + i, word);\n            self.store(at::STATE_ID + i, word);\n        }\n        self.settle();\n")]),
    ('R6 a state begun is given no name', S,
     [("            region.store(at::START + i, word);\n            region.store(at::STATE_ID + i, word);\n",
       "            region.store(at::START + i, word);\n")]),
    ('R7 at a commit the map names the cut in memory and writes no page', I,
     [("        if self.write_intent(false).is_err() {\n            self.fail();\n            self.settle_failed();\n        }\n    }\n}\n",
       "    }\n}\n")]),
    ('R8 the base read from the header has the number and no state', N,
     [("            state.copy_from_slice(&header[88..104]);\n", "            let _ = &mut state;\n")]),
    # Question 3: read_cut_from, its runs, the map from a block on
    ('C1 a copy is taken to follow a block of the image, and a block of the image a copy, where their numbers follow', I,
     [(FOLLOWS,
       "            (Place::Image(a), Place::Image(b)) | (Place::Copy(a), Place::Copy(b)) => b == a + 1,\n"
       "            (Place::Image(a), Place::Copy(b)) | (Place::Copy(a), Place::Image(b)) => b == a + 1,\n")]),
    ('C2 a place follows any earlier place of its file (a run over a gap)', I,
     [(FOLLOWS,
       "            (Place::Image(a), Place::Image(b)) | (Place::Copy(a), Place::Copy(b)) => b > a,\n"
       "            _ => false,\n")]),
    ('C3 a run goes on while a place follows the run\'s first, not the one before it', I,
     [("            while end < places.len() && places[end].follows(places[end - 1]) {\n",
       "            while end < places.len() && places[end].follows(places[i]) {\n")]),
    ('C4 every block is read alone', I,
     [("            while end < places.len() && places[end].follows(places[end - 1]) {\n",
       "            while end < i && places[end].follows(places[end - 1]) {\n")]),
    ('C5 a copy is read from the image at its slot', I,
     [("                Place::Copy(slot) => (open_copies(&self.dir, &mut self.copies)?, slot),\n",
       "                Place::Copy(slot) => (&mut self.home, slot),\n")]),
    ('C6 a copy that follows a copy in the next slot but one joins its run', I,
     [(FOLLOWS,
       "            (Place::Image(a), Place::Image(b)) => b == a + 1,\n"
       "            (Place::Copy(a), Place::Copy(b)) => b == a + 1 || b == a + 2,\n"
       "            _ => false,\n")]),
    ('C7 a block of the image that follows after one block skipped joins the run', I,
     [(FOLLOWS,
       "            (Place::Image(a), Place::Image(b)) => b == a + 1 || b == a + 2,\n"
       "            (Place::Copy(a), Place::Copy(b)) => b == a + 1,\n"
       "            _ => false,\n")]),
    ('C8 a whole cut\'s slice that ends at the volume\'s end says the map goes on', S,
     [("            return Some(((from..end).collect(), (end < self.blocks).then_some(end)));\n",
       "            return Some(((from..end).collect(), (end <= self.blocks).then_some(end)));\n")]),
    ('C9 blocks_from does not mask the first word below the block asked', M,
     [("                if word == first_word {\n                    bits &= u64::MAX << (from % 64);\n                }\n",
       "")]),
    ('C10 blocks_from does not mask the summary below the first word', M,
     [("            if i == first_word / 64 {\n                summary &= u64::MAX << (first_word % 64);\n            }\n",
       "")]),
    ('C11 blocks_from reads the summary from its first word on, whatever block is asked', M,
     [("        for i in first_word / 64..self.summary {\n", "        for i in 0..self.summary {\n")]),
    ('C12 blocks_from masks the summary\'s first word, not the word of the block asked', M,
     [("            if i == first_word / 64 {\n                summary &= u64::MAX << (first_word % 64);\n",
       "            if i == 0 {\n                summary &= u64::MAX << (first_word % 64);\n")]),
    ('C13 blocks_from reads the summary from the word after the block asked\'s', M,
     [("        for i in first_word / 64..self.summary {\n", "        for i in first_word / 64 + 1..self.summary {\n")]),
    ('C14 map_from lists a cut\'s first part only', S,
     [("        let maps: Vec<usize> = std::iter::once(cut.parts.first)\n            .chain(cut.parts.also)\n            .collect();\n        Some(self.maps.blocks_from(&self.region, &maps, from, limit))\n",
       "        let maps: Vec<usize> = std::iter::once(cut.parts.first).collect();\n        Some(self.maps.blocks_from(&self.region, &maps, from, limit))\n")]),
    ('C15 a slice takes one block more than the buffer holds', I,
     [("            .map_from(number, from, buf.len() / block_bytes)\n",
       "            .map_from(number, from, buf.len() / block_bytes + 1)\n")]),
    ('C16 a whole cut\'s map is listed from block 0 whatever block is asked', S,
     [("            let from = from.min(self.blocks);\n            let end = from.saturating_add(limit.max(1) as u64).min(self.blocks);\n",
       "            let from = from.min(self.blocks).min(0);\n            let end = from.saturating_add(limit.max(1) as u64).min(self.blocks);\n")]),
    # the versions
    ('V1 the start does not read the old header before the region is made again', S,
     [("                found_version = version_in(path);\n", "                found_version = None.or(version_in(path).filter(|_| false));\n")]),
    ('V2 a region of another length is refused for its version though the version is this one', I,
     [("                Some(version) if version != state::VERSION => {\n", "                Some(version) => {\n")]),
    ('V3 the header\'s version is read whatever its magic', S,
     [("    (word(at::MAGIC) == Some(MAGIC))\n", "    (word(at::MAGIC).is_some())\n")]),
    ('V4 a cut state of version 1 is read as version 2', S,
     [("        let version = region.load(at::VERSION);\n        if version != VERSION {\n",
       "        let version = region.load(at::VERSION);\n        if version != VERSION && version != 1 {\n")]),
    ('V5 the intent map\'s checksum is looked at before its version', N,
     [(VERSION_FIRST,
       "        if !whole(header) {\n"
       "            return Err(Distrust::Torn(0));\n"
       "        }\n"
       "        let version = u32_at(8);\n"
       "        if version != VERSION {\n"
       "            return Err(Distrust::Version(version));\n"
       "        }\n")]),
    ('V6 an intent map of version 1 is read as version 2', N,
     [("        let version = u32_at(8);\n        if version != VERSION {\n",
       "        let version = u32_at(8);\n        if version != VERSION && version != 1 {\n")]),
]


def tests(env):
    cmd = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=720)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', os.path.basename(ROOT) + '/target/debug/deps/skyzfs_local-'], check=False)
        return ['(did not end in 12 minutes)'], []
    out = (p.stdout + p.stderr).splitlines()
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out
                     if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    broken = [] if failed or p.returncode == 0 else [ln for ln in out if ln.startswith('error')][:3]
    return failed, broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if not ONLY:
        print(f'as the head has it: {tests(env)}', flush=True)
    for name, path, edits in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        text, ok = orig, True
        for old, new in edits:
            if text.count(old) != 1:
                print(f'{name}: NOT APPLIED ({text.count(old)} matches of {old[:60]!r})', flush=True)
                ok = False
                break
            text = text.replace(old, new)
        if not ok:
            continue
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed, broken = tests(env)
            if failed:
                verdict = f'KILLED by {len(failed)}: {", ".join(failed[:8])}'
            elif broken:
                verdict = 'DID NOT BUILD: ' + ' | '.join(broken)[:300]
            else:
                verdict = 'SURVIVED'
            print(f'{name}: {verdict}', flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    status = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True)
    print('git status --short: ' + (status.stdout.strip() or 'clean'), flush=True)


if __name__ == '__main__':
    main()
