"""tools/dev/board.py: small tasks started five at a time (batch), and health.sh's flag on a batch of
fewer than five while other small tasks waited, against a stand-in board (its items and events, the
writes recorded; the board's own rules, such as refusing a lone small task's start, are tested in
tools/devtools/test). Run by check.sh when board.py or this changes:
python3 -m unittest discover -s tools/dev/test -p 'test_board.py'."""
import json
import os
import subprocess
import sys
import threading
import unittest
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(os.path.dirname(HERE), "board.py")
T0, T1, T2, T3 = "2026-11-20T01:00:00.000Z", "2026-11-20T02:00:00.000Z", "2026-11-20T03:00:00.000Z", "2026-11-20T04:00:00.000Z"
B = "b1-20261120T0200"


class Board:
    """The board's /api/c/items and /api/c/events (all, or one item's, newest first), as lists of
    {id, data}; writes recorded."""

    def __init__(self):
        self.items, self.events, self.writes, self.reads = {}, [], [], []
        board = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def answer(self, body):
                b = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(b)))
                self.end_headers()
                self.wfile.write(b)

            def do_GET(self):
                u = urllib.parse.urlparse(self.path)
                col, q = u.path.rsplit("/", 1)[1], urllib.parse.parse_qs(u.query)
                board.reads.append(self.path)
                if col == "items":
                    docs = [{"id": k, "data": v} for k, v in board.items.items()]
                else:
                    evs = [e for e in board.events if "item" not in q or e["item"] == q["item"][0]]
                    if q.get("dir") == ["desc"]:
                        evs = sorted(evs, key=lambda e: e["t"], reverse=True)
                    docs = [{"id": str(i), "data": e} for i, e in enumerate(evs)]
                self.answer({"docs": docs, "total": len(docs)})

            def body(self):
                return json.loads(self.rfile.read(int(self.headers["content-length"])))

            def do_PATCH(self):
                id, b = self.path.rsplit("/", 1)[1], self.body()
                board.writes.append(("PATCH", id, b))
                board.items[id].update(b)
                self.answer({"id": id})

            def do_POST(self):
                b = self.body()
                board.writes.append(("POST", b["item"], b))
                board.events.append({**b, "t": T3})
                self.answer({"id": "e"})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"

    def task(self, num, status="", small=True, priority=None, small_at=T0, **more):
        d = {"kind": "task", "text": f"Task {num}", "parent": "", "num": num, "status": status, "assigned": "Opus subagent", "added": T0, **more}
        if small is not None:
            d["small"], d["smallAt"] = small, small_at
        if priority is not None:
            d["priority"] = priority
        self.items[f"i{num}"] = d

    def event(self, num, kind, text, t=T1):
        self.events.append({"item": f"i{num}", "kind": kind, "text": text, "t": t})

    def batch(self, nums, left="", t=T1, status="working"):
        """A batch started as board.py records it."""
        text = f"Batch {B} of small tasks: " + " ".join(f"#{n}" for n in nums) + (f"\nLeft out: {left}" if left else "")
        for n in nums:
            self.task(n, status=status, batch=B)
            self.event(n, "start", text, t)


def run(board, *args):
    p = subprocess.run([sys.executable, "-I", SCRIPT, *args], capture_output=True, text=True, env={**os.environ, "SCENIC_BOARD": board.url})
    return p.returncode, p.stdout, p.stderr


class Base(unittest.TestCase):
    def setUp(self):
        self.b = Board()
        self.addCleanup(self.b.server.server_close)
        self.addCleanup(self.b.server.shutdown)


