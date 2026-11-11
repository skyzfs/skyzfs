import subprocess, re, sys
R='.'  # a checkout of the branch under review
S='crates/skyzfs-local/src/store.rs'; L='crates/skyzfs-local/src/log.rs'; M='crates/skyzfs-blk/src/model.rs'
muts={
 'n1 superblock written without sync': (L,"""            .write_at(copy * SUPER_COPY_STRIDE, &next.encode())
            .and_then(|()| self.superblock.sync());""","""            .write_at(copy * SUPER_COPY_STRIDE, &next.encode());"""),
 'n4 a neighbour counts as logged whatever its record, durable or not': (S,".is_some_and(|source| source.record() < durable);",".is_some();"),
 'n6 preimage file bound to the batch end, not its start': (S,"Preimages::encode(self.log.key(), &volume, floor, &neighbours, &blocks);","Preimages::encode(self.log.key(), &volume, target, &neighbours, &blocks);"),
 'n8 lock released after the open': (S,"                _lock: lock,","                _lock: { drop(lock); None },"),
 'n9 the store own failure is not announced': (L,"""        self.failed = true;
        self.announce()
    }""","""        self.failed = true;
        write_marker(&self.dir, self.mutants)
    }"""),
 'n10 poll executes': (S,"""    fn poll(&mut self, _out: &mut Vec<Completion>) -> usize {
        0""","""    fn poll(&mut self, _out: &mut Vec<Completion>) -> usize {
        self.run(usize::MAX, _out)"""),
 'n11 restore: home not synced': (S,"""                home.write_at(number * BLOCK_SIZE, block)?;
            }
            home.sync()?;""","""                home.write_at(number * BLOCK_SIZE, block)?;
            }"""),
 'n13 block tag does not bind the start': (S,"""        input.extend_from_slice(&start.0.to_le_bytes());
        input.extend_from_slice(&number.to_le_bytes());""","""        input.extend_from_slice(&0u64.to_le_bytes());
        input.extend_from_slice(&number.to_le_bytes());"""),
 'n14 batches never split': (S,"if records > 0 && (saved + own > PREIMAGE_BATCH as u64 || records >= BATCH_RECORDS)","if false && records > 0 && (saved + own > PREIMAGE_BATCH as u64 || records >= BATCH_RECORDS)"),
 'n15 observer called twice': (L,"let mut observer = self.observer.take();","let mut observer = self.observer.take(); if let Some(Observer(call)) = observer.as_mut() { call(); }"),
 'n12 model: kill recovers nothing once the log failed, delivered or not': (M,"if self.recovering.is_some() || self.health == Health::Failed {","if self.recovering.is_some() || self.health != Health::Healthy {"),
}
sel=sys.argv[1:]
for name,(f,a,b) in muts.items():
    if sel and name.split()[0] not in sel: continue
    F=R+'/'+f; orig=open(F).read()
    if orig.count(a)!=1: print(name,': PATTERN COUNT',orig.count(a)); continue
    open(F,'w').write(orig.replace(a,b))
    p=subprocess.run(['cargo','test','-p','skyzfs-local','-p','skyzfs-blk'],cwd=R,capture_output=True,text=True)
    open(F,'w').write(orig)
    out=p.stdout+p.stderr
    failed=re.findall(r'^test (\S+) \.\.\. FAILED',out,re.M)
    if 'could not compile' in out: print(name,': COMPILE ERROR',[l for l in out.splitlines() if l.startswith('error')][:3])
    else: print(f"{name}: {'NOT NOTICED' if not failed else 'noticed by '+str(len(failed))+': '+', '.join(f.split('::')[-1] for f in failed[:5])}",flush=True)
