# How agents run tests (owner's rule, 16 Oct; affected tests only since 17 Oct, #269)

Enforced by tools/dev/test-guard.sh (a PreToolUse hook in .claude/settings.local.json), which blocks
a command that breaks these and says why. It finds a test run where a command starts (also after
`env`/`time`/`nice`/`command`/`xargs`, by a path to cargo, with cargo flags before `test`, as `cargo t`,
`cargo nextest r` or `cargo-nextest`) and inside the script of `sh -c`, `eval` and `python -c`; it
refuses runs it can't read (`$c test`, `cargo --config alias.…`, `cargo -C`, `make`/`just test`).

- Tests build optimised (`[profile.test]` in Cargo.toml, #505: opt-level 1 for the workspace, 3 for
  dependencies; debug assertions and overflow checks stay on), so the first `cargo test` in a fresh
  worktree or after a dependency change compiles the dependencies once (~5 min); after that an edit
  rebuilds only the workspace crates.
- Agents run only the affected tests: `tools/dev/affected-tests.sh` in the worktree prints them as
  `cargo test` commands, from the branch's diff against main (working tree included): each changed
  module (`agent/build.rs` → `-p pipeline --lib -- agent::build::`), the modules using it one level
  out, and the integration tests naming it. Run those, or narrower: the same `-p` and target, each
  filter starting with one it prints. No Rust change, nothing to run. `affected-tests.sh <commit>`
  or `<a>..<b>` shows the set for past work.
- Scan tests (#662): an integration test that reads the repo's own files (write_sets,
  inputs_accessor, cli, source_names …, through `common::root()`) names them on a
  `//! scans: <path prefix> …` line; a change to any file under one (any kind, deleted too) runs
  it whole. guards.sh refuses such a test without the line. (#660 changed agent/room.rs; check
  passed and publish failed on write_sets, which nothing mapped to it.)
- The tracing is same-crate only: `crate::<mod>`, the parent naming it, children using `super::`,
  the crate's own binaries, its `tests/`. Users in other crates and items re-exported from a crate
  root (`pub use`) aren't traced; publish's full run catches those. Python tests (dem/) are outside
  the script.
- Filters name a module path (`agent::tests::`, `agent::tests::unit_key`), never a bare word: a filter starts with one of the crate's modules, so
  `tests::` or `::` alone counts as a bare word. A single `--test`/`--bin` target may take bare names.
- No full suites from agents or reviewers, and no `--workspace`. Reviewers (in the main checkout)
  run only the tests the commit adds or changes, filtered (the guard can't tell which those are;
  it holds them to module-path filters). A full suite only to reproduce a failure
  the PM names: mark the command `#repro`.
