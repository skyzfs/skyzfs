#!/usr/bin/env python3
"""#100 at 07a9f2d: the plants of `plants.py` that no longer match the code, written again for
this head (R...), and new ones in the start token's code (K...). Run as `plants.py` runs its
own.

    plants_next.py CHECKOUT TARGET_DIR [PREFIX ...]
    plants_next.py CHECKOUT TARGET_DIR check      how often each plant's text is found
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
CHECK = sys.argv[3:] == ['check']
if CHECK:
    del sys.argv[3:]
import plants  # noqa: E402

STATE, INP, START = plants.STATE, plants.INP, plants.START
VOL = 'crates/skyzfs-local/src/volume.rs'

TOKEN = ("        for (i, word) in id_words(start).into_iter().enumerate() {\n"
         "            self.store(at::START + i, word);\n        }\n")
BEGIN = ("                CutState::begin(\n                    region,\n                    size,\n"
         "                    bound,\n                    state::Owner { start, ..owner },\n"
         "                    self.mutants,\n                )\n")
plants.PLANTS[:] = [
    # The plants of the first look whose text changed.
    ('RA6 a refused copy: the cuts let go are not counted', STATE,
     "            self.give(*held, Heir::Open);\n            self.store(at::ABANDONED, abandoned + 1);",
     "            self.give(*held, Heir::Open);\n            self.store(at::ABANDONED, abandoned);"),
    ('RA7 a refused copy: the copies of the cuts let go are not freed then', STATE,
     "            self.store(at::ABANDONED, abandoned + 1);\n        }\n        self.free_unneeded(None);\n    }",
     "            self.store(at::ABANDONED, abandoned + 1);\n        }\n    }"),
    ('RA10 a refused copy: the cuts give their maps to nobody', STATE,
     "            let abandoned = self.begin_abandoning(held);\n            self.give(*held, Heir::Open);",
     "            let abandoned = self.begin_abandoning(held);\n            self.drop_map(*held);"),
    ('RA12 a refused copy: the cut being let go is not recorded first', STATE,
     "            let abandoned = self.begin_abandoning(held);\n            self.give(*held, Heir::Open);",
     "            let abandoned = self.abandoned();\n            self.give(*held, Heir::Open);"),
    ('RT13 a state not trusted is taken up all the same where a view reads it', INP,
     BEGIN,
     "                match CutState::view(region, size, bound, owner, self.mutants) {\n"
     "                    Ok(state) => state.take_up(start),\n"
     "                    Err((_, region)) => CutState::begin(region, size, bound, "
     "state::Owner { start, ..owner }, self.mutants),\n                }\n"),
    ('RO3 the region is opened after the token is written and the marker goes', START,
     "            .map_err(OpenError::CutRegion)?;\n        let advance",
     "            .map_err(OpenError::CutRegion);\n        let advance",
     "        Ok((home, volume, geometry, marked, cuts))", "        Ok((home, volume, geometry, marked, cuts?))"),
    # The start token.
    ('K1 an open that holds no cuts writes no token', START,
     "        let advance = options.hold_cuts.is_some() || !mutants.has(Mutants::START_KEPT_WITHOUT_CUTS);",
     "        let advance = options.hold_cuts.is_some();"),
    ('K2 the token is not synced', VOL,
     "    file.write_at(0, &token)?;\n    file.sync()?;", "    file.write_at(0, &token)?;"),
    ('K3 the open reads no token', START,
     "        let found = crate::volume::start_token(dir);", "        let found: Option<[u8; 16]> = None;"),
    ('K4 a start that takes a state up does not record its token', STATE,
     TOKEN + "        self.settle();", "        let _ = start;\n        self.settle();"),
    ('K5 a start that takes a state up records its token after it has finished the rest', STATE,
     TOKEN + "        self.settle();", "        self.settle();\n" + TOKEN),
    ('K8 the start takes the state up before it has judged the copies', INP,
     "            match CutState::view(region, size, bound, owner, self.mutants) {",
     "            match CutState::view(region, size, bound, owner, self.mutants).map(|s| s.take_up(start)) {"),
    ('K11 a new state is begun under the token the open found', INP,
     BEGIN,
     "                CutState::begin(region, size, bound, owner, self.mutants)\n"),
    ('K13 a token file that is there is not written again', VOL,
     "    file.write_at(0, &token)?;\n    file.sync()?;",
     "    if created {\n        file.write_at(0, &token)?;\n    }\n    file.sync()?;"),
    ('K14 the token is written after the marker goes', START,
     "        let after = mutants.has(Mutants::START_AFTER_ACKNOWLEDGING);", "        let after = true;"),
]

if __name__ == '__main__':
    if CHECK:
        for name, path, *pairs in plants.PLANTS:
            text = open(os.path.join(plants.ROOT, path)).read()
            print(name.split()[0], [text.count(old) for old in pairs[0::2]])
    else:
        plants.main()
