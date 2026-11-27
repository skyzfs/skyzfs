#!/usr/bin/env python3
"""Write plants.json for the next look at #175 (895b18d), checking each
old text occurs once in the tree given."""
import json, sys, os
TREE = sys.argv[1]
D = "crates/skyzfs-upload/src/driver.rs"
U = "crates/skyzfs-upload/src/uploader.rs"
P = "crates/skyzfs-upload/src/publish.rs"
R = "crates/skyzfs-upload/src/remote.rs"
C = "bins/skyzfsd/src/control.rs"
H = "bins/skyzfsd/src/control/handler.rs"
UP = ["cargo", "test", "-p", "skyzfs-upload"]
DM = ["cargo", "test", "-p", "skyzfsd", "--features", "test-hooks", "--lib"]
plants = [
 # --- the author's six, made again ---
 dict(id="d4-kept", edits=[[D, "                self.publishing.retired(name);\n", "                let _ = name;\n"]], cmd=UP),
 dict(id="d4-at-other", edits=[[D, "Ok(crate::remote::Retired::Other(_)) => {}", "Ok(crate::remote::Retired::Other(_)) => self.publishing.retired(name),"]], cmd=UP),
 dict(id="split-txn", edits=[[R, "                RefOp::MetaDelete {\n                    key: snap_row_key(name),\n                },\n            ];\n            match self.refs.transact(self.vol, ops).await? {",
   "            ];\n            let row = vec![RefOp::MetaDelete { key: snap_row_key(name) }];\n            let first = self.refs.transact(self.vol, ops).await?;\n            if first == TxnOutcome::Committed {\n                self.refs.transact(self.vol, row).await?;\n            }\n            match first {"]], cmd=UP),
 dict(id="store-as-stopped", edits=[[U, "let _ = ask.reply.send(Err(RetireRefused::Store(retire_failed(&e))));", "let _ = (ask.reply.send(Err(RetireRefused::Stopped)), e);"]], cmd=UP),
 dict(id="doubled-prefix-channel", edits=[[C, 'format!("{e}; the retire may have landed: ask again")', 'format!("object storage: {e}; the retire may have landed: ask again")']], cmd=DM),
 dict(id="doubled-prefix-uploader", edits=[[U, "        other => other.to_string(),\n", '        other => format!("object storage: {other}"),\n']], cmd=UP),
 dict(id="drop-at-make", edits=[[U, "                        .with_publishing(publishing),\n                );\n", "                        .with_publishing(publishing),\n                );\n                self.retires.clear();\n"]], cmd=UP),
 # --- the reviewer's own ---
 # retire_failed: a conflict worded as a publish's (L1's second half undone)
 dict(id="conflict-text-generic", edits=[[U, "        RemoteError::Conflict => format!(", "        RemoteError::Conflict if false => format!("]], cmd=UP),
 # retire_failed: the conflict's text no longer names object storage
 dict(id="conflict-text-noprefix", edits=[[U, '"object storage: snap/T moved under each of {} tries"', '"snap/T moved under each of {} tries"']], cmd=UP),
 # Publishing::retired: body does nothing
 dict(id="retired-noop", edits=[[P, "        if self.outcomes.remove(name).is_some() {\n            self.order.retain(|kept| kept != name);\n        }\n    }\n\n    /// End each publish whose time is up",
   "        let _ = name;\n    }\n\n    /// End each publish whose time is up"]], cmd=UP),
 # Publishing::retired: the outcome removed, the name left in `order`
 dict(id="retired-no-order", edits=[[P, "        if self.outcomes.remove(name).is_some() {\n            self.order.retain(|kept| kept != name);\n        }\n    }\n\n    /// End each publish whose time is up",
   "        self.outcomes.remove(name);\n    }\n\n    /// End each publish whose time is up"]], cmd=UP),
 # Publishing::retired: a publish of the name that pends dropped too
 dict(id="retired-pending-too", edits=[[P, "        if self.outcomes.remove(name).is_some() {\n            self.order.retain(|kept| kept != name);\n        }\n    }\n\n    /// End each publish whose time is up",
   "        self.pending.retain(|p| p.request.name != name);\n        if self.outcomes.remove(name).is_some() {\n            self.order.retain(|kept| kept != name);\n        }\n    }\n\n    /// End each publish whose time is up"]], cmd=UP),
 # Driver::retire: cleared at Retired only, not at Gone
 dict(id="retired-only", edits=[[D, "Ok(crate::remote::Retired::Retired | crate::remote::Retired::Gone) => {", "Ok(crate::remote::Retired::Retired) => {"],
   [D, "Ok(crate::remote::Retired::Other(_)) => {}", "Ok(_) => {}"]], cmd=UP),
 # Driver::retire: cleared at Gone only, not at Retired
 dict(id="gone-only", edits=[[D, "Ok(crate::remote::Retired::Retired | crate::remote::Retired::Gone) => {", "Ok(crate::remote::Retired::Gone) => {"],
   [D, "Ok(crate::remote::Retired::Other(_)) => {}", "Ok(_) => {}"]], cmd=UP),
 # Driver::retire: cleared also on an error (the retire may not have landed)
 dict(id="cleared-on-error", edits=[[D, "            Err(e) => {\n                self.figures.errors += 1;", "            Err(e) => {\n                self.publishing.retired(name);\n                self.figures.errors += 1;"]], cmd=UP),
 # handler: a request dropped by the uploader reads the bare `ended` again
 dict(id="ended-text", edits=[[H, '        Some(Err(_)) => Err(Reply::Refused(\n            Code::Ended,\n            format!("the uploader has ended; {again}"),\n        )),', "        Some(Err(_)) => Err(ended()),"]], cmd=DM),
 # handler: a retire's waits given the publish's advice
 dict(id="retire-publish-advice", edits=[[H, "retired_reply(&wait(waiter, rx, RETIRE_AGAIN)?)", "retired_reply(&wait(waiter, rx, PUBLISH_AGAIN)?)"]], cmd=DM),
 # control: the store refusal's advice dropped
 dict(id="store-reply-bare", edits=[[C, 'format!("{e}; the retire may have landed: ask again")', "e.clone()"]], cmd=DM),
 # conflict-text-noprefix with context that S5 does not share (run with S4-S7 in)
 dict(id="conflict-text-noprefix-src", edits=[[U, '        RemoteError::Conflict => format!(\n            "object storage: snap/T moved under each of {} tries"', '        RemoteError::Conflict => format!(\n            "snap/T moved under each of {} tries"']], cmd=UP),
]
for p in plants:
    for f, old, new in p['edits']:
        n = open(os.path.join(TREE, f)).read().count(old)
        assert n == 1, (p['id'], f, n)
json.dump(plants, open(sys.argv[2], 'w'), indent=1)
print(len(plants), 'plants')
