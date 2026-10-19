"""Tests that measure the machine (task #441; run by tools/app/guards.sh). On the real clock a test
waits for what happens (store::clock::eventually, WATCHDOG) and checks code didn't wait by
store::clock::ended_before; it never asserts a short elapsed() bound of its own, nor sleeps a fixed
time and then asserts that something happened: a loaded Mac (builds beside the suite, jobs at
utility priority) misses both. A sleep before an assert that something did NOT happen (a stopped
job's file still the same size, a cancelled call never run) can't fail under load: mark its line
`// (absence)`. Prints each offence; exits 1 on any."""
import os
import re
import sys

FAST = re.compile(r'assert!\(.*elapsed\(\)[^;]*< *(std::time::)?(Duration::from_millis|Duration::from_secs\([0-9]{1,2}\)|ms\()')
SLEEP = re.compile(r'thread::sleep\(')
ASSERT = re.compile(r'\s*(assert(_eq|_ne)?!|debug_assert)')
bad = []
for dp, dn, fn in os.walk('crates'):
    dn[:] = [d for d in dn if d != 'target']
    for f in fn:
        if not f.endswith('.rs'):
            continue
        p = os.path.join(dp, f)
        lines = open(p).read().split('\n')
        whole = f.endswith('tests.rs') or os.sep + 'tests' + os.sep in p
        start = 0 if whole else next((i for i, l in enumerate(lines) if re.match(r'\s*#\[cfg\(test\)\]', l)), None)
        if start is None:
            continue
        for i in range(start, len(lines)):
            l = lines[i]
            if FAST.search(l):
                bad.append(f'{p}:{i + 1}: a short elapsed() bound: {l.strip()}')
            if SLEEP.search(l) and not l.lstrip().startswith('//') and '(absence)' not in l:
                nxt = next((x for x in lines[i + 1:i + 4] if x.strip()), '')
                if ASSERT.match(nxt):
                    bad.append(f'{p}:{i + 1}: a fixed sleep, then an assert: {l.strip()}')
for b in bad:
    print(b)
if bad:
    print('wait for the event (store::clock::eventually) or use store::clock::ended_before; a sleep before '
          "an assert that nothing happened is marked // (absence)")
    sys.exit(1)