- `--release` only for performance work: mark the command `#perf` (it may use `--workspace`).
- Full suites run in `tools/app/publish.sh` before a publish, the PM's; and once by the PM on main
  after a merge batch, in the background, before publishing. Publish runs (#848):
  - `cargo nextest run --workspace`: every workspace crate's libraries, binaries and integration
    tests (det, roadcore, pipeline, server, store, names, webpenc), on the test Mac;
  - then `tools/app/other-tests.zsh`, on the publishing Mac, its parts at once: `node --test` of
    `web/test/*.test.{ts,mjs}` and `tools/check/*.test.mjs` (by glob), `tools/taskboard/test/*.test.mjs`,
    the status app's `swiftc` build and `--test tools/status/test`, `tools/app/test/shell.zsh` and
    `tools/dev/test/test-guard.zsh`;
  - publish's clippy line.
  Every output goes whole to ~/Library/Logs/scenic/publish-tests-<time>.log; any failure stops the
  publish. Not run by publish: Python (producers/, dem/: check.sh only), doctests, the browser
  checks in tools/check and tools/bldcheck that are not `*.test.mjs` (run by hand).

## Before reporting: tools/dev/check.sh (#273, 18 Oct)

Run `tools/dev/check.sh` in your worktree after your last commit, and report only after it prints
PASS. It runs what publish would refuse your work on: `cargo fmt --check`, publish's clippy line
(`-D clippy::disallowed_methods`: processes only through `store::spawn`), `tools/app/guards.sh`,
`tools/app/wasm.sh` when a crate the wasm build compiles changed (an unguarded `crate::net` fails
there), when web/ or a `tools/check/*.test.mjs` changed: `node --test` of every `web/test/*.test.{ts,mjs}` and
`tools/check/*.test.mjs` (globbed, so a new test file runs without editing a list), the web typecheck and build, `node --test` of tools/taskboard/test when
tools/taskboard changed (#672), `tools/app/test/shell.zsh` (publish.sh's shell
steps in `tools/app/lib.zsh`, tested in a scratch folder) when tools/app changed, `tools/dev/test/test-guard.zsh` when the test guard changed, the Swift build and self-test when tools/status changed,
the tests `affected-tests.sh` prints, and pyflakes and unittest when producers/ or dem/ Python
changed. One PASS/FAIL line per part; whole outputs in ~/Library/Logs/scenic/check-<time>/. A pass
on a clean tree records HEAD in `.git/check-passed`.
Enforced twice: `tools/dev/report-guard.sh` (a SubagentStop hook) keeps a worktree agent with
commits beyond main from finishing until its HEAD passed, and the PM runs
`tools/dev/merge-ok.sh <commit>…` before cherry-picking.

## Shared compile cache (#439, 19 Oct)

Every cargo run inside `.claude/worktrees/` goes through sccache (brew), set by the untracked
`.claude/worktrees/.cargo/config.toml` (cargo reads config from each parent folder), so the main
checkout and publish.sh's tree are untouched. Third-party crates (debug, clippy's check builds, the
release wasm32 build) come from one cache shared by all worktrees, ~/Library/Caches/scenic-sccache,
capped at 10 GB by sccache; workspace crates still build incrementally per worktree (sccache passes
incremental builds through). `sccache --show-stats` shows hits. tools/dev/seed-target.sh still
clones main's compiled third-party crates into a new worktree (one cp per folder, ~7 s);
tools/dev/sweep-targets.sh empties the cache when free disk is below 20 GB.

## Tests on the test Mac (#442, 19 Oct)

check.sh's `tests` part and publish.sh's nextest suite run on the test Mac through
`tools/app/remote-tests.sh <key> <tree> <command>` (tracked). Key: the worktree's folder name
(`main-checkout` for the main checkout, `publish` for publish's snapshot).
- One run per key at a time: a lock on the test Mac (`~/scenic-check/<key>/.busy`, naming this Mac, the
  process, its start time and a nonce), taken before the copy and held to the end. A second run of
  the same key prints "key … is in use … waiting" and waits (30 min, then runs here). A lock whose
  process on this Mac is gone (or is another process under a reused PID), or older than 6 h, is
  taken over.
- The copy: the tree's tracked and untracked-not-ignored files and symlinks (uncommitted edits
  included) rsync into `~/scenic-check/<key>/src`; files and symlinks not in that list, then empty
  folders, are deleted there. Build folder `~/scenic-check/<key>/target`, kept between runs; rustc
  through the test Mac's sccache.
