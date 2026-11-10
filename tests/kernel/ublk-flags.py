#!/usr/bin/env python3
"""Render what tests/kernel/ublk-probe.sh found, as markdown.

    ublk-flags.py [--history] PROBE_OUT_DIR...

One column per probed kernel. The first table is the findings; the
second is every UBLK_F_* feature flag against whether that kernel
reported it through UBLK_U_CMD_GET_FEATURES.

Flag names and bits are read from header files, never from a list kept
here: each probe's own /usr/include/linux/ublk_cmd.h, and with
--history the upstream header at every release tag from v6.0 on, which
also gives the first release whose header defines each flag.
"""
import os
import re
import sys
import urllib.error
import urllib.request

RAW = 'https://raw.githubusercontent.com/torvalds/linux/{ref}/include/uapi/linux/ublk_cmd.h'
DEFINE = re.compile(r'^\s*#\s*define\s+(UBLK_F_\w+)\s+\(\s*1U?L?L?\s*<<\s*(\d+)\s*\)', re.M)

# Findings in display order: key in results.tsv, row label.
ROWS = [
    ('os', 'Runner image OS'),
    ('kernel', 'Kernel'),
    ('cpus_mem', 'Machine'),
    ('config_ublk', 'Kernel config'),
    ('ublk_drv', '`ublk_drv` loads'),
    ('ublk_module_file', '`ublk_drv` module file'),
    ('ublk_params', '`ublk_drv` parameters'),
    ('ublk_features', 'Feature mask (`UBLK_U_CMD_GET_FEATURES`)'),
    ('liburing', 'liburing (package)'),
    ('zfs_userland', 'ZFS userland (package)'),
    ('zfs_module', 'ZFS module'),
    ('zfs_module_file', 'ZFS module file'),
    ('mod_dm_log_writes', '`dm-log-writes`'),
    ('mod_dm_flakey', '`dm-flakey`'),
    ('mod_dm_dust', '`dm-dust`'),
    ('mod_dm_delay', '`dm-delay`'),
    ('mod_dm_snapshot', '`dm-snapshot`'),
    ('ublk_packages', 'ublk server in the package archive'),
    ('ublk_server', 'ublk server used'),
    ('serve_file', 'File served as `/dev/ublkb0` (loop target)'),
    ('ublk_dev_flags', 'Flags of that device'),
    ('ublk_target', 'Target as the server reports it'),
    ('queue_limits', 'Queue limits'),
    ('nr_hw_queues', 'Hardware queues'),
    ('raw_io', 'Raw write and read-back'),
    ('serve_again', 'Served again for ZFS'),
    ('zpool_create', '`zpool create`, `ashift=12`, with a log device'),
    ('vdev_path', 'Data vdev path'),
    ('vdev_partitioned', 'Did ZFS partition the device'),
    ('vdev_ashift', 'ashift of the data vdev'),
    ('zfs_write', 'Write 256 MiB and 200 synchronous 8 KiB writes'),
    ('slog_used', 'Log device used'),
    ('zpool_sync', '`zpool sync`'),
    ('zpool_scrub', '`zpool scrub -w`'),
    ('scrub_result', 'Scrub result'),
    ('zpool_export', '`zpool export`'),
    ('zpool_import', '`zpool import`'),
    ('verify_after_import', 'Checksums after import'),
    ('zpool_trim', '`zpool trim -w`'),
    ('backing_file_after_trim', 'Backing file after trim'),
    ('pool_after', 'Pool state at the end'),
    ('zdb_labels', 'Labels `zdb -l` reads from the device'),
    ('flush_requests', 'Flush requests during `zpool sync`'),
    ('zinject_from', '`zinject` comes from'),
    ('zinject_flush', '`zinject -e io -T flush` on the data vdev'),
    ('zinject_sync', 'Failing flush: `zpool sync`'),
    ('zinject_pool_state', 'Failing flush: pool state'),
    ('zinject_events', 'Failing flush: `zpool events` so far'),
    ('zinject_after_clear', 'Failing flush: after removing the fault'),
]
for _p, _t in (('rec', 'Recovery with reissue (`-r 1 -i 1`)'), ('rec0', 'Recovery without reissue (`-r 1 -i 0`)')):
    ROWS += [
        (_p + '_export', _t + ': export'),
        (_p + '_serve', _t + ': served'),
        (_p + '_dev_flags', _t + ': device flags'),
        (_p + '_import', _t + ': import'),
        (_p + '_server_pid', _t + ': server pid killed with `kill -9`'),
        (_p + '_state_after_kill', _t + ': device state after the kill'),
        (_p + '_inflight_at_kill', _t + ': requests in flight then (read, write)'),
        (_p + '_recover', _t + ': `ublk recover`'),
        (_p + '_recover_ms', _t + ': `ublk recover` took, ms'),
        (_p + '_state_after_recover', _t + ': device state afterwards'),
        (_p + '_pool_after_recover', _t + ': pool afterwards'),
        (_p + '_zpool_clear', _t + ': `zpool clear`'),
        (_p + '_pool_after_clear', _t + ': pool after `zpool clear`'),
        (_p + '_writer', _t + ': the fsync writer'),
        (_p + '_scrub', _t + ': scrub'),
        (_p + '_scrub_result', _t + ': scrub result'),
        (_p + '_verify', _t + ': earlier files'),
        (_p + '_outcome', _t + ': outcome'),
        (_p + '_export_end', _t + ': final export'),
    ]
