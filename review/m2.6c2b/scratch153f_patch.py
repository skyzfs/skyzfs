#!/usr/bin/env python3
"""Sixth look at #153 (M2.6c-2b), head 291746c: copy scratch153f.rs into a
scratch work tree as engine/review_scratch.rs and append its `mod` line to
engine/tests.rs. The fake itself is not changed. Never committed to the
product.

usage: python3 -I scratch153f_patch.py WORKTREE
"""
import pathlib
import shutil
import sys

ROOT = pathlib.Path(sys.argv[1])
HERE = pathlib.Path(__file__).resolve().parent
ENG = ROOT / "crates/skyzfs-agent/src/engine"
tests = ENG / "tests.rs"
tt = tests.read_text()
line = '\n#[path = "review_scratch.rs"]\nmod review_scratch;\n'
if line not in tt:
    tests.write_text(tt + line)
shutil.copy(HERE / "scratch153f.rs", ENG / "review_scratch.rs")
print("patched")
