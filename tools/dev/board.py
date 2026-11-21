#!/usr/bin/env python3
"""The PM's board tools: starting small tasks two to five at a time, and health.sh's look at the board.

  board.py batch <num> ... [--left-out TEXT]... [--note TEXT]
      Starts small tasks as one piece of work (tools/dev/agent-rules.md, "Starting tasks"). Each one
      queued, or waiting on the owner and in no batch, is set under way with the batch's id (`batch`:
      the board refuses to start a small task without one in the same request), and each gets the
      batch's `start` event, the same text on all:
          Batch b1601-20261120T0300 of small tasks: #1601 #1602 #1603 #1604 #1605
          Left out: #1610 (blocked by #1600); #1611 (conflicts with #1534)     (only with --left-out)
          Measure first, #1603: <its note's Measure first: line>               (for each with one)
          <the note>                                                          (only with --note)
      Two to five tasks, whatever is ready and conflicts with nothing under way (owner, 21 Nov): the
      queued ones it leaves need no reason. --left-out is an optional note of them, a list of
      entries, each a task's number then why it waits, separated by commas or semicolons outside
      parentheses; only an entry's leading #N is the task left out. Given more than once, the lists
      are merged.
      Refused, nothing written, unless every task named is a small task (the board's `small`),
      queued, waiting or in flight, not archived and in no other batch; one whose text or note has a
      line beginning "Predicted gain:" only with a line "Measure first: <the probe or the records>;
      dropped below <figure>" in its note (#1577: measured before it's built); at most five; every
      left-out entry has a reason; and one task alone only while no other small task is queued. A
      task in flight is put in the batch as it is (its status kept): a small task in no batch, such
      as one marked small while under way, is recorded so.
      A rerun with the same tasks and options finishes a batch cut short and writes nothing more:
      a task already in the batch isn't set again, and one whose latest batch event has the same
      text gets no second one. A batch grown later is recorded by running it with all of it (its id
      kept). A batch's size is every task its start events name (a member that left counts).
  board.py batch-drop <batch> <num> <why>
      A finding drops one task, not the batch: puts that member back to queued (the board drops its
      batch id) and records why on its item ("Left batch <id>: <why>", as leave does); the rest of
      the batch goes on. Refused, nothing written, for a task that batch's start doesn't name, one
      not in its work (in flight or waiting with that batch id), or no reason; run again once it is
      queued with its reason recorded, it writes nothing.
  board.py leave <num> <why>
      Records that a member left its started batch, with why, as an event on its own item (kind
      note, "Left batch <id>: <why>"; a note of the PM's starting "Left batch <id>" counts too). It
      must be out of the batch's work already: queued again (the board drops its batch), abandoned,
      or in another batch. Refused, nothing written, for a task no batch's start names, one still in
      flight, waiting or done in that batch (done in it is finished with it, not left), an archived
      one (read-only; it needs no record), or no reason; run again, it writes nothing once its
      leaving is recorded.
  board.py health
      health.sh's board part: the parked tasks (backgrounded or integrating) with their last events,
      and the small tasks in flight (working, backgrounded, integrating, in review; not waiting) by batch
      (their `batch`). A batch is judged by its start events: their members, a member that left
      (dropped) included, are its size; each that left is listed with its reason. Flagged: a member
      that left (queued again, abandoned, in another batch) with no reason recorded (board.py
      batch-drop or leave); a task in the batch no start event names; a batch no start event
      records (an id made up, sent around board.py); a small task in flight in no batch.

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
BATCH = 5  # small tasks started together, at most (the owner's rule, 21 Nov: two to five)
HEAD = re.compile(r"^Batch (\S+) of small tasks:")
IN_FLIGHT = ("working", "backgrounded", "integrating", "review")  # (tools/devtools/board.mjs IN_FLIGHT)
WAITS = ("", "waiting")  # not started: queued, or waiting on the owner (also before it starts)
OUT = ("", "abandoned")  # a member in one of these has left its batch's work (queued again, or dropped)
NUM = re.compile(r"#(\d+)")
ENTRY = re.compile(r"^#(\d+)\s*(.*)$", re.S)
# A task whose value is a predicted gain (#1577) says so by a line of its text or note beginning
# "Predicted gain:", and is measured before it's built: its note's "Measure first:" line names the
# probe or the records, then "; dropped below" the figure.
GAIN = re.compile(r"^\s*Predicted gain:", re.M | re.I)
MEASURE = re.compile(r"^[ \t]*Measure first:[ \t]*(\S.*?;[ \t]*dropped below[ \t]+\S.*)$", re.M | re.I)


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
    """A batch event's (id, members' numbers), or None for another event."""
    lines = text.splitlines()
    m = HEAD.match(lines[0]) if lines else None
    if not m:
        return None
    return m.group(1), [int(n) for n in NUM.findall(lines[0][m.end():])]


def starts(events, bid):
    """The members a batch's start events (any member's) name, or None when there are none."""
    got = [p for p in (parse(e.get("text", "")) for e in events if e.get("kind") == "start") if p and p[0] == bid]
    return {n for p in got for n in p[1]} if got else None


def left_why(events, id, bid):
    """The reason recorded for a member's leaving of batch `bid` (a note on its item saying "Left
    batch <bid>", board.py leave's or the PM's own words), or None."""
    head = re.compile(rf"Left batch {re.escape(bid)}(?![\w-])[:\s]*")
    for e in sorted(events, key=lambda e: e.get("t", ""), reverse=True):
        m = e.get("item") == id and e.get("kind") == "note" and head.match(e.get("text", ""))
        if m:
            return e["text"][m.end():]
    return None


