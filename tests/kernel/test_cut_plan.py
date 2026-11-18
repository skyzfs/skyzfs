#!/usr/bin/env python3
"""Tests of cut_plan.py: the jobs of skyzfs-cut-daemon.yml."""
import ast
import fnmatch
import json
import os
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cut_plan  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
WORKFLOW = os.path.join(ROOT, '.github', 'workflows', 'skyzfs-cut-daemon.yml')
# The crates whose code the upload courses run, and the rig's Python
# that they run (the judge, the watcher, the summary, the plan).
UPLOAD_ROOTS = ('crates/skyzfs-upload', 'bins/skyzfsd', 'bins/skyzfs-nodeb')
UPLOAD_SCRIPTS = ('cut_daemon_check.py', 'cut_watch.py', 'cut_summary.py', 'cut_plan.py')


def imports(scripts):
    """The modules of tests/kernel the scripts import, theirs included ->
    their paths, relative to the repository."""
    seen, todo = set(), list(scripts)
    while todo:
        name = todo.pop()
        if name in seen:
            continue
        seen.add(name)
        with open(os.path.join(HERE, name)) as f:
            tree = ast.parse(f.read())
        for node in ast.walk(tree):
            mods = [a.name for a in node.names] if isinstance(node, ast.Import) else \
                [node.module] if isinstance(node, ast.ImportFrom) and node.module else []
            todo += [m + '.py' for m in mods if os.path.exists(os.path.join(HERE, m + '.py'))]
    return {'tests/kernel/' + n for n in seen}


def workflow_paths():
    """The pull request's `paths` of skyzfs-cut-daemon.yml."""
    with open(WORKFLOW) as f:
        block = f.read().split('pull_request:', 1)[1].split('push:', 1)[0]
    return [ln.strip()[2:] for ln in block.splitlines() if ln.strip().startswith('- ')]


def in_workflow(path):
    return any(path.startswith(p[:-2]) if p.endswith('/**') else fnmatch.fnmatch(path, p) for p in workflow_paths())


def toml(path):
    with open(os.path.join(ROOT, path, 'Cargo.toml'), 'rb') as f:
        return tomllib.load(f)


def closure(roots):
    """The workspace crates the roots build with (their dependencies and
    build dependencies, on every target, not the dev ones), from the
    manifests -> their directories, relative to the repository."""
    ws = {n: d['path'] for n, d in toml('.')['workspace']['dependencies'].items()
          if isinstance(d, dict) and 'path' in d}
    seen, todo = set(), list(roots)
    while todo:
        crate = todo.pop()
        if crate in seen:
            continue
        seen.add(crate)
        m = toml(crate)
        tables = [m.get('dependencies', {}), m.get('build-dependencies', {})]
        for t in m.get('target', {}).values():
            tables += [t.get('dependencies', {}), t.get('build-dependencies', {})]
        for table in tables:
            for name, spec in table.items():
                if not isinstance(spec, dict):
                    continue
                if spec.get('workspace') and name in ws:
                    todo.append(ws[name])
                elif 'path' in spec:
                    todo.append(os.path.normpath(os.path.join(crate, spec['path'])))
    return seen


def stages(jobs):
    return [(j['image'], j['hold'], j['upload'], j['recordings'], j['label']) for j in jobs]


