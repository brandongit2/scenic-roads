# The review checklist

What every review of a change asks, written once (tools/dev/agent-rules.md, "Review"). A builder
goes through it over its own diff (`git diff main...HEAD`) as its last step before reporting, and
fixes what it finds; a reviewer reviews against it, plus what the brief asks. Each question comes
from findings reviews made (the observer's report, .claude/notes/DEV-EFFICIENCY.md §1.3, §2.7).
A question that doesn't apply is passed over; one that does is answered from the code, not from
memory of writing it.

## Keys and reuse

- **Can a key fail to change when its inputs do?** For every cache, memo, kept piece, content name
  or reuse rule the change makes or touches: list what the result reads, and check each is in the
  key. A key of a stand-in (the cache's key, not the value the plan used; a file's name, not its
  bytes) is the usual miss (#1076: `Context::rest` hashed the reach cache's key).
- Does a reader of an older record, file or format still read it, or refuse it by name?

## Tests

- **Are the tests meaningful?** Would each fail if the change were undone or broken (a mutation:
  flip the condition, drop the branch)? A test that passes either way proves nothing.
- **Do they cover the risky cases**, not only the easy one: the edges (empty, one, the limit), the
  error paths, each branch of a declaration or option, a restart or resume midway.
- Is a measurement's method sound: chosen on a train set and scored once on the test set (#1413),
  the before and after on the same inputs, the same Mac, the same load?
- Does a test hold to tools/dev/testing.md ("A test that can't flake", "Every test bounded")?

## Concurrency

- **Races and lock scope:** what two processes, Macs or threads can do at once with what this
  touches (a file, a record, the NAS, the board); whether a lock is held across the whole read,
  decide and write, and released on every path (an early return, a `?`, a panic); whether a lock
  is held across a wait it doesn't need.
- What one Mac's change does to the others' (per-Mac state written over another Mac's: #1506).

## The docs

- **Docs true to the code:** every doc line the change makes stale updated (grep the old names and
  numbers), planned work marked planned, no history of what used to be; comments say what the code
  does now.

## Surface

- **Dead code and surface left:** what the change made unused (functions, options, fields,
  settings, files, tests of removed paths, doc sections) deleted, and listed in the report.
- No hidden limit (a cap is a visible, configurable setting), no hand-kept figure.

## Where things go

- **Where outputs belong:** nothing under `inputs/` in git (the repo is public; NAS inputs go by a
  handover, #1451); scratch in the agent's own scratch folder, deleted; a measurement's figures in
  the report or a doc, not a stray file; logs whole (`tee`), never filtered in the pipe.
- No key, token or personal detail printed, logged or committed.

## The task

- Does it do what the task's text and the owner's note ask, all of it, and say what's left?
- Is it the general mechanism, not a fix of the one symptom?