def left_how(items, id, bid):
    """How a member named by batch `bid`'s start has left its work, or None while it is still in it
    (in flight or waiting in the batch, or done in it: finished with it): queued again, abandoned, in
    another batch or none, archived (the board keeps an archived item's history read-only, so its
    leaving needs no reason), or gone from the board."""
    d = items.get(id)
    if d is None:
        return "not on the board"
    if d.get("batch") == bid and d.get("status") not in OUT:
        return None
    if archived(items, id):
        return "archived"
    return "queued" if d.get("status") == "" else d["status"] if d.get("batch") == bid else f"in batch {d['batch']}" if d.get("batch") else "in no batch"


def label(items, id):
    d = items[id]
    p = d.get("priority")
    return f"#{d['num']}" + (f" (P{p:g})" if p is not None else "")


def health_small(items, events):
    print(f"small tasks in flight (started two to {BATCH} at a time, as one piece of work; tools/dev/board.py batch):")
    flying = [id for id, d in sorted(items.items(), key=lambda p: p[1].get("num", 0))
              if d.get("kind") == "task" and d.get("small") is True and d.get("status") in IN_FLIGHT and not archived(items, id)]
    for id in (i for i in flying if not items[i].get("batch")):
        print(f"  #{items[id]['num']} ({items[id]['status']}) is in no batch  <-- RECORD IT IN ONE (board.py batch #{items[id]['num']} …), OR MARK IT NOT SMALL")
    ids = sorted({items[id]["batch"] for id in flying if items[id].get("batch")})
    nums = by_num(items)
    tag = lambda ns: " ".join(f"#{n}" for n in sorted(ns))
    for bid in ids:
        held = {d["num"] for d in items.values() if d.get("batch") == bid and d.get("status") != ""}
        rec = starts(events, bid)
        named = rec or held
        flags = [] if rec else [f"no start event records it  <-- NOT STARTED THROUGH board.py batch: RECORD IT (board.py batch {tag(held)})"]
        if held - named:
            flags.append(f"{tag(held - named)} in it, named by no start event  <-- RECORD IT (board.py batch {tag(named | held)})")
        lefts = []
        for n in sorted(named):
            where = left_how(items, nums.get(n), bid)
            if where is None:
                continue
            why = left_why(events, nums.get(n), bid)
            if where == "archived" and why is None:
                lefts.append(f"      #{n} left (archived)")
            elif why is None:
                flags.append(f"#{n} left after its start ({where}) with no reason recorded  <-- RECORD WHY (board.py leave {n} 'why')")
            else:
                lefts.append(f"      #{n} left ({where}): {why}")
        print(f"  batch {bid} of {len(named)} ({tag(named)}): " + ("; ".join(flags) if flags else "ok"))
        for line in lefts:
            print(line)
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
        elif GAIN.search(d.get("text", "") + "\n" + d.get("note", "")) and not MEASURE.search(d.get("note", "")):
            errs.append(f"#{n} is a predicted gain: its note needs a line 'Measure first: <the probe, or the records, "
                        "that measure the gain>; dropped below <the figure>' before it's built")
    for n, _ in left:
        if n in nums:
            errs.append(f"#{n} is both in the batch and left out")
    members = [ids[n] for n in nums if n in ids]
    had = sorted({items[m]["batch"] for m in members if items[m].get("batch") and items[m].get("status") != ""})
    if len(had) > 1:
        errs.append(f"the tasks named are in different batches already ({', '.join(had)})")
    if not errs and len(nums) == 1 and not had:
        # (One alone only while no other small task waits to go with it.)
        other = sorted((id for id, d in items.items() if d.get("kind") == "task" and d.get("small") is True and d.get("status") == ""
                        and not archived(items, id) and d["num"] not in nums),
                       key=lambda id: (items[id].get("priority") is None, items[id].get("priority") or 0, items[id]["num"]))
        if other:
            errs.append(f"#{nums[0]} alone while these small tasks are queued: {', '.join(label(items, m) for m in other)}; "
                        "start it with one or more of them (two to five)")
    if errs:
        return die("; ".join(errs) + ". Nothing written.")
    bid = had[0] if had else f"b{min(nums)}-{datetime.now(timezone.utc):%Y%m%dT%H%M}"
    measure = [f"Measure first, #{n}: {m.group(1).strip()}" for n in nums if (m := MEASURE.search(items[ids[n]].get("note", "")))]
    text = "\n".join([f"Batch {bid} of small tasks: " + " ".join(f"#{n}" for n in nums)]
                     + ([f"Left out: {'; '.join(f'#{n} {why}' for n, why in left)}"] if left else []) + measure + ([note] if note else []))
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


