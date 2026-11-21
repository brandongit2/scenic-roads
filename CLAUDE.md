# Scenic Roads

A personal offline map, built by a pool of Macs and used by one person (the owner). The stakes are
low, and so is the need to ask: engineering choices are ours to make and ship, while taste and
product direction (what the map shows, how it looks) go to the owner.

Agents: the standing rules are in [tools/dev/agent-rules.md](tools/dev/agent-rules.md), and test
runs follow [tools/dev/testing.md](tools/dev/testing.md). This file holds what everyone working here
should know, plus how the PM session runs the project.

## The machines

| Mac | Specs | Repo | Role |
| --- | --- | --- | --- |
| M1 (`MacBook-Pro-de-Brandon`, `ssh m1`) | M1 Pro, 10 cores, 16 GB | `~/projets/scenic-roads` | the owner's laptop; often the PM session |
| M4 (`Brandons-MacBook-Pro`, `ssh m4`) | M4 Pro, 14 cores, 48 GB | `~/repos/scenic-roads` | **all tests run here**; pool member |
| Mac mini (`Mac-mini-de-Brandon`, `ssh m6`) | M6, 12 cores, 32 GB | | pool lead; NAS-heavy runs; the task board |

- Sessions move between the M1 and the M4: know which one you are on.
- SSH, git push and commit signing go through the 1Password agent. From the desktop app they must
  run outside the Bash sandbox, or 1Password never shows its prompt.
- Every shell is zsh: wrap remote globs in `bash -c` (zsh aborts on a glob with no match).

## The NAS

A Synology, mounted at `/Volumes/personal` on every Mac. The project lives in
`/Volumes/personal/projects/scenic-roads`, and it is the source of truth: the Macs mirror it.
`inputs/` is never in git, because this repo is public.

- **Small files are slow:** SMB manages about 20–55 small-file creates a second, and an fsync per
  file stalls the whole NAS. Keep per-file NAS writes and per-file probes out of hot paths; list a
  folder once. Big files are fine. Bulk copies go as `tar` over SSH, never rsync.
- **Stale SMB handles:** "Resource busy (os error 16)" on NAS files means a Mac left the network
  abruptly and its SMB session is still open. The owner kills that session in DSM's Resource
  Monitor; then restart the agent. Before taking a Mac off the network, pause it first.

## Engineering principles

These are the owner's, and they apply to every change.

- **Fix the root cause.** Fix the general mechanism, never special-case one symptom.
- **Cut surface area.** Delete dead code, and shrink vestigial code, options and flags, in whatever
  you touch.
- **Removed means deleted.** Removing a layer deletes its data and its dead code. Keep only shared
  inputs that something still reads.
- **No hidden limits.** Every bound on what the build uses is a visible, configurable build-page
  setting that takes effect exactly as set. No reserves or floors underneath it.
- **No hand-kept figures.** A step's requirements are measured, enforced where they are used, or
  derived from the code. A stale declaration may cost speed, never correctness.
- **Docs match the code.** Docs state only what is true: planned work is marked planned, gaps are
  filed as board tasks, and there is no history ("until X", "was Y"). Update docs in the same
  commit as the code.
- **Preferences, not rules.** When the code shows a better way than what the owner asked for, do
  the better thing and say why, with numbers. Hard constraints stay hard.
- **No page auth.** Every build-page button and setting works from any device.
- **Browsers:** the web app supports only Chrome and Safari.
- **UI taste:** dark, subtle and clean but information-dense; metric units; trackpad gestures (two-finger pan,
  pinch zoom); a progress indicator for anything that takes time.
- **Spelling:** Canadian English: -ize and -yze, with -our and -re.

## Data

- **Licences:** the map is personal and never shared. Non-commercial, share-alike, unclear or
  missing licences, personal-use-only terms and Internet Archive copies are all fine. Free sources
  that need a login count. Nothing paid. Record each source's real licence anyway.
- **Every source counts.** Modelled global sets and surface models (DSMs) are sources like any
  other; the finest grid wins. Never leave one out for being a surface model.
- **Fetching needs no go-ahead**, up to 5 GB per source.
- **Requests** identify as `scenic-roads/0.1 (personal offline map)`, with no contact details.
  robots.txt applies only to real crawling, meaning link-following or bulk fetches from one host;
  an unreadable robots.txt allows. Never get round a block, a login or a CAPTCHA, and create no
  accounts. A blocked download becomes a board task for the owner.
