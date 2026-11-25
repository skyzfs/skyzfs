"""Replay the publish rig's own recordings through publish_check.py's
pure functions, at one worktree's check. Run:
python3 -I replay72.py WORKTREE RECORDING_DIR [NAMED]"""
import sys

W, R = sys.argv[1], sys.argv[2]
named = int(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3] != 'none' else None
sys.path.insert(0, W + '/tests/kernel')
import publish_check as pc  # noqa: E402
import cut_daemon_check as cdc  # noqa: E402


def text(course, fn):
    with open(f'{R}/{course}/{fn}', errors='replace') as f:
        return f.read()


for course in ('prewind', 'pkill', 'pkillman', 'pkillpub'):
    ends = pc.read_ends(text(course, 'ends.tsv'))
    try:
        bad = [(k, a) for k, ok, a, _ in pc.judge_ends(course, ends) if not ok]
        print(f'{course}: judge_ends: {"all ok" if not bad else bad}')
    except pc.RigError as e:
        print(f'{course}: judge_ends: RigError: {e}')
    for t, label in pc.PUBLISHED[course].items():
        print(f'{course}: {t} commit from {label}: {str(pc.done_commit(ends.get(label, "")))[:12]}')

pub = text('prewind', 'published.tsv')
first = pc.read_published(pub, first=True)['r1'] if 'first' in pc.read_published.__code__.co_varnames \
    else pc.read_published(pub)['r1']
last = pc.read_published(pub)['r1']
cuts = pc.cut_lines(cdc.parse_log(text('prewind', 'requests.tsv'), []))
cut = cdc.read_row(text('prewind', 'rewind-row.txt'), 'rewind-row.txt')['cut']
print('prewind: row cut', cut, 'first C', first['after'], 'last C', last['after'])
print('prewind: premise with the first ask:', pc.rewind_committed(cuts, cut, first, named) or 'holds')
print('prewind: premise with the last ask: ', pc.rewind_committed(cuts, cut, last, named) or 'holds')
