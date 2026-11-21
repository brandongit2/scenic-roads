# Standing rules for agents

Rules every agent working on this repo follows, whatever its brief says. A brief adds the task;
it never needs to repeat these. Rules marked *enforced* are also checked by a hook; the rest rely
on the agent.

## Where things run

- **Tests, builds and check.sh run on the M4**, in your own worktree. Follow
  `tools/dev/testing.md` for test runs. *Enforced for test runs*: on any other Mac a `cargo test` or
  `cargo nextest run` is refused (the hook `tools/dev/test-guard.py` for the commands it reads, the
  `tools/dev/bin/cargo` shim, advisory, for the rest, from a worktree); run `tools/dev/check.sh`, or
  `tools/app/remote-tests.sh <key> <worktree> '<cargo command>'`, which send them to the M4. Only the PM lets
  tests run elsewhere (testing.md, "Where a test run goes"): a command of yours setting `SCENIC_TESTS_LOCAL`
  or the like is refused; if the M4 is away, say so in your report. Builds, clippy and fmt are not refused on
  another Mac: check.sh itself runs them where it is.
- **NAS-heavy runs go to the Mac mini** (`ssh m6`), where the NAS is local: anything that reads or
  writes a lot of NAS data, such as a bit-identity run (`scenic-build … --root
  /Volumes/personal/projects/scenic-roads`), measuring what a NAS-reading step reads, scanning large
  NAS folders, or bulk copies. A small read (a log, a status file, one record) is fine from any
  Mac. On the mini:
  - use a scratch folder of your own and delete it afterwards;
  - timings taken there are skewed by its build jobs: when the numbers matter, ask the PM for a
    quiet period rather than pausing anything yourself.
- **Never run `scenic pause` or `scenic resume`** (or the menu bar's Pause/Resume) on any Mac, `--here`
  or not. A Mac the owner paused stays paused, and resuming it undoes the owner's own choice; a Mac
  paused from the pool is still free for your measurements. For a quiet period, ask the PM.
- **Never mount or unmount the NAS share yourself:** each Mac's agent mounts it (by its LAN name at
  home, through Tailscale away) and remounts it as the network changes.
- **Memory:** nothing caps an agent's own run (the pool's memory guard covers only pool jobs), and
  runs on real data have crashed Macs. Watch the footprint of a long run on real data.

## Scratch

- **The session's scratchpad is shared:** every subagent of a PM session gets the same scratchpad
  folder (the system prompt's "Scratchpad directory",
  `/private/tmp/claude-<uid>/<project>/<session>/scratchpad`). Keep your files in a folder of your
  own inside it, made once at your start with `mktemp -d <scratchpad>/t<task>-XXXXXX` (for #1521,
  `mktemp -d <scratchpad>/t1521-XXXXXX`). Note the path it prints and use it from then on. Every
  agent does this, with a worktree or without: a reviewer works in its builder's worktree, so a
  name taken from the worktree would be the builder's.
- **Clean up only your own folder:** `rm -rf <the path mktemp printed>`, and say in your report
  where your files were. *Enforced*: `tools/dev/shell-guard.py` refuses `rm` or `trash` of the
  scratchpad, of a folder above it or of a glob directly in it (`<scratchpad>/*`), and a `find`
  from any of those that deletes (`-delete`, `-exec rm`, piped to `xargs rm`).

## Processes

- **Never signal the pool's processes** (`scenic agent` and its jobs, `scenic-build`, `scenic gc`,
  the map server) on any Mac. Report what you see; the owner decides.
- **Kill only your own processes, by exact PID** (keep `$!`). Never `pkill -f` or `killall`.
- **Don't edit a script that is running** (check `ps` first): shells read scripts as they go.
- **Never filter a test run in its pipe:** `tee` the full output to a log and search the log.
- Keep each foreground command under 10 minutes; split longer runs or run them in the background.

## Turns

Each turn re-reads the whole context, so a turn spent on a refusal, a slice or a poll is costly.

- **Edit with the Edit tool** after a Read (Write for a new file). For a change a script makes,
  write the script to your scratch folder and run it by its path. Never `python3 - <<EOF`,
  `cat > f <<EOF` or the like in a worktree: Claude Code's worktree check refuses a heredoc it
  can't verify ("too complex to verify"), and the retry costs a second turn. Give paths in full
  (or `git -C <worktree>`) rather than `cd`: a `cd` out of the working folders is refused or
  asks too.
- **Read the part of a file you need in one call:** find it with `grep -n`, then one Read with
  `offset` and `limit` round it, not successive `sed -n` slices of 40 lines, one turn each. Don't
  read a big file whole either: it stays in every later turn's context (testing.md is 900 lines;
  its headings are a `grep -n '^#'` away).
