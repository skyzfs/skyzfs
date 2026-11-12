#!/usr/bin/env python3
"""The Python inside topology-bench.sh compiles, and `summarize` runs.

    python3 tests/bench/test_embedded_python.py

The script runs its Python from here-documents (<<'PY' ... PY). Python
compiles a whole program before it runs a line, so one bad quote stops
the summary of every row, and nothing else runs that code before a
benchmark job ends.
"""
import os
import re
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, 'topology-bench.sh')
HEREDOC = re.compile(r"<<\s*'PY'[^\n]*\n(.*?)\n\s*PY\n", re.S)


class Embedded(unittest.TestCase):
    def test_every_here_document_compiles(self):
        with open(SCRIPT) as f:
            text = f.read()
        docs = HEREDOC.findall(text)
        self.assertGreaterEqual(len(docs), 3)
        for k, doc in enumerate(docs):
            compile(doc, f'{SCRIPT} here-document {k + 1}', 'exec')

    def test_summarize_runs_on_an_empty_output(self):
        with tempfile.TemporaryDirectory() as d:
            p = subprocess.run(['bash', SCRIPT, 'summarize', d], capture_output=True, text=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stderr[-2000:])
        self.assertIn('## topology-bench', p.stdout)


if __name__ == '__main__':
    unittest.main()
