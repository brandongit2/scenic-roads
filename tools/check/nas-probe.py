#!/usr/bin/env python3
"""The share's behaviour between two Macs (task #645): contended create-new, and how long one Mac's
reads stay stale after the other's write, rename or remove.

Run on one Mac (the timer); it starts its peer over ssh, the same script in `peer` mode, and talks
to it by lines of JSON on the ssh pipe. Every interval is timed on one Mac's own monotonic clock
(the one waiting), from the moment it is told the change is done: no clock offset enters; the
pipe's one-way latency (half the round trip, printed) is all it can understate by.

  nas-probe.py <peer-host> <scratch-folder> [--contend N] [--stale N] [--timeout S] [--out FILE]

The scratch folder must be one only this run uses, on the share, the same path on both Macs; it
is emptied and removed at the end. Rates stay far under the share's ~20 small-file creates/s.

Contend (N trials): both Macs open the same new name O_CREAT|O_EXCL (what the pool's create_new,
crates/pipeline/src/pool/nas.rs, does), the peer told to go at its next tick and this Mac aiming at
the same instant, jittered ±30 ms. Exactly one must win; the file's bytes must be the winner's.

Stale (N trials, the writer alternating between the Macs, kinds in turn):
  rewrite  the reader has read the file; the writer writes a temp and renames it over: until when
           the reader's plain read (`read`) and its F_NOCACHE read (`read_fresh`) still give old bytes
  create   the reader has stat'ed and listed the absent name; the writer creates it: until when
           stat, open and the folder's listing still say no such file
  remove   the reader has read, stat'ed and listed it; the writer removes it: until when stat,
           open and the listing still show it
Each probe's time to the new state is recorded; one still old at --timeout is censored (">").
"""
import json, os, random, shlex, subprocess, sys, time, fcntl

F_NOCACHE = 48  # <sys/fcntl.h>


def now():
    return time.monotonic()


def read_plain(p):
    try:
        with open(p, "rb") as f:
            return f.read()
    except FileNotFoundError:
        return None


def read_fresh(p):
    try:
        fd = os.open(p, os.O_RDONLY)
    except FileNotFoundError:
        return None
    try:
        fcntl.fcntl(fd, F_NOCACHE, 1)
        out = b""
        while True:
            b = os.read(fd, 65536)
            if not b:
                return out
            out += b
    finally:
        os.close(fd)


def exists(p):
    return os.path.exists(p)


def listed(d, name):
    return name in os.listdir(d)


def write_whole(p, b):
    t = p + ".tmp-probe"
    with open(t, "wb") as f:
        f.write(b)
        f.flush()
        os.fsync(f.fileno())
    os.rename(t, p)


def create_new(p, b):
    try:
        fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    except FileExistsError:
        return False
    try:
        os.write(fd, b)
        os.fsync(fd)
    finally:
        os.close(fd)
    return True


# The reader's side: prime, then poll until every probe sees the new state.
def prime(kind, d, name):
    p = os.path.join(d, name)
    if kind == "rewrite":
        read_plain(p), read_fresh(p)
    elif kind == "create":
        exists(p), read_plain(p), listed(d, name)
    else:
        read_plain(p), exists(p), listed(d, name)


def poll(kind, d, name, token, timeout):
    p = os.path.join(d, name)
    tok = token.encode()
    if kind == "rewrite":
        probes = {"read": lambda: read_plain(p) == tok, "read_fresh": lambda: read_fresh(p) == tok}
    elif kind == "create":
        probes = {"stat": lambda: exists(p), "open": lambda: read_plain(p) is not None, "list": lambda: listed(d, name)}
    else:
        probes = {"stat": lambda: not exists(p), "open": lambda: read_plain(p) is None, "list": lambda: not listed(d, name)}
    t0 = now()
    got = {}
    while len(got) < len(probes):
        el = now() - t0
        for k, f in probes.items():
            if k not in got and f():
                got[k] = round(now() - t0, 4)
        if el > timeout:
            for k in probes:
                got.setdefault(k, None)
            break
        time.sleep(0.02 if el < 2 else 0.2)
    return got


def act(op, d, name, token):
    p = os.path.join(d, name)
    if op == "seed":
        write_whole(p, token.encode())
    elif op == "rewrite":
        write_whole(p, token.encode())
    elif op == "create":
        create_new(p, token.encode())
    elif op == "remove":
        os.remove(p)
    elif op == "clean":
        try:
            os.remove(p)
        except FileNotFoundError:
            pass