class Health(Base):
    def small_part(self):
        code, out, err = run(self.b, "health")
        self.assertEqual(code, 0, err)
        return out.split("small tasks in flight", 1)[1]

    def test_a_batch_of_five_is_fine_whatever_waits(self):
        self.b.batch([1, 2, 3, 4, 5])
        self.b.items["i5"]["status"] = "review"
        self.b.task(6)
        out = self.small_part()
        self.assertIn(f"batch {B} of 5 (#1 #2 #3 #4 #5): ok", out)
        self.assertNotIn("<--", out)
        self.assertEqual(out.count("batch b"), 1, "a batch is listed once")

    def test_a_short_batch_is_flagged_for_a_task_small_at_its_start_neither_taken_nor_named(self):
        self.b.batch([1, 2], left="#3 (blocked by #4)")
        self.b.task(3, priority=2)
        self.b.task(4, priority=1)  # the blocker's number: not left out by being named in a reason
        out = self.small_part()
        self.assertIn(f"batch {B} of 2 (#1 #2): these small tasks were queued when it was recorded, 2026-11-20T02:00Z, and still are: #4 (P1)  <--", out)
        self.assertNotIn("#3 (P2)", out)

    def test_no_false_flag_for_a_task_marked_small_after_the_batch_or_not_queued(self):
        self.b.batch([1, 2])
        self.b.task(3, small_at=T2)  # marked small after the batch was recorded
        self.b.task(4, small=False)
        self.b.task(5, small=None)  # unclassified
        self.b.task(6, status="done")
        self.b.task(7, archived=True)
        self.b.task(8, status="working", small=False, batch="b8-old")  # (not small: no batch of small tasks)
        out = self.small_part()
        self.assertIn(f"batch {B} of 2 (#1 #2): ok", out)
        self.assertNotIn("<--", out)
        self.assertNotIn("#8", out)

    def test_a_small_task_in_flight_in_no_batch_is_flagged_whatever_its_status_but_waiting(self):
        for n, st in enumerate(["working", "backgrounded", "integrating", "review"], 1):
            self.b.task(n, status=st)
        self.b.task(5, status="waiting")  # (waiting on the owner before it starts: no batch yet)
        self.b.task(6, status="done")
        out = self.small_part()
        for n, st in enumerate(["working", "backgrounded", "integrating", "review"], 1):
            self.assertIn(f"#{n} ({st}) is in no batch  <-- RECORD IT IN ONE (board.py batch #{n} …), OR MARK IT NOT SMALL", out)
        self.assertNotIn("#5", out)
        self.assertNotIn("#6", out)

    def test_a_batch_in_flight_counts_whatever_its_members_statuses(self):
        self.b.batch([1, 2, 3, 4, 5], status="backgrounded")
        self.b.task(6)
        self.assertIn(f"batch {B} of 5 (#1 #2 #3 #4 #5): ok", self.small_part())

    def test_a_batch_without_its_start_event_is_flagged(self):
        self.b.task(1, status="working", batch="handmade")
        out = self.small_part()
        self.assertIn("batch handmade of 1 (#1): no start event records it  <-- NOT STARTED THROUGH board.py batch", out)

    def test_the_latest_batch_event_counts_and_nothing_under_way_says_none(self):
        self.b.batch([1], t=T0)
        self.b.event(1, "start", f"Batch {B} of small tasks: #1\nLeft out: #2 (conflicts with #9)", T1)
        self.b.task(2)
        self.assertIn(f"batch {B} of 1 (#1): ok", self.small_part())
        self.b.items["i1"]["status"] = "done"
        self.assertIn("\n  none", self.small_part())

    def test_parked_tasks_are_still_listed(self):
        self.b.task(1, status="backgrounded", small=False)
        self.b.event(1, "wait", "Waits on the rebuild.")
        code, out, _ = run(self.b, "health")
        self.assertIn("#1 backgrounded, last event", out)
        self.assertIn("Waits on the rebuild.", out)

    def test_an_unreadable_board_is_one_line(self):
        self.b.server.shutdown()
        self.b.server.server_close()
        code, out, err = run(self.b, "health")
        self.assertEqual(code, 0)
        self.assertRegex(out, r"^the board: UNREADABLE \(\w+")
        self.assertNotIn("Traceback", out + err)


