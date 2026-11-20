#!/usr/bin/env python3
"""The PM's board tools: starting small tasks five at a time, and health.sh's look at the board.

  board.py batch <num> ... [--left-out TEXT] [--note TEXT]
      Starts small tasks as one piece of work (tools/dev/agent-rules.md, "Starting tasks"): each
      queued one is set under way, and each gets the batch's `start` event, the same text on all:
          Batch of small tasks: #1601 #1602 #1603 #1604 #1605
          Left out: #1610 (blocked by #1600)        (only with --left-out)
          <the note>                                (only with --note)
      Refused, nothing written, unless every task named is a small task (the board's `small`), open
      and not archived; at most five; and, when fewer than five, every other small task queued now is
      named in --left-out with why (blocked by, or conflicting with, work under way). A task already
      under way may be named again: a batch grown later is recorded by running this with all of it.
  board.py health
      health.sh's board part: the parked tasks (backgrounded or integrating) with their last events,
      and the small tasks under way by batch, each batch of fewer than five flagged while other small
      tasks were queued when it started (and still are) without being named as left out. A small task
      under way with no batch event is a batch of one, started when its status last changed.

The definition of small is the board's (tools/devtools/schema.mjs SMALL_MEANS, GET /api/schema's
`smallMeans`). The board's address: SCENIC_BOARD, else http://mac-mini-de-brandon:8077 (board.sh).
"""
import json
import os
import re
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone

BOARD = os.environ.get("SCENIC_BOARD", "http://mac-mini-de-brandon:8077")
BATCH = 5  # small tasks started together (the owner's rule, 20 Nov)
HEAD = "Batch of small tasks:"
UNDER_WAY = ("working", "review")
NUM = re.compile(r"#(\d+)")