def peer():
    for line in sys.stdin:
        m = json.loads(line)
        c = m["cmd"]
        r = {}
        if c == "ping":
            pass
        elif c == "contend":
            # Go at the next 50 ms tick after the delay asked, aiming at the same moment as the timer.
            time.sleep(m["delay"])
            r["won"] = create_new(m["path"], m["token"].encode())
        elif c == "prime":
            prime(m["kind"], m["dir"], m["name"])
        elif c == "poll":
            r["got"] = poll(m["kind"], m["dir"], m["name"], m["token"], m["timeout"])
        elif c == "act":
            act(m["op"], m["dir"], m["name"], m["token"])
        elif c == "quit":
            break
        sys.stdout.write(json.dumps(r) + "\n")
        sys.stdout.flush()


def main():
    a = sys.argv[1:]
    host, d = a[0], a[1]
    opt = dict(zip(a[2::2], a[3::2]))
    n_contend = int(opt.get("--contend", 200))
    n_stale = int(opt.get("--stale", 300))
    timeout = float(opt.get("--timeout", 120))
    out = open(opt["--out"], "a") if "--out" in opt else None
    os.makedirs(d, exist_ok=True)
    me = os.uname().nodename
    src = open(os.path.abspath(__file__)).read()
    proc = subprocess.Popen(
        ["ssh", host, "python3 -c " + shlex.quote(src) + " peer"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

    def ask(**m):
        proc.stdin.write(json.dumps(m) + "\n")
        proc.stdin.flush()
        return json.loads(proc.stdout.readline())

    def log(rec):
        rec["at"] = time.strftime("%Y-%m-%dT%H:%M:%S")
        line = json.dumps(rec)
        print(line, flush=True)
        if out:
            out.write(line + "\n")
            out.flush()

    rtts = []
    for _ in range(20):
        t = now()
        ask(cmd="ping")
        rtts.append(now() - t)
    rtt = sorted(rtts)[len(rtts) // 2]
    log({"type": "rtt", "median_s": round(rtt, 4), "max_s": round(max(rtts), 4), "timer": me, "peer": host})

    # Contend.
    for i in range(n_contend):
        name = f"c{i:04d}"
        p = os.path.join(d, name)
        jitter = random.uniform(-0.03, 0.03)
        # The peer sleeps 0.2 s after reading the command (rtt/2 after it is sent); this Mac aims at
        # the same moment, plus the jitter.
        proc.stdin.write(json.dumps({"cmd": "contend", "delay": 0.2, "path": p, "token": "peer"}) + "\n")
        proc.stdin.flush()
        time.sleep(max(0, 0.2 + rtt / 2 + jitter))
        mine = create_new(p, b"timer")
        theirs = json.loads(proc.stdout.readline())["won"]
        body = read_fresh(p)
        ok = (mine != theirs) and body == (b"timer" if mine else b"peer")
        log({"type": "contend", "i": i, "jitter_s": round(jitter, 4), "timer_won": mine, "peer_won": theirs,
             "body": body.decode() if body is not None else None, "ok": ok})
        os.remove(p)
        time.sleep(0.4)

    # Stale.
    kinds = ["rewrite", "create", "remove"]
    for i in range(n_stale):
        kind = kinds[i % 3]
        writer = "timer" if (i // 3) % 2 == 0 else "peer"
        name = f"s{i:04d}"
        old, new = f"old-{i}", f"new-{i}-{random.random()}"

        def do(op, token):
            if writer == "timer":
                act(op, d, name, token)
            else:
                ask(cmd="act", op=op, dir=d, name=name, token=token)

        if kind in ("rewrite", "remove"):
            do("seed", old)
        # The reader primes its caches with the old state, then the writer changes it.
        if writer == "timer":
            ask(cmd="prime", kind=kind, dir=d, name=name)
            do(kind, new)
            got = ask(cmd="poll", kind=kind, dir=d, name=name, token=new, timeout=timeout)["got"]
        else:
            time.sleep(0.5)  # the peer's seed, seen here at least once before priming
            prime(kind, d, name)
            do(kind, new)
            got = poll(kind, d, name, new, timeout)
        log({"type": "stale", "i": i, "kind": kind, "writer": writer, "got": got})
        act("clean", d, name, "")
        time.sleep(0.5)

    proc.stdin.write(json.dumps({"cmd": "quit"}) + "\n")
    proc.stdin.close()
    proc.wait()
    for n in os.listdir(d):
        os.remove(os.path.join(d, n))
    os.rmdir(d)


if __name__ == "__main__":
    if sys.argv[1:2] == ["peer"]:
        peer()
    else:
        main()
