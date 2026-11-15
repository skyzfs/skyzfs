#!/usr/bin/env python3
"""#105 at 15c661b: plants in the fixed durability bookkeeping, one at a time, against
the intent map's tests. Each plant is a list of (old, new) edits of intent.rs; the file
is restored after each.

    plants2.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
INT = 'crates/skyzfs-local/src/inplace/intent.rs'
NARROW = ("                if let Some(covered) = self\n"
          "                    .syncing\n"
          "                    .as_mut()\n"
          "                    .and_then(|syncing| syncing.get_mut(&page))\n"
          "                    .filter(|_| narrowed)\n"
          "                {\n"
          "                    for (bits, word) in covered.iter_mut().zip(&words) {\n"
          "                        *bits &= word;\n"
          "                    }\n"
          "                }\n")
PLANTS = [
    ('N1 a page written while a sync runs leaves that sync\'s record as it was',
     [("                        *bits &= word;\n", "                        let _ = (bits, word);\n")]),
    ('N2 the first draft: the sync makes durable what it covered and the last write has set',
     [("    syncing: Option<BTreeMap<u64, Vec<u64>>>,\n    mutants: Mutants,\n}",
       "    syncing: Option<BTreeMap<u64, Vec<u64>>>,\n    orig: BTreeMap<u64, Vec<u64>>,\n    mutants: Mutants,\n}"),
      ("            syncing: None,\n            mutants,\n", "            syncing: None,\n            orig: BTreeMap::new(),\n            mutants,\n"),
      ("        let covered = std::mem::take(&mut self.unsynced);\n",
       "        let covered = std::mem::take(&mut self.unsynced);\n        self.orig = covered.clone();\n"),
      (NARROW,
       "                let orig = self.orig.get(&page).cloned();\n"
       "                if let (Some(covered), Some(orig)) = (\n"
       "                    self.syncing\n"
       "                        .as_mut()\n"
       "                        .and_then(|syncing| syncing.get_mut(&page))\n"
       "                        .filter(|_| narrowed),\n"
       "                    orig,\n"
       "                ) {\n"
       "                    for ((bits, word), o) in covered.iter_mut().zip(&words).zip(&orig) {\n"
       "                        *bits = o & word;\n"
       "                    }\n"
       "                }\n")]),
    ('N3 a sync that ends covers the writes issued after it began',
     [("        let Some(covered) = self.syncing.take() else {\n            return;\n        };\n",
       "        let Some(mut covered) = self.syncing.take() else {\n            return;\n        };\n"
       "        covered.extend(std::mem::take(&mut self.unsynced));\n")]),
    ('N4 kept_by does not ask for the header to be written',
     [("        self.token = token;\n        self.dirty.insert(0);\n", "        self.token = token;\n")]),
    ('N5 the token is compared by its first half',
     [("        if found != Some(self.token) && !self.mutants.has(Mutants::INTENT_TOKEN_UNCHECKED) {\n",
       "        if found.map(|f| f[..8].to_vec()) != Some(self.token[..8].to_vec())\n            && !self.mutants.has(Mutants::INTENT_TOKEN_UNCHECKED)\n        {\n")]),
    ('N6 the narrowing is of the page before (another page\'s record)',
     [("                    .and_then(|syncing| syncing.get_mut(&page))\n", "                    .and_then(|syncing| syncing.get_mut(&(page + 1)))\n")]),
    ('N7 a page written while a sync runs is covered by that sync (its record replaced)',
     [("                        *bits &= word;\n", "                        *bits = *word;\n")]),
    ('N8 a page written with a bit cleared leaves the bit durable',
     [("                    self.durable[from + i] &= word;\n", "")]),
]


def tests(env):
    p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast', 'intent'],
                       cwd=ROOT, capture_output=True, text=True, env=env)
    out = (p.stdout + p.stderr).splitlines()
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    broken = [] if failed or p.returncode == 0 else [ln for ln in out if ln.startswith('error')][:3]
    return failed, broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    full = os.path.join(ROOT, INT)
    if not ONLY:
        print(f'as the head has it: {tests(env)}', flush=True)
    for name, edits in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        orig = open(full).read()
        text = orig
        bad = [old for old, _ in edits if orig.count(old) != 1]
        if bad:
            print(f'{name}: NOT APPLIED ({len(bad)} edits did not match once)', flush=True)
            continue
        for old, new in edits:
            text = text.replace(old, new)
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed, broken = tests(env)
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed)}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()