- **Disputed areas** go to the effective holder, the side that actually holds and administers them
  (Crimea is Russia's). Where each side holds part, split along the line held. Where neither does,
  ask.
- Never print, log or commit keys (`inputs/keys.env`).

## Safety on the Macs

- Nothing caps an agent's own runs; whole-list runs on real data have panicked Macs. Watch a long
  run's footprint. Any memory guard measures phys_footprint, never RSS. A step that needs tens of
  GB is a defect in itself.
- Kill only by exact PID (keep `$!`). Never use `pkill`, `pkill -f` or `killall`.
- Never signal the pool's processes. Cancel a pool job only with `scenic job cancel <target>`.
- Never edit a script while something runs it: shells read scripts as they go.
- Wait on a PID (`while kill -0 $pid`) or a file, never with `pgrep -f`, which matches the waiting
  shell itself. Give every loop a deadline.
- Never filter a test run in its pipe: `tee` the full output to a log and search the log.
- Long background runs go under `tools/dev/longrun.sh`, never `nohup` or `&`.
- Check `df -h` before starting several agents on one Mac; a clone's `target/` is 2–10 GB.
- **After a crash:**
  1. restart the board if it ran there;
  2. clear stale locks whose PID is dead;
  3. resume the agents.

  If the lead is down, no other Mac takes over by itself: run `scenic lead take` on a working Mac.

## Publishing

- `zsh tools/app/publish.sh` publishes to the pool: tests, build, then a push to the lead's
  releases. Each Mac's updater picks the new app up within about 5 minutes. A Mac's agent switches
  to it when its current job ends, and its server switches once the map has been idle for a minute.
- Run publish in the background with its full log kept. To stop one, SIGTERM its exact PIDs and let
  the trap clean up; never `-9`. `scenic releases --json` says what is live.
- Commits are SSH-signed through 1Password, and every commit is pushed. If the owner can't approve
  a signature, commit with `git -c commit.gpgsign=false`, then re-sign and force-push `main` once
  they are back; rewriting `main` is fine here.
- After a conflicted cherry-pick, check the commit message survived (`git log -1 --format=%B`).

## Benchmarks

- Close every app tab in the benchmark browser except the one under test.
- Claims about 120 Hz come from windowed Chrome (`--native`), not headless.
- Performance measured on the PM's Mac needs a quiet period. The PM:
  1. moves agents' heavy work to the other Mac;
  2. pauses that Mac's build agent;
  3. takes the measurements;
  4. undoes both.

## How the PM session runs the project

The PM delegates the work, owns the design, reviews and integrates the results, and stays free to
answer the owner. It also merges, publishes, places NAS inputs and closes tasks; agents never merge,
push or publish. Small, reversible NAS input edits (a declaration field, metadata) the PM makes
itself, with a backup and a parse check.

### The task board

The board is at `http://mac-mini-de-brandon:8077`; its API is described in
`tools/devtools/README.md`.

- `POST /api/c/items` creates a task and `PATCH /api/c/items/<id>` edits one.
- `POST /api/c/events {item, kind, text}` adds history; `PATCH /api/c/meta/now {text}` sets the now
  line.
- The server sets ids, numbers and times.

How the PM keeps it:

- **Write at decision points only:** started, built (with its commit), merged, live, dropped, or a
  real finding.
- **Pick fresh:** re-read the board right before every pick, because the owner edits priorities.
  The PM may reorder priorities too, logging why.
- **Archived folders:** a task under one doesn't show on the board, so never file under an archived
  folder.
- **Statuses:** a dropped or merged-away task gets status `abandoned`, with no prefix in its name.
  `integrating` is a short wait; `backgrounded` is a long one, and frees the slot.

### Slots

- At most **three** tasks are working or in review at once. Fill free slots straight away.
- Reviews come first; then the lowest-P task that conflicts with no active work.
- Every task is sized before it starts. Small tasks run 2–5 at a time as one batch, via
  `tools/dev/board.py batch`, taking one slot. Pick the batch only when it starts. A finding drops
  one task (`batch-drop`).
- A merge or publish holds no slot.

### Briefs and reviews

- Every brief names `tools/dev/agent-rules.md`. It also says how the result will be judged (for
  example, a backtest on held-out data) and where each output goes. Diagnose before briefing a fix.
- Each task's "where it runs" field picks the agent's model. **Reviewers are always Opus.**
- Match review depth to risk. A doc-only, test-only or few-line change gets a quick read. A new
  mechanism, or anything touching the scheduler, memory or the NAS, gets the full review with
  mutation runs.
- Fix a review's lows in the same round, or drop them with a line saying why. A fix that needs a
  new mechanism becomes its own task.
- Start a fresh builder rather than resuming one far past ~100 M tokens of context.

### Merge, publish, close

- **Merge:** run `tools/dev/merge-ok.sh <branch>`, then fast-forward `main` and push.
- **Publish:** if a second merge is ready within about 15 minutes of a running publish, fold both
  into one publish.
- **A task closes when its publish succeeds.** It stays open, as `backgrounded`, only for a real
  risk that meets all three of these tests:
  1. a named way the change could fail live;
  2. nothing before publish could show it;
  3. it would do harm.

  Its line in `.claude/next-check.md` names its signals and a date. When that date passes
  unsettled, investigate; never re-defer. "Not yet seen working live" is acceptance, not risk.
- **Clean up at close:** delete the task's worktree, its `target/`, its scratch folder and any
  backups taken for its change, on every Mac. Never delete them while the task is still in review.

### Health checks

Every hour, run `zsh tools/dev/health.sh` and act on what it flags. Report only what is wrong or
has changed. Recurring duties are enforced by hooks and launchd, not memory:

- `health-due.sh` blocks when a health check is overdue;
- `.claude/gates.txt` holds the waits;
- `sweep-targets.sh` frees finished worktrees' `target/`.
