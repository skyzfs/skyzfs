TR = 'bash tests/kernel/test_agent_record.sh'
A = 'tests/kernel/agent-record.sh'
PLANTS = [(f'N{i} null plant', A, 'agent_quiet() {\n', 'agent_quiet() {\n', TR) for i in range(5)]
