#!/usr/bin/env python3
"""Records dev jobs for the dev job dashboard (tools/devtools, /jobs/; tools/dev/testing.md, "Dev jobs").

Every event goes first to this Mac's spool, ~/Library/Logs/scenic/devjobs/spool.ndjson (a JSON line
each, appended: devjob.zsh writes the same lines), and the sampler sends it to the dev tools app
(POST /api/jobs/events, tools/devtools/jobs.mjs has the events). So a job never waits on the
network, and nothing is lost while the app is away: what wasn't sent is sent by the next sampler.

  devjob.py wrap --kind K --name N [--parse cargo,tests,remote] [--attr k=v]… -- <command…>
      runs the command as a span (its exit status is the command's), its output passed on as it
      comes, each line's time noted; its children record under it (SCENIC_DEVJOB_PARENT). After,
      what --parse names is read from the output: cargo's compiles, the test suites and each test
      (cargo test's and nextest's), remote-tests.sh's phases.
  devjob.py sampler      (started by the first live span; one a Mac) every TICK_S: sends the spool,
      reads cargo's timing reports; every SAMPLE_S: the Mac's load, memory and swap, naming the live
      spans, and ends those whose process is gone ("gone"). It exits once no span is live.
  devjob.py who          the attributes of a run of its own: the checkout, branch, commit, task, by whom
SCENIC_DEVJOB=0 records nothing (the tests of the scripts that record); SCENIC_DEVJOB_DIR moves the spool.
"""
import calendar
import fcntl
import glob
import html
import json
import os
import re
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

DIR = os.environ.get("SCENIC_DEVJOB_DIR") or os.path.expanduser("~/Library/Logs/scenic/devjobs")
SPOOL = os.path.join(DIR, "spool.ndjson")
LIVE = os.path.join(DIR, "live")
# The sampler: a tick sends the spool; every SAMPLE_S it samples the Mac and names the live spans
# (the app shows a running span its Mac hasn't named for a minute as lost: jobs.mjs LOST_S).
TICK_S = 2
SAMPLE_S = 10
# At most this much a POST (the app takes 32 MB; a publish's whole suite is about 1 MB).
POST_MAX = 4 << 20
HERE = os.path.dirname(os.path.abspath(__file__))


def on():
    return os.environ.get("SCENIC_DEVJOB", "1") != "0"


def board():
    """The dev tools app's address: SCENIC_BOARD, else board.sh's default (one place says it)."""
    if os.environ.get("SCENIC_BOARD"):
        return os.environ["SCENIC_BOARD"].rstrip("/")
    with open(os.path.join(HERE, "board.sh")) as f:
        m = re.search(r"SCENIC_BOARD:-([^}]+)\}", f.read())
    return m.group(1).rstrip("/")


def mac():
    m = os.environ.get("SCENIC_DEVJOB_MAC")
    if not m:
        m = subprocess.run(["scutil", "--get", "LocalHostName"], capture_output=True, text=True).stdout.strip() or os.uname().nodename.split(".")[0]
        os.environ["SCENIC_DEVJOB_MAC"] = m
    return m


_n = 0


def new_id():
    global _n
    _n += 1
    return f"{mac()}-{time.time_ns() // 1000}-{os.getpid()}-p{_n}"


def emit(ev):
    """One event, appended to the spool in one write (lines from several processes never mix)."""
    os.makedirs(DIR, exist_ok=True)
    fd = os.open(SPOOL, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
    try:
        os.write(fd, (json.dumps(ev, separators=(",", ":")) + "\n").encode())
    finally:
        os.close(fd)


def lstart(pid):
    r = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)], capture_output=True, text=True)
    return " ".join(r.stdout.split())


def live(span_id, pid):
    """The span is live while process `pid` (as started then: a reused pid isn't it) is."""
    os.makedirs(LIVE, exist_ok=True)
    with open(os.path.join(LIVE, span_id), "w") as f:
        f.write(f"{pid} {lstart(pid)}\n")
    ensure_sampler()


def unlive(span_id):
    try:
        os.unlink(os.path.join(LIVE, span_id))
    except FileNotFoundError:
        pass


