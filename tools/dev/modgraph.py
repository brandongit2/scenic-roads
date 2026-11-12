#!/usr/bin/env python3
"""Task #916: a crate's module graph, from its sources (docs/crates.md).

  tools/dev/modgraph.py [<crate>] [--depth N] [--code] [--dot]
  tools/dev/modgraph.py <crate> --depth N [--code] --items <from> <to>
  tools/dev/modgraph.py <crate> --depth N [--code] --cut <module> …
  tools/dev/modgraph.py <crate> --depth N [--code] --edges

<crate>: a folder of crates/ (pipeline, agentcore …; pipeline when none is given).

A module is a file's module path cut to N parts (`--depth`, 1 by default: agent/flags.rs is
`agent::flags` at depth 2, `agent` at depth 1; agent/mod.rs is `agent`). For each module, the other
modules its code names: paths `crate::a::b`, `super::…`, `self::…`, a child module's bare `x::` in
the file declaring it, entries of `use crate::{a, b::c}` groups, and `<crate>::a` in the binaries.
A path is resolved to the deepest module it names; an item a module re-exports from its own child
(`pub use x::Item;`) counts as that child's. Prints each module's lines (code apart from tests) and
edges with counts of mentions, then the strongly connected components (cycles). Comment lines
(starting with //) are skipped, so doc links don't count.

Test code: an item under `#[cfg(test)]` (a `mod tests { … }`, a test-only fn, a `mod x;` whose file
is then test code whole). `--code` counts only the rest (what a dev or release build compiles;
`cfg(any(test, feature = "testkit"))` is code). Only the crate's own files: agentcore's unit tests
compile the plan's modules in by #[path] (pipeline's agent/build.rs …), which count as pipeline's.

`--items A B`: the paths module A names in module B, with counts (what a cut between them must
move). `--cut M …`: for the set of modules given (`M::*`: M's children), every edge leaving the set,
by item, so a set meant to be a crate below the rest names nothing of it when this prints none.
`--edges`: every edge, by item. `--dot` prints Graphviz instead.
"""
import os
import re
import sys
from collections import defaultdict

args = sys.argv[1:]


def opt(name, n=1):
    if name not in args:
        return None
    i = args.index(name)
    v = args[i + 1:i + 1 + n] if n else args[i + 1:]
    del args[i:i + 1 + len(v)]
    return v


depth = int((opt('--depth') or ['1'])[0])
items_ab = opt('--items', 2)
cut = opt('--cut', 0)
code_only = '--code' in args
dot = '--dot' in args
crate = next((a for a in args if not a.startswith('-')), 'pipeline')
root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                     '..', '..', 'crates', crate, 'src'))
if not os.path.isdir(root):
    sys.exit('modgraph.py: no crate folder %s' % root)


def path_of(p):
    """A file's module path: ['agent', 'flags'] for agent/flags.rs, ['bin:x'] for bin/x.rs."""
    rel = os.path.relpath(p, root).split(os.sep)
    if rel[0] == 'bin':
        return ['bin:' + rel[1][:-3]]
    if rel == ['lib.rs'] or rel == ['main.rs']:
        return []
    last = rel[-1][:-3]
    return rel[:-1] + ([] if last == 'mod' else [last])


def key(mp):
    return '::'.join(mp[:depth]) if mp else 'lib'


def strip(src):
    """Comment lines and string/char literals blanked (same length), so braces and paths count."""
    tok = re.compile(r'//[^\n]*|/\*(?:.|\n)*?\*/|b?r(#*)"(?:.|\n)*?"\1|b?"(?:\\[\s\S]|[^"\\])*"'
                     r"|b?'(?:\\.|[^'\\\n])'|\w+|.|\n")
    out = []
    for m in tok.finditer(src):
        t = m.group(0)
        if t.startswith('//') or t.startswith('/*') or t[-1:] in '"\'' and len(t) > 1:
            out.append(re.sub(r'[^\n]', ' ', t))
        else:
            out.append(t)
    return ''.join(out)


