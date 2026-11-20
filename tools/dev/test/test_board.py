"""tools/dev/board.py: small tasks started five at a time (batch), and health.sh's flag on a small
task under way in a batch of fewer than five while other small tasks waited, against a stand-in
board (its items and events, the writes recorded). Run by check.sh when board.py or this changes:
python3 -m unittest discover -s tools/dev/test -p 'test_board.py'."""
import json
import os
import subprocess
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(os.path.dirname(HERE), "board.py")
T0, T1, T2 = "2026-11-20T01:00:00.000Z", "2026-11-20T02:00:00.000Z", "2026-11-20T03:00:00.000Z"


class Board:
    """The board's /api/c/items and /api/c/events, as lists of {id, data}; writes recorded."""

    def __init__(self):
        self.items, self.events, self.writes = {}, [], []
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
                col = self.path.rsplit("/", 1)[1]
                docs = [{"id": k, "data": v} for k, v in board.items.items()] if col == "items" else [{"id": str(i), "data": e} for i, e in enumerate(board.events)]
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
                board.events.append({**b, "t": T2})
                self.answer({"id": "e"})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"

    def task(self, num, status="", small=True, priority=None, added=T0, **more):
        d = {"kind": "task", "text": f"Task {num}", "parent": "", "num": num, "status": status, "assigned": "Opus subagent", "added": added, **more}
        if small:
            d["small"] = True
        if priority is not None:
            d["priority"] = priority
        self.items[f"i{num}"] = d

    def event(self, num, kind, text, t=T1):
        self.events.append({"item": f"i{num}", "kind": kind, "text": text, "t": t})


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
        return out.split("small tasks under way", 1)[1]

    def test_a_small_task_started_alone_while_others_waited_is_flagged(self):
        self.b.task(1, status="working")
        self.b.event(1, "status", "Status Queued → Under way (by curl)")
        self.b.task(2, priority=3)
        self.b.task(3, priority=1)
        out = self.small_part()
        self.assertIn("batch of 1 (#1)", out)
        self.assertIn("#3 (P1), #2 (P3)  <-- ADD THEM", out)

    def test_a_batch_of_five_is_fine_whatever_waits(self):
        for n in range(1, 6):
            self.b.task(n, status="review" if n == 5 else "working")
            self.b.event(n, "start", "Batch of small tasks: #1 #2 #3 #4 #5")
        self.b.task(6)
        out = self.small_part()
        self.assertIn("batch of 5 (#1 #2 #3 #4 #5): ok", out)
        self.assertNotIn("<--", out)
        self.assertEqual(out.count("batch of"), 1, "a batch is listed once")

    def test_a_short_batch_is_fine_when_the_rest_are_named_left_out_or_came_later_or_are_not_queued(self):
        for n in (1, 2):
            self.b.task(n, status="working")
            self.b.event(n, "start", "Batch of small tasks: #1 #2\nLeft out: #3 (conflicts with #9)")
        self.b.task(3)
        self.b.task(4, added=T2)  # added after the batch started
        self.b.task(5, status="working", small=False)  # not small
        self.b.task(6, status="done")
        self.b.task(7, small=False)
        self.b.task(8, archived=True)
        out = self.small_part()
        self.assertIn("batch of 2 (#1 #2): ok", out)
        self.assertNotIn("<--", out)

    def test_a_short_batch_is_flagged_for_a_small_task_neither_taken_nor_named(self):
        for n in (1, 2):
            self.b.task(n, status="working")
            self.b.event(n, "start", "Batch of small tasks: #1 #2\nLeft out: #3 (blocked by #9)")
        self.b.task(3)
        self.b.task(4)
        out = self.small_part()
        self.assertIn("batch of 2 (#1 #2), started 2026-11-20T02:00Z", out)
        self.assertIn("queued, as they still are: #4  <--", out)

    def test_the_latest_batch_event_wins_and_nothing_under_way_says_none(self):
        self.b.task(1, status="working")
        self.b.event(1, "start", "Batch of small tasks: #1", T0)
        self.b.event(1, "start", "Batch of small tasks: #1\nLeft out: #2 (blocked by #9)", T1)
        self.b.task(2)
        self.assertIn("batch of 1 (#1): ok", self.small_part())
        self.b.items["i1"]["status"] = "done"
        self.assertIn("\n  none", self.small_part())

    def test_parked_tasks_are_still_listed(self):
        self.b.task(1, status="backgrounded", small=False)
        self.b.event(1, "wait", "Waits on the rebuild.")
        code, out, _ = run(self.b, "health")
        self.assertIn("#1 backgrounded, last event", out)
        self.assertIn("Waits on the rebuild.", out)


class Batch(Base):
    def test_five_small_tasks_are_started_with_one_event_each(self):
        for n in range(1, 7):
            self.b.task(n, status="review" if n == 1 else "")
        code, out, err = run(self.b, "batch", "1", "#2", "3", "4", "5", "--note", "One builder, worktree x.")
        self.assertEqual(code, 0, err)
        patched = sorted(w[1] for w in self.b.writes if w[0] == "PATCH")
        self.assertEqual(patched, ["i2", "i3", "i4", "i5"], "the queued ones set under way, not #1")
        posts = [w for w in self.b.writes if w[0] == "POST"]
        self.assertEqual(len(posts), 5)
        self.assertEqual({w[2]["text"] for w in posts}, {"Batch of small tasks: #1 #2 #3 #4 #5\nOne builder, worktree x."})
        self.assertEqual({w[2]["kind"] for w in posts}, {"start"})
        # (And the health check reads it back as one batch of five.)
        code, out, _ = run(self.b, "health")
        self.assertIn("batch of 5 (#1 #2 #3 #4 #5): ok", out)

    def refused(self, *args, says):
        code, _, err = run(self.b, "batch", *args)
        self.assertEqual(code, 2)
        self.assertIn(says, err)
        self.assertEqual(self.b.writes, [], "nothing written")

    def test_fewer_than_five_needs_every_other_queued_small_task_named(self):
        for n in range(1, 5):
            self.b.task(n, priority=n)
        self.refused("1", "2", says="while these small tasks are queued: #3 (P3), #4 (P4)")
        self.refused("1", "2", "--left-out", "#3 (blocked by #9)", says="queued: #4 (P4);")
        code, _, err = run(self.b, "batch", "1", "2", "--left-out", "#3 (blocked by #9), #4 (conflicts with #8)")
        self.assertEqual(code, 0, err)
        self.assertIn("Batch of small tasks: #1 #2\nLeft out: #3 (blocked by #9), #4 (conflicts with #8)", {w[2]["text"] for w in self.b.writes if w[0] == "POST"})

    def test_only_open_small_tasks_and_at_most_five(self):
        for n in range(1, 7):
            self.b.task(n)
        self.b.task(7, small=False)
        self.b.task(8, status="done")
        self.refused("1", "2", "3", "4", "5", "6", says="a batch is at most 5")
        self.refused("1", "2", "3", "4", "7", says="#7 isn't marked small")
        self.refused("1", "2", "3", "4", "8", says="#8 is done")
        self.refused("1", "2", "3", "4", "99", says="#99 isn't a task")
        self.refused("1", "1", says="named twice")


if __name__ == "__main__":
    unittest.main()
