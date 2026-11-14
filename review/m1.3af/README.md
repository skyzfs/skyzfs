# M1.3af (#76): the pool wake test pushes only when every thread is asleep

Review posted at a71a075 (issuecomment-6872684031): can merge once CI is
green, nothing blocks; one suggestion (say in futile_wakes' doc that 20 ms
is a margin).

Experiment (exp76.sh, output exp76.txt): a throwaway #[ignore] test
appended at a71a075, not committed, 2000 pool starts per cell:
gap 0 idle 42/2000 futile; 20 ms idle 0/2000; under 8 busy loops on 4
CPUs 0/2000 at both gaps. #76's test 200/200 under the busy loops.