PINS = [
    ('pin_no_partition_table', 'No partition table on the vdev (§6.2)'),
    ('pin_label_at_byte_0', 'First label at byte 0 (§4.2)'),
    ('pin_flush_delivered', 'Flushes reach the ublk queue (§4.2)'),
    ('pin_recovery_reissue', 'Daemon loss with reissue is invisible to ZFS (§4.2)'),
]


def flags_in(text):
    return {name: int(bit) for name, bit in DEFINE.findall(text)}


def fetch(ref):
    try:
        with urllib.request.urlopen(RAW.format(ref=ref), timeout=30) as r:
            return r.read().decode('utf-8', 'replace')
    except (urllib.error.URLError, OSError):
        return None


def version_key(tag):
    return tuple(int(x) for x in tag.lstrip('v').split('.'))


def upstream_history():
    """-> ({flag: bit}, {flag: first tag}, [tags read]) from the upstream headers."""
    tags = [f'v6.{n}' for n in range(0, 30)] + [f'v7.{n}' for n in range(0, 20)]
    bits, first, seen = {}, {}, []
    for tag in tags:
        text = fetch(tag)
        if text is None:
            continue
        seen.append(tag)
        for name, bit in flags_in(text).items():
            bits.setdefault(name, bit)
            first.setdefault(name, tag)
    text = fetch('master')
    if text is not None:
        for name, bit in flags_in(text).items():
            bits.setdefault(name, bit)
            first.setdefault(name, 'after ' + seen[-1] if seen else 'master')
    return bits, first, seen


def cell(v):
    return (v or '').replace('|', '\\|') or '–'


def main():
    args = sys.argv[1:]
    history = '--history' in args
    dirs = [a for a in args if a != '--history' and os.path.isdir(a)]
    probes = []
    for d in sorted(dirs):
        res = {}
        path = os.path.join(d, 'results.tsv')
        if os.path.exists(path):
            for line in open(path, encoding='utf-8', errors='replace'):
                k, _, v = line.rstrip('\n').partition('\t')
                res[k] = v
        hdr = os.path.join(d, 'ublk_cmd.distro.h')
        distro = flags_in(open(hdr, encoding='utf-8', errors='replace').read()) if os.path.exists(hdr) else {}
        probes.append({'dir': d, 'res': res, 'distro': distro})
    if not probes:
        print('No probe results found.')
        return

    names = [p['res'].get('kernel', os.path.basename(p['dir'])) for p in probes]
    print('## SkyZFS kernel probe\n')
    pinned = [(k, label) for k, label in PINS if any(p['res'].get(k) for p in probes)]
    if pinned:
        print('**Pinned properties.** A FAIL fails the probe.\n')
        print('| | ' + ' | '.join(names) + ' |')
        print('|---|' + '---|' * len(probes))
        for key, label in pinned:
            print(f'| {label} | ' + ' | '.join(cell(p['res'].get(key)) for p in probes) + ' |')
        print('\n**Findings.**\n')
    print('| | ' + ' | '.join(names) + ' |')
    print('|---|' + '---|' * len(probes))
    known = {k for k, _ in ROWS} | {k for k, _ in PINS}
    extra = sorted({k for p in probes for k in p['res']} - known)
    for key, label in ROWS + [(k, k) for k in extra]:
        vals = [p['res'].get(key) for p in probes]
        if any(vals):
            print(f'| {label} | ' + ' | '.join(cell(v) for v in vals) + ' |')

    bits, first, seen = upstream_history() if history else ({}, {}, [])
    for p in probes:
        for name, bit in p['distro'].items():
            bits.setdefault(name, bit)
    print('\n### ublk feature flags: kernel against flag\n')
    print('`yes` means the running kernel set the bit in its answer to `UBLK_U_CMD_GET_FEATURES`. '
          'Names and bits are read from `include/uapi/linux/ublk_cmd.h`'
          + (f', upstream at {seen[0]} to {seen[-1]} and at master' if seen else ', as installed on each runner')
          + '. `in header` is whether that runner\'s own `linux-libc-dev` header defines the flag.\n')
    head = '| flag | bit |' + (' first upstream release with it in the header |' if history else '')
    for n in names:
        head += f' {n} | in header |'
    print(head)
    print('|---|---|' + ('---|' if history else '') + '---|---|' * len(probes))
    masks = []
    for p in probes:
        try:
            masks.append(int(p['res'].get('ublk_features', ''), 16))
        except ValueError:
            masks.append(None)
    for name, bit in sorted(bits.items(), key=lambda kv: (kv[1], kv[0])):
        row = f'| `{name}` | {bit} |' + (f" {first.get(name, 'not upstream')} |" if history else '')
        for p, m in zip(probes, masks):
            got = '?' if m is None else ('yes' if m >> bit & 1 else 'no')
            row += f" {got} | {'yes' if name in p['distro'] else 'no'} |"
        print(row)
    named = 0
    for bit in bits.values():
        named |= 1 << bit
    for n, m in zip(names, masks):
        if m is None:
            print(f'\n{n}: no feature mask was read.')
        elif m & ~named:
            print(f'\n{n}: bits set that no header read here names: {m & ~named:#x}.')
    if history and not seen:
        print('\nThe upstream headers could not be fetched; the release column is empty.')


if __name__ == '__main__':
    main()
