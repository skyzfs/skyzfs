import subprocess, sys, re
U='crates/skyzfs-ublk/src/'; L='crates/skyzfs-local/src/'
P=[
("store: open_locked lets go of the lock when the open succeeds","skyzfs-local",[(L+'store.rs',"            Ok((mut store, recovery)) => {\n                store.core._lock = Some(lock);","            Ok((mut store, recovery)) => {\n                store.core._lock = None;\n                drop(lock);")]),
("detach: the lock is let go after the first look","skyzfs-ublk",[(U+'lifecycle.rs',"    let Some(located) = locate(control, records)? else {\n        return Ok(Detached::Nothing);\n    };\n    let number = located.number;\n    if located.whose","    let Some(located) = locate(control, records)? else {\n        return Ok(Detached::Nothing);\n    };\n    drop(_held);\n    let number = located.number;\n    if located.whose")]),
("lifecycle: the lock is let go before a failed volume's device is deleted when the record cannot be read","skyzfs-ublk",[(U+'lifecycle.rs',"            let stop = failed(control, records, wanted, mutants, why);\n            drop(held);\n            return Err(stop);","            drop(held);\n            let stop = failed(control, records, wanted, mutants, why);\n            return Err(stop);")]),
("lifecycle: a take-over does not compare the device's size with the store's","skyzfs-ublk",[(U+'lifecycle.rs',"            if shape.size != size {","            if false && shape.size != size {")]),
("lifecycle: a failed volume deletes a noted device with other values","skyzfs-ublk",[(U+'lifecycle.rs',"    if info.shape != attachment.shape {\n        return Stop::Failed(format!(","    if false && info.shape != attachment.shape {\n        return Stop::Failed(format!(")]),
("pump: a stopping pump does not try again to store","skyzfs-ublk",[(U+'pump.rs',"        for _ in 0..self.stop_tries {","        for _ in 0..0 {")]),
("pump: a stopping pump that gives up commits what waited","skyzfs-ublk",[(U+'pump.rs',"        if self.finished.is_empty() {\n            Some(Stopped::Orderly)\n        } else {","        if self.finished.is_empty() {\n            Some(Stopped::Orderly)\n        } else if true {\n            for (tag, result, buf) in self.finished.drain(..) { self.queue.commit(tag, result, buf); }\n            Some(Stopped::Orderly)\n        } else {")]),
("pump: a refusal by a target that went in that moment is a broken contract","skyzfs-ublk",[(U+'pump.rs',"        if self.is_gone() && !self.mutants.has(Mutants::GONE_AFTER_SUBMIT) {\n            Stopped::TargetGone\n        } else {","        if false {\n            Stopped::TargetGone\n        } else {")]),
("lifecycle: detach does not try the lock again","skyzfs-ublk",[(U+'lifecycle.rs',"match lock(records, volume, lock_retries) {","match lock(records, volume, 0) {")]),
]
for name,crate,edits in P:
    saved={}; ok=True
    for f,old,new in edits:
        s=open(f).read(); saved.setdefault(f,s)
        if s.count(old)!=1: print(f"SKIP  {name}: {s.count(old)} matches"); ok=False; break
        open(f,'w').write(s.replace(old,new))
    if ok:
        r=subprocess.run(['cargo','test','-q','-p',crate],capture_output=True,text=True); out=r.stdout+r.stderr
        if 'could not compile' in out: print(f"BUILD {name}"); print(out[-1500:])
        else:
            failed=sorted(set(re.findall(r'^    (\S+::\S+)$',out,re.M)))
            print(f"NOT NOTICED  {name}" if r.returncode==0 else f"caught ({len(failed)})  {name}: {', '.join(failed[:3])}")
    for f,s in saved.items(): open(f,'w').write(s)
    sys.stdout.flush()
