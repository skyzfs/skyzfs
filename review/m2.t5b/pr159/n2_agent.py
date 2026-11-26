# Plants on 2198025's agent rig changes, at 11f02ef. plant2.py WORKTREE n2_agent.py
TA = 'python3 -I tests/kernel/test_agent_check.py'
TR = 'bash tests/kernel/test_agent_record.sh'
PLANTS = [
 ('D11r destroy:after moved again', 'tests/kernel/agent_check.py',
  "         ('manifest', 'before'), ('manifest', 'after'), ('taken', 'before')}",
  "         ('destroy', 'after'), ('manifest', 'before'), ('manifest', 'after'), ('taken', 'before')}", TA),
 ('E1 arare: no line for a scenario with no point', 'tests/kernel/agent-record.sh',
  '        elif [ -z "$pts" ]; then', '        elif false; then', TR),
 ('E2 aplain: no line for a trace with no point', 'tests/kernel/agent-record.sh',
  '    elif [ -z "$points" ]; then', '    elif false; then', TR),
 ('E3 arare: the line for every scenario with points', 'tests/kernel/agent-record.sh',
  '        elif [ -z "$pts" ]; then', '        elif [ -n "$pts" ]; then', TR),
 ('E4 arare: the line to another file', 'tests/kernel/agent-record.sh',
  """no K-%s case ran\\n' "$s" "$s" >> "$RDIR/points-error.tsv\"""",
  """no K-%s case ran\\n' "$s" "$s" >> "$RDIR/points-none.tsv\"""", TR),
 ('E5 check: points-error is INFO, not ERROR', 'tests/kernel/agent_check.py',
  "            premise(f'premise_{c}_points', [f'no points from the trace of case {c}: {m}'], '')",
  "            put(f'premise_{c}_points', 'INFO', 'rig', m)", TA),
]
