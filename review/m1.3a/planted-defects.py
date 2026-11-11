#!/usr/bin/env python3
"""Planted defects for skyzfs/skyzfs#10 at 5716df5: each edits one rule that has no
crate-private switch, runs the crate's tests, and restores the file.
Run from the root of a checkout: python3 plant.py [name-prefix ...]"""
import subprocess, sys, re
U='crates/skyzfs-ublk/src/'
L='crates/skyzfs-local/src/'
P=[
# name, crate, [(file, old, new)]
("lifecycle: a failed volume deletes a device whose note has another nonce","skyzfs-ublk",[(U+'lifecycle.rs',"Ok(Some(nonce)) if nonce == attachment.nonce);","Ok(Some(_)));")]),
("lifecycle: a failed volume deletes by a record of another boot","skyzfs-ublk",[(U+'lifecycle.rs',"let ours = attachment.boot == records.boot()\n            && matches!","let ours = true\n            && matches!")]),
("lifecycle: a failed volume deletes its live device","skyzfs-ublk",[(U+'lifecycle.rs',"state: DeviceState::Waiting | DeviceState::Added,\n                ..\n            })) = info","state: DeviceState::Waiting | DeviceState::Added | DeviceState::Live,\n                ..\n            })) = info")]),
("lifecycle: a failed volume does not wait for its live device to settle","skyzfs-ublk",[(U+'lifecycle.rs',"settled(control, records, number, info, wanted, Mutants::default())","Ok(info)")]),
("lifecycle: start follows a record of another boot","skyzfs-ublk",[(U+'lifecycle.rs',"let Some(attachment) = attachment.filter(|a| a.boot == records.boot()) else {","let Some(attachment) = attachment.filter(|_| true) else {")]),
("lifecycle: locate follows a record of another boot","skyzfs-ublk",[(U+'lifecycle.rs',"records.attachment()?.filter(|a| a.boot == records.boot())","records.attachment()?.filter(|_| true)")]),
("lifecycle: a deleted-and-held device without our note is judged like any other","skyzfs-ublk",[(U+'lifecycle.rs',"        if info.state == DeviceState::Dead {\n            return Ok(afresh);\n        }\n","")]),
("lifecycle: a never-started device with our values and no note is deleted","skyzfs-ublk",[(U+'lifecycle.rs','DeviceState::Added => left("it was added and never started, and has no note"),',"DeviceState::Added => Ok(Found::Afresh { stale: Some(number), left: None }),")]),
("lifecycle: our deleted-and-held device is taken over","skyzfs-ublk",[(U+'lifecycle.rs',"        DeviceState::Dead => Ok(afresh),\n    }\n}","        DeviceState::Dead => Ok(Found::TakeOver(number, info.shape)),\n    }\n}")]),
("lifecycle: our never-started device is left, not deleted","skyzfs-ublk",[(U+'lifecycle.rs',"DeviceState::Added => Ok(Found::Afresh {\n            stale: Some(number),","DeviceState::Added => Ok(Found::Afresh {\n            stale: None,")]),
("lifecycle: a number seen taken is recorded all the same","skyzfs-ublk",[(U+'lifecycle.rs',"        if seen.is_some() {\n            continue;\n        }","        if false && seen.is_some() {\n            continue;\n        }")]),
("lifecycle: a locked store is not tried again","skyzfs-ublk",[(U+'lifecycle.rs',"Err(OpenError::Locked) if tries < wanted.lock_retries => {","Err(OpenError::Locked) if false && tries < wanted.lock_retries => {")]),
("lifecycle: an unreadable acknowledged record counts as none","skyzfs-ublk",[(U+'lifecycle.rs',"        Ok(must_reach) => must_reach,\n","        Ok(must_reach) => must_reach,\n        Err(_) if true => None,\n")]),
("lifecycle: an unreadable note counts as no note","skyzfs-ublk",[(U+'lifecycle.rs',"    let Ok(note) = records.note(number) else {\n        return Err(Stop::Failed(\n            \"the note for the volume's device cannot be read\",\n        ));\n    };","    let note = records.note(number).unwrap_or(None);")]),
("lifecycle: one look more than stated at a live device","skyzfs-ublk",[(U+'lifecycle.rs',"for _ in 0..wanted.live_retries {","for _ in 0..wanted.live_retries + 1 {")]),
("detach: a live device is deleted","skyzfs-ublk",[(U+'lifecycle.rs',"DeviceState::Live => Ok(Detached::Live(number)),","DeviceState::Live if false => Ok(Detached::Live(number)),"),(U+'lifecycle.rs',"DeviceState::Waiting | DeviceState::Added => {\n            delete(control, records, number, mutants)?;","DeviceState::Waiting | DeviceState::Added | DeviceState::Live => {\n            delete(control, records, number, mutants)?;")]),
("detach: a deleted-and-held device is deleted again","skyzfs-ublk",[(U+'lifecycle.rs',"DeviceState::Dead => Ok(Detached::Held(number)),","DeviceState::Dead if false => Ok(Detached::Held(number)),"),(U+'lifecycle.rs',"DeviceState::Waiting | DeviceState::Added => {\n            delete(control, records, number, mutants)?;","DeviceState::Waiting | DeviceState::Added | DeviceState::Dead => {\n            delete(control, records, number, mutants)?;")]),
("detach: a device that cannot be told is deleted","skyzfs-ublk",[(U+'lifecycle.rs',"if located.whose != Whose::Ours {","if located.whose == Whose::Anothers {")]),
("detach: the attachment record is removed first","skyzfs-ublk",[(U+'lifecycle.rs',"            delete(control, records, number, mutants)?;\n            let after = control.info(number)?;\n            records.remove_attachment()?;","            records.remove_attachment()?;\n            delete(control, records, number, mutants)?;\n            let after = control.info(number)?;")]),
("detach: the attachment record is kept","skyzfs-ublk",[(U+'lifecycle.rs',"            let after = control.info(number)?;\n            records.remove_attachment()?;","            let after = control.info(number)?;")]),
("locate: no note and other values cannot be told","skyzfs-ublk",[(U+'lifecycle.rs',"None if info.shape != attachment.shape => Whose::Anothers,","None if false => Whose::Anothers,")]),
("locate: another's note is ours","skyzfs-ublk",[(U+'lifecycle.rs',"Some(_) => Whose::Anothers,","Some(_) => Whose::Ours,")]),
("records: an acknowledged record of another boot counts","skyzfs-ublk",[(U+'records.rs',"stored.filter(|s| s.boot == self.boot)","stored.filter(|_| true)")]),
("records: the attachment record is not synced before the rename","skyzfs-ublk",[(U+'records.rs',"    if durable {\n        file.sync_all()?;\n    }\n","")]),
("records: the directory is not synced after the rename","skyzfs-ublk",[(U+'records.rs',"        fs::File::open(dir.unwrap_or(Path::new(\".\")))?.sync_all()?;","        let _ = dir;")]),
("records: removing a note that is not there is an error","skyzfs-ublk",[(U+'records.rs',"Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),","Err(e) => Err(e),")]),
("records: removing the attachment does not sync the directory","skyzfs-ublk",[(U+'records.rs',"        fs::File::open(&self.volume_dir)?.sync_all()\n","        Ok(())\n")]),
("ack: position_for ignores the boot","skyzfs-ublk",[(U+'ack.rs',"(self.identity == *identity && self.boot == *boot)","(self.identity == *identity)")]),
("ack: open keeps a record of another store","skyzfs-ublk",[(U+'ack.rs',"Some(stored) if stored.identity == identity && stored.boot == boot => {","Some(stored) if stored.boot == boot => {")]),
("ack: open keeps a record of another boot","skyzfs-ublk",[(U+'ack.rs',"Some(stored) if stored.identity == identity && stored.boot == boot => {","Some(stored) if stored.identity == identity => {")]),
("ack: a record with bytes behind the position decodes","skyzfs-ublk",[(U+'ack.rs'," || bytes[48..].iter().any(|b| *b != 0)","")]),
("pump: a FUA flush is submitted although the operation failed","skyzfs-ublk",[(U+'pump.rs',"if result.is_ok() && waits {","if waits {")]),
("pump: a stop still takes what is fetched","skyzfs-ublk",[(U+'pump.rs',"        if !self.stopping {\n            self.counters.in_flight","        if true {\n            self.counters.in_flight"),(U+'pump.rs',"while !self.stopping && self.room() > 0 {","while self.room() > 0 {")]),
("pump: an orderly stop does not wait for a flush that waits for room","skyzfs-ublk",[(U+'pump.rs',"let drained = self.in_target == 0 && self.wants_flush.is_empty();","let drained = self.in_target == 0;")]),
("pump: the position is stored again although it did not move","skyzfs-ublk",[(U+'pump.rs',"        if self.stored.is_some_and(|stored| position <= stored) {\n            return Ok(());\n        }\n","")]),
("pump: a completion nobody waits for is ignored","skyzfs-ublk",[(U+'pump.rs',"            _ => Err(Stopped::TargetFault),\n        }\n    }\n\n    /// A request is finished","            _ => Ok(()),\n        }\n    }\n\n    /// A request is finished")]),
("pump: a tag that is in flight is taken again","skyzfs-ublk",[(U+'pump.rs',"        if !matches!(slot, Slot::Idle) {\n            // The queue reused a tag that is not committed.\n            return Err(Stopped::QueueDown);\n        }\n","")]),
("pump: a FUA request whose flush failed is answered as done","skyzfs-ublk",[(U+'pump.rs',"            Slot::Flushing(plan, kept) if is_flush => {\n                self.finish(tag, &plan, &result, kept);","            Slot::Flushing(plan, kept) if is_flush => {\n                self.finish(tag, &plan, &Ok(()), kept);")]),
("pump: unstored is not counted","skyzfs-ublk",[(U+'pump.rs',"self.counters.unstored = self.finished.len() as u64;","self.counters.unstored = 0;")]),
("translate: FUA on a read asks for a flush","skyzfs-ublk",[(U+'translate.rs',"then_flush: fua && op != op::READ,","then_flush: fua,")]),
("translate: a read or write above 1 MiB passes","skyzfs-ublk",[(U+'translate.rs',"if carries_data && len > MAX_IO_BYTES {","if false && carries_data && len > MAX_IO_BYTES {")]),
("translate: an empty request passes","skyzfs-ublk",[(U+'translate.rs',"        if sectors == 0 {\n            return Err(Refusal::Empty);\n        }\n","")]),
("translate: shared-memory zero copy is not refused","skyzfs-ublk",[(U+'translate.rs',"pub const DATA_ELSEWHERE: u32 = NEED_REG_BUF | INTEGRITY | SHMEM_ZC;","pub const DATA_ELSEWHERE: u32 = NEED_REG_BUF | INTEGRITY;")]),
("translate: integrity is not refused","skyzfs-ublk",[(U+'translate.rs',"pub const DATA_ELSEWHERE: u32 = NEED_REG_BUF | INTEGRITY | SHMEM_ZC;","pub const DATA_ELSEWHERE: u32 = NEED_REG_BUF | SHMEM_ZC;")]),
("translate: the last block of the device is refused","skyzfs-ublk",[(U+'translate.rs',".filter(|(_, end)| *end <= self.size);",".filter(|(_, end)| *end < self.size);")]),
("translate: a failed read is answered with its length","skyzfs-ublk",[(U+'translate.rs',"        if outcome.is_err() {\n            return -EIO;\n        }","        if outcome.is_err() && !matches!(plan.action, Action::Read { .. }) {\n            return -EIO;\n        }")]),
("status: a stop does not write the last counters","skyzfs-ublk",[(U+'status.rs',"if matches!(due, Due::Now) || (last && due != Due::Never) {","if matches!(due, Due::Now) {")]),
("status: failed writes are not counted","skyzfs-ublk",[(U+'status.rs',"                        failures += 1;\n","")]),
("status: counters of another boot are reported","skyzfs-ublk",[(U+'status.rs',"let status = status.filter(|s| s.boot == *boot);","let status = status.filter(|_| true);")]),
("status: counters are always the serving daemon's","skyzfs-ublk",[(U+'status.rs',"let live = ours.is_some_and(|l| l.state == DeviceState::Live);","let live = true;")]),
("status: post wakes nobody","skyzfs-ublk",[(U+'status.rs',"            posted.counters = counters;\n            self.changed.notify_all();","            posted.counters = counters;")]),
("store: a log that ends exactly at the position passes","skyzfs-local",[(L+'log.rs',"(compared <= must_reach).then_some","(compared < must_reach).then_some")]),
("store: a position for another store is applied","skyzfs-local",[(L+'log.rs',".is_none_or(|identity| identity == crate::format::store_identity(&current.key));",".is_none_or(|_| true);")]),
("store: a position for another store, or none named, is ignored","skyzfs-local",[(L+'log.rs',".is_none_or(|identity| identity == crate::format::store_identity(&current.key));",".is_some_and(|identity| identity == crate::format::store_identity(&current.key));")]),
("store: the worker's end wakes nobody","skyzfs-local",[(L+'store.rs',"        self.ended.store(true, Ordering::SeqCst);\n        if let Some(Waker(wake)) = &self.waker {\n            wake();\n        }","        self.ended.store(true, Ordering::SeqCst);")]),
("store: the worker's end is not flagged","skyzfs-local",[(L+'store.rs',"        self.ended.store(true, Ordering::SeqCst);\n        if let","        if let")]),
("store: a completion wakes nobody","skyzfs-local",[(L+'store.rs',"                            if let Some(Waker(wake)) = &waker {\n                                wake();\n                            }","")]),
("store: Served reports what the worker appended, not what was handed out","skyzfs-local",[(L+'store.rs',"        if let Some(start) = start {\n            self.acknowledged = Some(start);\n        }\n        out.push(completion);","        let _ = start;\n        out.push(completion);")]),
]
want=sys.argv[1:]
for name,crate,edits in P:
    if want and not any(name.startswith(w) for w in want): continue
    saved={}
    ok=True
    for f,old,new in edits:
        s=open(f).read()
        saved.setdefault(f,s)
        if s.count(old)!=1:
            print(f"SKIP  {name}: pattern found {s.count(old)} times in {f}"); ok=False; break
        open(f,'w').write(s.replace(old,new))
    if ok:
        r=subprocess.run(['cargo','test','-q','-p',crate],capture_output=True,text=True)
        out=r.stdout+r.stderr
        if 'error[' in out or 'could not compile' in out:
            print(f"BUILD {name}: does not compile")
        else:
            failed=sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED',out,re.M))|set(re.findall(r'^    (\S+::\S+)$',out,re.M)))
            if r.returncode==0: print(f"NOT NOTICED  {name}")
            else: print(f"caught ({len(failed)})  {name}: {', '.join(failed[:4])}")
    for f,s in saved.items(): open(f,'w').write(s)
    sys.stdout.flush()
