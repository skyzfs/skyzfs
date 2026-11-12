"""The `withheld` control's image at its mark, against the true image at the fsync mark before it."""
import hashlib, os, sys, tempfile
sys.path.insert(0, sys.argv[1])
import logwrites, replay_check
import replay_daemon as rd
import test_replay_daemon as t

d = tempfile.mkdtemp()
log = os.path.join(d, 'log')
logwrites.write_log(log, t.SS, t.lower_log())
idx = logwrites.index(log, 0)
records = idx['records']
w = rd.withheld_schedule(records)
fm = replay_check.fsync_marks(records)
k = [n for _, n in fm].index(w['file'] + 1)
p0 = fm[k - 1][0]

def image(n, sched=None):
    p = os.path.join(d, f'img-{n}-{bool(sched)}')
    with open(p, 'wb') as f:
        f.truncate(1 << 20)
    logwrites.advance(log, idx, p, 0, n, sched)
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()[:16]

print('control: mark at schedule prefix', w['mark'], 'points', w['points'], 'file', w['file'], 'victims', w['victims'])
print('true image at the fsync mark before it (prefix %d):' % p0, image(p0))
for n in w['points']:
    print('control image at schedule prefix %d:              ' % n, image(n, w['schedule']))