def ensure_sampler():
    """Starts the sampler unless one runs (its lock held), detached, out of any checkout."""
    os.makedirs(DIR, exist_ok=True)
    with open(os.path.join(DIR, "sampler.lock"), "a") as f:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return
        fcntl.flock(f, fcntl.LOCK_UN)
    log = open(os.path.join(DIR, "sampler.log"), "a")
    subprocess.Popen([sys.executable, os.path.abspath(__file__), "sampler"], cwd="/", stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)


# ---- who started a run ---------------------------------------------------------------------------

def who(cwd=None):
    """A run of its own: its checkout (a worktree's name, "main checkout", "publish"), branch,
    commit, task (SCENIC_TASK, else the branch's latest "#<n>" in its commits since main) and whether
    an agent ran it (Claude Code's shells set CLAUDECODE)."""
    def git(*a):
        r = subprocess.run(["git", *a], cwd=cwd, capture_output=True, text=True)
        return r.stdout.strip() if r.returncode == 0 else ""
    out = {"by": "agent" if os.environ.get("CLAUDECODE") else "terminal"}
    if os.environ.get("CLAUDE_CODE_SESSION_ID"):
        out["session"] = os.environ["CLAUDE_CODE_SESSION_ID"][:8]
    top = git("rev-parse", "--show-toplevel")
    if top:
        out["checkout"] = os.path.basename(top) if "/.claude/worktrees/" in top else "publish" if "/scenic-publish/" in top else "main checkout"
        out["branch"] = git("rev-parse", "--abbrev-ref", "HEAD")
        out["commit"] = git("rev-parse", "--short", "HEAD")
    task = os.environ.get("SCENIC_TASK", "")
    if not task and top and out["checkout"] not in ("main checkout", "publish"):
        for s in git("log", "--format=%s", "main..HEAD").splitlines():
            m = re.search(r"#(\d+)", s)
            if m:
                task = m.group(1)
                break
    if task:
        out["task"] = task
    return out


# ---- reading a job's output ------------------------------------------------------------------------

