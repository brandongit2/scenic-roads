"""Tests that measure the machine (task #441; run by tools/app/guards.sh). On the real clock a test
waits for what happens (store::clock::eventually, WATCHDOG) and checks code didn't wait by
store::clock::ended_before. It never holds a timing to a short bound of its own (under 30 s): an
assert of `elapsed() <` one (on one line or several, as a Duration or a count of seconds or
milliseconds, or of a variable holding an elapsed time, within the variable's block), or an
`if ...elapsed() > short { panic!`. Nor does it sleep a fixed time (std's thread::sleep, by path or
imported, or tokio's) and then assert, anywhere in the statements after the sleep up to the next
sleep or the end of its block: a loaded Mac (builds beside the suite, jobs at utility priority)
misses both. A sleep before asserts that something did NOT happen (a stopped job's file still the
same size, a cancelled call never run) can't fail under load: mark it `// (absence)`. Any other
sleep whose asserts hold however long it took (file times set apart, a head start whose asserts
hold either way) says why: `// (sleep ok: <why>)`. A mark goes on the sleep's line or the line
above. Comments are skipped. Prints each offence; exits 1 on any.
`python3 tools/app/test_timing.py --self-test` checks the guard on sample snippets (every scan runs
it first)."""
import os
import re
import sys

# A short bound: under 30 s, as a Duration or a count of seconds or milliseconds.
SHORT = (r'(?:(?:std::time::)?Duration::from_millis\(|(?:std::time::)?Duration::from_secs\((?:[0-9]|[12][0-9])\)'
         r'|ms\(|(?:std::time::)?Duration::from_secs_f(?:32|64)\()')
NUM = r'([0-9][0-9_]*)'
UNDER = {'secs': 30, 'millis': 30_000}
LESS = re.compile(r'elapsed\(\)(?:\.unwrap\(\))?[^;]*?<\s*' + SHORT)
LESS_N = re.compile(r'elapsed\(\)(?:\.unwrap\(\))?\.as_(secs|millis)\(\)\s*<\s*' + NUM)
PANIC_IF = re.compile(r'\bif\b[^{;]*elapsed\(\)(?:\.unwrap\(\))?\s*>=?\s*' + SHORT + r'[^{;]*\{[^}]*panic!')
TIMING_VAR = re.compile(
    r'let\s+(?:mut\s+)?(\w+)\s*(?::[^=]+)?=\s*[^;]*?\.elapsed\(\)(?:\.unwrap\(\))?(\.as_(?:secs|millis)\(\))?[^;]*;')
ASSERT = re.compile(r'(?<![\w])(?:debug_)?assert(?:_eq|_ne)?!\s*\(')
# A sleep: std's thread::sleep or tokio's, by path; `sleep(` alone when imported. Not a clock's `.sleep(`.
SLEEP = re.compile(r'(?:\bthread::|\btokio::time::|(?<![\w:])time::)sleep\(')
BARE_SLEEP = re.compile(r'(?<![\w.:])(?<!fn )sleep\(')
IMPORTS_SLEEP = re.compile(r'use\s+(?:std::thread|tokio::time)::(?:sleep\b|\{[^}]*\bsleep\b)')


def short(unit, n):
    return int(n.replace('_', '')) < UNDER[unit]


def statement(text, i):
    """The text from i to the end of its statement (the next ';' outside brackets), whitespace collapsed."""
    depth, j = 0, i
    while j < len(text):
        c = text[j]
        if c in '([{':
            depth += 1
        elif c in ')]}':
            depth -= 1
            if depth < 0:
                break
        elif c == ';' and depth == 0:
            break
        j += 1
    return ' '.join(text[i:j + 1].split())


def block_end(text, i):
    """Where the block holding position i closes, or the text's end."""
    depth = 0
    for j in range(i, len(text)):
        if text[j] == '{':
            depth += 1
        elif text[j] == '}':
            depth -= 1
            if depth < 0:
                return j
    return len(text)


