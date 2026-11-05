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
  out, and the integration tests naming it (a crate's whole targets on one line, one cargo run
  building them in parallel). Run those, or narrower: the same `-p` and target, each
  filter starting with one it prints. No Rust change, nothing to run. `affected-tests.sh <commit>`
  or `<a>..<b>` shows the set for past work.
- Scan tests (#662): an integration test that reads the repo's own files (write_sets,
  inputs_accessor, cli, source_names …, through `common::root()`) names them on a
  `//! scans: <path prefix> …` line; a change to any file under one (any kind, deleted too) runs
  it whole. guards.sh refuses such a test without the line. (#660 changed agent/room.rs; check
  passed and publish failed on write_sets, which nothing mapped to it.)
- The tracing goes one level: `crate::<mod>`, the parent naming it, children using `super::`,
  the crate's own binaries, its `tests/`; other crates' modules naming `<crate>::<mod>`, or
  `crate::<mod>` where their root re-exports it (`pub use <crate>::…`, #916), and their binaries.
  Users of those users aren't traced; publish's full run catches them. Python tests (dem/) are outside
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
    tests (det, roadcore, pipeline, server, store, names, pyport, codecs, jobrt, buildstore, landmarks, mapview, webpenc), on the test Mac;
  - then `tools/app/other-tests.zsh`, on the publishing Mac, its parts at once: `node --test` of
    `web/test/*.test.{ts,mjs}` and `tools/check/*.test.mjs` (by glob), `tools/devtools/test/*.test.mjs`,
    `tools/dev/test/devjob.zsh`, the status app's `swiftc` build and `--test tools/status/test`, `tools/app/test/shell.zsh` and
    `tools/dev/test/test-guard.zsh`;
  - publish's clippy line.
  Every output goes whole to ~/Library/Logs/scenic/publish-tests-<time>.log; any failure stops the
  publish. Not run by publish: Python (producers/, dem/: check.sh only), doctests, the browser
  checks in tools/check and tools/bldcheck that are not `*.test.mjs` (run by hand).

## Before reporting: tools/dev/check.sh (#273, 18 Oct)

Run `tools/dev/check.sh` in your worktree after your last commit, and report only after it prints
PASS. It runs what publish would refuse your work on: `cargo fmt --check`, publish's clippy line
(`-D clippy::disallowed_methods`: processes only through `store::spawn`), `tools/app/guards.sh`,
when web/ or a `tools/check/*.test.mjs` changed: `node --test` of every `web/test/*.test.{ts,mjs}` and
`tools/check/*.test.mjs` (globbed, so a new test file runs without editing a list), the web typecheck and build (after `tools/dev/web-deps.sh`, which gives a fresh worktree its `web/node_modules`: `pnpm install --frozen-lockfile --offline` from this Mac's shared pnpm store, only when node_modules is missing or not a finished install of this lockfile (its `node_modules/.web-deps-lock` stamp); a package the store lacks is an error naming the one online install that fills it; publish.sh runs it with `--online`), `node --test` of tools/devtools/test when
tools/devtools or the build page's timeline modules it serves changed (#672), `tools/dev/test/devjob.zsh` when the
dev jobs' recorder changed (#946), `tools/app/test/shell.zsh` (publish.sh's shell
steps in `tools/app/lib.zsh`, tested in a scratch folder) when tools/app changed, `tools/dev/test/test-guard.zsh` when the test guard changed, the Swift build and self-test when tools/status changed,
the tests `affected-tests.sh` prints, and pyflakes and unittest when producers/ or dem/ Python
changed. The tests run as one `cargo nextest run` (#969; `affected-tests.sh --nextest <file>`
turns the `cargo test` lines into it, saved in the log folder's `tests-nextest`): one build of
every package together, where each line's own cargo had compiled the shared crates again under its
own feature set (resolver 2 unifies features over a run's packages: a `-p server` line rebuilt
store, codecs, buildstore and pipeline, ~170 s on the M4); one pool of test processes across the
binaries, each line's filters on its own binary (a filterset); and every failure, not the first
line's. A package with a lib whose unit tests no line asks for goes to a second run, not to build
them for nothing. Each test is its own process, as in publish's suite. guards.sh runs there without its own fmt check and its two tests (inputs_accessor,
source_names: scan tests of crates/ and dem/, run by the tests part whenever a file they read
changed; #887: they had built the pipeline's tests on this Mac, ~5 min, beside the test Mac's run).
Tests build without debug info (`debug = false` in Cargo.toml's `[profile.test]`, #918), every run alike: check.sh's, publish's and an agent's own.
`tools/dev/test/check-reuse.zsh` runs when the reuse rule's files (check.sh, check-reuse.py,
affected-tests.sh, merge-ok.sh) or the test changed.
One PASS/FAIL/REUSE line per part with its wall and CPU time here (the tests part also the test Mac's);
whole outputs in ~/Library/Logs/scenic/check-<time>/. A pass on a clean tree records HEAD in
`.git/check-passed`; when nothing is untracked, also its tree key (`tree` HEAD's tree, `base` the
merge-base's tree), `at` the merge-base, `tests` the hash of its test commands and a `reused
<part>=<commit>` for each part it took from an earlier pass. A later run on a clean checkout with
the same tree key passes at once, recording its HEAD with that pass's fields and `reused
all=<commit>` (an amend of the message, a cherry-pick onto the same base: nothing a part reads changed).

A rebased branch (#950): on a clean checkout (nothing untracked), check.sh asks
`tools/dev/check-reuse.py` whether an earlier pass P still holds for clippy and for the tests
part, the two that take minutes (fmt, guards and the rest are never reused: each runs by its own rule, as without a pass). It holds
only when HEAD's tree is P's plus main's changes since P's base (every file differing from P is
one main changed, and each of those is main's version), and none of main's changes is:
- of no known kind: a top-level entry outside the script's `KNOWN` (both parts run);
- a Rust build input: `crates/`, Cargo.toml and Cargo.lock, rust-toolchain, mise.toml, clippy.toml,
  rustfmt.toml, `.cargo/`, `.config/`; the package folders (Cargo.toml's members and exclude, and
  the paths the manifests name: path dependencies, `[patch]`); any file a crate's source
  `include_*!`s or `#[path]`s, names as a macro's or attribute's first `"../"` literal, or a build
  script or proc macro reads; the scripts running the parts (`RUNNER`: check.sh, check-reuse.py,
  affected-tests.sh, tools/dev/bin, tools/app/remote-tests.sh, and the recorder wrapping them,
  tools/dev/devjob.zsh and devjob.py). Then both parts run.
For the tests part, also: HEAD's affected tests are P's (the same hash), and the tests that read
one of main's changes at run time run again, those of them HEAD's commands run: a scan test whose
`//! scans:` line covers it (affected-tests.sh over main's changes; that line is trusted for
what the test reads), a test without one naming a `CARGO_MANIFEST_DIR` path or a `".."` path
over it, and a source's such path inside a `#[test]` function (that test). A path whose base
can't be told (`join("..")`, `"{}/../x"`, a `CARGO_MANIFEST_DIR` whose parent is taken) is the
whole repo; one leaving the repo from the crate's folder is its tail under any folder. Such a read
outside a `#[test]` function, inside a `macro_rules!`, or in a crate whose targets aren't cargo's
defaults for its folder (its name, no `[lib]`/`[[bin]]`/`[[test]]`, not both lib.rs and main.rs)
runs the whole part. Both parts run on a doubt: a symlink anywhere in the repo, an `include!` or
`#[path]` outside the package folders, `.cargo/config` naming paths. A part reused prints `REUSE`
with P and why it holds (main's changed files, which tests ran again); a part run says why it
wasn't reused (no such pass, a lookup failing, the file that is an input). Its test,
`tools/dev/test/check-reuse.zsh`, runs the rule on toy repos (docs only, dev tools only, a file a
test reads, a Rust change, a Cargo.lock change, an included file, the runner changed, an unknown
file, the branch changed after the rebase, no earlier pass, an untracked file, and the reads above
that need a further look: `join("..")`, a deeper base, a symlink, a `#[path]` outside, a macro's
test, lib and main, a test folder, a renamed package, a `[patch]` path, a macro attribute's
folder) and checks the lists: every top-level entry of the repo is in `KNOWN`, and the runner
scripts' code names no repo script outside `RUNNER`. publish.sh doesn't reuse: its suite is whole.

Enforced twice: `tools/dev/report-guard.sh` (a SubagentStop hook) keeps a worktree agent with
commits beyond main from finishing until its HEAD passed, and the PM runs
`tools/dev/merge-ok.sh <commit>…` before cherry-picking: ok when a pass is recorded at the commit or
a later commit of its branch and holds, saying what that pass reused and from which commit; a pass
holds when every pass it reused from is recorded and holds in turn (the reason given when none does).

## One build folder per Mac (#915, 31 Oct)

Every agent worktree on a Mac builds into one folder outside the repo, ~/Library/Caches/scenic-target
(`SCENIC_SHARED_TARGET` overrides), through `tools/dev/bin/cargo`, which Claude Code puts first on
the PATH of every Bash command (`CLAUDE_ENV_FILE` = tools/dev/agent-env.zsh, below) and check.sh puts
there itself. Third-party crates and unchanged code compile once per Mac, not once per worktree.
- One build at a time in that folder: a second cargo prints
  `cargo: waiting for the shared build folder … (N s; place P in line; held by: <worktree> pid …
  cargo <args>)` at once and every minute, then `got the shared build folder after N s` (check.sh
  shows it on its terminal, not only in the part's log). On a 16 GB laptop this is the point: two
  heavy rustc at once restarted the M1 (31 Oct). A cargo started by a build under the lock passes
  through. The shim runs cargo as its child and lets the lock go when it ends.
- First come, first served (#941): fcntl doesn't queue its waiters (zsh's `flock -t` tries once a
  second), so on 4 Nov a cargo waiting an hour lost to newer ones. Each waiter takes a numbered
  ticket in `<shared folder>/.scenic-queue/` and holds an fcntl lock on it while it waits; only the
  first live ticket tries the build lock (every 0.2 s). A ticket whose lock can be taken is a gone
  cargo's (killed, even by `kill -9`) and is deleted by the next waiter, so it holds no one up. A
  cargo of an older shim (another worktree not yet on this one) doesn't queue and can still jump in.
  A waiter stopped (`kill -STOP`, Ctrl-Z) holds its ticket and so everyone behind it.
- A whole `cargo clean` in a worktree empties the shared folder, every worktree's build, but keeps
  its `.scenic-*` files (the lock, the queue, the holds, `.scenic-runs`): cargo's own deleted the
  folder, and with it the lock its waiters wait on (they gave up, and a cargo starting meanwhile
  made a new lock, building beside the clean). `clean -p`, `--workspace`, `--release`, `--profile`, `--target`,
  `--doc`, `--target-dir` and `--dry-run` are cargo's.
- A long hold is reported, never killed (#941): each cargo letting go appends its hold's seconds to
  `<shared folder>/.scenic-holds` (the last 100). Once the holder has held the folder longer than
  nine holds in ten of those (from the first line when none are recorded), and again each time its
  hold doubles, the waiting line adds `cargo: the folder has been held N s (…); its holder, pid
  P:`, then the holder's process tree (pid, `%cpu`, CPU time, command) and `using CPU: N%` or `using no CPU:
  it may be stuck`. Third-party crates compile in sccache's server, outside that tree. A stuck
  holder is for whoever owns it to kill (by its pid: the shim passes the signal on).
- Programs and tests run outside the lock (#937): `cargo run`, `test` and `bench` build under it
  with the shim as cargo's target runner, which records each program cargo would start (path,
  arguments, environment, folder); the lock goes, and they run as cargo would start them, from APFS
  clones in `<shared folder>/.scenic-runs/<pid>/bin/<its folder's name>/` (a run's program with
  the programs beside it; a test's in `deps`, as `store::spawn::in_test_program` reads it), so
  another worktree's build can't replace them meanwhile; the `CARGO_BIN_EXE_*` cargo and nextest
  set as a test runs name clones too. `cargo nextest run` runs its test binaries from such clones,
  made as it lists them; the lock goes when its first test starts. A signal to the shim (a `kill` of
  the pid `ps` shows as cargo) goes on to the cargo or program it waits for, and the shim keeps the
  lock until that is gone (the build scripts and rustc of a killed cargo still run on, as they
  would from a plain cargo killed by its pid). Kept under
  the lock: a test binary naming a program of the shared folder (`env!("CARGO_BIN_EXE_…")`, as
  pipeline's and server's `tests/cli.rs`), whose path a clone can't change (cargo test runs it
  first; nextest keeps the lock to the end), doctests (rustdoc compiles and runs them in one go),
  and programs for another target (wasm32). On 4 Nov a `cargo run --release --example` held the
  lock 24 minutes while two check.sh runs waited; with a 60 s `cargo run` and two toy checks
  started 2 s after it, the checks waited 58 and 59 s before and 1 and 0 s after (M4, 4 Nov).
  A clones folder of a gone process is deleted by the next such run.
- Correct across worktrees: cargo judges a workspace crate fresh by mtimes, and its artifact names
  don't depend on the checkout's path, so worktree A's build would pass as fresh for worktree B's
  older sources. When the folder's last build (`.scenic-built-by`) came from another worktree, the
  workspace crates' fingerprints are deleted first, under the lock: they compile again, reusing their
  incremental state where the code is the same. Test: tools/dev/test/shared-target.zsh (check.sh).
- Outside `.claude/worktrees/` (the main checkout) cargo builds as it would, in its own folder,
  without the lock, only recorded (as the shim's child: Dev jobs, below). Passed straight to cargo:
  commands that never build (the shim's `reads`: fmt, metadata, tree, update, version, …, found
  after cargo's own options, `--color never fmt` too) and any command asking `-h`/`--help`/`-V`/
  `--version` (remote-tests.sh's `cargo nextest --version` waited behind builds, 4 Nov). Under the
  lock: the shim's `builds` (build, check,
  clippy, test, bench, run, doc, rustc, rustdoc, fix, clean, install, package, publish, report,
  miri, nextest) and any subcommand in neither list (another `cargo-*`, a config's alias), which
  says so. shared-target.zsh checks that every built-in of `cargo --list` is in one list.
- A worktree's own `target/` isn't written: run a built program through `cargo run` (which runs
  a clone), never `target/…/<bin>` or a path in the shared folder, which the next worktree's build
  may have replaced. remote-tests.sh builds on the test Mac in its own folder; the shared one is
  never copied there.
- sccache still wraps rustc inside `.claude/worktrees/` (the untracked
  `.claude/worktrees/.cargo/config.toml`, #439; ~/Library/Caches/scenic-sccache, 10 GB cap): it
  refills third-party crates after a sweep.
- Room (#954): tools/dev/sweep-targets.sh (launchd, every 20 min) keeps the Mac's room target free
  for the pool. Once free disk falls below the room target (`room-target.json` in the agent's
  folder, the build page's "Disk room target") plus the sweep's margin (`sweep-margin.json` beside
  it, set on the build page's Machines tab, Settings, "Dev sweep margin"; none set:
  `SWEEP_MARGIN_DEFAULT_GB` in crates/pipeline/src/agent/settings.rs, which the sweep reads from that
  line), it removes the shared folder's entries (each profile's deps, incremental, build,
  .fingerprint and examples children, and the programs linked beside them), the least recently
  used (read or written) first, until free space is back above both; still below, it empties the compile cache.
  With no room target it keeps the margin alone free. The folder's lock is only tried: held, the
  log says so and the next run tries again (the compile cache still goes); taken, the entries unused since they were listed are only renamed
  into `.scenic-sweep-trash` under it and deleted after. Each removal, with why, goes to
  `.claude/sweep-targets.log`; `sweep-targets.sh --dry-run` prints what it would do. Nothing of the shared folder is removed by age alone. Test: tools/dev/test/sweep-targets.zsh (check.sh), on a fake disk.

## Dev jobs: every run recorded (#946, 5 Nov)

The dev job dashboard (the dev tools app's `/jobs/`, tools/devtools/README.md) shows every run of
check.sh (each part, its compiles from cargo's output), publish.sh (each step), remote-tests.sh
(the waits for the test Mac's key and a slot, the copy, the run there, each compile, suite and
test from what it prints: cargo test's tests by when they ended, nextest's by their own times) and
every cargo build through tools/dev/bin/cargo (in a worktree: its wait for the shared folder, the reset of the
workspace's fingerprints, the build with each crate compiled from cargo's `--timings` report, which
the shim adds and moves out of the shared folder, each program run: a test binary a suite; a
measurement run, an example or a release run, and a backtest, `--replay` or a backtest test, so
named; in the main checkout: the build and its crates). A span started inside another's (a cargo in a check's part) is part of it: each job exports
its id as `SCENIC_DEVJOB_PARENT`. A run of its own says who ran it: its checkout, branch, commit,
task (`SCENIC_TASK`, else the latest `#<n>` in the branch's commits since main) and whether an agent
did (Claude Code's `CLAUDECODE`).
- The scripts record through `tools/dev/devjob.zsh` (`dj_start`, `dj_end`, `dj_span`, `dj_attr`,
  `dj_wrap`) and `tools/dev/devjob.py` (`wrap`: a command run as a span, its output passed on as it
  comes and read after; `sampler`). Each event is a JSON line appended to this Mac's spool,
  `~/Library/Logs/scenic/devjobs/spool.ndjson`; the sampler (one a Mac, started by the first live
  span, gone once none is) sends it every 2 s, samples the Mac's load, memory and swap every 10 s,
  reads cargo's timing reports and ends a span whose process is gone (`gone`). The app away, the
  events wait, one batch (`sending-*.ndjson`) and the spool, which grows meanwhile (a few MB a day of
  jobs, no cap: the app away for long is worth fixing, not hiding), for the next sampler; nothing
  in a job waits on the network. Its own log: `~/Library/Logs/scenic/devjobs/sampler.log`.
- **Recording never stops a job:** an event that can't be written (a full disk) is dropped, the
  dj_* functions return 0 (publish.sh runs under errexit), and `devjob.py wrap` runs its command
  whatever the spool. What a job pays: a `python3 devjob.py who` (~0.1 s: git's branch and commits)
  at a run of its own's start, a line appended per event. A wrapped command (check.sh's parts,
  remote-tests.sh, publish's release build and clippy) writes to a pipe, its stderr with its stdout:
  no terminal, so cargo prints no colour there.
- **A test that runs a recording script** (check.sh, publish.sh, remote-tests.sh, the shim) sets
  `SCENIC_DEVJOB=0` (nothing recorded) or a scratch `SCENIC_DEVJOB_DIR` with `SCENIC_BOARD` on a
  closed port, as `tools/dev/test/shared-target.zsh` does to check the shim's records: a test's
  toy runs are not the owner's. remote-tests.sh doesn't send `SCENIC_DEVJOB*` to the test Mac.
- The shim runs each program as its child, the last too (it had replaced itself with the last, so
  neither its time nor its status could be known); a signal to the shim goes on to it, and its
  clones go when it ends.
Test: `tools/dev/test/devjob.zsh` (check.sh when the recorder changes; publish's other tests):
devjob.py's own (`test_devjob.py`: reading outputs and timing reports, the wrapper, the sampler's
send and its end of a gone span) and devjob.zsh's events, whatever the names hold.

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
  folders, are deleted there. Build folder `~/scenic-check/<key>/target`, kept between runs, set by
  `~/scenic-check/<key>/.cargo/config.toml`, not CARGO_TARGET_DIR (sccache hashes every CARGO_*
  variable, so a per-key one missed the cache for every key: 0 % Rust hits on the mini, #887); rustc
  through the test Mac's sccache, third-party crates shared between keys.
- The run: nice 20 at utility priority (`taskpolicy -c utility`, as the pool's jobs, whose nice 0
  wins the CPU, #887; and so the lead's
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
- Runs here instead: on the test Mac itself (the M4 since 31 Oct; publish runs on the mini, so its suite goes
  to the M4); when the test Mac is down or the copy fails; when the tree
  has `data/build` (ignored test data that tests skip without, which the copy wouldn't carry); with
  `SCENIC_CHECK_LOCAL=1` (also `SCENIC_TESTS_LOCAL=1`).
- Disk on the test Mac (#912): before a run, other keys' folders go until the test Mac has its
  agent's room target free (`room-target.json` in its agent's folder, set from the menu bar) plus
  what the run needs (the key's last build folder; a new key, the largest there): first those
  whose worktree is gone (a worktree's key records its origin, the calling Mac and repo; each run
  sends that repo's worktrees), then the least recently used. Never a locked key (a lock younger
  than 6 h); a folder is removed by taking its lock first. The sweep also drops a removed
  worktree's key (`remote-tests.sh --drop <key>`). Tests: `tools/app/test/remote-tests.zsh`.
fmt, clippy, guards, web, swift and python stay on this Mac.

## A test that can't flake (#662, 25 Oct)

The suite runs three at a time on the test Mac at nice 20 and utility priority, beside builds: what takes milliseconds
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
- **"It came from the event, not a timer": switch the timer off** (`#[cfg(test)]` hooks such as
  control's `never_sweep()` and the beat's `never_tick()`), so anything that happens at all was the
  event's doing and the test needs no "within a second".
- **"It measured at least this much": make the thing measured last that long by construction.**
  A sleep inside a wait is at least the sleep; two waits at once are held open by events (one
  begins, signals, and ends only when the other has ended), never two sleeps hoped to overlap; CPU
  time is spun until the process's CPU clock has moved, never for a wall-clock span a loaded Mac
  may not give it (tools/check/timings_py.py). A load can then only make it longer. "It costs little"
  is CPU work per call, by this thread's CPU clock (`testkit::thread_cpu_ns`), never a wall-clock loop.
- **Process-wide settings:** pass the value in. Where the code reads an environment variable, a
  test sets it with `EnvVar::set(k, v)` (restored on drop, tests taking turns), and only for a
  variable no other test in that binary reads. A static a test changes is thread-local in test
  builds where it can be (the agent's `TEST_ROOT`), so tests in one process can't see
  each other's; the one that can't, the plain-open cap, is set through a guard
  (`store::range::cap_plain_opens`) that tests take in turn and that restores it on drop (other
  tests in a `cargo test` process run under it meanwhile: a lower cap only closes files sooner).
- **Hermetic by default:** temp folders (`tempfile::tempdir()`), never the real NAS or app home
  (`agent::is_live_path`); coordinators, brokers and viewers on loopback ports from `coord::test_port()` (each test agent
  its own, its `coord_port()`): the test program holds each such port for good, a stopped one's
  listeners kept for the next on it (`coord::KEPT`), and refuses a listen anew on any other but
  port 0 (#914); no
  `.local` names (`net::urls` gives 127.0.0.1 in tests).
- **Never a live program's port (#906):** the tests run on the Macs beside the live agent (the
  mini runs check.sh's remote tests and publish's suite), and a test listening on 127.0.0.1:8090
  took the agent's jobs' connections. `store::spawn::POOL_PORTS` (the coordinator's 8090, the
  map's 7311, the dev tools app's 8077): a test program's `store::spawn::listen` on one panics, and
  `tools/app/guards.sh` refuses test code naming one on a loopback or wildcard address (or
  `coord::PORT`, `DEFAULT_PORT`) unless the line says `pool port: not bound`. The coordinator and
  the map's server hold 127.0.0.1 beside their wildcard socket, so no other process takes it; a
  server that finds it taken waits, serving nothing, till it's free.

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
{"env": {"CLAUDE_ENV_FILE": "<main checkout>/tools/dev/agent-env.zsh"},
 "hooks": {
  "PreToolUse": [
    {"matcher": "Bash|Agent", "hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/tools/dev/test-guard.sh"}]},
    {"matcher": "Bash", "hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/tools/dev/shell-guard.sh"}]}
  ],
  "SubagentStop": [
    {"hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/tools/dev/report-guard.sh"}]}
  ]
}}
```

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
