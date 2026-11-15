#!/usr/bin/env python3
"""#105 at 15c661b: would the model reach D1's exact case with one more schedule?

The variant: a write that arrives while a sync runs, into a region whose bit is not
durable, has its page written at once (and still waits for the next sync), instead of
at the next sync's beginning. The map's rules allow a caller that; M2.3b's store, as
the answer describes it, does not do it.

Runs the map's tests with the variant: at the head, and with intent.rs changed to
what the first draft did (plants2.py's N2), to what N7 does (a page written while a
sync runs is covered by it) and to N3 (a sync covers what was issued after it began).
Files are restored after each.

    model_variant.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
sys.argv = [sys.argv[0], ROOT, TARGET]
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plants2  # noqa: E402

INT = plants2.INT
MODEL = 'crates/skyzfs-local/src/inplace/intent/model.rs'
VARIANT = ("                self.waiting.push((from, n));\n"
           "                if self.syncing.is_none() {\n"
           "                    self.begin_sync()?;\n"
           "                }\n",
           "                self.waiting.push((from, n));\n"
           "                if self.syncing.is_none() {\n"
           "                    self.begin_sync()?;\n"
           "                } else {\n"
           "                    self.write_pages()?;\n"
           "                }\n")


def run(title, edits, variant):
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    orig = {p: open(os.path.join(ROOT, p)).read() for p in (INT, MODEL)}
    try:
        text = orig[INT]
        for old, new in edits:
            assert text.count(old) == 1, old
            text = text.replace(old, new)
        model = orig[MODEL]
        if variant:
            assert model.count(VARIANT[0]) == 1
            model = model.replace(*VARIANT)
        for p, t in ((INT, text), (MODEL, model)):
            with open(os.path.join(ROOT, p), 'w') as f:
                f.write(t)
        failed, broken = plants2.tests(env)
        model_failed = [t for t in failed if t.startswith(('after_a_power_cut', 'the_model_test'))]
        print(f'{title}: failed {failed or "nothing"}' + (f' (did not build: {broken})' if broken else '')
              + f'; of the model\'s tests: {model_failed or "none"}', flush=True)
    finally:
        for p, t in orig.items():
            with open(os.path.join(ROOT, p), 'w') as f:
                f.write(t)


def main():
    by_name = {name.split()[0]: edits for name, edits in plants2.PLANTS}
    run('the head, with the variant', [], True)
    for key, what in (('N2', 'the first draft'), ('N7', 'a page written while a sync runs is covered by it'),
                      ('N3', 'a sync covers what was issued after it began')):
        run(f'{key} ({what}), the model as it is', by_name[key], False)
        run(f'{key} ({what}), with the variant', by_name[key], True)


if __name__ == '__main__':
    main()