def test_spans(s):
    """[(start, end)] of the items under #[cfg(test)], and the `mod x;` names among them."""
    spans, mods = [], set()
    for m in re.finditer(r'#\[cfg\(test\)\]', s):
        i = m.end()
        # the item: up to its `;` or its matched `{ … }`, past further attributes
        j = i
        while True:
            a = re.match(r'\s*#\[[^\]]*\]', s[j:])
            if not a:
                break
            j += a.end()
        # the item's first `;` or `{` outside brackets (`-> [u8; 4]` is still its head)
        semi = brace = -1
        d = 0
        for k in range(j, len(s)):
            c = s[k]
            if c in '([':
                d += 1
            elif c in ')]':
                d -= 1
            elif d == 0 and c in ';{':
                semi, brace = (k, len(s)) if c == ';' else (-1, k)
                break
        head = s[j:semi if semi >= 0 else len(s)]
        mm = re.match(r'\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*$', head)
        if mm:
            mods.add(mm.group(1))
        if brace < 0 or (0 <= semi < brace):
            spans.append((m.start(), semi + 1))
            continue
        d, k = 1, brace + 1
        while d and k < len(s):
            d += {'{': 1, '}': -1}.get(s[k], 0)
            k += 1
        spans.append((m.start(), k))
    return spans, mods


files = {}  # path -> (module path, stripped source, test spans)
for d, _, fs in os.walk(root):
    for f in fs:
        if f.endswith('.rs'):
            p = os.path.join(d, f)
            src = open(p, encoding='utf-8').read()
            s = strip(src)
            files[p] = (path_of(p), s, test_spans(s), src.count('\n'))

modpaths = {tuple(mp) for mp, *_ in files.values()}
# a file declared `#[cfg(test)] mod x;` is test code whole, and so its children
test_files = set()
for p, (mp, s, (spans, tmods), _) in files.items():
    for x in tmods:
        test_files.add(tuple(mp + [x]))


def is_test_file(mp):
    return any(tuple(mp[:i]) in test_files for i in range(1, len(mp) + 1))


# re-exports from a child: `pub use x::Item;`, `pub use x::{A, B};`, `pub use self::x::…`
reexp = defaultdict(dict)  # module path -> item -> child module path
for p, (mp, s, _, _) in files.items():
    for m in re.finditer(r'\bpub(?:\([^)]*\))?\s+use\s+(?:self::)?(\w+)::([^;]+);', s):
        child = tuple(mp + [m.group(1)])
        if child not in modpaths:
            continue
        body = m.group(2).strip()
        names = re.findall(r'(\w+)\s*(?:as\s+(\w+))?', body.strip('{}'))
        for n, alias in names:
            if n not in ('self', 'as'):
                reexp[tuple(mp)][alias or n] = child


def resolve(parts):
    """An absolute path's deepest module, and the rest of the path (the item)."""
    mp = []
    while len(mp) < len(parts) and tuple(mp + [parts[len(mp)]]) in modpaths:
        mp.append(parts[len(mp)])
    rest = parts[len(mp):]
    if rest and rest[0] in reexp.get(tuple(mp), {}):
        mp = list(reexp[tuple(mp)][rest[0]])
    return mp, rest


def group_entries(g):
    """`a, b::c, d::{e, f}` -> [['a'], ['b', 'c'], ['d', 'e'], ['d', 'f']]."""
    out, d, cur = [], 0, ''
    for ch in g + ',':
        if ch == '{':
            d += 1
        elif ch == '}':
            d -= 1
        if d == 0 and ch == ',':
            cur = cur.strip()
            if cur:
                mm = re.match(r'([\w:]*?)::\{(.*)\}$', cur, re.S)
                if mm:
                    base = [x for x in mm.group(1).split('::') if x]
                    out += [base + e for e in group_entries(mm.group(2))]
                else:
                    out.append([x for x in re.split(r'\s+as\s+', cur)[0].split('::') if x])
            cur = ''
        else:
            cur += ch
    return out


