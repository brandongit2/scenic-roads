"""What a change to a Rust file changed, by the names of its items (#1539), for
tools/dev/affected-tests.sh: a module's users are traced by these names, not by the module's path
alone (agentcore::agent, 21,000 lines, changed anywhere ran every test of pipeline's agent::build,
which names one of its items). When in doubt it names more: a test left out is a miss, one run
more isn't.

  changed(old, new, hunks) -> Names, or None for every user of the file, as by the module's path:
  a changed line outside every item (a file's inner attribute, a macro called at the top level), a
  changed macro, or a changed name a macro's definition or call names (the items a macro makes
  aren't named in the file: pipeline's agent/idents.rs).

The items: each `fn`, `struct`, `enum`, `union`, `trait`, `type`, `const`, `static`, `mod`,
`macro_rules!`, `use` and `impl`, and those inside an `impl`, `trait` or inline `mod` block, found
by their braces, brackets and semicolons in the source with its comments, strings and char literals
blanked (check-reuse.py's `blank`); an item's attributes and doc comments are its own, and the
names in an attribute's strings count (`#[serde(default = "f")]` names f). A changed line names:
  - nothing when only comments or blank space changed on it;
  - the innermost item holding it, by its name; a method (an `impl` block's `fn`) as a method
    (below); a trait's item, or a trait `impl`'s (`impl T for X`), also by the trait and the type;
    an inherent `impl`'s other item (an associated const or type) by its type (and trait); an
    `impl` or `trait` block's own lines (its header) by its type and trait and every method in it
    (a blanket `impl<T: Clone> Tr for T` reaches `5u8.d()`); a `struct`'s line, also its fields
    (each `f:` on it); an `enum`'s line, also its variant (the line's first word); a `use` line,
    also what it brings in (each path's last names), which this file's items then name.
Then, within the file, every item naming a changed one is changed too, until none more is (a
private helper changed changes the public items calling it).

A method `m` is named by `.m(`, `.m::<` or `::m`, a field `f` by `.f`, whatever type the file
names (its value's type may never be named). Every other name is named by the word, but for a lower-case word before `::`
(a module on a path: `agentcore::agent::jobs` names no function `agent`).
Another crate sees only what is `pub` (not `pub(crate)` nor `pub(super)`) and not under
`#[cfg(test)]` (that alone: `cfg(any(test, feature = …))` is another crate's too): a user in
another crate is matched against those names alone.

A user names the change when its source (comments and strings blanked, attributes' strings kept)
names one of the names. What this misses, publish's whole run catches: a trait method reached without naming the trait, the type
or the method (formatting with `{}`); a type reached through an alias of it.
"""
import importlib.util
import os
import re

_spec = importlib.util.spec_from_file_location("check_reuse", os.path.join(os.path.dirname(os.path.abspath(__file__)), "check-reuse.py"))
_reuse = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_reuse)
blank = _reuse.blank

WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
# (A word not a module on a path: not lower-case before `::` and a name.)
NAMED = re.compile(r"\b(?:[A-Z]\w*|[a-z_]\w*(?!\s*::\s*[A-Za-z_{*]))")
HEAD = re.compile(r"\b(fn|struct|enum|union|trait|type|const|static|mod)\s+(?:mut\s+)?(?!(?:fn|unsafe|async|extern|mut)\b)(?:r#)?([A-Za-z_]\w*)"
                  r"|\bmacro_rules!\s*([A-Za-z_]\w*)|\b(impl|use)\b")
PUB = re.compile(r"\bpub\b(?!\s*\()")
# (A field on a struct's line: `pub n: u8`, `n: u8`; group 1 its `pub`, not `pub(crate)`.)
FIELD = re.compile(r"(\bpub\s+)?(?<![\w:'])(?:r#)?([A-Za-z_]\w*)\s*:(?!:)")
TESTCFG = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]")
KEYWORDS = set("""as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod
move mut pub ref return self Self static struct super trait true type union unsafe use where while macro_rules""".split())


def words_of(b):
    return set(NAMED.findall(b))


def with_attrs(src, b):
    """Blanked source b with its attributes (`#[…]`, `#![…]`) as src has them, their strings' quotes
    as spaces: the names an attribute's strings hold (`default = "f"`) read as words. (Offsets
    kept; for words only: an attribute's string may hold a brace.)"""
    out = list(b)
    for m in re.finditer(r"#!?\[", b):
        depth, j = 0, m.end() - 1
        while j < len(b):
            if b[j] == "[": depth += 1
            elif b[j] == "]":
                depth -= 1
                if depth == 0: break
            j += 1
        for k in range(m.start(), min(j + 1, len(b))):
            c = src[k]
            out[k] = " " if c in "\"'" else c
    return "".join(out)


