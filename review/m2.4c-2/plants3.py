#!/usr/bin/env python3
"""Plants for the third look at #118, at 4c733d3 (M2.4c-2).

    plants3.py WORKTREE TARGET_DIR GROUP [ID ...]

GROUP is `new` (plants in what changed since da0b4c9: the runtime's
drop, `create`'s limit, the create that loses the head's swap, the
ambient region) or `again` (the plants of the last look in
bins/skyzfsd/src/upload.rs, whose code was moved or is beside what
moved, run again on this head). The runner is plants2.py's: one change
at a time, the daemon's tests, the file restored; KILLED, SURVIVED or
NOT APPLIED, appended to plants3.out.
"""
import os
import sys

import plants2
from plants2 import D, P, UPLOAD

plants2.OUT = os.path.join(plants2.HERE, "plants3.out")

NEW = [
    # --- the runtime (2.1; the author's T1 to T4) -----------------------
    ("T1", UPLOAD, "        if let Some(runtime) = self.inner.take() {\n            runtime.shutdown_background();\n        }",
     "        if let Some(runtime) = self.inner.take() {\n            drop(runtime);\n        }",
     "the runtime's drop waits for its blocking work", [D]),
    ("T2", UPLOAD, "                drop(runtime);\n                let _ = said.send(end);",
     "                let _ = said.send(end);\n                drop(runtime);",
     "the thread drops its runtime after its answer, not before", [D]),
    ("T3", UPLOAD, "        .block_on(async { tokio::time::timeout(limit, work).await })",
     "        .block_on(async { tokio::time::timeout(limit * 100, work).await })",
     "the limit of the work in object storage is a hundred times longer", [D]),
    ("T4", UPLOAD,
     "    let runtime = runtime().map_err(|e| format!(\"a runtime for object storage: {e}\"))?;\n    runtime\n        .block_on(async { tokio::time::timeout(limit, work).await })\n        .map_err(|e| format!(\"a runtime for object storage: {e}\"))?\n",
     "    let runtime = tokio::runtime::Builder::new_current_thread()\n        .enable_all()\n        .build()\n        .map_err(|e| format!(\"a runtime for object storage: {e}\"))?;\n    runtime\n        .block_on(async { tokio::time::timeout(limit, work).await })\n",
     "the work in object storage runs on a plain runtime", [D]),
    ("T5", UPLOAD,
     "    within(PLACE_LIMIT, async {\n        let (objects, refs) = connect(remote).await;\n        place(objects, refs, remote, size).await\n    })",
     "    within(PLACE_LIMIT * 100, async {\n        let (objects, refs) = connect(remote).await;\n        place(objects, refs, remote, size).await\n    })",
     "create's limit is a hundred times PLACE_LIMIT", [D]),
    # --- the create that loses the head's swap (2.2; U1 to U3) ----------
    ("U1", UPLOAD, "            Err(EngineError::Conflict) => {\n                unserved(",
     "            Err(EngineError::Conflict) if false => {\n                unserved(",
     "a create that loses the head's swap ends with the engine's error", [D]),
    ("U2", UPLOAD, "                unserved(&refs, vol, &named).await?;\n                let head = refs",
     "                let head = refs",
     "no lease check after the lost swap", [D]),
    ("U3", UPLOAD, "                return found(&store, vol, &named, &head, size).await;",
     "                let _ = head;\n                return Ok(Placed::Found);",
     "no check of the volume found after the lost swap", [D]),
    ("U3b", UPLOAD, "                return found(&store, vol, &named, &head, size).await;",
     "                let _ = head;\n                return Ok(Placed::Made);",
     "a create that loses the head's swap says it made the volume", [D]),
    ("U6", UPLOAD, "    unserved(&refs, vol, &named).await?;\n    let head = refs",
     "    let head = refs",
     "no lease check before the head is read (the first, as it was)", [D]),
    ("U7", UPLOAD, "            \"{named} has been served: its lease names {} at epoch {}. The name is another \\\n             volume's\",\n            lease.holder, lease.epoch\n        )),\n        None => Ok(()),",
     "            \"{named} has been served: its lease names {} at epoch {}. The name is another \\\n             volume's\",\n            lease.holder, lease.epoch\n        ))\n        .or(Ok(())),\n        None => Ok(()),",
     "a lease row refuses nothing", [D]),
    ("U8", UPLOAD, "    };\n    found(&store, vol, &named, &head, size).await\n}",
     "    };\n    let _ = head;\n    Ok(Placed::Found)\n}",
     "no check of a volume found under the name (the first, as it was)", [D]),
    # --- the ambient region (2.4; U4, U5) -------------------------------
    ("U4", UPLOAD, "    env(\"AWS_REGION\")\n        .filter(|region| !region.is_empty())\n        .is_none()",
     "    env(\"AWS_REGION\")\n        .is_none()",
     "an empty AWS_REGION is left to the SDK on the ambient path", [D]),
    ("U4b", UPLOAD, "        .is_none()\n        .then_some(REGION)", "        .map_or(Some(REGION), |_| Some(REGION))",
     "the ambient client is given us-east-1 whatever AWS_REGION says", [D]),
    ("U5", UPLOAD, "            let region = ambient_region(env);", "            let region: Option<&str> = None;",
     "connect gives the ambient client no region", [D]),
    ("Y04b", UPLOAD, "    env(\"AWS_REGION\")\n        .filter(|region| !region.is_empty())\n        .unwrap_or_else(|| REGION.to_owned())",
     "    env(\"AWS_REGION\")\n        .unwrap_or_else(|| REGION.to_owned())",
     "an empty AWS_REGION is the region of fixed keys", [D]),
]

SKIP = {"N01", "Y04"}  # their text is gone or doubled: T1 and Y04b stand for them
AGAIN = [p for p in plants2.DAEMON if p[1] == UPLOAD and p[0] not in SKIP]

if __name__ == "__main__":
    if len(sys.argv) < 4 or sys.argv[3] not in ("new", "again"):
        sys.exit(__doc__)
    plants2.DAEMON = {"new": NEW, "again": AGAIN}[sys.argv[3]]
    sys.argv[3] = "daemon"
    plants2.main()
