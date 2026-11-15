#!/usr/bin/env python3
"""Node B (M2.T2, design §9) on one recording of the cut rig.

Usage: nodeb_check.py NAME DIR LOG BASE DEV_BYTES [LOG_DEVICE_IMAGE]

Called by tests/kernel/cut-record.sh as its CUT_CHECK, after each
recording. Environment: SKZ_NODEB (the skyzfs-nodeb binary),
SKZ_S3_ENDPOINT and SKZ_S3_BUCKET (an object store and a bucket of this
run's own), AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY.

The snapshot t is one the recording took (snaps.tsv): the middle of
those whose session has a cut before their group and one at or after
it. The published cut is the first cut of that session whose group is
at or after t's (`createtxg`): the cut that holds t. The stand-in for the
uploader (skyzfs-nodeb put) puts the volume at that cut's instant into
the object store through the engine, as a volume of its own with the
snapshot ref snap/t.

Node B is a separate process that has the bucket and nothing else. It
lists the volume's refs, reads snap/t into a file (skyzfs-nodeb get),
imports it read-only (with -m where the pool has a log device: node B
has the data device alone, as a restore would), mounts POOL@t
read-only, and compares every file it holds and its SHA-256 with what
the source held when the snapshot was taken (snap-t.sha256). Then it
exports the pool and checks every copy of every block the pool reaches
(zdb -bbbbb, the replay rig's reader). That is stronger than zdb -cc,
which reports a block only when no copy of it passes. Which ZFS snapshot
to look for is given to node B here; the manifest that will name it is
M2.6's. zdb has no -m: where the pool has a log device, the rig's zdb
gets the log device as it was created (labels, no intent log) beside
the image, or its walk aborts on the intent log's first block (B1 of
the review of #97). Every block judged is on the image from the bucket.

Node B's stages, in order (STAGES): refs, content, import, snapshot,
files, copies. Each control is a volume of its own, and node B must
catch it at a stage `EXPECTED` allows; a control that is missed, or
caught at another stage, fails the run:

- digest: the published cut, against the source's digests with one
  changed. Caught at the files: the comparison is live.

- early: the cut before, one group early. Caught at the snapshot, and
  the line requires the pool otherwise clean: it imports and every copy
  passes. Design §9's criterion asks for the snapshot's content, not a
  pool that imports, and this is why.
- lacks: the published cut without the last write before its uberblock
  write that holds a block the cut reaches.
- later: the published cut with the first write after its instant that
  overlaps a block it reaches, or, where none does (ZFS writes a new
  group's blocks to free space), with the first uberblock write of a
  later group after the instant: a later state's root over a pool that
  lacks that state's blocks. The line names the kind.
- hole: the published cut with a live block of a file as a hole, which
  the stand-in writes as zeroes explicitly (a discard in the upload).

results.tsv gets the lines, which cut_summary.py prints. Exit 0: the
published cut passed and every control was caught where expected. Exit
1: a verdict. Exit 2: the rig's own error, which is also what any
exception ends in, and a multihost pool, which node B cannot import
read-only yet.
"""
import hashlib
import os
import re
import shutil
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_check  # noqa: E402
import cut_ref  # noqa: E402
import replay_check  # noqa: E402
from cut_check import POOL, RigError  # noqa: E402
from replay_check import run  # noqa: E402

CONTROLS = ('digest', 'early', 'lacks', 'later', 'hole')
# Node B's stages, in order: the ref is listed and found, its content
# read, the pool imported, the snapshot found and mounted, its files
# compared, every copy checked.
STAGES = ('refs', 'content', 'import', 'snapshot', 'files', 'copies')
# Where node B must catch each control (review of #97, S1). early must
# also be otherwise clean: it imports and every copy passes.
EXPECTED = {
    'digest': ('files',),
    'early': ('snapshot',),
    'lacks': ('import', 'files', 'copies'),
    'later': ('import', 'files', 'copies'),
    'hole': ('import', 'files', 'copies'),
}


def read_snaps(rdir):
    """snaps.tsv -> [(name, group, session)]."""
    out = []
    path = os.path.join(rdir, 'snaps.tsv')
    if not os.path.exists(path):
        return out
    with open(path) as fh:
        text = fh.read()
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) != 3 or not f[1].isdigit() or not f[2].isdigit():
            raise RigError(f'snaps.tsv: a line that is not name, group, session: {ln.strip()!r}')
        out.append((f[0], int(f[1]), int(f[2])))
    return out