- **Wait on a long run with one background command** (the Bash tool's `run_in_background`) and
  end your turn: its notification wakes you when it exits. Not a poll loop in slices
  (`for i in $(seq 1 19); do …; sleep 9; done`, one turn each), and while a check.sh of your
  worktree runs, report-guard lets your stop through.
- **When a command is moved to the background** (a foreground one past its time), wait for its
  notification; never start it again beside it.
- **The bounded waits the hooks accept**, when a wait must be a loop:
  - on a process: `while kill -0 <pid> 2>/dev/null; do sleep 30; done` (it ends with the process);
  - on a file's text: `until grep -q DONE <log>; do [ -e <log> ] || break; [ $SECONDS -gt 1800 ]
    && break; sleep 20; done`, the same over `ssh`;
  - the Monitor tool, with an until-loop, for a condition.
  Refused: a bare `sleep N` in the foreground (Claude Code); a loop over `pgrep -f`, or on a PID
  taken from one inline (`kill -0 $(pgrep -f x)`), or, with no other bound, from a variable set by
  one (it matches its own shell; a `[p]attern` can't); and any `while`/`until` loop (zsh's `while c { … }`,
  `for ((;;))`) that sleeps, or an `until grep`, with no bound in its own condition or body: on the
  command line, in a substitution, `sh -c`, `eval`, a function, over `ssh`, or in a heredoc or
  here-string fed or piped to a shell (tools/dev/shell-guard.py). A bound elsewhere on the line,
  in a loop inside it, or in a comment doesn't count. Not caught: a script run from a file, a loop
  in another language (`python3 -c 'while True: …'`), a string piped to a shell (`echo … | bash`).
- **Every wait loop has a deadline and stops when what it watches is gone** (the file, the
  folder, the process): a loop whose log was deleted with its scratch folder never ends (#1361's
  ran for two days). *Enforced* for the deadline: shell-guard.py refuses a loop that sleeps with
  no bound in it. Bounds: a comparison (`[`, `[[`, `test`, `(( … ))`; -lt/-le/-gt/-ge/-eq/-ne,
  `=`, `!=`), as the loop's condition or ending it (`&& break`, `exit`, `return`, `if …; then
  break`), of the clock (`$SECONDS`, `$(date +%s)`, also inside `$(( … ))`: `[ $((SECONDS-S))
  -gt 580 ]`) or of a counter the loop changes by arithmetic (`let i++`, `(( i++ ))`,
  `i=$((i+1))`); a process's wait as the condition (`kill -0`, `ps -p`, `pgrep -x`, any of them
  among ssh's remote commands: `until ssh m6 'grep -q done log || ! kill -0 N'`) or `kill -0 <pid>
  || break`; `timeout` round the whole loop (`timeout 600 zsh -c '…'`). A loop reading lines
  (`while read`) passes. Real commands and their verdicts: `tools/dev/test/wait-loops.tsv`.
- **Before reporting, stop every background task of yours still running** (TaskStop, or your
  process's exact PID) and list them in the report.

## Git

- Work only in your own worktree; never write in another agent's (*enforced*:
  `tools/dev/git-guard.py`).
- End commit messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Run `zsh tools/dev/check.sh` in your worktree before reporting, and report only after it prints
  PASS. `tools/dev/report-guard.sh` keeps an agent with unchecked commits working (a stop passes
  while that worktree's check.sh runs, whoever started it) but can't recall a report already
  delivered: the gate that holds is `tools/dev/merge-ok.sh`, which the PM runs before a merge.
- Don't merge, push or publish: the PM does.

## The work

- **Fix the general mechanism**, never one symptom.
- **Cut surface area:** delete dead code, vestigial code and surface area in what you touch, and
  list what you cut in your report.
- **Docs say only what is true of the code:** planned work marked as planned, gaps filed as board
  tasks, no history of what used to be.
- **Removed means deleted:** removing a layer deletes its data and dead code, keeping only shared
  inputs still read.
- **No hidden limits** in the build: a cap is a visible, configurable setting that takes effect as
  set.
- **No hand-kept figures:** a step's requirements are measured, enforced where used, or derived from
  the code.

## Spelling

- **Canadian English** in everything a person reads (the app's text, docs, comments, commit
  messages, board notes, reports): -ize and -yze, not -ise and -yse (equalize, generalize,
  rasterize, analyze); -our (colour), -re (centre, metre) as they are. `tools/check/spelling.py`
  (run by check.sh and publish, through tools/app/guards.sh) refuses an -ise/-yse spelling from its
  word list; a deliberate use (a stored name kept, a quotation, a body's proper name) says
  `spelling: kept` and why on its own line.

## Screenshots

- **Proof screenshots are full resolution, saved to a file:** the real capture at 2× pixel density,
  never the downscaled preview a screenshot tool hands back to you (it is shrunk to fit your image
  budget, and `scale` shrinks it more). For a page, run `node tools/dev/shot.mjs <url> <out.png>
  [--size 1440x900] [--scale 2] [--hide-ui] [--press KEYS] [--click X,Y]` (all options in its
  header): headless Chrome with its own throwaway profile, the page captured once idle (for a
  second no frame requested, no request in flight, the page's or its workers', and on the map no
  task in its status line), the PNG checked at size × scale; exit 2 on a timeout, saying what kept
  the page busy; no Chrome or profile left behind. Use `screencapture` for a native window. Report
  the files' paths.
- **Use shot.mjs, or `tools/dev/chrome.mjs` for a scripted run, not an improvised CDP client:**
  Node's built-in WebSocket drops any message past 4 MiB (a 2× PNG of the map) and the call never
  answers; chrome.mjs talks over Chrome's debugging pipe. Chrome's own `--screenshot` captures
  before the map has loaded.
- **On the map, press I to hide the UI** (`--hide-ui`) when the panels hide what you need to see.

## Outside services

- Requests identify themselves as `scenic-roads/0.1 (personal offline map)`, with no contact
  details.
- robots.txt is for crawlers, and is read only before real crawling: a script following links or
  fetching many pages of a host in bulk. Reading a page, a catalogue record, an API answer or a
  published file as a person would (research, a declared source's fetch) needs no robots.txt check;
  follow the provider's own usage policy and rate limits. Never get round a block, a login or a
  CAPTCHA.
- Never print, log or commit keys (`inputs/keys.env` on the NAS).

## The board

- Post progress to your task's board item: `POST http://mac-mini-de-brandon:8077/api/c/events`
  with `{item, kind, text}`, `item` the task's id as a string, `i<number>` (`"i1521"` for #1521).
  Leave the item's status to the PM unless your brief says otherwise.

## Starting tasks (the PM)

- **Small tasks start five at a time** (owner, 20 Nov). Every task's size is classified on the
  board before it starts (its `small`: true, false, or unset until classified; the one definition is
  `SMALL_MEANS` in `tools/devtools/schema.mjs`, read generously). A small task is never started
  alone: when the next task picked is small, it starts with the four next highest-priority small
  tasks, as one piece of work: one slot, one builder, one branch, one review, one merge and publish.
  A small task blocked by, or conflicting with, work under way is left out. Each task keeps its own
  board item, events and close.
- **A batch is chosen when it starts, never before** (owner, 20 Nov): read the board right then and
  take the four next small tasks by priority as they stand. Don't plan, name or hold a next batch
  ahead of time, on the board, in notes or in reports.
- **The board enforces it:** a task in flight is working, backgrounded, integrating or in review;
  waiting on you isn't, as a task often waits on the owner before it starts. Moving a task into a
  status in flight from one that isn't (queued, waiting, done, abandoned), or into working from any
  other status, is refused while its `small` is unset, and, for a small task, unless it has a batch
  id (`batch`): sent in the same request, or, unless it comes from queued, the one it holds, so a
  batched task resumes as it is from any status. `tools/dev/board.py batch` sends the id; a request
  that makes one up goes through, and health.sh flags its batch as having no start event. Queued ↔
  waiting needs neither. A task queued again loses its batch, so its restart, the page's undo of a
  requeue too, is refused like any start: "small task starting: start it through
  tools/dev/board.py batch"; a small task waiting before it ever started is "in no batch", and
  starts through board.py batch too.
- **Start a batch with** `tools/dev/board.py batch <the five numbers> [--note '…']`: it sets each
  one queued, or waiting and in no batch, working with the batch's id and gives each the same
  `start` event ("Batch <id> of
  small tasks: #a #b …"). Fewer than five is refused while another small task is queued, unless
  `--left-out` names each with why: entries separated by commas or semicolons, each the task's
  number then its reason (`--left-out '#1610 (blocked by #1600); #1611 (conflicts with #1534)'`),
  only an entry's leading number counting; given twice, the lists are merged. Run again with the
  same arguments, it finishes a batch cut short without a second event; a batch grown later is
  recorded by running it with all of it. A small task already in flight in no batch is put in one
  as it is, its status kept.
- **A batch's size is its start's:** every task its start events name, a member that left
  included, and the tasks it should have taken are those queued and small when it first started.
  A member leaves a started batch by going out of its work (queued again, abandoned) and
  `tools/dev/board.py leave <num> '<why>'`, which records the one reason as a note on its item
  ("Left batch <id>: …"). A member done in its batch has finished, not left; an archived one needs
  no record.
- **`tools/dev/health.sh` flags** a batch in flight whose start events name fewer than five when a
  task queued now was queued and already small when it first started (its `smallAt`, set by the
  board; its status events) and is neither in it nor named as left out; a member that left with no
  reason recorded; a task in a batch no start event names; a batch with no start event recording it
  (an id made up around board.py); and a small task in flight in no batch (one marked small while
  under way; a waiting one isn't flagged).

## Review

- **The reviewer's standing checklist is `tools/dev/review-checklist.md`.** Before its final
  check.sh, a builder passes through it over its own diff (`git diff main...HEAD`), fixing what it
  finds; a reviewer reviews against the same list, plus what the brief asks. Briefs need not
  repeat it.

## Your report

- The branch and sha, what changed, what you cut, the check.sh PASS line, what is left or
  untested, the background tasks you stopped, and what your pass through the checklist found.
