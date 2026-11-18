#!/usr/bin/env python3
"""The jobs of skyzfs-cut-daemon.yml: which images, with which stage.

Usage: cut_plan.py EVENT IMAGES UPLOAD HOLD RECORDINGS [CHANGED_FILES]

EVENT is github.event_name; IMAGES, UPLOAD, HOLD and RECORDINGS are the
dispatch's inputs (empty on other events); CHANGED_FILES is a file that
lists the pull request's changed paths, one a line. Prints the matrix as
JSON, `{"include": [...]}`, each entry with `image`, `hold` and `upload`
('1' or '0'), `recordings` and `label` (the job's name and the artifact's
suffix).

- A dispatch: its images, with its inputs. The upload stage takes the
  place of the held cuts' stage (the daemon refuses `release` while it
  uploads).
- A push (a testing branch): ubuntu-26.04, the held cuts' stage.
- A pull request: ubuntu-26.04 and ubuntu-26.04-arm, the held cuts'
  stage; and where it changes the uploader, the daemon that drives it,
  the rig's upload files or this workflow (UPLOAD_PATHS; the lead's
  decision on the review of #119, G7), one job more on ubuntu-26.04
  with courses (a) to (d) (PR_UPLOAD_RECORDINGS). Courses (e) to (g)
  run by dispatch only.
"""
import json
import sys

PR_IMAGES = ['ubuntu-26.04', 'ubuntu-26.04-arm']
PR_UPLOAD_IMAGE = 'ubuntu-26.04'
PR_UPLOAD_RECORDINGS = 'life umoved urow ukill ulease ulost'
UPLOAD_PATHS = (
    'crates/skyzfs-upload/',
    'bins/skyzfsd/',
    'crates/skyzfs-local/',
    'crates/skyzfs-ublk/',
    'bins/skyzfs-nodeb/',
    'tests/kernel/cut_watch.py',
    'tests/kernel/cut-record.sh',
    'tests/kernel/cut_daemon_check.py',
    '.github/workflows/skyzfs-cut-daemon.yml',
)


def touches_upload(changed):
    """Whether one of the changed paths is under UPLOAD_PATHS. A path
    ending in / names a directory, any other one file."""
    return any(c == p or (p.endswith('/') and c.startswith(p)) for c in changed for p in UPLOAD_PATHS)


def entry(image, hold, upload, recordings):
    label = image + ('-upload' if upload else '')
    return {'image': image, 'hold': '1' if hold else '0', 'upload': '1' if upload else '0',
            'recordings': recordings, 'label': label}


def plan(event, images, upload, hold, recordings, changed):
    if event == 'workflow_dispatch':
        up = upload == 'true'
        return [entry(i, hold == 'true' and not up, up, recordings) for i in json.loads(images)]
    if event == 'push':
        return [entry('ubuntu-26.04', True, False, '')]
    if event == 'pull_request':
        jobs = [entry(i, True, False, '') for i in PR_IMAGES]
        if touches_upload(changed):
            jobs.append(entry(PR_UPLOAD_IMAGE, False, True, PR_UPLOAD_RECORDINGS))
        return jobs
    raise ValueError(f'no plan for the event {event!r}')


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    event, images, upload, hold, recordings = sys.argv[1:6]
    changed = []
    if len(sys.argv) == 7:
        with open(sys.argv[6]) as f:
            changed = [ln.strip() for ln in f if ln.strip()]
    print(json.dumps({'include': plan(event, images, upload, hold, recordings, changed)}))


if __name__ == '__main__':
    main()
