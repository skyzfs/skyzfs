import subprocess, re, time
R='.'  # a checkout of the branch under review
F=R+'/crates/skyzfs-blk/src/conformance.rs'
o=open(F).read()
muts={
 'p1 no guard on invalid requests in submit':("if !self.can(Capability::InvalidRequests) && self.geometry.validate(&op).is_err() {","if false {"),
 'p3 bases() always both':("""    if h.capabilities().has(Capability::Base) {
        vec![Spec::EMPTY, Spec::CLONE]""","""    if true {
        vec![Spec::EMPTY, Spec::CLONE]"""),
 'p6 power_cut_if_able never cuts (full harness loses strength)':("""        if self.can(Capability::PowerCut) {
            self.power_cut(keep)
        } else {""","""        if false {
            self.power_cut(keep)
        } else {"""),
 'p7 on_base always empty (full harness loses the clone)':("""    if h.capabilities().has(Capability::Base) {
        spec
    } else {""","""    if false {
        spec
    } else {"""),
 'p8 random programs never have an invalid request':("    if has.has(Capability::InvalidRequests) {\n        choices.push","    if false {\n        choices.push"),
 'p9 a weaker run is reported as passed':("        if left_out.is_empty() {\n            return Ok(Status::Passed);\n        }","        if true {\n            return Ok(Status::Passed);\n        }"),
 'p10 no guard on a read-only spec':("if spec.read_only && !has.has(Capability::ReadOnly) {","if false {"),
 'p11 log failures in programs without the capability':("        if has.has(Capability::FailLog) {\n            choices.push((1, Just(Step::FailLog).boxed()));","        if true {\n            choices.push((1, Just(Step::FailLog).boxed()));"),
 'p12 power cuts in programs without the capability':("        if has.has(Capability::PowerCut) {\n            choices.push((4, (0..12u64)","        if true {\n            choices.push((4, (0..12u64)"),
 'p13 guard on power_cut removed':("        if !self.can(Capability::PowerCut) {\n            return overstepped(Capability::PowerCut);\n        }",""),
 'p14 guard on fail_log removed':("        if !self.can(Capability::FailLog) {\n            return overstepped(Capability::FailLog);\n        }",""),
 'p15 required capabilities not checked':("            if let Some(lacking) = self.requires.iter().find(|c| !has.has(**c)) {","            if let Some(lacking) = self.requires.iter().find(|_| false) {"),
}
for name,(a,b) in muts.items():
    if o.count(a)!=1: print(name,': PATTERN COUNT',o.count(a),flush=True); continue
    open(F,'w').write(o.replace(a,b))
    t=time.time()
    p=subprocess.run(['cargo','test','-p','skyzfs-blk'],cwd=R,capture_output=True,text=True)
    open(F,'w').write(o)
    out=p.stdout+p.stderr
    failed=re.findall(r'^test (\S+) \.\.\. FAILED',out,re.M)
    if 'could not compile' in out: print(name,': COMPILE ERROR',[l for l in out.splitlines() if l.startswith('error')][:3],flush=True)
    else: print(f"{name}: {'NOT NOTICED' if not failed else 'noticed by '+str(len(failed))+': '+', '.join(f.split('::')[-1] for f in failed[:4])} ({time.time()-t:.0f}s)",flush=True)
