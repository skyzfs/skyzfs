import subprocess, re, sys
R='.'  # a checkout of the branch under review
F=R+'/crates/skyzfs-local/src/store.rs'
orig=open(F).read()
muts={
 'a checkpoint never invalidates preimages': [("""        if let Some(file) = self.preimages.as_mut() {
            if Preimages::invalidate(file).is_err() {
                return Err(self.fail());
            }
        }""","")],
 'b restore: no home sync before invalidate': [("""                home.write_at(number * BLOCK_SIZE, block)?;
            }
            home.sync()?;""","""                home.write_at(number * BLOCK_SIZE, block)?;
            }""")],
 'c neighbours: only before the run': [("for block in (head..start).chain(end..tail) {","for block in (head..start).chain(end..end) {")],
 'c2 neighbours: only after the run': [("for block in (head..start).chain(end..tail) {","for block in (start..start).chain(end..tail) {")],
 'd restore does not invalidate': [("""        if file.len()? >= PREIMAGE_HEADER as u64 {
            Self::invalidate(&mut file)?;
        }""","")],
 'e decode ignores block crc': [("if number >= device_blocks || word(&entry[8..12]) != crc32c::crc32c(&block) {","if number >= device_blocks {")],
 'f flush with nothing to sync does not probe': [("""            self.log.probe()
        } else {""","""            Ok(())
        } else {""")],
 'g read failure does not fail the volume': [("""            if !read {
                // A store that cannot read its own files does not go on
                // serving.
                return Err(self.fail());""","""            if !read {
                return Err(BlkError::Failed);""")],
 'h fail does not mark the log': [("""        if !self.log.is_failed() {
            self.log.mark_failed();
        }""","")],
 'i make_room without flush': [("""        self.flush()?;
        self.checkpoint().map(|_| ())""","""        self.checkpoint().map(|_| ())""")],
 'j checkpoint: write_zeroes not applied to home': [("Kind::WriteZeroes => write_zeros(home, record.offset, record.length),","Kind::WriteZeroes => Ok(()),")],
 'k rebuild: write_zeroes not indexed': [("""                    Kind::WriteZeroes => index.insert(start, blocks, Source::Zero { record: at }),
                    _ => {}""","""                    _ => {}""")],
 'l invalidate before home sync': [("""        let early = self.mutants.has(Mutants::RECLAIM_BEFORE_HOME_SYNC);
        if !early && self.home.sync().is_err() {
            return Err(self.fail());
        }""","""        let early = self.mutants.has(Mutants::RECLAIM_BEFORE_HOME_SYNC);
        if let Some(file) = self.preimages.as_mut() {
            if Preimages::invalidate(file).is_err() {
                return Err(self.fail());
            }
        }
        if !early && self.home.sync().is_err() {
            return Err(self.fail());
        }""")],
 'm create: home not synced': [("""        home.sync().map_err(&creating)?;
        dir.rename(HOME_NEW_NAME, HOME_NAME).map_err(&creating)?;""","""        dir.rename(HOME_NEW_NAME, HOME_NAME).map_err(&creating)?;""")],
 'n decode ignores volume id': [("            || &head[16..32] != volume\n","")],
 'o checkpoint failure of home write ignored': [("if replayed.is_err() || written.is_err() {","if replayed.is_err() {")],
 'p preimage file created without dir sync': [("""                dir.sync().map_err(|_| ())?;
                file""","""                file""")],
 'q served: hold change does not retry': [("""                            core.log.set_hold(hold);
                            held = false;""","""                            core.log.set_hold(hold);""")],
 'r discard indexed as zeroes': [("""            // A discard leaves the blocks as they were: it is in the log
            // for whoever uploads, and changes nothing here.
            _ => {}""","""            _ => self.index.insert(start, blocks, Source::Zero { record: appended.start }),""")],
 's failed store still checkpoints': [("""        if self.core.failed {
            return Err(BlkError::Failed);
        }
        self.core.checkpoint()""","""        self.core.checkpoint()""")],
 't home sync failure ignored': [("""        if !early && self.home.sync().is_err() {
            return Err(self.fail());
        }""","""        if !early { let _ = self.home.sync(); }""")],
 'u preimage: neighbours read after... covered check dropped': [("if !covered && neighbours.last() != Some(&block) {","if neighbours.last() != Some(&block) {")],
}
sel=sys.argv[1:]
for name,edits in muts.items():
    if sel and name.split()[0] not in sel: continue
    src=orig; ok=True
    for a,b in edits:
        if src.count(a)!=1: print(name,': PATTERN COUNT',src.count(a)); ok=False; break
        src=src.replace(a,b)
    if not ok: continue
    open(F,'w').write(src)
    p=subprocess.run(['cargo','test','-p','skyzfs-local'],cwd=R,capture_output=True,text=True)
    out=p.stdout+p.stderr
    failed=re.findall(r'^test (\S+) \.\.\. FAILED',out,re.M)
    if 'could not compile' in out: print(name,': COMPILE ERROR',[l for l in out.splitlines() if l.startswith('error')][:3])
    else: print(f"{name}: {'NOT NOTICED' if not failed else 'noticed by '+str(len(failed))+': '+', '.join(f.split('::')[-1] for f in failed[:4])}",flush=True)
open(F,'w').write(orig)
