import subprocess
TREE="/home/user/skyzfs-pr85"; RIG=TREE+"/tests/kernel/control-rig.sh"
P=[("none", None, None),
   ("F5: the verdict does not look at 'sent on tag 0'",
    'if [ "$sent" = "sent on tag 0" ] && [[ $said == refused:* ]]; then RF_VERDICT=PASS; fi',
    'if [[ $said == refused:* ]]; then RF_VERDICT=PASS; fi'),
   ("H1: the pin passes on the start's word alone", "        PASS:*/PASS:*) pin", "        PASS:*/*) pin"),
   ("H2: a step that did not run is a FAIL, not the rig's error",
    '        "not run"*|*/"not run") pin refused_fetch_sent_and_refused ERROR', '        "not run"*|*/"not run") pin refused_fetch_sent_and_refused FAIL'),
   ("H3: the take-over's words never recorded", '        RF_PIN_TAKE="$RF_VERDICT: $(rf_words)"\n', "")]
for name, old, new in P:
    if old is not None:
        s=open(RIG).read()
        if s.count(old)!=1: print(name, "ANCHOR"); continue
        open(RIG,"w").write(s.replace(old,new))
    p=subprocess.run(["bash","tests/kernel/test_control_fetch.sh"],cwd=TREE,capture_output=True,text=True)
    fails=[l for l in p.stdout.splitlines() if l.startswith("FAIL")]
    oks=sum(1 for l in p.stdout.splitlines() if l.startswith("ok"))
    print(f"{name}: rc {p.returncode}; {oks} ok; " + ("; ".join(f[:120] for f in fails) if fails else "nothing fails"), flush=True)
    if old is not None: subprocess.run(["git","checkout","--","tests/kernel/control-rig.sh"],cwd=TREE)
print("clean:", subprocess.run(["git","status","--short"],cwd=TREE,capture_output=True,text=True).stdout.strip()=="")
