"""Textual plants on the publish rig at 72a3b5e (the check, the agent and
cut-record.sh), one at a time, each file restored from its saved text in
`finally`. Run: WORKTREE=<path> python3 -I rigplants72.py"""
import os, subprocess, sys

W = os.environ["WORKTREE"]
K = os.path.join(W, "tests", "kernel")
PY = ["test_publish_check.py", "test_publish_check_main.py"]
AG = ["test_publish_agent.py", "test_publish_check_main.py"]
SH = ["test_cut_record.sh"]
P = [
 # The check: EXPECTED and PUBLISHED, both directions.
 ("K1 prewind's r1 expected done again", "publish_check.py",
  "        'r1': ('refused rewound',),", "        'r1': ('done ',),", PY),
 ("K2 pkill's tk-again expected accepted again", "publish_check.py",
  "        'tk-again': ('refused rewound',),", "        'tk-again': ('accepted',),", PY),
 ("K3 p1-end widened to taken or done", "publish_check.py",
  "        'p1-end': ('refused taken',),", "        'p1-end': ('refused taken', 'done '),", PY),
 ("K4 p1 judged by p1-end", "publish_check.py",
  "    'pkillpub': {'p1': 'p1-look'},", "    'pkillpub': {'p1': 'p1-end'},", PY),
 ("K5 read_published ignores first", "publish_check.py",
  "        if first and f[0] in out:\n            continue\n", "", PY),
 ("K6 prewind's premise of the last ask", "publish_check.py",
  "            r1 = read_published(text_of('published.tsv'), first=True).get('r1')\n",
  "            r1 = published.get('r1')\n", PY),
 ("K7 a snap answer names no commit", "publish_check.py",
  "((len(f) == 2 and f[0] == 'done') or (len(f) == 7 and f[0] == 'snap'))",
  "(len(f) == 2 and f[0] == 'done')", PY),
 ("K8 a snap answer of any length names a commit", "publish_check.py",
  "(len(f) == 7 and f[0] == 'snap')", "(f[0] == 'snap')", PY),
 ("K9 the found-pin reads p1-end", "publish_check.py",
  "                end = done_commit(ends.get(PUBLISHED[name][t], ''))\n",
  "                end = done_commit(ends.get(f'{t}-end', ''))\n", PY),
 ("K10 the premise takes a cut opening as itself", "publish_check.py",
  "    elif any(c[2] for c in same):\n        bad.append(f'cut {number} (group {group}) opens as itself: no daemon would pass it over')\n",
  "", PY),
 # The agent.
 ("A1 a snap's state not checked", "publish_agent.py",
  "not (is_hex(fields['state'], 32) and is_hex(fields['manifest-hash'], 64)",
  "not (is_hex(fields['manifest-hash'], 64)", AG),
 ("A2 look asks publish-progress", "publish_agent.py",
  "        return self.ask('look', '--name', name)\n", "        return self.ask('publish-progress', '--name', name)\n", AG),
 ("A3 an unreadable's commit not checked", "publish_agent.py",
  "    if verb in ('done', 'snap', 'unreadable') and not is_hex(fields['commit'], 64):",
  "    if verb in ('done', 'snap') and not is_hex(fields['commit'], 64):", AG),
 ("A4 ask look runs progress", "publish_agent.py",
  "'look': agent.look}[req]", "'look': agent.progress}[req]", AG),
 ("A5 manifest-len not checked as digits", "publish_agent.py",
  "for k in ('number', 'group', 'manifest-len'))", "for k in ('number', 'group'))", AG),
 # cut-record.sh.
 ("S1 panew asks with the old count", "cut-record.sh",
  "    c=$(sed -n 's/.* count=\\([0-9]*\\) .*/\\1/p' <<< \"$out\")\n",
  "    c=$(preq \"$2\" 5)\n", SH),
 ("S2 panew records the opposite way", "cut-record.sh",
  "    [ \"${3:-}\" != record ] || printf '%s\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$2\" \"$g\"",
  "    [ \"${3:-}\" = record ] || printf '%s\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$2\" \"$g\"", SH),
 ("S3 past_top stops at any cut", "cut-record.sh",
  "        [[ $out == *\" opens=itself\" ]]", "        [[ $out == *\" opens=\"* ]]", SH),
 ("S4 past_top does not sync", "cut-record.sh",
  "        z 60 zpool sync \"$POOL\"\n        sleep 0.5\n    done\n", "        sleep 0.5\n    done\n", SH),
 ("S5 prewind without past_top", "cut-record.sh",
  "    past_top 600\n", "", SH),
 ("S6 prewind's r1 anew not recorded", "cut-record.sh",
  "    panew r1-anew r1 record\n", "    panew r1-anew r1\n", SH),
 ("S7 pkill without panew", "cut-record.sh",
  "    panew tk-anew tk record\n", "", SH),
 ("S8 pkillpub's p1 anew recorded", "cut-record.sh",
  "        panew \"$t-anew\" \"$t\"\n        pwait", "        panew \"$t-anew\" \"$t\" record\n        pwait", SH),
 ("S9 pkillpub without look", "cut-record.sh",
  "        pask \"$t-look\" look \"$t\"\n", "", SH),
 ("S10 pkillman's m1 anew not recorded", "cut-record.sh",
  "        panew \"$t-anew\" \"$t\" record\n", "        panew \"$t-anew\" \"$t\"\n", SH),
 ("S11 pkillat without the ask again", "cut-record.sh",
  "    pagain \"$t-again\" \"$t\" \"$(preq \"$t\" 2)\"\n", "", SH),
 ("S12 prewind: r1 anew before the store comes back", "cut-record.sh",
  "    pflow r1 r1 --limit-s 300\n    bash -c \"$S3_UP\"",
  "    pflow r1 r1 --limit-s 300\n    panew r1-anew r1 record\n    bash -c \"$S3_UP\"", SH),
]
for name, path, old, new, tests in P:
    f = os.path.join(K, path)
    with open(f) as h:
        saved = h.read()
    if saved.count(old) != 1:
        print(f"{name}: NOT APPLIED ({saved.count(old)} matches)", flush=True)
        continue
    try:
        with open(f, "w") as h:
            h.write(saved.replace(old, new))
        failed = []
        for t in tests:
            cmd = ["bash", t] if t.endswith(".sh") else [sys.executable, "-I", t]
            try:
                r = subprocess.run(cmd, cwd=K, capture_output=True, text=True, timeout=600)
                rc, out = r.returncode, r.stdout + r.stderr
            except subprocess.TimeoutExpired:
                rc, out = "timeout", ""
            if rc != 0:
                lines = [l for l in out.splitlines() if l.startswith(("FAIL:", "ERROR:", "not ok", "FAILED"))]
                failed.append(f"{t} rc={rc} {lines[:3]}")
        print(f"{name}: {'KILLED ' + '; '.join(failed) if failed else 'SURVIVED'}", flush=True)
    finally:
        with open(f, "w") as h:
            h.write(saved)