def strip_comments(text):
    """`text` with its comments (// and /* */, outside strings, roughly) blanked, its lines kept."""
    out, k, q = [], 0, False
    while k < len(text):
        c = text[k]
        if q:
            out.append(c)
            if c == '\\' and k + 1 < len(text):
                out.append(text[k + 1])
                k += 2
                continue
            q = c != '"'
        elif c == '"':
            q = True
            out.append(c)
        elif text.startswith('//', k):
            e = text.find('\n', k)
            e = len(text) if e < 0 else e
            out.append(' ' * (e - k))
            k = e
            continue
        elif text.startswith('/*', k):
            e = text.find('*/', k + 2)
            e = len(text) if e < 0 else e + 2
            out.append(''.join(ch if ch == '\n' else ' ' for ch in text[k:e]))
            k = e
            continue
        else:
            out.append(c)
        k += 1
    return ''.join(out)


def marked(raw, n):
    """Whether the sleep on line n is marked fine, on its line or the line above."""
    return any('(absence)' in raw[i] or '(sleep ok:' in raw[i] for i in (n, n - 1) if 0 <= i < len(raw))


def check(text, name='<text>'):
    """The offences in `text` (a test region's source)."""
    bad = []
    raw = text.split('\n')
    code = strip_comments(text)
    lines = code.split('\n')
    starts = [0]
    for l in lines:
        starts.append(starts[-1] + len(l) + 1)

    def lineno(pos):
        return next(n for n in range(len(starts)) if starts[n + 1] > pos) + 1

    # Asserts of a short elapsed() bound, on one line or several.
    for m in ASSERT.finditer(code):
        s = statement(code, m.start())
        if LESS.search(s) or any(short(u, n) for u, n in LESS_N.findall(s)):
            bad.append(f'{name}:{lineno(m.start())}: a short elapsed() bound asserted: {s[:120]}')
    # A timing held in a variable, then asserted against a short bound within the variable's block.
    for m in TIMING_VAR.finditer(code):
        v, unit = m.group(1), (m.group(2) or '')[4:-2]
        w = r'\b' + re.escape(v) + r'\b'
        for a in ASSERT.finditer(code, m.end(), block_end(code, m.end())):
            s = statement(code, a.start())
            if unit:
                hit = any(short(unit, n) for n in re.findall(w + r'\s*<\s*' + NUM, s))
            else:
                hit = bool(re.search(w + r'\s*<\s*' + SHORT, s)) or any(
                    short(u, n) for u, n in re.findall(w + r'\.as_(secs|millis)\(\)\s*<\s*' + NUM, s))
            if hit:
                bad.append(f'{name}:{lineno(a.start())}: a short bound asserted on {v} (an elapsed time): {s[:120]}')
    # if ...elapsed() > short { panic!
    for m in PANIC_IF.finditer(code):
        bad.append(f'{name}:{lineno(m.start())}: a panic past a short elapsed() bound: {" ".join(m.group(0).split())[:120]}')
    # A fixed sleep, then an assert before the next sleep or the end of its block.
    sleeps = [SLEEP] + ([BARE_SLEEP] if IMPORTS_SLEEP.search(code) else [])

    def sleep_in(l):
        found = [f for f in (x.search(l) for x in sleeps) if f]
        return min(found, key=lambda f: f.start()) if found else None

    for n, l in enumerate(lines):
        sm = sleep_in(l)
        if not sm or marked(raw, n):
            continue
        # The sleep statement's end (`.await` included), then the statements after it.
        depth, j = 0, starts[n] + sm.start()
        while j < len(code) and not (code[j] == ';' and depth == 0):
            depth += code[j] in '([{'
            depth -= code[j] in ')]}'
            j += 1
        depth, k = 0, j + 1
        while k < len(code):
            c = code[k]
            if c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
                if depth < 0:
                    break
            if any(x.match(code, k) for x in sleeps):
                break
            if ASSERT.match(code, k):
                bad.append(f'{name}:{n + 1}: a fixed sleep, then an assert {lineno(k) - n - 1} lines on: {l.strip()}')
                break
            k += 1
    return bad


def regions():
    for dp, dn, fn in os.walk('crates'):
        dn[:] = [d for d in dn if d != 'target']
        for f in fn:
            if not f.endswith('.rs'):
                continue
            p = os.path.join(dp, f)
            lines = open(p).read().split('\n')
            whole = f.endswith('tests.rs') or os.sep + 'tests' + os.sep in p
            start = 0 if whole else next((i for i, l in enumerate(lines) if re.match(r'\s*#\[cfg\(test\)\]', l)), None)
            if start is not None:
                yield p, start, '\n'.join(lines[start:])


