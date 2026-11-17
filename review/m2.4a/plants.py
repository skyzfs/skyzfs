#!/usr/bin/env python3
"""#110 at dcf2df3 (M2.4a): plants in the engine's upload path and in FaultStore's
after-mode, one at a time, against the tests of skynbd-store and skynbd-engine. The
file is restored after each; a run that does not end in 15 minutes is stopped and
reported so.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
E = 'crates/skynbd-engine/src/lib.rs'
F = 'crates/skynbd-store/src/fault.rs'

SWAP_AND_OPS = "                txn.extend(ops);\n                match self.refs.transact(self.vol_id, txn).await? {\n"
COMMIT_PUT = ("        let commit_hash = commit.content_hash()?;\n"
              "        self.store\n"
              "            .put(\n"
              "                &keys::commit(self.vol_id, &commit_hash),\n"
              "                commit.encode()?.into(),\n"
              "            )\n"
              "            .await?;\n")
ADOPT = "        // 6. Only now adopt the new state (a failure above leaves acked\n"

PLANTS = [
    # put_sealed
    ('A1 put_sealed counts a segment as stored without a PUT', E,
     [("            if let Err(e) = self\n                .store\n                .put(&keys::segment(self.vol_id, *id), bytes.clone())\n                .await\n            {\n                outcome = Err(e.into());\n                break;\n            }\n            self.stored_segments.push(segment_meta(*id, bytes));\n",
       "            if let Err(e) = self\n                .store\n                .exists(&keys::segment(self.vol_id, *id))\n                .await\n            {\n                outcome = Err(e.into());\n                break;\n            }\n            self.stored_segments.push(segment_meta(*id, bytes));\n")]),
    ('A2 put_sealed does not record what it PUT, so the commit does not list it', E,
     [("            self.stored_segments.push(segment_meta(*id, bytes));\n            put += 1;\n",
       "            put += 1;\n")]),
    ('A3 put_sealed keeps the bytes of what it PUT', E,
     [("        for id in self.sealed_order.drain(..put) {\n            self.pending_segments.remove(&id);\n        }\n",
       "        for id in self.sealed_order.drain(..put) {\n            let _ = id;\n        }\n")]),
    ('A4 put_sealed drops the bytes of the segment whose PUT failed', E,
     [("        for id in self.sealed_order.drain(..put) {\n            self.pending_segments.remove(&id);\n",
       "        let put = if outcome.is_err() { put + 1 } else { put };\n        for id in self.sealed_order.drain(..put) {\n            self.pending_segments.remove(&id);\n")]),
    ('A5 put_sealed seals and PUTs the open segment too', E,
     [("    pub async fn put_sealed(&mut self) -> Result<()> {\n        let mut put = 0;\n",
       "    pub async fn put_sealed(&mut self) -> Result<()> {\n        self.seal_open_segment();\n        let mut put = 0;\n")]),
    ('A6 after a failed PUT, put_sealed forgets the segments it had PUT in that call', E,
     [("            self.stored_segments.push(segment_meta(*id, bytes));\n            put += 1;\n        }\n",
       "            self.stored_segments.push(segment_meta(*id, bytes));\n            put += 1;\n        }\n        if outcome.is_err() {\n            let kept = self.stored_segments.len() - put;\n            self.stored_segments.truncate(kept);\n        }\n")]),
    # what the commit does with the segments PUT early
    ('B1 the commit lists the segments PUT early after those it PUTs itself', E,
     [("        let mut seg_metas = self.stored_segments.clone();\n        for id in &self.sealed_order {\n",
       "        let mut seg_metas = Vec::new();\n        for id in &self.sealed_order {\n"),
      ("            seg_metas.push(segment_meta(*id, bytes));\n        }\n\n        // 2. Group the overlay by shard",
       "            seg_metas.push(segment_meta(*id, bytes));\n        }\n        seg_metas.extend(self.stored_segments.clone());\n\n        // 2. Group the overlay by shard")]),
    ('B2 a commit that lands keeps its list of segments PUT early', E,
     [("        self.stored_segments.clear();\n        self.next_group = None;\n",
       "        self.next_group = None;\n")]),
    ('B3 a commit that fails forgets the segments PUT early', E,
     [("        let mut seg_metas = self.stored_segments.clone();\n",
       "        let mut seg_metas = std::mem::take(&mut self.stored_segments);\n")]),
    ('B4 a discard keeps the list of segments PUT early', E,
     [("        self.sealed_order.clear();\n        self.stored_segments.clear();\n        self.open_seg = OpenSegment::new();\n    }\n\n    /// True when no un-committed writes exist.\n",
       "        self.sealed_order.clear();\n        self.open_seg = OpenSegment::new();\n    }\n\n    /// True when no un-committed writes exist.\n")]),
    # the group
    ('C1 the group goes into no commit', E,
     [("            group: self.next_group,\n", "            group: None,\n")]),
    ('C2 a commit that lands keeps the group for the next', E,
     [("        self.stored_segments.clear();\n        self.next_group = None;\n",
       "        self.stored_segments.clear();\n")]),
    ('C3 a commit that fails loses the group', E,
     [("            group: self.next_group,\n", "            group: self.next_group.take(),\n")]),
    ('C4 a group with nothing written makes no commit', E,
     [("        if self.overlay.is_empty() && self.next_group.is_none() && ops.is_none() {\n",
       "        if self.overlay.is_empty() && ops.is_none() {\n")]),
    ('C5 ops with nothing written make no commit', E,
     [("        if self.overlay.is_empty() && self.next_group.is_none() && ops.is_none() {\n",
       "        if self.overlay.is_empty() && self.next_group.is_none() {\n")]),
    # the ops and the swap
    ('D1 the head an engine holds after a commit with ops keeps the old generation', E,
     [("                        generation: self.head.generation + 1,\n",
       "                        generation: self.head.generation,\n")]),
    ('D2 the index of an op that did not hold counts the swap', E,
     [("                            op_index: op_index - 1,\n", "                            op_index,\n")]),
    ('D3 with ops, a moved ref is reported as an op that did not hold', E,
     [("                    } => return Err(self.ref_moved(actual)),\n",
       "                    } => {\n                        let _ = actual;\n                        return Err(EngineError::OpConflict { op_index: 0 });\n                    }\n")]),
    ('D4 the ops may swap the ref the engine commits to', E,
     [("            RefOp::CasRef { name, .. } if *name == self.target_ref => Some(name),\n",
       "            RefOp::CasRef { name, .. } if *name == self.target_ref && name.is_empty() => Some(name),\n")]),
    ('D5 the swap, then the ops in a transaction of their own', E,
     [(SWAP_AND_OPS,
       "                let rest = ops;\n"
       "                let first = self.refs.transact(self.vol_id, txn).await?;\n"
       "                if first == TxnOutcome::Committed && !rest.is_empty() {\n"
       "                    let _ = self.refs.transact(self.vol_id, rest).await?;\n"
       "                }\n"
       "                match first {\n")]),
    ('D6 the ops in a transaction of their own, then the swap', E,
     [(SWAP_AND_OPS,
       "                if !ops.is_empty() {\n"
       "                    if let TxnOutcome::Conflict { op_index, .. } =\n"
       "                        self.refs.transact(self.vol_id, ops).await?\n"
       "                    {\n"
       "                        return Err(EngineError::OpConflict { op_index });\n"
       "                    }\n"
       "                }\n"
       "                match self.refs.transact(self.vol_id, txn).await? {\n")]),
    ('D7 the head is swapped before the commit object is PUT', E,
     [(COMMIT_PUT, "        let commit_hash = commit.content_hash()?;\n"),
      (ADOPT,
       "        self.store\n"
       "            .put(\n"
       "                &keys::commit(self.vol_id, &commit_hash),\n"
       "                commit.encode()?.into(),\n"
       "            )\n"
       "            .await?;\n" + ADOPT)]),
    # the shard size
    ('E1 create takes any shard size', E,
     [("        if cfg.shard_bytes == 0 || !cfg.shard_bytes.is_multiple_of(SECTOR) {\n",
       "        if cfg.shard_bytes == u64::MAX {\n")]),
    # FaultStore
    ('F1 the after-mode fails the operation before the store', F,
     [("                        FaultWhen::After => Ok(Verdict::FailAfter),\n",
       "                        FaultWhen::After => Err(StoreError::Backend(\"injected fault\".into())),\n")]),
    ('F2 the after-mode returns the operation\'s own result', F,
     [("            Verdict::FailAfter => {\n                let _ = inner.await;\n                Err(StoreError::Backend(\n                    \"injected fault (after the operation completed)\".into(),\n                ))\n            }\n",
       "            Verdict::FailAfter => inner.await,\n")]),
    ('F3 the after-mode does not poison: every later operation lands and fails', F,
     [("                    f.poisoned = true;\n                    return match f.when {\n",
       "                    f.poisoned = f.when == FaultWhen::Before;\n                    return match f.when {\n")]),
]


def tests(env):
    cmd = ['cargo', 'test', '-p', 'skynbd-store', '-p', 'skynbd-engine', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', os.path.basename(ROOT) + '/target/debug/deps/'], check=False)
        return ['(did not end in 15 minutes)'], []
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
                ok = False
                break
            text = text.replace(old, new)
        if not ok:
            print(f'{name}: NOT APPLIED (the text to replace is not there exactly once)', flush=True)
            continue
        try:
            open(full, 'w').write(text)
            failed, broken = tests(env)
        finally:
            open(full, 'w').write(orig)
        if broken:
            print(f'{name}: DID NOT BUILD: {broken}', flush=True)
        elif failed:
            print(f'{name}: KILLED by {len(failed)}: {", ".join(failed)}', flush=True)
        else:
            print(f'{name}: SURVIVED (every test passes)', flush=True)
    st = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True)
    print(f'git status --short in the checkout: {st.stdout.strip() or "(clean)"}', flush=True)


if __name__ == '__main__':
    main()
