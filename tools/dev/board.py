#!/usr/bin/env python3
"""The PM's board tools: starting small tasks five at a time, and health.sh's look at the board.

  board.py batch <num> ... [--left-out TEXT]... [--note TEXT]
      Starts small tasks as one piece of work (tools/dev/agent-rules.md, "Starting tasks"). Each one
      queued, or waiting on the owner and in no batch, is set under way with the batch's id (`batch`:
      the board refuses to start a small task without one in the same request), and each gets the
      batch's `start` event, the same text on all:
          Batch b1601-20261120T0300 of small tasks: #1601 #1602 #1603 #1604 #1605
          Left out: #1610 (blocked by #1600); #1611 (conflicts with #1534)     (only with --left-out)
          <the note>                                                          (only with --note)
      --left-out is a list of entries, each a task's number then why it waits, separated by commas
      or semicolons outside parentheses; only an entry's leading #N is the task left out. Given more
      than once, the lists are merged.
      Refused, nothing written, unless every task named is a small task (the board's `small`),
      queued, waiting or in flight, not archived and in no other batch; at most five; every left-out
      entry has a reason; and, when fewer than five, every other queued small task is named in
      --left-out (a waiting one needn't be). A task in flight is put in the batch as it is (its status
      kept): a small task in no batch, such as one marked small while under way, is recorded so.
      A rerun with the same tasks and options finishes a batch cut short and writes nothing more:
      a task already in the batch isn't set again, and one whose latest batch event has the same
      text gets no second one. A batch grown later is recorded by running it with all of it (its id
      kept).
  board.py health
      health.sh's board part: the parked tasks (backgrounded or integrating) with their last events,
      and the small tasks in flight (working, backgrounded, integrating, in review; not waiting) by batch
      (their `batch`). A batch of fewer than five is flagged when a task queued now was already small
      when the batch was recorded (its `smallAt`, the server's, no later than the batch's latest start
      event) and is neither in it nor named as left out; a batch no start event records (an id made
      up, sent around board.py) is flagged as not recorded; a small task in flight in no batch is
      flagged.

The definition of small is the board's (tools/devtools/schema.mjs SMALL_MEANS, GET /api/schema's
`smallMeans`). The board's address: SCENIC_BOARD, else http://mac-mini-de-brandon:8077 (board.sh).
"""
import json
import os
import re
import sys
import urllib.parse
import urllib.request
from datetime import datetime, timezone

BOARD = os.environ.get("SCENIC_BOARD", "http://mac-mini-de-brandon:8077")
BATCH = 5  # small tasks started together (the owner's rule, 20 Nov)
HEAD = re.compile(r"^Batch (\S+) of small tasks:")
IN_FLIGHT = ("working", "backgrounded", "integrating", "review")  # (tools/devtools/board.mjs IN_FLIGHT)
WAITS = ("", "waiting")  # not started: queued, or waiting on the owner (also before it starts)
NUM = re.compile(r"#(\d+)")
ENTRY = re.compile(r"^#(\d+)\s*(.*)$", re.S)


