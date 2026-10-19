"""Tests that measure the machine (task #441; run by tools/app/guards.sh). On the real clock a test
waits for what happens (store::clock::eventually, WATCHDOG) and checks code didn't wait by
store::clock::ended_before. It never holds a timing to a short bound of its own: an assert of
`elapsed() <` one (on one line or several, or of a variable holding an elapsed time), or an
`if ...elapsed() > short { panic!`. Nor does it sleep a fixed time and then assert, anywhere in the
statements after the sleep up to the next sleep or the end of its block: a loaded Mac (builds beside
the suite, jobs at utility priority) misses both. A sleep before asserts that something did NOT
happen (a stopped job's file still the same size, a cancelled call never run) can't fail under
load: mark the sleep's line `// (absence)`. Any other sleep whose asserts hold however long it
took (file times set apart, a head start whose asserts hold either way) says why on its line:
`// (sleep ok: <why>)`. Prints each offence; exits 1 on any.
`python3 tools/app/test_timing.py --self-test` checks the guard on sample snippets."""
import os
import re
import sys

# A short bound: milliseconds, seconds under a minute.
SHORT = r'(?:(?:std::time::)?Duration::from_millis\(|(?:std::time::)?Duration::from_secs\([0-9]{1,2}\)|ms\(|(?:std::time::)?Duration::from_secs_f(?:32|64)\()'
LESS = re.compile(r'elapsed\(\)(?:\.unwrap\(\))?[^;]*?<\s*' + SHORT + r'|elapsed\(\)(?:\.unwrap\(\))?\.as_(?:secs\(\)\s*<\s*[0-9]{1,2}\b|millis\(\)\s*<\s*[0-9])')
PANIC_IF = re.compile(r'\bif\b[^{;]*elapsed\(\)(?:\.unwrap\(\))?\s*>=?\s*' + SHORT + r'[^{;]*\{[^}]*panic!')
TIMING_VAR = re.compile(r'let\s+(?:mut\s+)?(\w+)\s*(?::[^=]+)?=\s*[^;]*\.elapsed\(\)[^;]*;')
ASSERT = re.compile(r'\b(?:debug_)?assert(?:_eq|_ne)?!\s*\(')
SLEEP = re.compile(r'thread::sleep\(')


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


def strip_comments(line):
    """The line without a trailing // comment (not inside a string, roughly)."""
    q = False
    for k, c in enumerate(line):
        if c == '"' and (k == 0 or line[k - 1] != '\\'):
            q = not q
        elif not q and line.startswith('//', k):
            return line[:k]
    return line


def check(text, name='<text>'):
    """The offences in `text` (a test region's source)."""
    bad = []
    raw = text.split('\n')
    lines = [strip_comments(l) for l in raw]
    code = '\n'.join(lines)
    starts = [0]
    for l in lines:
        starts.append(starts[-1] + len(l) + 1)
    lineno = lambda pos: next(n for n in range(len(starts)) if starts[n + 1] > pos) + 1
    # Asserts of a short elapsed() bound, on one line or several.
    for m in ASSERT.finditer(code):
        s = statement(code, m.start())
        if LESS.search(s):
            bad.append(f'{name}:{lineno(m.start())}: a short elapsed() bound asserted: {s[:120]}')
    # A timing held in a variable, then asserted against a short bound.
    for m in TIMING_VAR.finditer(code):
        v = m.group(1)
        held = re.compile(r'\b' + re.escape(v) + r'\b(?:\s*<\s*' + SHORT + r'|\.as_secs\(\)\s*<\s*[0-9]{1,2}\b|\.as_millis\(\)\s*<\s*[0-9]|\s*<\s*[0-9])')
        for a in ASSERT.finditer(code, m.end()):
            s = statement(code, a.start())
            if held.search(s):
                bad.append(f'{name}:{lineno(a.start())}: a short bound asserted on {v} (an elapsed time): {s[:120]}')
    # if ...elapsed() > short { panic!
    for m in PANIC_IF.finditer(code):
        bad.append(f'{name}:{lineno(m.start())}: a panic past a short elapsed() bound: {" ".join(m.group(0).split())[:120]}')
    # A fixed sleep, then an assert before the next sleep or the end of its block.
    for n, l in enumerate(lines):
        if not SLEEP.search(l) or '(absence)' in raw[n] or '(sleep ok:' in raw[n]:
            continue
        pos = starts[n] + l.index('thread::sleep(')
        # Skip to the sleep statement's own end in the source.
        depth, j = 0, pos
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
            if code.startswith('thread::sleep(', k):
                break
            a = ASSERT.match(code, k)
            if a and (k == 0 or not (code[k - 1].isalnum() or code[k - 1] == '_')):
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
    'if t.elapsed() > Duration::from_secs(5) {\n    panic!("not within five seconds");\n}',
    'std::thread::sleep(Duration::from_millis(300));\nassert!(done());',
    'std::thread::sleep(Duration::from_millis(300));\nlet s = read();\nlog(&s);\nassert_eq!(s, 2);',
]
SAMPLES_OK = [
    'store::clock::ended_before(t, Duration::from_secs(30), "at once");',
    'assert!(t.elapsed() < store::clock::WATCHDOG, "it ends");',
    'assert!(t.elapsed() < Duration::from_secs(600), "touched");',
    'std::thread::sleep(Duration::from_millis(300)); // (absence)\nassert_eq!(size(), before);',
    'while !done() {\n    std::thread::sleep(Duration::from_millis(10));\n}\nassert!(ok());',
    'loop {\n    assert!(Instant::now() < end);\n    std::thread::sleep(Duration::from_millis(250));\n}',
    'std::thread::sleep(Duration::from_millis(400));\ndrop(held);\n});\nassert!(a.pool.is_some());',
    'let took = t.elapsed();\nassert!(took >= Duration::from_millis(1500));',
    'std::thread::sleep(ms(1));\nstd::thread::sleep(ms(1));',
    'assert!(began.elapsed().as_secs() < 600, "no worker came");',
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
              "asserts that nothing happened is marked // (absence)")
        sys.exit(1)