CARGO_UNIT = re.compile(r"^\s+(Compiling|Checking|Documenting) (\S+) v")
CARGO_DONE = re.compile(r"^\s+Finished ")
CARGO_FAIL = re.compile(r"^error: could not compile ")
CARGO_LOCK = re.compile(r"^\s+Blocking waiting for file lock")
LT_RUN = re.compile(r"^\s+Running (?:(unittests|tests|benches) )?(\S+)(?: \((\S+)\))?")
LT_DOC = re.compile(r"^\s+Doc-tests (\S+)")
LT_TEST = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)\b")
LT_RESULT = re.compile(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out; finished in ([\d.]+)s")
NX_START = re.compile(r"^\s+Starting (\d+) tests? across (\d+) binar")
NX_TEST = re.compile(r"^\s+(?:TRY \d+ )?(PASS|FAIL|FLAKY|SIGSEGV|SIGABRT|SIGKILL|SIGTERM|SIGBUS|ABORT|TIMEOUT|LEAK|LEAK-FAIL)\s+\[\s*([\d.]+)s\]\s+(?:\(\s*\d+/\d+\)\s+)?(\S+)\s+(.+?)\s*$")
NX_SUMMARY = re.compile(r"^\s+Summary \[\s*([\d.]+)s\] (\d+) tests? run: (\d+) passed")
RT = "remote-tests: "
RT_KEY_WAIT = re.compile(r"^remote-tests: key \S+ is in use on the test Mac \((.*)\): waiting")
RT_KEY_GOT = re.compile(r"^remote-tests: got key \S+ after (\d+) s")
RT_COPIED = re.compile(r"^remote-tests: tree copied to the test Mac in (\d+) s \(this Mac's load ([\d.]+)\)")
RT_SLOT_WAIT = re.compile(r"^remote-tests: the test Mac's \d+ test slots are taken")
RT_SLOT_GOT = re.compile(r"^remote-tests: got a test-Mac slot after (\d+) s")
RT_ON = re.compile(r"^remote-tests: on the test Mac (\S+) \(load ([\d.]+), slot (\d+)")
RT_TOOK = re.compile(r"^remote-tests: the run took ([\d.]+)s on the test Mac, cpu ([\d.]+)s user \+ ([\d.]+)s sys")
RT_EXIT = re.compile(r"^remote-tests: exit (\d+) on the test Mac \(load ([\d.]+)\)")
RT_HERE = re.compile(r"^remote-tests: running here \((.*)\)$")
RT_RERUN = "--- remote result discarded, local rerun ---"
WANT = re.compile(r"^(\s+(Compiling|Checking|Documenting|Finished|Blocking|Running|Doc-tests|Starting|Summary|PASS|FAIL|FLAKY|SIG|ABORT|TIMEOUT|LEAK|TRY)|test |error: could not compile|remote-tests: |--- remote)")


def span(kind, name, t0, t1, status="ok", **attrs):
    return {"kind": kind, "name": name, "t0": t0, "t1": t1, "status": status, "attrs": {k: v for k, v in attrs.items() if v is not None}, "kids": []}


def suite_name(kind, target, path):
    """cargo test's `Running unittests src/lib.rs (…/deps/pipeline-1a2b…)`: "pipeline (unittests src/lib.rs)"."""
    exe = os.path.basename(path or target)
    exe = re.sub(r"-[0-9a-f]{16}$", "", exe)
    return f"{exe} ({kind + ' ' if kind else ''}{target})" if path else exe


def parse(lines, kinds, t_start, t_end):
    """Spans read from a job's output: [(time, line)] → [span], each with its `kids`."""
    out = []
    if "cargo" in kinds:
        comp = None
        lock = None
        for t, l in lines:
            if lock and not CARGO_LOCK.match(l):
                out.append(span("wait", "cargo's build-folder lock", lock, t))
                lock = None
            if CARGO_LOCK.match(l):
                lock = lock or t
            elif m := CARGO_UNIT.match(l):
                if comp is None:
                    comp = span("compile", "compile", t, t, crates=[])
                comp["attrs"]["crates"].append(m.group(2))
            elif comp is not None and (CARGO_DONE.match(l) or CARGO_FAIL.match(l)):
                comp["t1"] = t
                comp["status"] = "ok" if CARGO_DONE.match(l) else "fail"
                out.append(comp)
                comp = None
        if comp is not None:
            comp["t1"] = t_end
            comp["status"] = "fail"
            out.append(comp)
    if "tests" in kinds:
        suite = None
        nx = {}  # nextest: binary → suite
        nx_run = None
        summary = False

        def close(t, status=None):
            nonlocal suite
            if suite is not None:
                suite["t1"] = t
                if status:
                    suite["status"] = status
                else:
                    suite["status"] = "fail"
                    suite["attrs"]["why"] = "no result line (cut short, or the program crashed)"
                out.append(suite)
                suite = None
        for t, l in lines:
            if m := LT_RUN.match(l):
                close(t)
                suite = span("suite", suite_name(m.group(1), m.group(2), m.group(3)), t, t, "running")
            elif m := LT_DOC.match(l):
                close(t)
                suite = span("suite", f"{m.group(1)} (doctests)", t, t, "running")
            elif (m := LT_TEST.match(l)) and suite is not None:
                if m.group(2) != "ignored":
                    # (libtest says when a test ended, not when it began: a mark at its end.)
                    suite["kids"].append(span("test", m.group(1), t, t, "ok" if m.group(2) == "ok" else "fail", untimed="1"))
            elif (m := LT_RESULT.match(l)) and suite is not None:
                suite["t0"] = max(suite["t0"], t - float(m.group(7)))
                suite["attrs"].update(passed=m.group(2), failed=m.group(3), ignored=m.group(4), filtered=m.group(6))
                close(t, "ok" if m.group(1) == "ok" else "fail")
            elif m := NX_START.match(l):
                nx_run = span("tests", "nextest run", t, t, "running", tests=m.group(1), binaries=m.group(2))
            elif m := NX_SUMMARY.match(l):
                summary = True
                if nx_run is not None:
                    nx_run["t1"] = t
                    nx_run["attrs"].update(run=m.group(2), passed=m.group(3))
            elif (m := NX_TEST.match(l)) and not summary:
                st, dur, binary, name = m.group(1), float(m.group(2)), m.group(3), m.group(4)
                s = nx.get(binary)
                if s is None:
                    s = nx[binary] = span("suite", binary, t - dur, t)
                    s["_tests"] = {}
                s["_tests"][name] = span("test", name, t - dur, t, "ok" if st in ("PASS", "FLAKY") else "fail", **({"result": st} if st not in ("PASS", "FAIL") else {}))
                s["t0"], s["t1"] = min(s["t0"], t - dur), max(s["t1"], t)
        close(t_end)
        for s in nx.values():
            s["kids"] = sorted(s.pop("_tests").values(), key=lambda k: k["t0"])
            if any(k["status"] == "fail" for k in s["kids"]):
                s["status"] = "fail"
        if nx_run is not None:
            if nx_run["status"] == "running":
                nx_run["t1"] = nx_run["t1"] if summary else t_end
                nx_run["status"] = "fail" if any(s["status"] == "fail" for s in nx.values()) or not summary else "ok"
            nx_run["kids"] = sorted(nx.values(), key=lambda s: s["t0"])
            out.append(nx_run)
        else:
            out.extend(nx.values())
    attrs = {}
    if "remote" in kinds:
        key_wait = slot_wait = on = None
        for t, l in lines:
            if not l.startswith(RT) and l != RT_RERUN:
                continue
            if m := RT_KEY_WAIT.match(l):
                key_wait = (t, m.group(1))
            elif (m := RT_KEY_GOT.match(l)) and key_wait:
                out.append(span("wait", "the test Mac's key", key_wait[0], t, held_by=key_wait[1]))
                key_wait = None
            elif m := RT_COPIED.match(l):
                out.append(span("copy", "copy to the test Mac", t - int(m.group(1)), t, load=m.group(2)))
            elif RT_SLOT_WAIT.match(l):
                slot_wait = t
            elif (m := RT_SLOT_GOT.match(l)) and slot_wait:
                out.append(span("wait", "a test-Mac slot", slot_wait, t))
                slot_wait = None
            elif m := RT_ON.match(l):
                on = (t, m.group(1), m.group(2), m.group(3))
                attrs["ran_on"] = m.group(1)
            elif (m := RT_TOOK.match(l)) and on:
                wall = float(m.group(1))
                run = span("run", "run on the test Mac", max(on[0], t - wall), t, ran_on=on[1], load=on[2], slot=on[3],
                           cpu_user=m.group(2), cpu_sys=m.group(3))
                out.append(run)
            elif m := RT_EXIT.match(l):
                attrs["exit_on_test_mac"] = m.group(1)
                attrs["load_after"] = m.group(2)
            elif m := RT_HERE.match(l):
                attrs["ran_here"] = m.group(1)
            elif l == RT_RERUN:
                attrs["rerun_here"] = "the test Mac's run ended without a result"
        # The compiles and tests on the test Mac go under its run.
        runs = [s for s in out if s["kind"] == "run"]
        for r in runs:
            inside = [s for s in out if s is not r and s["kind"] in ("compile", "suite", "tests", "wait") and s["name"] != "the test Mac's key" and s["name"] != "a test-Mac slot"
                      and r["t0"] - 1 <= s["t0"] and s["t1"] <= r["t1"] + 1]
            r["kids"] += inside
            out = [s for s in out if s not in inside]
    return out, attrs


def emit_spans(spans, parent, m):
    for s in spans:
        sid = new_id()
        emit({"op": "span", "id": sid, "parent": parent, "kind": s["kind"], "name": s["name"], "mac": m,
              "t0": round(s["t0"], 3), "t1": round(max(s["t0"], s["t1"]), 3), "status": s["status"],
              "attrs": {k: v if isinstance(v, (list, str)) else str(v) for k, v in s["attrs"].items()}})
        emit_spans(s["kids"], sid, m)


def wrap(argv):
    kind = name = None
    kinds, attrs = set(), {}
    i = 0
    while i < len(argv) and argv[i] != "--":
        a = argv[i]
        if a == "--kind":
            kind = argv[i + 1]
        elif a == "--name":
            name = argv[i + 1]
        elif a == "--parse":
            kinds |= set(filter(None, argv[i + 1].split(",")))
        elif a == "--attr":
            k, _, v = argv[i + 1].partition("=")
            attrs[k] = v
        else:
            sys.exit(f"devjob.py wrap: unexpected {a}")
        i += 2
    cmd = argv[i + 1:]
    if not (kind and name and cmd):
        sys.exit("usage: devjob.py wrap --kind K --name N [--parse cargo,tests,remote] [--attr k=v]… -- <command…>")
    if not on():
        os.execvp(cmd[0], cmd)
    sid, m, parent = new_id(), mac(), os.environ.get("SCENIC_DEVJOB_PARENT") or None
    if not parent:
        attrs.update(who())
    t_start = time.time()
    emit({"op": "start", "id": sid, "parent": parent, "kind": kind, "name": name, "mac": m, "t": round(t_start, 3), "attrs": attrs})
    live(sid, os.getpid())
    env = dict(os.environ, SCENIC_DEVJOB_PARENT=sid)
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    # (A signal to the wrapper goes on to the command, which decides how it ends.)
    for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(sig, lambda n, _f: p.send_signal(n))
    out = sys.stdout.buffer
    kept, part = [], b""
    while True:
        try:
            b = os.read(p.stdout.fileno(), 65536)
        except InterruptedError:
            continue
        if not b:
            break
        try:
            out.write(b)
            out.flush()
        except BrokenPipeError:
            pass
        t = time.time()
        lines = (part + b).split(b"\n")
        part = lines.pop()
        for ln in lines:
            s = ln.decode("utf-8", "replace").rstrip("\r")
            if kinds and WANT.match(s):
                kept.append((t, s))
    # (Its CPU and its largest process's memory: what wait4 says of it and the processes it waited for.)
    while True:
        try:
            _, st, ru = os.wait4(p.pid, 0)
            break
        except InterruptedError:
            continue
    rc = os.waitstatus_to_exitcode(st)
    p.returncode = rc
    t_end = time.time()
    if part and kinds:
        s = part.decode("utf-8", "replace")
        if WANT.match(s):
            kept.append((t_end, s))
    try:
        spans, more = parse(kept, kinds, t_start, t_end)
        emit_spans(spans, sid, m)
    except Exception as e:  # (the job's status matters more than its parts)
        spans, more = [], {"parse_error": repr(e)[:500]}
    code = rc if rc >= 0 else 128 - rc
    more.update(exit=str(code), cpu=f"{ru.ru_utime + ru.ru_stime:.1f}", peak_mb=str(ru.ru_maxrss >> 20))  # (macOS: ru_maxrss in bytes)
    emit({"op": "end", "id": sid, "t": round(t_end, 3), "status": "ok" if rc == 0 else "stopped" if rc < 0 else "fail", "attrs": more})
    unlive(sid)
    ensure_sampler()
    return code


# ---- cargo's timing reports (tools/dev/bin/cargo --timings) -----------------------------------------

def timings(path, parent, m):
    """A report's crates, each a span under the cargo build `parent` (only those compiled)."""
    with open(path, encoding="utf-8", errors="replace") as f:
        s = f.read()
    units = json.loads(re.search(r"const UNIT_DATA = (\[.*?\]);\n", s, re.S).group(1))
    text = html.unescape(re.sub(r"<[^>]+>", " ", s))
    b = re.search(r"Build start:\s+(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(\.\d+)?Z", text)
    if not b:
        return 0
    start = calendar.timegm(time.strptime(b.group(1), "%Y-%m-%dT%H:%M:%S")) + float(b.group(2) or 0)
    info = {}
    for k, pat in (("fresh", r"Fresh units:\s+(\d+)"), ("dirty", r"Dirty units:\s+(\d+)"), ("concurrency", r"Max concurrency:\s+(\d+)")):
        if mm := re.search(pat, text):
            info[k] = mm.group(1)
    if info:
        emit({"op": "attr", "id": parent, "attrs": info})
    n = 0
    for u in units:
        if not u.get("duration"):
            continue
        target = u.get("target", "").strip().replace('"', "").strip("()")
        name = u["name"] + (f" ({target})" if target else "")
        a = {"version": u.get("version", ""), "mode": u.get("mode", "")}
        if u.get("rmeta_time") is not None:
            a["rmeta"] = str(u["rmeta_time"])
        emit({"op": "span", "id": new_id(), "parent": parent, "kind": "crate", "name": name, "mac": m,
              "t0": round(start + u["start"], 3), "t1": round(start + u["start"] + u["duration"], 3), "status": "ok", "attrs": a})
        n += 1
    return n


# ---- the sampler ---------------------------------------------------------------------------------

def sysctl(*names):
    r = subprocess.run(["sysctl", "-n", *names], capture_output=True, text=True)
    return r.stdout.splitlines()


def sample(live_ids):
    vals = sysctl("kern.memorystatus_level", "vm.swapusage", "hw.ncpu", "hw.memsize", "machdep.cpu.brand_string")
    ev = {"op": "sample", "mac": mac(), "t": round(time.time(), 3), "load": round(os.getloadavg()[0], 2), "live": live_ids}
    try:
        ev["memfree"] = float(vals[0])
        if mm := re.search(r"used = ([\d.]+)M", vals[1]):
            ev["swap"] = float(mm.group(1))
        ev["ncpu"] = int(vals[2])
        ev["mem"] = round(int(vals[3]) / 2**30, 1)
        ev["chip"] = vals[4].strip()
    except (IndexError, ValueError):
        pass
    emit(ev)


def flush(log):
    """Sends what the spool holds, oldest first; kept for the next try when the app can't be reached."""
    if os.path.exists(SPOOL):
        os.replace(SPOOL, os.path.join(DIR, f"sending-{time.time_ns()}.ndjson"))
        time.sleep(0.1)  # (a writer that opened it just before still writes into the file moved)
    for f in sorted(glob.glob(os.path.join(DIR, "sending-*.ndjson"))):
        with open(f, "rb") as o:
            data = o.read()
        chunks, cur = [], b""
        for ln in data.splitlines(keepends=True):
            if len(cur) + len(ln) > POST_MAX and cur:
                chunks.append(cur)
                cur = b""
            cur += ln
        if cur:
            chunks.append(cur)
        for c in chunks:
            try:
                req = urllib.request.Request(board() + "/api/jobs/events", data=c, method="POST", headers={"content-type": "application/x-ndjson", "user-agent": "devjob.py"})
                with urllib.request.urlopen(req, timeout=20) as r:
                    ans = json.load(r)
            except (urllib.error.URLError, OSError, ValueError) as e:
                log(f"can't send {os.path.basename(f)} to {board()}: {e}")
                return False
            for x in ans.get("refused", []):
                log(f"refused, {os.path.basename(f)}: {x}")
        os.unlink(f)
    return True


def sampler():
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    os.makedirs(LIVE, exist_ok=True)
    lock = open(os.path.join(DIR, "sampler.lock"), "a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        return 0
    said = {}

    def log(msg):  # (a message once a minute at most, the app away for hours)
        if time.time() - said.get(msg[:40], 0) >= 60:
            said[msg[:40]] = time.time()
            print(time.strftime("%Y-%m-%dT%H:%M:%S"), msg, flush=True)
    last = 0
    while True:
        ids = sorted(os.listdir(LIVE))
        if time.time() - last >= SAMPLE_S:
            last = time.time()
            started = {}
            r = subprocess.run(["ps", "-A", "-o", "pid=,lstart="], capture_output=True, text=True)
            for ln in r.stdout.splitlines():
                p, _, st = ln.strip().partition(" ")
                started[p] = " ".join(st.split())
            alive = []
            for i in ids:
                try:
                    with open(os.path.join(LIVE, i)) as f:
                        pid, _, st = f.read().strip().partition(" ")
                except FileNotFoundError:
                    continue
                # (" sample": a process this Mac is sampled for, no span: devjob.zsh dj_sampling.)
                only = st.endswith(" sample")
                st = st.removesuffix(" sample")
                if started.get(pid) == st:
                    if not only:
                        alive.append(i)
                elif os.path.exists(os.path.join(LIVE, i)):
                    if not only:
                        emit({"op": "end", "id": i, "t": round(time.time(), 3), "status": "gone", "attrs": {"why": f"its process ({pid}) ended without saying how"}})
                    unlive(i)
            ids = alive
            sample(ids)
        for h in glob.glob(os.path.join(DIR, "*.timings.html")):
            parent = os.path.basename(h)[: -len(".timings.html")]
            try:
                timings(h, parent, mac())
            except Exception as e:
                log(f"can't read {h}: {e!r}")
            os.unlink(h)
        ok = flush(log)
        if not os.listdir(LIVE):
            # (One more tick for what the last job wrote as it ended.)
            time.sleep(TICK_S)
            if not os.listdir(LIVE) and not glob.glob(os.path.join(DIR, "*.timings.html")):
                if os.path.exists(SPOOL) or not ok:
                    flush(log)
                return 0
        time.sleep(TICK_S)


def main(argv):
    if not argv:
        sys.exit(__doc__)
    if argv[0] == "wrap":
        return wrap(argv[1:])
    if argv[0] == "sampler":
        return sampler()
    if argv[0] == "who":
        print(json.dumps(who()))
        return 0
    sys.exit(f"devjob.py: unknown command {argv[0]}\n{__doc__}")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
