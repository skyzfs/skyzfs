#!/usr/bin/env python3
"""#107 at b6aaad8 (M2.2d): plants in the cut state's change (plan, copy, record), one
at a time, against the tests of skyzfs-local with its test hooks. The file is restored
after each; a run that does not end in 12 minutes is stopped and reported so.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
ST = 'crates/skyzfs-local/src/inplace/state.rs'
PLANTS = [
    ("A1 a part's bits are set before its records", ST,
     [("        for copy in &plan.copies[..made] {\n            let base = self.record_base(copy.slot);\n",
       "        self.set_bits(plan.from, plan.copies.get(made).map_or(plan.to, |c| c.block));\n        for copy in &plan.copies[..made] {\n            let base = self.record_base(copy.slot);\n")]),
    ('A2 after a failed run, the copies not made are recorded too', ST,
     [("        for copy in &plan.copies[..made] {\n            let base = self.record_base(copy.slot);\n",
       "        for copy in &plan.copies {\n            let base = self.record_base(copy.slot);\n"),
      ("        for copy in plan.copies[made..].iter().rev() {\n            self.free_records.push(copy.slot);\n        }\n", "")]),
    ('A3 after a failed run, the blocks before it get no bits', ST,
     [("        let block = plan.copies[made].block;\n        self.set_bits(plan.from, block);\n",
       "        let block = plan.copies[made].block;\n")]),
    ("A4 after a failed run, its first block gets no bit", ST,
     [("        let open = self.open();\n        self.maps.insert(&mut self.region, open.first, block);\n        Ok(block + 1)\n",
       "        Ok(block + 1)\n")]),
    ('A5 after a failed run, the slots of the copies not made stay taken', ST,
     [("        for copy in plan.copies[made..].iter().rev() {\n            self.free_records.push(copy.slot);\n        }\n", "")]),
    ('A6 after a failed run, the cuts that need its first block stay', ST,
     [("        self.abandon_needing(&holds, block);\n        let open = self.open();\n",
       "        let _ = &holds;\n        let open = self.open();\n")]),
    ('B1 a run takes consecutive blocks whatever their slots', ST,
     [("                if last.block + 1 == copy.block && last.slot + 1 == copy.slot {\n",
       "                if last.block + 1 == copy.block {\n")]),
    ('B2 a run takes consecutive slots whatever their blocks', ST,
     [("                if last.block + 1 == copy.block && last.slot + 1 == copy.slot {\n",
       "                if last.slot + 1 == copy.slot {\n")]),
    ('C1 an abandon while planning drops every copy planned so far', ST,
     [("            if self.unneeded(&record, &holds, all_of) {\n                self.free_records.push(copy.slot);\n",
       "            if true || self.unneeded(&record, &holds, all_of) {\n                self.free_records.push(copy.slot);\n")]),
    ('C2 after an abandon while planning, the held cuts are not read again', ST,
     [("                    self.unplan(&mut plan, gone);\n                }\n                holds = self.holds();\n                open = self.open();\n",
       "                    self.unplan(&mut plan, gone);\n                }\n")]),
    ("C3 the bound is looked at only for a part's first copy", ST,
     [("            if self.copied() + BLOCK_SIZE > self.bound() {\n                while self.copied() + BLOCK_SIZE > self.bound() {\n                    let Some(gone)",
       "            if plan.copies.is_empty() && self.copied() + BLOCK_SIZE > self.bound() {\n                while self.copied() + BLOCK_SIZE > self.bound() {\n                    let Some(gone)")]),
    ('D1 a record a dead instance made for this change is not looked for', ST,
     [("            if done || self.wanted(&holds, block).is_none() {\n",
       "            let _ = done;\n            if self.wanted(&holds, block).is_none() {\n")]),
    ('D2 the second word of a record is stored after the first', ST,
     [("            self.store_record(base + 1, copy.lo | (plan.epoch << 32));\n            self.store_record(base, copy.block + 1);\n",
       "            self.store_record(base, copy.block + 1);\n            self.store_record(base + 1, copy.lo | (plan.epoch << 32));\n")]),
]


def tests(env):
    cmd = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=720)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', os.path.basename(TARGET) + '/debug/deps/skyzfs_local-'], check=False)
        return ['(did not end in 12 minutes)'], []
    out = (p.stdout + p.stderr).splitlines()
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
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
                print(f'{name}: NOT APPLIED ({text.count(old)} matches of {old[:50]!r})', flush=True)
                ok = False
                break
            text = text.replace(old, new)
        if not ok:
            continue
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed, broken = tests(env)
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:8])}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()
