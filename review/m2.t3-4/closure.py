#!/usr/bin/env python3
"""#121: the workspace crates under skyzfs-upload, skyzfsd and node B,
against cut_plan.py's UPLOAD_PATHS and the workflow's pull_request paths.

    cargo metadata --format-version 1 --locked --all-features > META.json   (in CHECKOUT)
    closure.py CHECKOUT META.json

For each root: the workspace members it reaches through normal and build
dependencies (what its binary is made of), and those it reaches only
through a dev-dependency on the way (its tests alone). A crate's
directory is "covered" by a list when a file in it matches an entry.
"""
import fnmatch
import json
import os
import re
import sys

ROOTS = ('skyzfs-upload', 'skyzfsd', 'skyzfs-nodeb')


def main():
    checkout, meta = os.path.abspath(sys.argv[1]), json.load(open(sys.argv[2]))
    sys.path.insert(0, os.path.join(checkout, 'tests/kernel'))
    import cut_plan
    wf = open(os.path.join(checkout, '.github/workflows/skyzfs-cut-daemon.yml')).read()
    block = wf.split('  pull_request:\n    paths:\n', 1)[1].split('  push:', 1)[0]
    wf_paths = re.findall(r'^\s+- (\S+)$', block, re.M)
    members = set(meta['workspace_members'])
    pkg = {p['id']: p for p in meta['packages']}
    nodes = {n['id']: n for n in meta['resolve']['nodes']}
    name_id = {pkg[i]['name']: i for i in members}

    def rel(i):
        return os.path.relpath(os.path.dirname(pkg[i]['manifest_path']), checkout) + '/'

    def reach(root, dev):
        seen, todo = set(), [root]
        while todo:
            i = todo.pop()
            if i in seen:
                continue
            seen.add(i)
            for d in nodes[i]['deps']:
                kinds = {k['kind'] for k in d['dep_kinds']}
                if kinds & {None, 'build'} or (dev and 'dev' in kinds):
                    todo.append(d['pkg'])
        return {i for i in seen if i in members}

    def in_plan(path):
        return cut_plan.touches_upload([path + 'src/lib.rs'])

    def in_wf(path):
        return any(fnmatch.fnmatch(path + 'src/lib.rs', p.replace('**', '*')) for p in wf_paths)

    print('workflow pull_request paths:', ' '.join(wf_paths))
    print('UPLOAD_PATHS:', ' '.join(cut_plan.UPLOAD_PATHS))
    allb, alld = set(), set()
    for r in ROOTS:
        b = reach(name_id[r], False)
        d = reach(name_id[r], True) - b
        allb |= b
        alld |= d
        print(f'\n{r}: built from {len(b)} workspace crates; {len(d)} more through dev-dependencies')
        for i in sorted(b, key=rel):
            print(f'  {rel(i):34} plan {"yes" if in_plan(rel(i)) else "NO ":3}  workflow {"yes" if in_wf(rel(i)) else "NO"}')
        for i in sorted(d, key=rel):
            print(f'  {rel(i):34} plan {"yes" if in_plan(rel(i)) else "NO ":3}  workflow {"yes" if in_wf(rel(i)) else "NO"}  (dev only)')
    print('\nworkspace crates the lists name that no root is built from:')
    for i in sorted(members, key=rel):
        if i not in allb and (in_plan(rel(i)) or in_wf(rel(i))):
            print(f'  {rel(i):34} plan {"yes" if in_plan(rel(i)) else "no":3}  workflow {"yes" if in_wf(rel(i)) else "no"}'
                  + ('  (dev only)' if i in alld else ''))
    print('\nUPLOAD_PATHS entries the workflow does not start on:')
    for p in cut_plan.UPLOAD_PATHS:
        probe = p + 'x' if p.endswith('/') else p
        if not any(fnmatch.fnmatch(probe, w.replace('**', '*')) for w in wf_paths):
            print('  ' + p)
    print('(end)')


if __name__ == '__main__':
    main()
