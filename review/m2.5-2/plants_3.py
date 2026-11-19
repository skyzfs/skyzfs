"""Plants at 3a3985b: those of plants_2.py, with A4 rewritten for the
new text of a closed desk's refusal, and three more (N9, N10, N11).
Run from a worktree of the head, as plants_2.py:
WORKTREE=<path> python3 plants_3.py [names...]"""
import os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
head, tail = open(os.path.join(HERE, "plants_2.py")).read().split("only = sys.argv[1:]")
exec(head)
P = [p for p in P if not p[0].startswith("A4 ")]
P += [
 ("A4 the handler answers a closed desk's None as a count of nothing", HD,
  "        Ok(None) => Err(Reply::Refused(\n            Code::Ended,\n            \"the store is no longer served\".into(),\n        )),",
  "        Ok(None) => Ok(skyzfs_upload::CutsNow::default()),", "d"),
 ("N9 a closed desk's cuts reads that the uploader has ended (the nit undone)", HD,
  "        Ok(None) => Err(Reply::Refused(\n            Code::Ended,\n            \"the store is no longer served\".into(),\n        )),",
  "        Ok(None) => Err(ended()),", "d"),
 ("N10 F7 undone: the count of the turns starts again while only one side is due", UL,
  "        } else {\n            if !publish_due && !cut_due {\n",
  "        } else {\n            self.restart();\n            if !publish_due && !cut_due {\n", "u"),
 ("N11 the close answers the asks of the count waiting with a count of nothing", QU,
  "        slot.counts.clear();\n",
  "        for waiting in slot.counts.drain(..) {\n            let _ = waiting.send(CutsNow::default());\n        }\n", "ud"),
]
exec("only = sys.argv[1:]" + tail)