def read_sums(text):
    """sha256sum's output -> {file: digest}."""
    out = {}
    for ln in text.splitlines():
        m = re.fullmatch(r'([0-9a-f]{64}) [ *](.+)', ln)
        if not m:
            raise RigError(f'a checksum line that is not sha256sum\'s: {ln!r}')
        out[m.group(2)] = m.group(1)
    return out


def spans_of(records):
    """Each session's records -> {K: (its begin mark, the next session's begin mark or None)}."""
    begins = cut_check.marks_of(records, 'begin')
    order = sorted(begins.values())
    return {k: (b, next((x for x in order if x > b), None)) for k, b in begins.items()}


def choose(snaps, cs, ok, spans):
    """The snapshot t and the cuts around it -> (snapshot, published cut,
    the cut before it) or None. Among the cuts of the snapshot's own
    session (spans, from its begin mark to the next session's), the
    first whose group is at or after the snapshot's publishes it, and
    the one before it is a group early; a snapshot qualifies when both
    exist and may be used (ok: not stale, opens as itself). Of those
    that qualify, the middle one."""
    picks = []
    for name, group, session in snaps:
        if session not in spans:
            continue
        lo, hi = spans[session]
        span = [c for c in cs if lo < c['ub_record'] and (hi is None or c['ub_record'] < hi)]
        at = next((c for c in span if c['txg'] >= group), None)
        if at is None or not ok(at):
            continue
        k = span.index(at)
        if k == 0 or not ok(span[k - 1]) or span[k - 1]['txg'] >= group:
            continue
        picks.append(((name, group), at, span[k - 1]))
    return picks[len(picks) // 2] if picks else None


def later_uberblock(ubs, cut):
    """The first uberblock write after the cut's instant of a later group -> record number or None."""
    for u in ubs:
        if u['record'] >= cut['prefix'] and u['txg'] > cut['txg']:
            return u['record']
    return None


def live_block(bps):
    """A copy of a file's data block the cut reaches -> (offset on the device, bytes) or None."""
    for bp in bps:
        if bp['type'] == 'ZFS plain file' and bp['level'] == 0 and bp['dvas']:
            _, off, asize = bp['dvas'][0]
            return off + replay_check.DVA_OFFSET, asize
    return None


def compare(want, got):
    """The source's digests and node B's -> (ok, detail)."""
    missing = sorted(set(want) - set(got))
    extra = sorted(set(got) - set(want))
    differ = sorted(k for k in set(want) & set(got) if want[k] != got[k])
    if missing or extra or differ:
        return False, (f'{len(want)} files at the source; node B: {len(missing)} missing {missing[:3]}, '
                       f'{len(extra)} extra {extra[:3]}, {len(differ)} differ {differ[:3]}')
    return True, f'{len(got)} files, each as the source held it'


def flat(text):
    """One line for results.tsv: no tabs, no newlines."""
    return ' '.join(str(text).split())


class Seen:
    """What node B saw of one volume, stage by stage (STAGES), in order."""

    def __init__(self):
        self.stages = {}

    def put(self, stage, ok, detail):
        self.stages[stage] = (ok, flat(detail))

    def failed(self):
        """The first stage that failed, or None."""
        return next((st for st in STAGES if st in self.stages and not self.stages[st][0]), None)

    def ok(self, stage):
        return self.stages.get(stage, (False, ''))[0]

    def text(self):
        return '; '.join(f"{st} {'ok' if ok else 'FAILED'}: {d}" for st in STAGES if st in self.stages
                         for ok, d in [self.stages[st]])


def control_verdict(seen, want):
    """A control's line -> (verdict, as expected?). Caught at a stage in
    want; early must also be otherwise clean: the pool imports and every
    copy passes (review of #97, S1), which is the criterion's point."""
    st = seen.failed()
    if st is None:
        return 'missed', False
    verdict = f'caught at {st}'
    return verdict, st in want


class NodeB:
    """The stand-in's put, and node B's side: refs, get, import, compare, every copy."""

    def __init__(self, rdir, logdev):
        self.tool = os.environ.get('SKZ_NODEB', '')
        self.endpoint = os.environ.get('SKZ_S3_ENDPOINT', '')
        self.bucket = os.environ.get('SKZ_S3_BUCKET', '')
        if not (self.tool and self.endpoint and self.bucket):
            raise RigError('SKZ_NODEB, SKZ_S3_ENDPOINT and SKZ_S3_BUCKET must be set')
        self.logdev = logdev
        self.dir = os.path.join(rdir, 'nodeb')
        self.img = os.path.join(self.dir, 'img')
        self.image = os.path.join(self.img, 'nodeb.img')
        self.mnt = os.path.join(self.dir, 'mnt')
        self.alt = os.path.join(self.dir, 'alt')
        for d in (self.img, self.mnt, self.alt):
            os.makedirs(d, exist_ok=True)
        # zdb has no -m. Where the pool has a log device, the rig's zdb
        # gets the log device as it was when the pool was created (its
        # labels, no intent log) beside the image read from the bucket:
        # without it zdb's walk reads the intent log's first block on a
        # device that is not there, and aborts (review of #97, B1: run
        # 43666257621, slog, both images). It holds nothing of the cut,
        # and every block judged is on the image. Node B's import takes
        # the image alone, with -m.
        if logdev:
            rc, out, _ = run(['cp', '--sparse=always', logdev, os.path.join(self.img, 'log-device.img')])
            if rc:
                raise RigError(f'copying the log device failed: {out.strip()}')

    def tool_run(self, args, timeout=600):
        rc, out, _ = run([self.tool] + args[:1] + ['--endpoint', self.endpoint, '--bucket', self.bucket]
                         + args[1:], timeout)
        if rc in replay_check.RIG_RC:
            raise RigError(f'skyzfs-nodeb {args[0]}: {replay_check.stage(args[0], rc)}')
        return rc, out

    def put(self, volume, image, snapshot, holes=()):
        args = ['put', '--volume', volume, '--image', image, '--snapshot', snapshot]
        for o, n in holes:
            args += ['--hole', f'{o}:{n}']
        rc, out = self.tool_run(args)
        if rc:
            raise RigError(f'the stand-in could not put {volume}: exit {rc}: {out.strip()[-300:]}')
        return out

    def verify(self, volume, ref, snap, want, damaged):
        """Node B on one volume -> Seen. Every stage that can run runs:
        the every-copy check runs after a failed import or a missing
        snapshot too. damaged: a control, whose image is damaged on
        purpose, so that zdb's abort on it is a catch (as in the replay
        rig); on the published cut it is the rig's error."""
        seen = Seen()
        rc, out = self.tool_run(['refs', '--volume', volume])
        if rc:
            raise RigError(f'skyzfs-nodeb refs {volume}: exit {rc}: {out.strip()[-300:]}')
        names = [ln.split('\t')[0] for ln in out.splitlines() if '\t' in ln]
        if ref not in names:
            seen.put('refs', False, f'no ref {ref} among {names}')
            return seen
        if os.path.exists(self.image):
            os.unlink(self.image)
        rc, out = self.tool_run(['get', '--volume', volume, '--ref', ref, '--out', self.image])
        if rc == 3:
            seen.put('refs', False, f'{ref} listed, then not found')
            return seen
        seen.put('refs', True, f'{ref} among {len(names)} refs')
        if rc == 4:
            seen.put('content', False, f'{ref} is listed and its content cannot be read: {out.strip()[-300:]}')
            return seen
        if rc:
            raise RigError(f'skyzfs-nodeb get {volume} {ref}: exit {rc}: {out.strip()[-300:]}')
        seen.put('content', True, out.strip().split('\n')[-1])
        self.snapshot_files(snap, want, seen)
        rc, text, _ = run(['zdb', '-e', '-p', self.img, '-bbbbb', POOL], 600)
        failed, d2, counts = replay_check.every_copy_verdict(rc, text, self.image)
        if failed and failed.startswith('rig:') and not damaged:
            raise RigError(f'{failed}: {d2}')
        seen.put('copies', not failed, f'{failed}: {d2}' if failed else
                 f"{counts.get('blocks', 0)} blocks, {counts.get('copies', 0)} copies")
        return seen

    def snapshot_files(self, snap, want, seen):
        """Import read-only, mount POOL@snap, compare: the stages import, snapshot, files."""
        rc, out, _ = run(['zpool', 'list', '-H', '-o', 'name', POOL], 60)
        if rc == 0:
            raise RigError(f'a pool named {POOL} is still imported')
        rc, out, _ = run(['losetup', '-r', '--find', '--show', self.image])
        if rc:
            raise RigError(f'losetup: {out.strip()}')
        loop = out.strip()
        mounted = imported = False
        try:
            rc, out, _ = run(['zpool', 'import', '-d', loop, '-o', 'readonly=on', '-o', 'cachefile=none', '-N', '-f',
                              '-R', self.alt] + (['-m'] if self.logdev else []) + [POOL], 180)
            if rc in replay_check.RIG_RC:
                raise RigError(f'zpool import: {replay_check.stage("import", rc)}')
            seen.put('import', rc == 0, out.strip()[-300:] if rc else
                     'read-only' + (', without its log device (-m)' if self.logdev else ''))
            if rc:
                return
            imported = True
            rc, out, _ = run(['zfs', 'list', '-H', '-t', 'snapshot', '-o', 'name', POOL], 60)
            have = out.split()
            if f'{POOL}@{snap}' not in have:
                seen.put('snapshot', False, f'no snapshot {snap}: it has {len(have)} '
                         f'({", ".join(h.split("@")[-1] for h in have[-4:])}{", ..." if len(have) > 4 else ""})')
                return
            rc, out, _ = run(['mount', '-t', 'zfs', '-o', 'ro', f'{POOL}@{snap}', self.mnt], 60)
            if rc:
                seen.put('snapshot', False, f'mount of {POOL}@{snap}: {out.strip()[-200:]}')
                return
            mounted = True
            seen.put('snapshot', True, f'{POOL}@{snap}')
            got = {}
            for fn in sorted(os.listdir(self.mnt)):
                p = os.path.join(self.mnt, fn)
                if not os.path.isfile(p):
                    continue
                h = hashlib.sha256()
                try:
                    with open(p, 'rb') as f:
                        for b in iter(lambda: f.read(1 << 20), b''):
                            h.update(b)
                except OSError as e:
                    seen.put('files', False, f'reading {fn}: {e}')
                    return
                got[fn] = h.hexdigest()
            seen.put('files', *compare(want, got))
        finally:
            if mounted:
                run(['umount', self.mnt], 60)
            if imported:
                erc, eout, _ = run(['zpool', 'export', POOL], 120)
                if erc:
                    raise RigError(f'zpool export: {eout.strip()[-300:]}')
            run(['losetup', '-d', loop])


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    name, rdir, log, base, dev = sys.argv[1:5] + [int(sys.argv[5])]
    logdev = sys.argv[6] if len(sys.argv) == 7 else None
    t0 = time.monotonic()
    lines = []

    def put(key, verdict, detail):
        lines.append(f'{key}\t{verdict}\t{flat(detail)}')
        print(f'{name}: {key}: {verdict}: {flat(detail)}', flush=True)

    rc = 0
    try:
        nb = NodeB(rdir, logdev)
        ck = cut_check.Check(name, rdir, log, base, dev, logdev)
        ubs = cut_ref.scan(ck.records, ck.read, dev)
        if any(u.get('mmp_delay') for u in ubs):
            raise RigError('a multihost pool: node B does not import one yet. ZFS 2.4.1 refuses a read-only import '
                           'of a multihost pool that was not exported (M2.T1), so node B would need a writable import '
                           'of a copy')
        cs, _ = cut_ref.cuts(ck.records, ubs)
        if not cs:
            raise RigError('the reference finds no cut in the record')
        op = cut_ref.opens(ck.records, ck.read, dev, cs)
        stale = cut_ref.stale(cs)
        snaps = read_snaps(rdir)
        pick = choose(snaps, cs, lambda c: c['n'] not in stale and op[c['n']]['itself'], spans_of(ck.records))
        if pick is None:
            raise RigError(f'no snapshot has, in its session, a cut before its group and one at or after it '
                           f'({len(snaps)} snapshots, {len(cs)} cuts)')
        (snap, group), cut, early = pick
        sums = os.path.join(rdir, f'snap-{snap}.sha256')
        if not os.path.exists(sums):
            raise RigError(f'no snap-{snap}.sha256')
        with open(sums) as f:
            want = read_sums(f.read())
        if not want:
            raise RigError(f'snap-{snap}.sha256 lists no file')
        put('nodeb_snapshot', 'INFO', f"{POOL}@{snap}, taken in group {group} with {len(want)} files; published by "
            f"cut {cut['n']} (group {cut['txg']}, record {cut['prefix']}); one group early is cut {early['n']} "
            f"(group {early['txg']})")
        ref = 'snap/t'
        put('nodeb_zdb_device', 'INFO', 'zdb (the every-copy check) was given the image read from the bucket '
            + ('and, beside it, the log device as the pool was created (its labels, no intent log), since zdb has no '
               '-m: the rig\'s stand-in, which node B never had. Node B\'s import took the image alone, with -m.'
               if logdev else 'alone; the pool has no log device.'))

        def image_at(prefix, schedule=None):
            ck.at = None
            if schedule is None:
                ck.advance(prefix)
            else:
                ck.advance(0)
                ck.advance(len(schedule), schedule)
                ck.at = None
            return ck.image

        # The published cut.
        image_at(cut['prefix'])
        failed, _, _, bps = ck.copies()
        if failed or not bps:
            raise RigError(f"the published cut {cut['n']} does not list its blocks at the source: {failed}")
        out = nb.put(f'{name}-cut', ck.image, 't')
        put('nodeb_put', 'INFO', ' '.join(out.splitlines()[:2]))
        seen = nb.verify(f'{name}-cut', ref, snap, want, damaged=False)
        put('nodeb_cut', 'PASS' if not seen.failed() else 'FAIL', f"cut {cut['n']} through the object store: "
            + seen.text())
        if seen.failed():
            rc = 1

        # The controls: each is caught at a stage EXPECTED allows.
        got = {}
        # The comparison is live: the published cut, against the source's
        # digests with one of them changed (review of #97, S2).
        k = sorted(want)[0]
        bad = dict(want, **{k: ('0' if want[k][0] != '0' else '1') + want[k][1:]})
        got['digest'] = (nb.verify(f'{name}-cut', ref, snap, bad, damaged=False),
                         f'the published cut, against the source\'s digests with that of {k} changed')
        image_at(early['prefix'])
        nb.put(f'{name}-early', ck.image, 't')
        got['early'] = (nb.verify(f'{name}-early', ref, snap, want, damaged=True),
                        f"cut {early['n']} (group {early['txg']})")
        v = cut_check.pick_withheld(ck.records, cut, bps)
        if v is None:
            got['lacks'] = (None, 'no write before the uberblock write holds a block the cut reaches')
        else:
            image_at(0, [i for i in range(cut['prefix']) if i != v])
            nb.put(f'{name}-lacks', ck.image, 't')
            r = ck.records[v]
            got['lacks'] = (nb.verify(f'{name}-lacks', ref, snap, want, damaged=True),
                            f"without record {v} ({r['bytes']} bytes at {r['offset']})")
        v, kind = cut_check.pick_later(ck.records, cut, bps), 'a write over a block the cut reaches'
        if v is None:
            v, kind = later_uberblock(ubs, cut), 'an uberblock of a later group'
        if v is None:
            got['later'] = (None, 'nothing after the instant overlaps a block the cut reaches, and no later group '
                            'writes an uberblock')
        else:
            image_at(0, list(range(cut['prefix'])) + [v])
            nb.put(f'{name}-later', ck.image, 't')
            r = ck.records[v]
            got['later'] = (nb.verify(f'{name}-later', ref, snap, want, damaged=True),
                            f"kind: {kind}; record {v} ({r['bytes']} bytes at {r['offset']}, "
                            f"{v - cut['prefix']} records after the instant)")
        blk = live_block(bps)
        if blk is None:
            got['hole'] = (None, 'the cut reaches no file data block')
        else:
            image_at(cut['prefix'])
            nb.put(f'{name}-hole', ck.image, 't', [blk])
            got['hole'] = (nb.verify(f'{name}-hole', ref, snap, want, damaged=True),
                           f"{blk[1]} bytes at {blk[0]} (a file's block) as a hole")
        for ctl in CONTROLS:
            cseen, what = got[ctl]
            want_st = EXPECTED[ctl]
            if cseen is None:
                verdict, ok, said = 'not built', False, ''
            else:
                verdict, ok = control_verdict(cseen, want_st)
                said = cseen.text()
                if ctl == 'early' and not (cseen.ok('import') and cseen.ok('copies')):
                    ok = False
                    said += '; the pool is not otherwise clean'
            put(f'ctl_{ctl}', verdict.upper().replace(' ', '_'),
                f"{what}: {said} (expected: caught at {' or '.join(want_st)}"
                + ('; the import and every copy pass' if ctl == 'early' else '') + f"{'' if ok else '; DIFFERS'})")
            if not ok:
                rc = 1
    except RigError as e:
        put('rig_error', 'ERROR', str(e))
        rc = 2
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        put('rig_error', 'ERROR', f'{type(e).__name__}: {e}')
        rc = 2
    put('time', 'INFO', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in lines))
    work = os.path.join(rdir, 'work')
    if rc == 0 and os.path.isdir(work):
        shutil.rmtree(work)
    for d in (os.path.join(rdir, 'nodeb', 'img'),):
        if os.path.isdir(d):
            shutil.rmtree(d)
    sys.exit(rc)


if __name__ == '__main__':
    main()