PATH = r'(?:::\w+)*'
lines = defaultdict(lambda: [0, 0])  # key -> [code, test]
edges = defaultdict(lambda: defaultdict(int))
named = defaultdict(lambda: defaultdict(int))  # (from key, to key) -> item -> count
for p, (mp, s, (spans, _), n) in files.items():
    whole_test = is_test_file(mp)
    k = key(mp)
    test_n = n if whole_test else sum(s.count('\n', a, b) for a, b in spans)
    lines[k][0] += n - test_n
    lines[k][1] += test_n
    children = {x[len(mp)] for x in modpaths if len(x) == len(mp) + 1 and list(x[:len(mp)]) == mp}

    def in_test(pos):
        return whole_test or any(a <= pos < b for a, b in spans)

    # inline modules (`mod tests { … }`): super and self inside one start from it
    inline = []
    for m in re.finditer(r'(?<!\w)mod\s+(\w+)\s*\{', s):
        d, j = 1, m.end()
        while d and j < len(s):
            d += {'{': 1, '}': -1}.get(s[j], 0)
            j += 1
        inline.append((m.start(), j, m.group(1)))

    def here(pos):
        return list(mp) + [nm for a, b, nm in sorted(inline) if a <= pos < b]

    found = []  # (pos, absolute path)
    for m in re.finditer(r'(?<![\w:])((?:crate|self|(?:super::)*super)|' + re.escape(crate) +
                         r')::(\{)?', s):
        head = m.group(1)
        if head == crate and not (mp and mp[0].startswith('bin:')):
            continue
        at = here(m.start())
        if head.startswith('super'):
            base = at[:len(at) - head.count('super')]
        elif head == 'self':
            base = at
        else:
            base = []
        if m.group(2):
            i = j = m.end()
            d = 1
            while d and j < len(s):
                d += {'{': 1, '}': -1}.get(s[j], 0)
                j += 1
            for e in group_entries(s[i:j - 1]):
                found.append((m.start(), base + [x for x in e if x != 'self']))
        else:
            tail = re.match(r'\w+' + PATH, s[m.end():])
            if tail:
                found.append((m.start(), base + tail.group(0).split('::')))
    # a child module named bare in the file declaring it
    for c in children:
        for m in re.finditer(r'(?<![\w:])' + re.escape(c) + r'::(\w+' + PATH + ')', s):
            found.append((m.start(), mp + [c] + m.group(1).split('::')))
    for pos, parts in found:
        if code_only and in_test(pos):
            continue
        tmp, rest = resolve(parts)
        if is_test_file(tmp) and not is_test_file(mp):
            continue
        t = key(tmp)
        if t == k:
            continue
        edges[k][t] += 1
        named[(k, t)]['::'.join(tmp + rest[:1])] += 1

mods = sorted(k for k in lines if not k.startswith('bin:') and k != 'lib')
idx, low, on, st, sccs, cnt = {}, {}, set(), [], [], [0]


def strong(v):
    idx[v] = low[v] = cnt[0]
    cnt[0] += 1
    st.append(v)
    on.add(v)
    for w in edges[v]:
        if w not in lines:
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
for v in mods:
    if v not in idx:
        strong(v)


def ln(k):
    c, t = lines[k]
    return f'{c}' if code_only else f'{c} code + {t} test'


if items_ab:
    a, b = items_ab
    for it, c in sorted(named[(a, b)].items(), key=lambda x: (-x[1], x[0])):
        print(f'{c:5} {it}')
    sys.exit()
if '--edges' in args:
    for a in sorted(edges):
        for b, c in sorted(edges[a].items()):
            print(f'{a} -> {b}: {c}: ' + ', '.join(
                f'{it}:{n}' for it, n in sorted(named[(a, b)].items(), key=lambda x: -x[1])))
    sys.exit()
if cut is not None:
    inside = {k for k in lines
              if any(k == m or m.endswith('::*') and k.startswith(m[:-1]) for m in cut)}
    total = 0
    for a in sorted(inside):
        for b, c in sorted(edges[a].items()):
            if b in inside or b not in lines:
                continue
            total += c
            print(f'{a} -> {b}: {c}: ' + ', '.join(
                f'{it}:{n}' for it, n in sorted(named[(a, b)].items(), key=lambda x: -x[1])))
    print(f'{total} mentions leave the set ({sum(lines[k][0] for k in inside)} code lines, '
          f'{sum(lines[k][1] for k in inside)} test lines)')
    sys.exit()
if dot:
    print('digraph %s {' % crate)
    for a in sorted(edges):
        for b in sorted(edges[a]):
            print(f'  "{a}" -> "{b}";')
    print('}')
    sys.exit()
for a in sorted(lines):
    print(f'{a} ({ln(a)} lines) -> ' + ', '.join(f'{b}:{c}' for b, c in sorted(edges[a].items())))
print()
for c in sorted(sccs, key=len, reverse=True):
    if len(c) > 1:
        print(f'cycle ({len(c)} modules, {sum(lines[x][0] for x in c)} code lines, '
              f'{sum(lines[x][1] for x in c)} test lines): ' + ' '.join(c))