def user_text(src):
    """A user's source as names are matched in it."""
    return with_attrs(src, blank(src))


class Item:
    __slots__ = ("kind", "name", "a", "b", "kids", "owner", "trait", "pub", "test", "words")

    def __init__(self, kind, name, a, b, owner, trait, pub, test):
        self.kind, self.name, self.a, self.b = kind, name, a, b
        self.kids, self.owner, self.trait, self.pub, self.test, self.words = [], owner, trait, pub, test, None


def impl_names(head):
    """(the type, the trait or None) an `impl` header names: each one's last path segment, its
    generic arguments left out."""
    h = re.sub(r"\bimpl\b", " ", head, count=1)
    h = re.split(r"\bwhere\b", h)[0]
    while True:   # (generic arguments out, innermost first)
        h2 = re.sub(r"<[^<>]*>", " ", h)
        if h2 == h: break
        h = h2
    trait = None
    if re.search(r"\bfor\b", h):
        t, h = re.split(r"\bfor\b", h, maxsplit=1)
        ws = [w for w in WORD.findall(t) if w not in KEYWORDS]
        trait = ws[-1] if ws else None
    ws = [w for w in WORD.findall(h) if w not in KEYWORDS]
    return (ws[-1] if ws else None), trait


def items(b, a=0, z=None, owner=None, test=False):
    """The items of blanked source b between offsets a and z, each with its kids (an `impl`'s,
    `trait`'s or inline `mod`'s)."""
    z = len(b) if z is None else z
    out, i = [], a
    while i < z:
        # The item's text: to its `;` at depth 0, or to the `}` closing its first block.
        j, depth, block = i, 0, None
        while j < z:
            c = b[j]
            if c in "{([":
                if c == "{" and depth == 0 and block is None: block = j
                depth += 1
            elif c in "})]":
                depth -= 1
                if depth < 0: break
                if depth == 0 and c == "}" and block is not None:
                    j += 1; break
            elif c == ";" and depth == 0:
                j += 1; break
            j += 1
        if j <= i or not b[i:j].strip():
            break
        head = b[i:block] if block is not None else b[i:j]
        m = HEAD.search(head)
        kind, name, trait = "other", None, None
        if m:
            if m.group(1): kind, name = m.group(1), m.group(2)
            elif m.group(3): kind, name = "macro", m.group(3)
            else: kind = m.group(4)
        if kind == "impl":
            name, trait = impl_names(head[m.start():])
        pre = head[:m.start()] if m else head
        t = test or bool(TESTCFG.search(pre))
        it = Item(kind, name, i, j, owner, trait, bool(PUB.search(pre)), t)
        if kind in ("impl", "trait", "mod") and block is not None:
            it.kids = items(b, block + 1, j - 1, it if kind in ("impl", "trait") else None, t)
        out.append(it)
        i = j
    return out


def flat(its):
    for it in its:
        yield it
        yield from flat(it.kids)


def innermost(its, at):
    for it in its:
        if it.a <= at < it.b:
            return innermost(it.kids, at) or it
    return None


