#!/usr/bin/env python3
"""Tests of cut_plan.py: the jobs of skyzfs-cut-daemon.yml."""
import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cut_plan  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))


def stages(jobs):
    return [(j['image'], j['hold'], j['upload'], j['recordings'], j['label']) for j in jobs]


class Plan(unittest.TestCase):
    def test_a_pull_request_that_touches_no_upload_path(self):
        jobs = cut_plan.plan('pull_request', '', '', '', '', ['crates/skyzfs-blk/src/lib.rs', 'Cargo.lock'])
        self.assertEqual(stages(jobs), [('ubuntu-26.04', '1', '0', '', 'ubuntu-26.04'),
                                        ('ubuntu-26.04-arm', '1', '0', '', 'ubuntu-26.04-arm')])

    def test_a_pull_request_that_touches_the_uploader(self):
        for path in ('crates/skyzfs-upload/src/lib.rs', 'bins/skyzfsd/src/serve.rs', 'crates/skyzfs-local/x.rs',
                     'crates/skyzfs-ublk/src/a.rs', 'bins/skyzfs-nodeb/src/main.rs', 'tests/kernel/cut_watch.py',
                     'tests/kernel/cut-record.sh', 'tests/kernel/cut_daemon_check.py',
                     '.github/workflows/skyzfs-cut-daemon.yml'):
            jobs = cut_plan.plan('pull_request', '', '', '', '', ['README.md', path])
            self.assertEqual(len(jobs), 3, path)
            self.assertEqual(stages(jobs)[2], ('ubuntu-26.04', '0', '1', 'life umoved urow ukill ulease ulost',
                                               'ubuntu-26.04-upload'), path)

    def test_paths_that_only_look_alike(self):
        for path in ('crates/skyzfs-ublk-sys/src/lib.rs', 'crates/skyzfs-uploader/x', 'bins/skyzfsd-extra/a',
                     'tests/kernel/cut_watch.py.orig', '.github/workflows/skyzfs-cut-rig.yml'):
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
