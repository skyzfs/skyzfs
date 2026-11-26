# U19 and U20 again, alone (n2_old_sh.py), to tell a kill from a flake under load.
PLANTS = [
 ('U19 late: no asked verdict, goes on', 'tests/kernel/agent-record.sh', '    wait_point asked after 60 || { verdict asked "the agent did not reach asked:after in 60 s"; return 0; }', '    wait_point asked after 60 || true', 'bash tests/kernel/test_agent_record.sh'),
 ('U20 late: no landed verdict', 'tests/kernel/agent-record.sh', '    snaprow "$t" landed.txt || { verdict landed "snap/$t had not landed 120 s after the thaw"; return 0; }', '    snaprow "$t" landed.txt || true', 'bash tests/kernel/test_agent_record.sh'),
]
