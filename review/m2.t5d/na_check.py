"""na_check.py ROOT_BASE ROOT_HEAD: for the earlier plants NOT APPLIED at this head, count the
old text's matches at the base (59c4ad5) and at the head (d3b75af). Each ROOT holds the rig's
files at that commit (`git show REV:PATH`). Beside this script: p132.py, a copy of
review/m2.t5b/plants_next.py, and p139.py, a copy of review/m2.t7/next_plants_py.py. Run with
python3 -I."""
import importlib.util, sys, os
base, head = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
def load(name):
    src = open(os.path.join(here, name)).read()
    return src
# p132: P list of (label, file, old, new); p139: PLANTS of (id, file, label, [(old,new)])
import ast
def plants(name, var):
    tree = ast.parse(load(name))
    consts = {}
    for node in tree.body:
        if isinstance(node, ast.Assign):
            for t in node.targets:
                if isinstance(t, ast.Name):
                    try:
                        consts[t.id] = ast.literal_eval(node.value)
                    except Exception:
                        pass
                elif isinstance(t, ast.Tuple) and isinstance(node.value, ast.Tuple):
                    for a, b in zip(t.elts, node.value.elts):
                        try:
                            consts[a.id] = ast.literal_eval(b)
                        except Exception:
                            pass
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == var for t in node.targets):
            src = ast.unparse(node.value)
            return eval(src, {}, consts)
def count(root, f, old):
    try:
        return open(os.path.join(root, f)).read().count(old)
    except FileNotFoundError:
        return 'no file'
want132 = ('N1 ', 'N6 ', 'N7 ', 'N14 ', 'N15 ', 'N16 ', 'R10 ', 'R12 ')
for p in plants('p132.py', 'P'):
    label, f, old = p[0], p[1], p[2]
    if label.startswith(want132):
        print('p132', label.split(' ', 1)[0], 'base', count(base, f, old), 'head', count(head, f, old))
want139 = ('E13', 'E14', 'P2', 'P3', 'Q9', 'A14', 'A19', 'S1', 'S2')
for p in plants('p139.py', 'PLANTS'):
    if p[0] in want139:
        print('p139', p[0], 'base', [count(base, p[1], o) for o, _ in p[3]], 'head', [count(head, p[1], o) for o, _ in p[3]])