def leave(args):
    if len(args) != 2 or not re.fullmatch(r"#?\d+", args[0]):
        return die("usage: board.py leave <num> <why>")
    n, why = int(args[0].lstrip("#")), args[1].strip()
    if not why:
        return die(f"#{n}: give why it left its batch. Nothing written.")
    items = read_items()
    id = by_num(items).get(n)
    if id is None:
        return die(f"#{n} isn't on the board. Nothing written.")
    evs = item_events(id)
    bids = [p[0] for p in (parse(e.get("text", "")) for e in evs if e.get("kind") == "start") if p and n in p[1]]
    bid = items[id].get("batch") if items[id].get("batch") in bids else next(iter(bids), None)  # (else its newest)
    if bid is None:
        return die(f"#{n} is named by no batch's start event. Nothing written.")
    how = left_how(items, id, bid)
    if how is None:
        return die(f"#{n} is still {items[id]['status']} in batch {bid}: take it out of the batch's work first "
                   "(queued again or abandoned), then record why. Nothing written.")
    if how == "archived":
        return die(f"#{n} is archived: its history is read-only, and its leaving needs no record. Nothing written.")
    if left_why(evs, id, bid) is not None:
        print(f"#{n}: its leaving of batch {bid} is recorded already")
        return 0
    call("POST", "events", {"item": id, "kind": "note", "text": f"Left batch {bid}: {why}"})
    print(f"#{n}: left batch {bid}, recorded")
    return 0


def batch_drop(args):
    if len(args) != 3 or not re.fullmatch(r"#?\d+", args[1]):
        return die("usage: board.py batch-drop <batch> <num> <why>")
    bid, n, why = args[0], int(args[1].lstrip("#")), args[2].strip()
    if not why:
        return die(f"#{n}: give why it's dropped from batch {bid}. Nothing written.")
    items = read_items()
    id = by_num(items).get(n)
    if id is None:
        return die(f"#{n} isn't on the board. Nothing written.")
    evs = item_events(id)
    rec = starts(evs, bid)
    if rec is None or n not in rec:
        return die(f"#{n} is named by no start event of batch {bid}. Nothing written.")
    d = items[id]
    if left_why(evs, id, bid) is not None and d.get("status") == "":
        print(f"#{n}: dropped from batch {bid} already")
        return 0
    if d.get("batch") != bid or d.get("status") not in WAITS + IN_FLIGHT:
        return die(f"#{n} is {d.get('status') or 'queued'}{' in batch ' + d['batch'] if d.get('batch') else ''}, not in batch {bid}'s work: "
                   "board.py leave records why a member left otherwise. Nothing written.")
    call("PATCH", f"items/{id}", {"status": ""})
    call("POST", "events", {"item": id, "kind": "note", "text": f"Left batch {bid}: {why}"})
    print(f"#{n}: dropped from batch {bid}, queued again, why recorded; the rest of the batch goes on")
    return 0


def die(msg):
    print(f"board.py: {msg}", file=sys.stderr)
    return 2


def main(argv):
    if argv[:1] == ["health"] and len(argv) == 1:
        return health()
    if argv[:1] in (["batch"], ["leave"], ["batch-drop"]):
        try:
            return {"batch": batch, "leave": leave, "batch-drop": batch_drop}[argv[0]](argv[1:])
        except (OSError, ValueError) as e:
            return die(f"the board: {e} (what was written before this stays; run it again to finish)")
    if argv[:1] in (["-h"], ["--help"]):
        print(__doc__)
        return 0
    return die("usage: board.py batch <num> ... [--left-out TEXT]... [--note TEXT] | board.py batch-drop <batch> <num> <why> | "
               "board.py leave <num> <why> | board.py health")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
