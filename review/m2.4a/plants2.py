#!/usr/bin/env python3
"""#110 at 4a51efc (M2.4a, the next look): the plants of plants.py on the new head, as
far as they still apply, with those of put_sealed written again for its new shape, the
two that passed at dcf2df3 (B2, B3), and new ones for what the two commits changed
(L: the group at a discard and a rollback; N: put_sealed dropped at a PUT). One at a
time against the tests of skynbd-store and skynbd-engine; the file is restored after
each. Results are appended to plants2.out one by one, and a plant with a line there is
not run again.

    plants2.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import sys

import plants

E = plants.E
OLD = {name.split()[0]: (name, path, edits) for name, path, edits in plants.PLANTS}

LOOP = ("        while let Some(&id) = self.sealed_order.first() {\n"
        "            let bytes = self.pending_segments[&id].clone();\n"
        "            self.store\n"
        "                .put(&keys::segment(self.vol_id, id), bytes.clone())\n"
        "                .await?;\n"
        "            self.sealed_order.remove(0);\n"
        "            self.pending_segments.remove(&id);\n"
        "            self.stored_segments.push(segment_meta(id, &bytes));\n"
        "        }\n"
        "        Ok(())\n")
LANDED = "        self.stored_segments.clear();\n        self.next_group = None;\n        Ok(commit_hash)\n"
DISCARD = ("        self.stored_segments.clear();\n        self.next_group = None;\n"
           "        self.open_seg = OpenSegment::new();\n    }\n\n    /// True when no un-committed writes exist.\n")
ROLLBACK = ("        self.stored_segments.clear();\n        self.next_group = None;\n"
            "        self.open_seg = OpenSegment::new();\n        Ok(snap.commit)\n")

NEW = [
    # put_sealed, in its new shape
    ('A1 put_sealed counts a segment as stored without a PUT', E,
     [("                .put(&keys::segment(self.vol_id, id), bytes.clone())\n                .await?;\n            self.sealed_order.remove(0);\n",
       "                .exists(&keys::segment(self.vol_id, id))\n                .await?;\n            self.sealed_order.remove(0);\n")]),
    ('A2 put_sealed does not record what it PUT, so the commit does not list it', E,
     [("            self.pending_segments.remove(&id);\n            self.stored_segments.push(segment_meta(id, &bytes));\n",
       "            self.pending_segments.remove(&id);\n")]),
    ('A3 put_sealed keeps the bytes of what it PUT', E,
     [("            self.sealed_order.remove(0);\n            self.pending_segments.remove(&id);\n",
       "            self.sealed_order.remove(0);\n")]),
    ('A4 a segment leaves the engine before its PUT: one whose PUT fails is lost', E,
     [(LOOP,
       "        while let Some(&id) = self.sealed_order.first() {\n"
       "            let bytes = self.pending_segments[&id].clone();\n"
       "            self.sealed_order.remove(0);\n"
       "            self.pending_segments.remove(&id);\n"
       "            self.store\n"
       "                .put(&keys::segment(self.vol_id, id), bytes.clone())\n"
       "                .await?;\n"
       "            self.stored_segments.push(segment_meta(id, &bytes));\n"
       "        }\n"
       "        Ok(())\n")]),
    ('A5 put_sealed seals and PUTs the open segment too', E,
     [("    pub async fn put_sealed(&mut self) -> Result<()> {\n        while let Some(&id)",
       "    pub async fn put_sealed(&mut self) -> Result<()> {\n        self.seal_open_segment();\n        while let Some(&id)")]),
    ('A7 put_sealed goes on after a failed PUT and reports no error', E,
     [("                .put(&keys::segment(self.vol_id, id), bytes.clone())\n                .await?;\n            self.sealed_order.remove(0);\n",
       "                .put(&keys::segment(self.vol_id, id), bytes.clone())\n                .await\n                .ok();\n            self.sealed_order.remove(0);\n")]),
    # N1: a call dropped at a PUT
    ('N1 put_sealed as it was at dcf2df3: the sealed list is emptied only after the loop', E,
     [(LOOP,
       "        let mut put = 0;\n"
       "        let mut outcome = Ok(());\n"
       "        for id in &self.sealed_order {\n"
       "            let bytes = &self.pending_segments[id];\n"
       "            if let Err(e) = self\n"
       "                .store\n"
       "                .put(&keys::segment(self.vol_id, *id), bytes.clone())\n"
       "                .await\n"
       "            {\n"
       "                outcome = Err(e.into());\n"
       "                break;\n"
       "            }\n"
       "            self.stored_segments.push(segment_meta(*id, bytes));\n"
       "            put += 1;\n"
       "        }\n"
       "        for id in self.sealed_order.drain(..put) {\n"
       "            self.pending_segments.remove(&id);\n"
       "        }\n"
       "        outcome\n")]),
    ('N2 a segment is recorded as PUT before its PUT ends', E,
     [(LOOP,
       "        while let Some(&id) = self.sealed_order.first() {\n"
       "            let bytes = self.pending_segments[&id].clone();\n"
       "            self.stored_segments.push(segment_meta(id, &bytes));\n"
       "            self.store\n"
       "                .put(&keys::segment(self.vol_id, id), bytes.clone())\n"
       "                .await?;\n"
       "            self.sealed_order.remove(0);\n"
       "            self.pending_segments.remove(&id);\n"
       "        }\n"
       "        Ok(())\n")]),
    # the segments PUT early and the commit (B2 and B4 with the text around them as it is now)
    OLD['B1'],
    ('B2 a commit that lands keeps its list of segments PUT early', E,
     [(LANDED, "        self.next_group = None;\n        Ok(commit_hash)\n")]),
    OLD['B3'],
    ('B4 a discard keeps the list of segments PUT early', E,
     [(DISCARD, DISCARD.replace("        self.stored_segments.clear();\n", ""))]),
    # the group
    OLD['C1'],
    ('C2 a commit that lands keeps the group for the next', E,
     [(LANDED, "        self.stored_segments.clear();\n        Ok(commit_hash)\n")]),
    OLD['C3'], OLD['C4'], OLD['C5'],
    ('L1 a discard keeps the group', E,
     [(DISCARD, DISCARD.replace("        self.next_group = None;\n", ""))]),
    ('L2 a rollback keeps the group', E,
     [(ROLLBACK, ROLLBACK.replace("        self.next_group = None;\n", ""))]),
    # the ops and the swap, the shard size, FaultStore: as in plants.py
    OLD['D1'], OLD['D2'], OLD['D3'], OLD['D4'], OLD['D5'], OLD['D6'], OLD['D7'],
    OLD['E1'], OLD['F1'], OLD['F2'], OLD['F3'],
    # G2: what only a kill at the commit's own PUT of a segment shows
    ('G2 the commit PUTs its own segments after the head swap', E,
     [("            self.store\n                .put(&keys::segment(self.vol_id, *id), bytes.clone())\n                .await?;\n            seg_metas.push(segment_meta(*id, bytes));\n",
       "            seg_metas.push(segment_meta(*id, bytes));\n"),
      (plants.ADOPT,
       "        for id in &self.sealed_order {\n"
       "            let bytes = &self.pending_segments[id];\n"
       "            self.store\n"
       "                .put(&keys::segment(self.vol_id, *id), bytes.clone())\n"
       "                .await?;\n"
       "        }\n" + plants.ADOPT)]),
]

def main():
    """As plants.main, but each result is appended to plants2.out as it comes, and a
    plant that already has a line there is skipped: a run that is cut short is taken up
    where it stopped. It refuses to start on a checkout that is not clean."""
    import os
    import subprocess
    root, target = plants.ROOT, plants.TARGET
    here = os.path.dirname(os.path.abspath(__file__))
    out_path = os.path.join(here, 'plants2.out')
    done = open(out_path).read() if os.path.exists(out_path) else ''

    def say(line):
        with open(out_path, 'a') as f:
            f.write(line + '\n')
        print(line, flush=True)

    def clean():
        st = subprocess.run(['git', 'status', '--short'], cwd=root, capture_output=True, text=True)
        return st.stdout.strip()

    if clean():
        print(f'the checkout is not clean, nothing run: {clean()}')
        return 2
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=target)
    if 'A6 of plants.py' not in done:
        say('A6 of plants.py (after a failed PUT, the segments PUT in that call are forgotten): '
            'NOT APPLIED (put_sealed no longer counts a call\'s PUTs; A4 and A7 are the failure path now)')
    if 'as the head has it' not in done:
        say(f'as the head has it: {plants.tests(env)}')
    for name, path, edits in NEW:
        if plants.ONLY and not name.startswith(plants.ONLY):
            continue
        if f'\n{name}: ' in '\n' + done:
            continue
        full = os.path.join(root, path)
        orig = open(full).read()
        text, ok = orig, True
        for old, new in edits:
            if text.count(old) != 1:
                ok = False
                break
            text = text.replace(old, new)
        if not ok:
            say(f'{name}: NOT APPLIED (the text to replace is not there exactly once)')
            continue
        try:
            open(full, 'w').write(text)
            failed, broken = plants.tests(env)
        finally:
            open(full, 'w').write(orig)
        if broken:
            say(f'{name}: DID NOT BUILD: {broken}')
        elif failed:
            say(f'{name}: KILLED by {len(failed)}: {", ".join(failed)}')
        else:
            say(f'{name}: SURVIVED (every test passes)')
    say(f'git status --short in the checkout: {clean() or "(clean)"}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