class Names:
    """What a change changed: words, methods and fields, each with whether another crate sees it."""

    def __init__(self):
        self.words, self.methods, self.fields = {}, {}, {}

    def _put(self, d, k, seen):
        d[k] = d.get(k, False) or seen

    def add(self, it, line=None):
        """Item it changed (line: the text of its changed line, when one of its own)."""
        o = it.owner
        if it.kind == "fn" and o is not None and o.kind == "impl":
            self._put(self.methods, it.name, (it.pub or o.trait is not None) and not it.test)
        if o is not None and (it.kind != "fn" or o.trait is not None or o.kind == "trait"):
            seen = not it.test and (o.trait is not None or (o.pub if o.kind == "trait" else it.pub))
            for w in (o.name, o.trait):
                if w: self._put(self.words, w, seen)
        if o is not None and o.kind == "trait" and it.name:
            self._put(self.methods, it.name, o.pub and not it.test)
        elif it.kind == "impl":
            for w in (it.name, it.trait):
                if w: self._put(self.words, w, not it.test)
        elif o is None and it.name:
            self._put(self.words, it.name, it.pub and not it.test)
        if it.kind in ("impl", "trait") and line is None:
            # (Its own lines, the header: every method of it, as callers reach them (a blanket
            # `impl<T: Clone> Tr for T` changed reaches `5u8.d()`, naming neither Tr nor T).)
            for k in it.kids:
                if k.kind == "fn" and k.name:
                    self._put(self.methods, k.name, (k.pub or it.trait is not None or it.kind == "trait") and not k.test and not it.test)
        if line is None: return
        if it.kind in ("struct", "union") and not line.lstrip().startswith("#"):
            # (A field, read as `.f` through a value whose type may be named nowhere.)
            body = line.split("{", 1)[1] if re.search(r"\b(struct|union)\b", line) and "{" in line else line
            for m in FIELD.finditer(body):
                self._put(self.fields, m.group(2), it.pub and bool(m.group(1)) and not it.test)
        if it.kind == "enum" and not line.lstrip().startswith("#"):
            ws = [w for w in WORD.findall(line) if w not in KEYWORDS]
            for w in ws[:1]: self._put(self.words, w, it.pub and not it.test)   # (a variant)
        elif it.kind == "use":
            for m in WORD.finditer(line):
                if m.group(0) not in KEYWORDS and not re.match(r"\s*::", line[m.end():]):
                    self._put(self.words, m.group(0), it.pub and not it.test)

    def named_in(self, b, ws=None, cross=False):
        """Whether text b (its words ws, when known; in another crate: `cross`) names one of these."""
        ws = words_of(b) if ws is None else ws
        if any(w in ws and (seen or not cross) for w, seen in self.words.items()): return True
        for m, seen in self.methods.items():
            if (cross and not seen) or m not in ws:
                continue
            if re.search(r"\.\s*%s\s*(?:\(|::\s*<)|::\s*%s\b" % (re.escape(m), re.escape(m)), b): return True
        for f, seen in self.fields.items():
            if (cross and not seen) or f not in ws:
                continue
            if re.search(r"\.\s*%s\b" % re.escape(f), b): return True
        return False

    def size(self):
        return tuple(len(d) + sum(d.values()) for d in (self.words, self.methods, self.fields))

    def __bool__(self):
        return bool(self.words or self.methods or self.fields)

    def __str__(self):
        return ", ".join(sorted(self.words) + [".%s()" % x for x in sorted(self.methods)] + [".%s" % x for x in sorted(self.fields)])


def line_spans(src, lines):
    """[(offset of the line's start, its end)] of 1-based line numbers in src."""
    starts = [0] + [m.end() for m in re.finditer(r"\n", src)]
    return [(starts[n - 1], starts[n] if n < len(starts) else len(src)) for n in lines if 0 < n <= len(starts)]


def changed(old, new, hunks):
    """The names a change of a file changed (old, new: its source before and after, "" for a file
    new or gone; hunks: [(old start, old count, new start, new count)], a unified diff's
    `@@ -a,b +c,d @@` lines), or None: every user (above)."""
    names = Names()
    every, macros = [], []
    for src, side in ((old, 0), (new, 1)):
        if not src: continue
        b, bc = blank(src), blank(src, strings=False)
        bw = with_attrs(src, b)
        its = items(b)
        lines = [n for h in hunks for n in range(h[2 * side], h[2 * side] + h[2 * side + 1])]
        for a, z in line_spans(src, lines):
            if not bc[a:z].strip():
                continue   # (comments and blank space alone)
            it = innermost(its, a + len(bc[a:z]) - len(bc[a:z].lstrip()))
            if it is None or it.kind in ("other", "macro"):
                return None   # (outside every item, a macro's definition or call)
            names.add(it, bw[a:z] if not it.kids else None)
        for it in flat(its):
            if it.kids: continue
            # (Its own name left out: a test helper `used` changed isn't the module's `fn used`.)
            it.words = words_of(bw[it.a:it.b]) - {it.name}
            (macros if it.kind in ("other", "macro") else every).append((it, bw[it.a:it.b]))
    # Within the file: an item naming a changed one is changed, until none more is.
    hit = set()
    while True:
        n = names.size()
        for it, text in every:
            if id(it) not in hit and names.named_in(text, it.words):
                hit.add(id(it)); names.add(it)
        if names.size() == n: break
    # A macro naming what changed may make items of it no line of the file names: every user.
    if any(names.named_in(text, it.words) for it, text in macros):
        return None
    return names
