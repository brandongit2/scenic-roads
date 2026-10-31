#!/usr/bin/env python3
"""Task #916: the pipeline crate's module graph, from its sources (docs/crates.md).

For each top-level module of crates/pipeline/src (a file `x.rs` and/or folder `x/`), the other
top-level modules its code names by `crate::<m>` (also inside `use crate::{a, b::c}` groups), or by
`pipeline::<m>` in the binaries. Prints each module's lines and edges (with counts of mentions),
then the strongly connected components (cycles). `--dot` prints Graphviz instead.
Comment lines (starting with //) are skipped, so doc links don't count.
"""
import os
import re
import sys
from collections import defaultdict

root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                     '..', '..', 'crates', 'pipeline', 'src'))


def mod_of(path):
    rel = os.path.relpath(path, root).split(os.sep)
    if rel[0] == 'bin':
        return 'bin:' + rel[1][:-3]
    if rel[0] == 'lib.rs':
        return 'lib'
    return rel[0][:-3] if rel[0].endswith('.rs') else rel[0]


def names_in_group(g):
    out, depth, cur = [], 0, ''
    for ch in g:
        if ch == '{':
            depth += 1
        elif ch == '}':
            depth -= 1
        if depth == 0 and ch == ',':
            out.append(cur)
            cur = ''
        else:
            cur += ch
    out.append(cur)
    return [m.group(1) for m in (re.match(r'\s*(\w+)', x) for x in out) if m]


mods, lines = set(), defaultdict(int)
edges = defaultdict(lambda: defaultdict(int))
texts = {}
for d, _, fs in os.walk(root):
    for f in fs:
        if f.endswith('.rs'):
            p = os.path.join(d, f)
            m = mod_of(p)
            src = open(p, encoding='utf-8').read()
            texts[p] = (m, src)
            lines[m] += src.count('\n')
            if not m.startswith('bin:') and m != 'lib':
                mods.add(m)

for p, (m, src) in texts.items():
    code = '\n'.join(l for l in src.split('\n') if not l.lstrip().startswith('//'))
    pfx = r'\bpipeline' if m.startswith('bin:') else r'\bcrate'
    targets = [x.group(1) for x in re.finditer(pfx + r'::(\w+)', code)]
    for x in re.finditer(pfx + r'::\{', code):
        i = j = x.end()
        depth = 1
        while depth and j < len(code):
            depth += {'{': 1, '}': -1}.get(code[j], 0)
            j += 1
        targets += names_in_group(code[i:j - 1])
    for t in targets:
        if t != m:
            edges[m][t if t in mods else '<root>'] += 1

idx, low, on, st, sccs, n = {}, {}, set(), [], [], [0]


def strong(v):
    idx[v] = low[v] = n[0]
    n[0] += 1
    st.append(v)
    on.add(v)
    for w in edges[v]:
        if w not in mods:
            continue
        if w not in idx:
            strong(w)
            low[v] = min(low[v], low[w])
        elif w in on:
            low[v] = min(low[v], idx[w])
    if low[v] == idx[v]:
        c = []
        while True:
            w = st.pop()
            on.discard(w)
            c.append(w)
            if w == v:
                break
        sccs.append(sorted(c))


sys.setrecursionlimit(10000)
for v in sorted(mods):
    if v not in idx:
        strong(v)

if '--dot' in sys.argv:
    print('digraph pipeline {')
    for a in sorted(edges):
        for b in sorted(edges[a]):
            print(f'  "{a}" -> "{b}";')
    print('}')
    sys.exit()
for a in sorted(lines):
    print(f'{a} ({lines[a]} lines) -> ' + ', '.join(f'{b}:{c}' for b, c in sorted(edges[a].items())))
print()
for c in sorted(sccs, key=len, reverse=True):
    if len(c) > 1:
        print(f'cycle ({len(c)} modules, {sum(lines[x] for x in c)} lines): ' + ' '.join(c))
