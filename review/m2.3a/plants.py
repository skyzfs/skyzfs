#!/usr/bin/env python3
"""#105 at ccd17dd: plants in the intent map and in CutState::changed_after, one at a
time, against the intent map's tests (`cargo test -p skyzfs-local --features test-hooks
intent`: its unit tests and its model test; nothing else calls this code yet). The file
is restored after each.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
INT = 'crates/skyzfs-local/src/inplace/intent.rs'
STATE = 'crates/skyzfs-local/src/inplace/state.rs'
RUNS = [['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast', 'intent']]
PLANTS = [
    ('A1 a sync makes durable what it covered, whatever was written since', INT,
     "self.durable[from + i] |= word & self.written[from + i];", "self.durable[from + i] |= word;"),
    ('A2 a page written with a bit cleared leaves the bit durable', INT,
     "                    self.durable[from + i] &= word;\n", ""),
    ('A3 a failed sync makes its writes durable', INT,
     "        if !ok {\n            return;\n        }\n", "        let _ = ok;\n"),
    ('A4 a sync that ends covers the writes issued after it began', INT,
     "        let Some(covered) = self.syncing.take() else {\n            return;\n        };\n",
     "        let Some(mut covered) = self.syncing.take() else {\n            return;\n        };\n"
     "        covered.extend(std::mem::take(&mut self.unsynced));\n"),
    ('A5 a bit cleared and not yet written covers a write', INT,
     ".all(|region| self.is_set(&self.bits, region) && self.is_set(&self.durable, region))",
     ".all(|region| self.is_set(&self.durable, region))"),
    ('A6 a bit set covers a write, durable or not', INT,
     ".all(|region| self.is_set(&self.bits, region) && self.is_set(&self.durable, region))",
     ".all(|region| self.is_set(&self.bits, region))"),
    ('A7 a bit is durable once its page is written, with no sync', INT,
     "                    self.durable[from + i] &= word;\n", "                    self.durable[from + i] = *word;\n"),
    ('A8 a new map is durable before it is written', INT,
     "            map.bits[word] |= bit;\n", "            map.bits[word] |= bit;\n            map.durable[word] |= bit;\n"),
    ('A9 set does not ask for the page to be written', INT,
     "            if !self.is_set(&self.durable, region) {\n                self.dirty.insert(region / PER_PAGE + 1);\n            }\n", ""),
    ('A13 the writes a failed sync covered are covered by the next sync', INT,
     "        if !ok {\n            return;\n        }\n",
     "        if !ok {\n            for (page, words) in covered {\n                self.unsynced.entry(page).or_insert(words);\n            }\n            return;\n        }\n"),
    ('A14 a page written again keeps the bits of its earlier unsynced write', INT,
     "                self.unsynced.insert(page, words);\n",
     "                let earlier = self.unsynced.insert(page, words.clone());\n"
     "                if let (Some(earlier), Some(now)) = (earlier, self.unsynced.get_mut(&page)) {\n"
     "                    for (n, e) in now.iter_mut().zip(earlier) {\n                        *n |= e;\n                    }\n                }\n"),
    ('C1 a commit does not ask for the header to be written', INT,
     "        self.base = Some(base);\n        self.dirty.insert(0);\n", "        self.base = Some(base);\n"),
    ('C2 a clear does not ask for the page to be written', INT,
     "            self.dirty.insert(region / PER_PAGE + 1);\n            cleared += 1;\n", "            cleared += 1;\n"),
    ('P1 changed_after leaves out the newer held cuts', STATE,
     "                .filter(|h| h.cut.number > number)\n", "                .filter(|_| false)\n"),
    ('P2 changed_after leaves out the open map', STATE,
     "        let parts = std::iter::once(self.open()).chain(\n", "        let parts = std::iter::empty::<Parts>().chain(\n"),
    ('C5 changed_after leaves out the second part of a map (an abandoned cut handed on)', STATE,
     "            maps.push(parts.first);\n            maps.extend(parts.also);\n", "            maps.push(parts.first);\n"),
    ('C6 changed_after leaves out the cut after the committed one', STATE,
     "                .filter(|h| h.cut.number > number)\n", "                .filter(|h| h.cut.number > number + 1)\n"),
    ('C7 changed_after leaves out the first part of a map that has two', STATE,
     "            maps.push(parts.first);\n            maps.extend(parts.also);\n",
     "            if parts.also.is_none() {\n                maps.push(parts.first);\n            }\n            maps.extend(parts.also);\n"),
    ('T1 a map with no base is taken up', INT,
     "        if self.base.is_none() {\n            return Err(Distrust::NoBase);\n        }\n", ""),
    ('T2 a bit page\'s number is not checked', INT,
     "            if u64::from_le_bytes(bits[..8].try_into().unwrap_or([0; 8])) != p {\n                return Err(Distrust::Torn(p));\n            }\n", ""),
    ('T3 the header\'s checksum is not checked', INT,
     "        if !whole(header) {\n            return Err(Distrust::Torn(0));\n        }\n", ""),
    ('T4 kept_by does not ask for the header to be written', INT,
     "        self.token = token;\n        self.dirty.insert(0);\n", "        self.token = token;\n"),
    ('T5 the volume\'s identity is not checked', INT,
     "        if header[16..32] != shape.identity {\n            return Err(Distrust::Identity);\n        }\n", ""),
    ('T6 the region size and the volume\'s length in the header are not checked', INT,
     "        if u64_at(32) != shape.region_blocks || u64_at(40) != shape.blocks {\n            return Err(Distrust::Shape);\n        }\n", ""),
    ('T7 the token is compared by its first half', INT,
     "        if found != Some(self.token) && !self.mutants.has(Mutants::INTENT_TOKEN_UNCHECKED) {\n",
     "        if found.map(|f| f[..8].to_vec()) != Some(self.token[..8].to_vec())\n            && !self.mutants.has(Mutants::INTENT_TOKEN_UNCHECKED)\n        {\n"),
    ('F1 the checksum covers the first sector only', INT,
     "    crc32c::crc32c(&page[..SUM_AT]) == sum\n", "    crc32c::crc32c(&page[..512]) == sum\n"),
]


def tests(env):
    failed, broken = [], []
    for cmd in RUNS:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        failed += [ln.split()[1].split('::')[-1] for ln in out.splitlines()
                   if ln.startswith('test ') and ln.rstrip().endswith('FAILED')]
        if p.returncode != 0 and not any(ln.startswith('test ') and ln.rstrip().endswith('FAILED') for ln in out.splitlines()):
            broken += [ln for ln in out.splitlines() if ln.startswith('error')][:3]
    return sorted(set(failed)), broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if not ONLY or ONLY == ('none',):
        failed, broken = tests(env)
        print(f'as the head has it: failed {failed}, broken {broken}', flush=True)
        if ONLY == ('none',):
            return
    for name, path, old, new in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        if orig.count(old) != 1:
            print(f'{name}: NOT APPLIED (found {orig.count(old)} times)', flush=True)
            continue
        try:
            with open(full, 'w') as f:
                f.write(orig.replace(old, new))
            failed, broken = tests(env)
            if failed:
                print(f'{name}: KILLED by {len(failed)}: {", ".join(failed)}', flush=True)
            elif broken:
                print(f'{name}: DID NOT BUILD OR RUN: ' + ' | '.join(broken)[:300], flush=True)
            else:
                print(f'{name}: SURVIVED', flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()