def call(method, path, body=None):
    req = urllib.request.Request(BOARD + "/api/c/" + path, method=method, data=None if body is None else json.dumps(body).encode(),
                                 headers={"content-type": "application/json", "user-agent": "board.py"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)


def read_items():
    return {d["id"]: d["data"] for d in call("GET", "items")["docs"]}


def item_events(id):
    """One item's events, newest first."""
    return [d["data"] for d in call("GET", "events?" + urllib.parse.urlencode({"item": id, "orderBy": "t", "dir": "desc"}))["docs"]]


def nowiso():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def archived(items, id):
    seen = set()
    while id and id in items and id not in seen:
        if items[id].get("archived"):
            return True
        seen.add(id)
        id = items[id].get("parent")
    return False


def by_num(items):
    return {d["num"]: id for id, d in items.items() if "num" in d}


def entries(text):
    """--left-out's entries: [(number, reason)], split at commas and semicolons outside parentheses.
    Raises ValueError for an entry that doesn't start with #N or gives no reason."""
    parts, depth, cur = [], 0, ""
    for ch in text:
        depth += {"(": 1, ")": -1}.get(ch, 0)
        if ch in ",;" and depth <= 0:
            parts.append(cur)
            cur = ""
        else:
            cur += ch
    parts.append(cur)
    out = []
    for p in (p.strip() for p in parts):
        if not p:
            continue
        m = ENTRY.match(p)
        if not m:
            raise ValueError(f"--left-out entry {p!r} doesn't start with the task's #number")
        if not m.group(2).strip(" ()"):
            raise ValueError(f"--left-out entry {p!r} gives no reason (e.g. '#{m.group(1)} (blocked by #N)')")
        out.append((int(m.group(1)), m.group(2).strip()))
    return out


def parse(text):
    """A batch event's (id, members' numbers, left-out numbers), or None for another event."""
    lines = text.splitlines()
    m = HEAD.match(lines[0]) if lines else None
    if not m:
        return None
    left = next((l[len("Left out:"):] for l in lines[1:] if l.startswith("Left out:")), "")
    try:
        out = {n for n, _ in entries(left)}
    except ValueError:
        out = set()
    return m.group(1), [int(n) for n in NUM.findall(lines[0][m.end():])], out


def missed(items, members, left_out, at):
    """The small tasks a batch recorded at `at` should have taken: none when it has five; else each
    queued now that was already small at `at`, neither in it nor named as left out, by priority."""
    if len(members) >= BATCH:
        return []
    out = [id for id, d in items.items() if d.get("kind") == "task" and d.get("small") is True and d.get("status") == ""
           and d.get("smallAt", "") <= at and not archived(items, id) and id not in members and d["num"] not in left_out]
    return sorted(out, key=lambda id: (items[id].get("priority") is None, items[id].get("priority") or 0, items[id]["num"]))


def label(items, id):
    d = items[id]
    p = d.get("priority")
    return f"#{d['num']}" + (f" (P{p:g})" if p is not None else "")


def health_small(items, events):
    print(f"small tasks in flight (started {BATCH} at a time, as one piece of work; tools/dev/board.py batch):")
    flying = [id for id, d in sorted(items.items(), key=lambda p: p[1].get("num", 0))
              if d.get("kind") == "task" and d.get("small") is True and d.get("status") in IN_FLIGHT and not archived(items, id)]
    for id in (i for i in flying if not items[i].get("batch")):
        print(f"  #{items[id]['num']} ({items[id]['status']}) is in no batch  <-- RECORD IT IN ONE (board.py batch #{items[id]['num']} …), OR MARK IT NOT SMALL")
    ids = sorted({items[id]["batch"] for id in flying if items[id].get("batch")})
    for bid in ids:
        members = [id for id, d in sorted(items.items(), key=lambda p: p[1].get("num", 0)) if d.get("batch") == bid and d.get("status") != ""]
        names = " ".join(f"#{items[m]['num']}" for m in members)
        rec = max((e for e in events if e.get("item") in members and e.get("kind") == "start" and (parse(e.get("text", "")) or ("",))[0] == bid),
                  key=lambda e: e.get("t", ""), default=None)
        left, at = (parse(rec["text"])[2], rec["t"]) if rec else (set(), nowiso())
        miss = missed(items, members, left, at)
        flags = [] if rec else ["no start event records it  <-- NOT STARTED THROUGH board.py batch: RECORD IT (board.py batch " + names + ")"]
        if miss:
            flags.append(f"these small tasks were queued{' when it was recorded, ' + at[:16] + 'Z,' if rec else ''} and still are: "
                         f"{', '.join(label(items, m) for m in miss[:BATCH])}{' …' if len(miss) > BATCH else ''}  <-- ADD THEM TO ITS WORK, "
                         f"OR NAME WHY EACH IS LEFT OUT (board.py batch {names} … --left-out '#N (why)')")
        print(f"  batch {bid} of {len(members)} ({names}): " + ("; ".join(flags) if flags else "ok"))
    if not flying:
        print("  none")


def health_parked(items, events):
    print("parked tasks (backgrounded or integrating): check what each waits on, then affirm or correct its status:")
    last = {}
    for e in events:
        if e.get("t", "") > last.get(e.get("item"), {}).get("t", ""):
            last[e.get("item")] = e
    now = datetime.now(timezone.utc)
    rows = [(id, d) for id, d in items.items() if d.get("status") in ("backgrounded", "integrating")]
    for id, d in sorted(rows, key=lambda p: (p[1].get("status"), p[1].get("num", 0))):
        e = last.get(id, {})
        age = ""
        if e.get("t"):
            dt = now - datetime.fromisoformat(e["t"].replace("Z", "+00:00"))
            age = f"{dt.days} d" if dt.days else f"{dt.seconds // 3600} h"
        print(f"  #{d.get('num')} {d.get('status')}, last event {age or 'none'} ago: {d.get('text', '')[:70]}")
        if e.get("text"):
            print(f"      {e['text'][:150]}")
    if not rows:
        print("  none")


def health():
    try:
        items = read_items()
        events = [d["data"] for d in call("GET", "events")["docs"]]
        health_parked(items, events)
        health_small(items, events)
    except Exception as e:  # (health.sh goes on: any failure is one line, never a traceback)
        print(f"the board: UNREADABLE ({type(e).__name__}: {e})")
    return 0


def batch(args):
    nums, lefts, note = [], [], ""
    while args:
        a = args.pop(0)
        if a in ("--left-out", "--note"):
            if not args:
                return die(f"{a} needs a value")
            v = args.pop(0)
            if a == "--left-out":
                lefts.append(v)
            else:
                note = v
        elif re.fullmatch(r"#?\d+", a):
            nums.append(int(a.lstrip("#")))
        else:
            return die(f"not a task number or option: {a}")
    if not nums:
        return die("no tasks named")
    if len(set(nums)) != len(nums):
        return die("a task is named twice")
    if len(nums) > BATCH:
        return die(f"{len(nums)} tasks: a batch is at most {BATCH}")
    try:
        left = [e for t in lefts for e in entries(t)]
    except ValueError as e:
        return die(f"{e}. Nothing written.")
    if len(lefts) > 1:
        print(f"board.py: --left-out given {len(lefts)} times: merged into one list", file=sys.stderr)
    items = read_items()
    ids = by_num(items)
    errs = []
    for n in nums:
        d = items.get(ids.get(n), {})
        if d.get("kind") != "task":
            errs.append(f"#{n} isn't a task on the board")
        elif archived(items, ids[n]):
            errs.append(f"#{n} is archived")
        elif d.get("small") is not True:
            errs.append(f"#{n} isn't marked small: " + ("classify it first" if d.get("small") is None else "start it alone"))
        elif d.get("status") not in WAITS + IN_FLIGHT:
            errs.append(f"#{n} is {d['status']}, neither queued, waiting nor in flight")
    for n, _ in left:
        if n in nums:
            errs.append(f"#{n} is both in the batch and left out")
    members = [ids[n] for n in nums if n in ids]
    had = sorted({items[m]["batch"] for m in members if items[m].get("batch") and items[m].get("status") != ""})
    if len(had) > 1:
        errs.append(f"the tasks named are in different batches already ({', '.join(had)})")
    seen = {n for n, _ in left}
    miss = [] if errs else missed(items, members, seen, nowiso())
    if miss:
        errs.append(f"{len(nums)} tasks, fewer than {BATCH}, while these small tasks are queued: {', '.join(label(items, m) for m in miss)}; "
                    "add them, or name each in --left-out with why (blocked by, or conflicting with, work under way)")
    if errs:
        return die("; ".join(errs) + ". Nothing written.")
    bid = had[0] if had else f"b{min(nums)}-{datetime.now(timezone.utc):%Y%m%dT%H%M}"
    text = "\n".join([f"Batch {bid} of small tasks: " + " ".join(f"#{n}" for n in nums)]
                     + ([f"Left out: {'; '.join(f'#{n} {why}' for n, why in left)}"] if left else []) + ([note] if note else []))
    for n in nums:
        id, d = ids[n], items[ids[n]]
        did = []
        if d["status"] in WAITS and d.get("batch") != bid:
            call("PATCH", f"items/{id}", {"status": "working", "batch": bid})
            did.append("started" + (" (it was waiting on you)" if d["status"] else ""))
        elif d.get("batch") != bid:
            call("PATCH", f"items/{id}", {"batch": bid})
            did.append(f"already {d['status']}, put in the batch")
        last = next((e for e in item_events(id) if e.get("kind") == "start" and parse(e.get("text", ""))), None)
        if last and last["text"] == text:
            did.append("its batch event already there")
        else:
            call("POST", "events", {"item": id, "kind": "start", "text": text})
            did.append("batch event posted")
        print(f"#{n}: {', '.join(did)} (batch {bid})")
    return 0


def die(msg):
    print(f"board.py: {msg}", file=sys.stderr)
    return 2


def main(argv):
    if argv[:1] == ["health"] and len(argv) == 1:
        return health()
    if argv[:1] == ["batch"]:
        try:
            return batch(argv[1:])
        except (OSError, ValueError) as e:
            return die(f"the board: {e} (what was written before this stays; run it again to finish)")
    if argv[:1] in (["-h"], ["--help"]):
        print(__doc__)
        return 0
    return die("usage: board.py batch <num> ... [--left-out TEXT]... [--note TEXT] | board.py health")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