SAMPLES_BAD = [
    'assert!(t.elapsed() < Duration::from_millis(100), "at once");',
    'assert!(\n    t0.elapsed()\n        < std::time::Duration::from_secs(2),\n    "{:?}",\n    t0.elapsed()\n);',
    'let took = t.elapsed();\nfoo();\nassert!(took < Duration::from_secs(3));',
    'let ms_taken = t.elapsed().as_millis();\nassert!(ms_taken < 500);',
    'let n = t.elapsed().as_millis();\nassert!(n < 29_999);',
    'let took = t.elapsed();\nassert!(took.as_secs() < 5);',
    'assert!(t.elapsed().as_secs() < 2);',
    'if t.elapsed() > Duration::from_secs(5) {\n    panic!("not within five seconds");\n}',
    'std::thread::sleep(Duration::from_millis(300));\nassert!(done());',
    'std::thread::sleep(Duration::from_millis(300));\nlet s = read();\nlog(&s);\nassert_eq!(s, 2);',
    'tokio::time::sleep(Duration::from_millis(50)).await;\nassert!(done());',
    'use std::thread::sleep;\nfn f() {\n    sleep(Duration::from_millis(50));\n    assert!(done());\n}',
    '/* a note */ assert!(\n  t.elapsed() < ms(5)\n);',
]
SAMPLES_OK = [
    'store::clock::ended_before(t, Duration::from_secs(30), "at once");',
    'assert!(t.elapsed() < store::clock::WATCHDOG, "it ends");',
    'assert!(t.elapsed() < Duration::from_secs(600), "touched");',
    'assert!(began.elapsed().as_secs() < 600, "no worker came");',
    'assert!(t.elapsed().as_millis() < 30_000);',
    'let s = t.elapsed().as_secs();\nassert!(s < 600);',
    'let n = t.elapsed().as_millis();\nassert!(n < 30_000);',
    'let took = t.elapsed();\nassert!(took >= Duration::from_millis(1500));',
    'fn a() {\n    let took = t.elapsed();\n}\nfn b(took: u64) {\n    assert!(took < Duration::from_secs(3));\n}',
    '// assert!(t.elapsed() < Duration::from_millis(1));\n/* assert!(t.elapsed() < ms(1)); */',
    'std::thread::sleep(Duration::from_millis(300)); // (absence)\nassert_eq!(size(), before);',
    '// (sleep ok: the file times apart)\nstd::thread::sleep(Duration::from_millis(20));\nassert!(newer());',
    'while !done() {\n    std::thread::sleep(Duration::from_millis(10));\n}\nassert!(ok());',
    'loop {\n    assert!(Instant::now() < end);\n    std::thread::sleep(Duration::from_millis(250));\n}',
    'std::thread::spawn(move || {\n    std::thread::sleep(Duration::from_millis(400));\n    drop(held);\n});\nassert!(a.pool.is_some());',
    'std::thread::sleep(ms(1));\nstd::thread::sleep(ms(1));',
    'clock.sleep(Duration::from_secs(1));\nassert!(done());',
    'sleep(Duration::from_secs(1));\nassert!(done());',
]


def self_test():
    fails = 0
    for s in SAMPLES_BAD:
        if not check(s):
            print('missed:', s.replace('\n', ' | '))
            fails += 1
    for s in SAMPLES_OK:
        got = check(s)
        if got:
            print('flagged wrongly:', s.replace('\n', ' | '), got)
            fails += 1
    print(f'self-test: {len(SAMPLES_BAD) + len(SAMPLES_OK) - fails} of {len(SAMPLES_BAD) + len(SAMPLES_OK)} right')
    return fails == 0


if __name__ == '__main__':
    if not self_test():
        sys.exit(2)
    if '--self-test' in sys.argv:
        sys.exit(0)
    bad = []
    for p, start, text in regions():
        for b in check(text, p):
            f, n, rest = b.split(':', 2)
            bad.append(f'{f}:{int(n) + start}:{rest}')
    for b in bad:
        print(b)
    if bad:
        print('wait for the event (store::clock::eventually) or use store::clock::ended_before; a sleep before '
              "asserts that nothing happened is marked // (absence), any other that's fine // (sleep ok: why)")
        sys.exit(1)