def call(method, path, body=None):
    req = urllib.request.Request(BOARD + "/api/c/" + path, method=method, data=None if body is None else json.dumps(body).encode(),
                                 headers={"content-type": "application/json", "user-agent": "board.py"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)


def read():
    """Every item ({id: data}) and every event (data), as the board has them."""
    items = {d["id"]: d["data"] for d in call("GET", "items")["docs"]}
    return items, [d["data"] for d in call("GET", "events")["docs"]]


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


def queued_small(items, at):
    """The small tasks queued now that were already on the board at `at`, by priority."""
    out = [id for id, d in items.items() if d.get("kind") == "task" and d.get("small") and d.get("status") == ""
           and not archived(items, id) and d.get("added", "") <= at]
    return sorted(out, key=lambda id: (items[id].get("priority") is None, items[id].get("priority") or 0, items[id]["num"]))


def missed(items, members, left_out, at):
    """The small tasks a batch should have taken: none when it has five; else every small task queued
    at `at` and still queued, neither in it nor named as left out."""
    if len(members) >= BATCH:
        return []
    return [id for id in queued_small(items, at) if id not in members and items[id]["num"] not in left_out]


def parse(text):
    """A batch event's members' and left-out tasks' numbers, or None for another event."""
    lines = text.splitlines()
    if not lines or not lines[0].startswith(HEAD):
        return None
    left = next((l for l in lines[1:] if l.startswith("Left out:")), "")
    return [int(n) for n in NUM.findall(lines[0])], {int(n) for n in NUM.findall(left)}


def batches(items, events, now):
    """Each batch with a small task under way: (members' ids, left-out numbers, its start time),
    once each."""
    nums = by_num(items)
    latest, changed = {}, {}
    for e in events:
        if e.get("kind") == "start" and parse(e.get("text", "")) and e.get("t", "") > latest.get(e["item"], {}).get("t", ""):
            latest[e["item"]] = e
        if e.get("kind") == "status" and e.get("t", "") > changed.get(e.get("item"), ""):
            changed[e["item"]] = e["t"]
    out, seen = [], set()
    for id, d in sorted(items.items(), key=lambda p: p[1].get("num", 0)):
        if d.get("kind") != "task" or not d.get("small") or d.get("status") not in UNDER_WAY or archived(items, id):
            continue
        e = latest.get(id)
        if e:
            ns, left = parse(e["text"])
            members, at = tuple(nums[n] for n in ns if n in nums), e["t"]
        else:
            members, left, at = (id,), set(), changed.get(id, now)
        if members not in seen:
            seen.add(members)
            out.append((members, left, at))
    return out


def label(items, id):
    d = items[id]
    p = d.get("priority")
    return f"#{d['num']}" + (f" (P{p:g})" if p is not None else "")


def health_small(items, events, now):
    print(f"small tasks under way (started {BATCH} at a time, as one piece of work; tools/dev/board.py batch):")
    rows = batches(items, events, now)
    for members, left, at in rows:
        names = " ".join(f"#{items[m]['num']}" for m in members)
        miss = missed(items, members, left, at)
        if miss:
            print(f"  batch of {len(members)} ({names}), started {at[:16]}Z, while these small tasks were queued, as they still are: "
                  f"{', '.join(label(items, m) for m in miss[:BATCH])}{' …' if len(miss) > BATCH else ''}  <-- ADD THEM TO ITS WORK, OR NAME WHY EACH IS LEFT OUT "
                  f"(board.py batch {names} … --left-out '…')")
        else:
            print(f"  batch of {len(members)} ({names}): ok")
    if not rows:
        print("  none")


def health_parked(items, events, now):
    print("parked tasks (backgrounded or integrating): check what each waits on, then affirm or correct its status:")
    last = {}
    for e in events:
        if e.get("t", "") > last.get(e.get("item"), {}).get("t", ""):
            last[e.get("item")] = e
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
        items, events = read()
    except (OSError, ValueError, KeyError) as e:
        print(f"the board: UNREADABLE ({e})")
        return 0
    now = datetime.now(timezone.utc)
    health_parked(items, events, now)
    health_small(items, events, now.isoformat(timespec="milliseconds").replace("+00:00", "Z"))
    return 0


def batch(args):
    nums, left_text, note = [], "", ""
    while args:
        a = args.pop(0)
        if a in ("--left-out", "--note") and args:
            v = args.pop(0)
            if a == "--left-out":
                left_text = v
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
    items, _ = read()
    ids = by_num(items)
    errs = []
    for n in nums:
        d = items.get(ids.get(n), {})
        if d.get("kind") != "task":
            errs.append(f"#{n} isn't a task on the board")
        elif archived(items, ids[n]):
            errs.append(f"#{n} is archived")
        elif not d.get("small"):
            errs.append(f"#{n} isn't marked small: start it alone, or mark it small first if it is")
        elif d.get("status") not in ("",) + UNDER_WAY:
            errs.append(f"#{n} is {d['status']}, neither queued nor under way")
    members = [ids[n] for n in nums if n in ids]
    left = {int(n) for n in NUM.findall(left_text)}
    now = datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")
    miss = [] if errs else missed(items, members, left, now)
    if miss:
        errs.append(f"{len(nums)} tasks, fewer than {BATCH}, while these small tasks are queued: {', '.join(label(items, m) for m in miss)}; "
                    "add them, or name each in --left-out with why (blocked by, or conflicting with, work under way)")
    if errs:
        return die("; ".join(errs) + ". Nothing written.")
    text = "\n".join([f"{HEAD} " + " ".join(f"#{n}" for n in nums)] + ([f"Left out: {left_text}"] if left_text else []) + ([note] if note else []))
    for n in nums:
        id = ids[n]
        if items[id]["status"] == "":
            call("PATCH", f"items/{id}", {"status": "working"})
        call("POST", "events", {"item": id, "kind": "start", "text": text})
        print(f"#{n}: {'started' if items[id]['status'] == '' else 'already ' + items[id]['status']}, batch recorded")
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
            return die(f"the board: {e}")
    if argv[:1] in (["-h"], ["--help"]):
        print(__doc__)
        return 0
    return die("usage: board.py batch <num> ... [--left-out TEXT] [--note TEXT] | board.py health")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