- The run: nice 10 at utility priority (`taskpolicy -c utility`, as the pool's jobs, so the lead's
  agent loop isn't starved: #700), 8192 open files, `mise env` of the tree, with this side's exported RUST_LOG,
  RUST_BACKTRACE, RUST_TEST_THREADS, RUST_MIN_STACK, CARGO_TERM_COLOR, CARGO_INCREMENTAL,
  CARGO_BUILD_JOBS, CARGO_PROFILE_*, NEXTEST_* and SCENIC_*. Both sides print their rustc, cargo
  and nextest versions and whether the NAS is mounted. Three runs at a time on the test Mac; the rest
  queue (30 min, then here). A stopped run is stopped on the test Mac too.
- The result: the test Mac's side ends with a status line (read, not shown); the exit status is the
  command's. Without one (connection lost, no tree, no slot) it prints
  `--- remote result discarded, local rerun ---` and runs here: what follows the last such line is
  the result (publish.sh reads its Summary and FAIL lines only from there).
- Connecting never hangs (#637): a fresh ssh asks the ssh agent (1Password), which, locked, waits
  for an approval for ever. The probe (a fresh connection) has 20 s, the lock calls 60 s, and the run
  must print its first line (`remote-tests: connected to the test Mac`, at once) within 120 s; past a bound it runs here, saying why.
- Runs here instead: on the test Mac itself (the Mac mini, which also publishes, so publish's suite always runs
  there); when the test Mac is down or the copy fails; when the tree
  has `data/build` (ignored test data that tests skip without, which the copy wouldn't carry); with
  `SCENIC_CHECK_LOCAL=1` (also `SCENIC_TESTS_LOCAL=1`).
- Disk on the test Mac: a key unused 3 days goes; past 60 GB in all the least recently used go; never a
  locked key. The sweep drops a removed worktree's key (`remote-tests.sh --drop <key>`).
fmt, clippy, guards, wasm, web, swift and python stay on this Mac.

## A test that can't flake (#662, 25 Oct)

The suite runs three at a time on the test Mac at nice 10 and utility priority, beside builds: what takes milliseconds
idle takes seconds there. Every flaky test so far measured that machine. The way to write a test
that can't is the shortest one, in `store::testkit` (test builds only; other crates' tests get it
through their dev-dependency on store, `features = ["testkit"]`):

- **Time in code under test: a `store::clock::Clock`.** Code whose decisions turn on how long
  something took or waited reads the time and sleeps through one; its test hands it a
  `clock::Virtual`, which moves only when slept on (`at`/`after` run the other party at named
  moments). Such a test never sleeps and can't race.
- **Waiting for something on the real clock: `eventually(what, cond)` or `wait_for(what, || Option)`.**
  They poll every 10 ms under `WATCHDOG` (5 min), which only turns a hang into a failure. Never a
  loop of your own with a deadline: `while t.elapsed() < 5 s` is what failed takeover's test.
- **A thread or callback saying "done": `Signal`** (`set` / `wait(what)`), not a flag polled
  with sleeps. For "has it NOT happened", a sleep marked `// (absence)` is fine: a slow Mac can only
  make it pass.
- **A stand-in child process: `testkit::sh(script, args)`.** It runs in its own process group; the
  script makes `$READY` once set up (`: >"$READY"`), `ready()` waits for that, and dropping it
  (a panic included) kills the whole group and reaps it. No fixed head start, no leaked `sleep`.
- **"It didn't wait": `ended_before(t0, never, what)`**, half of what waiting would take, never
  a fixed "fast" bound.
- **Process-wide settings:** pass the value in. Where the code reads an environment variable, a
  test sets it with `EnvVar::set(k, v)` (restored on drop, tests taking turns), and only for a
  variable no other test in that binary reads. A static a test changes is thread-local in test
  builds where it can be (the agent's `TEST_PORT`, `TEST_ROOT`), so tests in one process can't see
  each other's; the one that can't, the plain-open cap, is set through a guard
  (`store::range::cap_plain_opens`) that tests take in turn and that restores it on drop (other
  tests in a `cargo test` process run under it meanwhile: a lower cap only closes files sooner).
- **Hermetic by default:** temp folders (`tempfile::tempdir()`), never the real NAS or app home
  (`agent::is_live_path`); coordinators on loopback ports from `coord::test_port()` (held until the
  coordinator takes them); no `.local` names (`net::urls` gives 127.0.0.1 in tests).

`tools/app/guards.sh` (publish and check.sh) runs `tools/app/test_timing.py`, which refuses what
these replace: an assert of a short `elapsed()` bound, a fixed sleep then an assert, and (#662) a
wait's own watchdog under a minute (`Instant::now() + 20 s`, a loop on `elapsed() < 5 s`,
`recv_timeout(10 s)` expected to succeed). A bound that isn't a wait says why: `// (bound ok: …)`.

## Setting up a Mac (tools/dev is tracked, #548)

Every script finds the repo from its own location (the main checkout from the shared `.git`, the
checkout it runs in from the top level), so a worktree has its own copy and no path is held. Per Mac, once:
- `tools/dev/install-sweep.sh` fills `local.scenic.sweep.plist` (launchd can't compute paths) with the
  main checkout's path, installs it in ~/Library/LaunchAgents and loads it.
- The hooks go in the untracked `.claude/settings.local.json`. Claude Code sets `$CLAUDE_PROJECT_DIR`
  (the session's project folder) for hook commands, so no absolute path is needed:
```json
{"hooks": {
  "PreToolUse": [
    {"matcher": "Bash|Agent", "hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/tools/dev/test-guard.sh"}]},
    {"matcher": "Bash", "hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/tools/dev/shell-guard.sh"}]},
    {"matcher": "Bash", "hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/tools/dev/seed-target.sh", "timeout": 120}]}
  ],
  "SubagentStop": [
    {"hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/tools/dev/report-guard.sh"}]}
  ]
}}
```
  (`health-due.sh` is a PostToolUse/Stop hook of the same form; not registered since 16 Oct.)

Generated state, all ignored: `.claude/health-stamp`, `.claude/sweep-targets.log`, `.claude/sweep-keep`
(hand-made), `~/Library/Logs/scenic/`, `.git/check-passed`.

## The build page on a phone: touch-test.mjs and safari-sim.sh (#614, 23 Oct)

Every brief that changes web/work runs both and looks at the screenshots. Neither presses a button
that changes anything: the page opens as `?view` from a read-only server (`touch-test.mjs --serve`,
port 8097) that serves web/work and forwards only the reading calls (swarm, steps, regions,
settings with `{}`, history, live) to this Mac's agent on 127.0.0.1:8090, refusing the rest.
- `node tools/dev/touch-test.mjs [--out DIR]` (35–55 s on the M1): headless Chrome as a 390×844 phone (DPR 3,
  mobile, 5 touch points, iPhone user agent), real touches through CDP. Each tab: no sideways
  scroll (else the elements past the edge), no console errors, a screenshot (`chrome-<tab>.png`,
  1 px a CSS px). Then: a card pins on a tap and unpins on a tap elsewhere; a tap on the
  map pins the stack under it, each tap its own, and one off it pins nothing (#615); the world map zooms
  with a pinch (its touch-action turns `none`, its zoomed-in sign) and pans
  with a drag (pixels changed once it held still), the page staying put; the Jobs tab's stages counting done and left, no job squares; no share of a whole in the header, the Overview or the legend; a region row lighting its region on the map (a mouse); the panels' split (#685) stacked at phone width
  and side by side 2:1 at 1280 px, its bar dragged, a double click back to 2:1; the schedule's timeline (#787) pans with one finger sideways and zooms with a pinch, no dropdown; settings fields focus on a
  tap without a jump (never Set); the map (#685: in its own panel, published tiles off at first, a step's legend item soloing it while hovered and holding it on a click or tap, its published tiles painted by age; the age controls' timeline, mark labels never overlapping, a mark hovered lighting its tiles, the range inputs' swap and one-span rule; `--catalogs FILE` answers `/work/catalogs` from `scenic catalogues > FILE`, `--width W --scheme dark|light --only-map` for its screenshots). At 720 px or wider (`--width`) the "no text cut off" check is skipped: a desktop keeps labels on one line, ellipsis and all (#617). `--agent H:P` reads another Mac's agent (the lead's, for its forecast's queue: an agent before #685 is asked a Mac at a time). `--member NAME --tocome FILE` checks a member's queued tiles (#622). One PASS/FAIL line a check, exit 1 on a FAIL. The other Macs'
  live/settings calls fail from this test origin (CORS) or when away: counted on an INFO line.
  Chrome is its own child, killed by PID; its profile is a temp folder, deleted.
- `tools/dev/safari-sim.sh [out-dir] [device]` (~1.5 min; this M1 only, the M4 has no simulators):
  boots the simulator (default iPhone 18 Pro), opens each tab in Mobile Safari and saves
  `safari-<tab>.png` (`xcrun simctl io`), then shuts the server down and the simulator if it booted it (one already running, someone's, stays), on any exit. `simctl openurl` may say
  "Operation timed out" while Safari starts; the page opens anyway. For touch, set
  `SCENIC_SIM_HOLD=<s>` and drive it meanwhile with the Simulator control tools (tap, swipe,
  touch2_path for a pinch, screenshot; the first input after a boot may need a few retries). Port 8097 held
  (another agent's run): `SCENIC_SIM_PORT=<n>`; a server that can't listen fails the run, not
  showing someone else's page.