class Batch(Base):
    def posts(self):
        return [w for w in self.b.writes if w[0] == "POST"]

    def test_five_small_tasks_are_started_with_their_batch_id_and_one_event_each(self):
        for n in range(1, 7):
            self.b.task(n)
        code, out, err = run(self.b, "batch", "1", "#2", "3", "4", "5", "--note", "One builder, worktree x.")
        self.assertEqual(code, 0, err)
        patches = [w for w in self.b.writes if w[0] == "PATCH"]
        self.assertEqual(sorted(w[1] for w in patches), ["i1", "i2", "i3", "i4", "i5"])
        bids = {w[2]["batch"] for w in patches}
        self.assertEqual(len(bids), 1, "one batch id on all")
        bid = bids.pop()
        self.assertRegex(bid, r"^b1-\d{8}T\d{4}$")
        self.assertTrue(all(w[2] == {"status": "working", "batch": bid} for w in patches), "the id in the start's own request")
        self.assertEqual({w[2]["text"] for w in self.posts()}, {f"Batch {bid} of small tasks: #1 #2 #3 #4 #5\nOne builder, worktree x."})
        self.assertEqual(len(self.posts()), 5)
        self.assertEqual({w[2]["kind"] for w in self.posts()}, {"start"})
        self.assertFalse([r for r in self.b.reads if r.rstrip("?").endswith("/events")], "no read of every event")
        code, out, _ = run(self.b, "health")
        self.assertIn(f"batch {bid} of 5 (#1 #2 #3 #4 #5): ok", out)

    def test_a_rerun_finishes_a_batch_cut_short_without_a_second_event(self):
        for n in range(1, 6):
            self.b.task(n)
        text = f"Batch {B} of small tasks: #1 #2 #3 #4 #5"
        for n in (1, 2):  # (the first run got this far)
            self.b.items[f"i{n}"].update(status="working", batch=B)
            self.b.event(n, "start", text, T2)
        self.b.items["i3"].update(status="working", batch=B)
        code, out, err = run(self.b, "batch", "1", "2", "3", "4", "5")
        self.assertEqual(code, 0, err)
        self.assertEqual(sorted(w[1] for w in self.b.writes if w[0] == "PATCH"), ["i4", "i5"], "the batch's id kept, the started ones left")
        self.assertEqual(sorted(w[1] for w in self.posts()), ["i3", "i4", "i5"])
        self.assertEqual({w[2]["text"] for w in self.posts()}, {text})
        self.b.writes.clear()
        self.assertEqual(run(self.b, "batch", "1", "2", "3", "4", "5")[0], 0)
        self.assertEqual(self.b.writes, [], "a third run writes nothing")

    def refused(self, *args, says):
        code, _, err = run(self.b, "batch", *args)
        self.assertEqual(code, 2, err)
        self.assertIn(says, err)
        self.assertEqual(self.b.writes, [], "nothing written")

    def test_fewer_than_five_needs_every_other_queued_small_task_named_with_a_reason(self):
        for n in range(1, 5):
            self.b.task(n, priority=n)
        self.refused("1", "2", says="while these small tasks are queued: #3 (P3), #4 (P4)")
        # (A blocker's number in a reason leaves nothing out: only an entry's leading #N does.)
        self.refused("1", "2", "--left-out", "#3 (blocked by #4)", says="queued: #4 (P4);")
        self.refused("1", "2", "--left-out", "#3, #4 (conflicts with #8)", says="'#3' gives no reason")
        self.refused("1", "2", "--left-out", "blocked: #3, #4", says="doesn't start with the task's #number")
        self.refused("1", "2", "--left-out", "#2 (busy), #3 (x), #4 (y)", says="#2 is both in the batch and left out")
        code, _, err = run(self.b, "batch", "1", "2", "--left-out", "#3 (blocked by #9, #10)", "--left-out", "#4 conflicts with #8")
        self.assertEqual(code, 0, err)
        self.assertIn("--left-out given 2 times: merged", err)
        self.assertIn("Left out: #3 (blocked by #9, #10); #4 conflicts with #8", {w[2]["text"] for w in self.posts()}.pop())

    def test_only_open_small_tasks_and_at_most_five(self):
        for n in range(1, 7):
            self.b.task(n)
        self.b.task(7, small=False)
        self.b.task(8, status="done")
        self.b.task(9, small=None)
        self.b.task(10, status="working", batch="other")
        self.refused("1", "2", "3", "4", "5", "6", says="a batch is at most 5")
        self.refused("1", "2", "3", "4", "7", says="#7 isn't marked small: start it alone")
        self.refused("1", "2", "3", "4", "9", says="#9 isn't marked small: classify it first")
        self.refused("1", "2", "3", "4", "8", says="#8 is done")
        self.refused("1", "2", "3", "4", "99", says="#99 isn't a task")
        self.refused("1", "1", says="named twice")
        self.refused("1", "2", "3", "4", "8", says="#8 is done, neither queued, waiting nor in flight")
        self.b.items["i1"].update(status="working", batch=B)
        self.refused("1", "10", "2", "3", "4", says="in different batches already (b1-20261120T0200, other)")

    def test_a_small_task_in_flight_in_no_batch_is_put_in_one_as_it_is(self):
        self.b.task(1, status="backgrounded")  # (marked small while under way)
        for n in range(2, 6):
            self.b.task(n)
        code, out, err = run(self.b, "batch", "1", "2", "3", "4", "5")
        self.assertEqual(code, 0, err)
        patch1 = [w[2] for w in self.b.writes if w[0] == "PATCH" and w[1] == "i1"]
        self.assertEqual(len(patch1), 1)
        self.assertEqual(set(patch1[0]), {"batch"}, "its status kept")
        self.assertIn("#1: already backgrounded, put in the batch", out)

    def test_a_small_task_waiting_on_the_owner_starts_with_its_batch_and_needs_no_naming(self):
        self.b.task(1, status="waiting")
        for n in range(2, 6):
            self.b.task(n)
        self.b.task(6, status="waiting")  # (waiting: not a queued task the batch must take or name)
        code, out, err = run(self.b, "batch", "1", "2", "3", "4")
        self.assertEqual(code, 2, "queued #5 must be taken or named")
        code, out, err = run(self.b, "batch", "1", "2", "3", "4", "5")
        self.assertEqual(code, 0, err)
        patch1 = [w[2] for w in self.b.writes if w[0] == "PATCH" and w[1] == "i1"]
        self.assertEqual(patch1[0]["status"], "working")
        self.assertIn("#1: started (it was waiting on you)", out)


if __name__ == "__main__":
    unittest.main()
