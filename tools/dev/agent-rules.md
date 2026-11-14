# Standing rules for agents

Rules every agent working on this repo follows, whatever its brief says. A brief adds the task;
it never needs to repeat these. Every agent brief the PM writes names this file:
`tools/dev/test-guard.py` refuses an Agent call whose prompt doesn't mention
`tools/dev/agent-rules.md` (agents that never touch the repo are exempt: web-fetch,
claude-code-guide, statusline-setup). Rules marked *enforced* are also checked by a hook; the rest
rely on the agent.

## Where things run

- **Tests, builds and check.sh run on the M4**, in your own worktree. Follow
  `tools/dev/testing.md` for test runs (*enforced*: `tools/dev/test-guard.py`).
- **NAS-heavy runs go to the Mac mini** (`ssh m6`), where the NAS is local: anything that reads or
  writes a lot of NAS data, such as a bit-identity run (`scenic-build … --root
  /Volumes/personal/projects/scenic-roads`), measuring what a NAS-reading step reads, scanning large
  NAS folders, or bulk copies. A small read (a log, a status file, one record) is fine from any
  Mac. On the mini:
  - use a scratch folder of your own and delete it afterwards;
  - timings taken there are skewed by its build jobs: when the numbers matter, ask the PM for a
    quiet period rather than pausing anything yourself.
- **Never mount or unmount the NAS share yourself:** each Mac's agent mounts it (by its LAN name at
  home, through Tailscale away) and remounts it as the network changes.
- **Memory:** nothing caps an agent's own run (the pool's memory guard covers only pool jobs), and
  runs on real data have crashed Macs. Watch the footprint of a long run on real data.

## Processes

- **Never signal the pool's processes** (`scenic agent` and its jobs, `scenic-build`, `scenic gc`,
  the map server) on any Mac. Report what you see; the owner decides.
- **Kill only your own processes, by exact PID** (keep `$!`). Never `pkill -f` or `killall`.
- **Don't edit a script that is running** (check `ps` first): shells read scripts as they go.
- **Never filter a test run in its pipe:** `tee` the full output to a log and search the log.
- Keep each foreground command under 10 minutes; split longer runs or run them in the background.

## Git

- Work only in your own worktree; never write in another agent's (*enforced*:
  `tools/dev/git-guard.py`).
- End commit messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Run `zsh tools/dev/check.sh` in your worktree before reporting, and report only after it prints
  PASS (*enforced* for agents with commits: `tools/dev/report-guard.sh`).
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
  with `{item, kind, text}`. Leave the item's status to the PM unless your brief says otherwise.

## Your report

- The branch and sha, what changed, what you cut, the check.sh PASS line, and what is left or
  untested.