class Plan(unittest.TestCase):
    def test_a_pull_request_that_touches_no_upload_path(self):
        jobs = cut_plan.plan('pull_request', '', '', '', '', ['crates/skynbd-nbd/src/lib.rs',
                                                             'tests/kernel/control_check.py'])
        self.assertEqual(stages(jobs), [('ubuntu-26.04', '1', '0', '', 'ubuntu-26.04'),
                                        ('ubuntu-26.04-arm', '1', '0', '', 'ubuntu-26.04-arm')])

    def test_a_pull_request_that_touches_the_uploader(self):
        for path in ('crates/skyzfs-upload/src/lib.rs', 'bins/skyzfsd/src/serve.rs', 'crates/skyzfs-local/x.rs',
                     'crates/skyzfs-ublk/src/a.rs', 'crates/skyzfs-blk/src/lib.rs', 'bins/skyzfs-nodeb/src/main.rs',
                     'crates/skynbd-engine/src/lib.rs', 'crates/skynbd-refs/src/s3.rs', 'crates/skynbd-store/Cargo.toml',
                     'crates/skynbd-format/src/x.rs', 'crates/skynbd-types/src/lib.rs', 'Cargo.toml', 'Cargo.lock',
                     'rust-toolchain.toml', 'tests/lib/gnu-dd.sh', 'tests/kernel/cut_watch.py', 'tests/kernel/cut-record.sh',
                     'tests/kernel/cut_daemon_check.py', 'tests/kernel/cut_plan.py', 'tests/kernel/cut_summary.py',
                     '.github/workflows/skyzfs-cut-daemon.yml'):
            jobs = cut_plan.plan('pull_request', '', '', '', '', ['README.md', path])
            self.assertEqual(len(jobs), 3, path)
            self.assertEqual(stages(jobs)[2], ('ubuntu-26.04', '0', '1', 'life umoved urow ukill ulease ulost',
                                               'ubuntu-26.04-upload'), path)

    def test_every_crate_under_the_uploader(self):
        """Each workspace crate the uploader, the daemon and node B build
        with, and each module of tests/kernel the rig's Python imports,
        starts the upload job and the workflow (the review of #121, B1),
        so that a new dependency cannot slip past the list."""
        crates = closure(UPLOAD_ROOTS)
        self.assertTrue({'crates/skyzfs-upload', 'crates/skynbd-engine', 'crates/skyzfs-ublk-sys'} <= crates, crates)
        for c in sorted(crates):
            with self.subTest(c):
                self.assertTrue(cut_plan.touches_upload([c + '/src/lib.rs']), c)
                self.assertTrue(in_workflow(c + '/src/lib.rs'), c)
        scripts = imports(UPLOAD_SCRIPTS)
        self.assertTrue({'tests/kernel/cut_check.py', 'tests/kernel/logwrites.py'} <= scripts, scripts)
        for p in sorted(scripts):
            with self.subTest(p):
                self.assertTrue(cut_plan.touches_upload([p]), p)
                self.assertTrue(in_workflow(p), p)

    def test_the_closure_from_cargo(self):
        """The manifests' closure is cargo's (where cargo is at hand)."""
        cargo = shutil.which('cargo')
        if not cargo:
            self.skipTest('no cargo')
        got = subprocess.run([cargo, 'metadata', '--format-version', '1', '--offline'], cwd=ROOT,
                             capture_output=True, text=True)
        if got.returncode != 0:
            self.skipTest('cargo metadata: ' + got.stderr[-200:])
        m = json.loads(got.stdout)
        pk = {p['id']: os.path.relpath(os.path.dirname(p['manifest_path']), ROOT) for p in m['packages']}
        nodes = {n['id']: n for n in m['resolve']['nodes']}
        ws = set(m['workspace_members'])
        seen, todo = set(), [i for i in ws if pk[i] in UPLOAD_ROOTS]
        while todo:
            i = todo.pop()
            if i in seen:
                continue
            seen.add(i)
            todo += [d['pkg'] for d in nodes[i]['deps'] if any(k.get('kind') in (None, 'build') for k in d['dep_kinds'])]
        self.assertEqual({pk[i] for i in seen if i in ws}, closure(UPLOAD_ROOTS))

    def test_paths_that_only_look_alike(self):
        for path in ('crates/skyzfs-ublk-sysx/src/lib.rs', 'crates/skyzfs-uploader/x', 'bins/skyzfsd-extra/a',
                     'tests/kernel/cut_watch.py.orig', '.github/workflows/skyzfs-cut-rig.yml',
                     'crates/skynbd-nbd/src/lib.rs', 'crates/skynbd-engine-x/a', 'crates/Cargo.toml',
                     'tests/libx/a.sh', 'tests/kernel/cut_plan.pyc', 'crates/rust-toolchain.toml'):
            self.assertFalse(cut_plan.touches_upload([path]), path)

    def test_a_dispatch(self):
        imgs = '["ubuntu-26.04","ubuntu-24.04-arm"]'
        self.assertEqual(stages(cut_plan.plan('workflow_dispatch', imgs, 'true', 'true', 'urow', [])),
                         [('ubuntu-26.04', '0', '1', 'urow', 'ubuntu-26.04-upload'),
                          ('ubuntu-24.04-arm', '0', '1', 'urow', 'ubuntu-24.04-arm-upload')])
        self.assertEqual(stages(cut_plan.plan('workflow_dispatch', imgs, 'false', 'true', '', [])),
                         [('ubuntu-26.04', '1', '0', '', 'ubuntu-26.04'),
                          ('ubuntu-24.04-arm', '1', '0', '', 'ubuntu-24.04-arm')])
        self.assertEqual(stages(cut_plan.plan('workflow_dispatch', '["ubuntu-26.04"]', 'false', 'false', 'life', [])),
                         [('ubuntu-26.04', '0', '0', 'life', 'ubuntu-26.04')])

    def test_a_dispatch_with_no_images(self):
        for images in ('', '[]', '"ubuntu-26.04"', '[""]', 'ubuntu-26.04'):
            with self.subTest(images), self.assertRaisesRegex(ValueError, 'the input images'):
                cut_plan.plan('workflow_dispatch', images, 'false', 'true', '', [])
        got = subprocess.run([sys.executable, os.path.join(HERE, 'cut_plan.py'), 'workflow_dispatch', '', 'false',
                              'true', ''], capture_output=True, text=True)
        self.assertEqual((got.returncode, got.stdout), (1, ''))
        self.assertIn('the input images must be a JSON list', got.stderr)
        self.assertNotIn('Traceback', got.stderr)

    def test_a_push(self):
        self.assertEqual(stages(cut_plan.plan('push', '', '', '', '', [])),
                         [('ubuntu-26.04', '1', '0', '', 'ubuntu-26.04')])

    def test_another_event(self):
        with self.assertRaises(ValueError):
            cut_plan.plan('schedule', '', '', '', '', [])

    def test_the_command(self):
        with tempfile.NamedTemporaryFile('w', suffix='.txt', delete=False) as f:
            f.write('crates/skyzfs-upload/src/lib.rs\n\n')
        try:
            out = subprocess.run([sys.executable, os.path.join(HERE, 'cut_plan.py'), 'pull_request', '', '', '', '',
                                  f.name], capture_output=True, text=True, check=True).stdout
        finally:
            os.unlink(f.name)
        self.assertEqual([j['label'] for j in json.loads(out)['include']],
                         ['ubuntu-26.04', 'ubuntu-26.04-arm', 'ubuntu-26.04-upload'])


if __name__ == '__main__':
    unittest.main()
