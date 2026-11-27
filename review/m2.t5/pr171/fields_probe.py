"""How agent_check.read_outputs (argv[1]: tests/kernel of a worktree) reads
round lines with fields after under_way: before #172, with #172's bound=,
reordered, unknown, duplicated, prefixed, and malformed. Run with python3 -I."""
import sys
sys.path.insert(0, sys.argv[1])
import agent_check as ac  # noqa: E402
from cut_check import RigError  # noqa: E402

H = 'round 4 ok took_ms=5 late_ms=0 skipped=0 begun=t4 under_way=1'
for more in ('', ' synced=3', ' synced=3 bound=0', ' synced=0 bound=1', ' bound=1 synced=3',
             ' synced=3 bound=1 later=x', ' later=x', ' syncedx=9', ' synced_total=9 synced=3',
             ' synced=3 synced_total=9', ' bound=1', ' synced=03', ' synced=3 bound=0 bound=1',
             ' synced=3 bound=', ' synced=3  bound=0', ' synced=3 bound=0 ', ' synced=3 Bound=0',
             ' synced=3 bound2=0', ' synced=-1', ' synced=3 x=a=b', ' synced=3\tbound=0'):
    try:
        r = ac.read_outputs(H + more + '\n')['rounds'][0]
        got = f"synced={r['synced']!r} n={r['n']} under_way={r['under_way']}"
    except RigError as e:
        got = f'RigError: {str(e)[:70]}'
    except Exception as e:  # noqa: BLE001
        got = f'{type(e).__name__}: {e}'
    print(f'{more!r:40} -> {got}')
# #172's own round_line, as it prints a round that synced (program.rs at edf9a17).
line = 'round 1 ok took_ms=4200 late_ms=0 skipped=0 begun=skz-20261127T004500Z under_way=1 synced=4 bound=0'
print('#172 line ->', ac.read_outputs(line + '\n')['rounds'][0])
print('judge_synced on #172 lines ->', ac.judge_synced(
    ac.read_outputs(line + '\nround 2 ok took_ms=10 late_ms=0 skipped=0 begun=b under_way=1 synced=0 bound=0\n')['rounds'],
    {'skz-20261127T004500Z': 'c'}, ''))
