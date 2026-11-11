# How agents run tests (owner's rule, 16 Oct; affected tests only since 17 Oct, #269)

Enforced by tools/dev/test-guard.sh (a PreToolUse hook in .claude/settings.local.json), which blocks
a command that breaks these and says why. It finds a test run where a command starts (also after
`env`/`time`/`nice`/`command`/`xargs`, by a path to cargo, with cargo flags before `test`, as `cargo t`,
`cargo nextest r` or `cargo-nextest`) and inside the script of `sh -c`, `eval` and `python -c`; it
refuses runs it can't read (`$c test`, `cargo --config alias.…`, `cargo -C`, `make`/`just test`, a run whose
words hold `$( … )`, `<( … )`, `>( … )` or backticks outside single quotes (an assignment before it, a redirect's target
and a comment aren't its words), and `sh -c`, `eval` or a command word built from a substitution whose script mentions cargo test); a run inside such a substitution is found.
Not seen: a run through a shell function or alias, a script file, or a substitution's output that names cargo without the script saying it (`$(cat cmd.txt)`).

- Tests build optimised (`[profile.test]` in Cargo.toml, #505: opt-level 1 for the workspace, 3 for
  dependencies; debug assertions and overflow checks stay on), so the first `cargo test` in a fresh
  worktree or after a dependency change compiles the dependencies once (~5 min); after that an edit
  rebuilds only the workspace crates.
- Agents run only the affected tests: `tools/dev/affected-tests.sh` in the worktree prints them as
  `cargo test` commands, from the branch's diff against main (working tree included): each changed
  module (`agent/build.rs` → `-p pipeline --lib -- agent::build::`), the modules using it one level
  out, and the integration tests naming it (a crate's whole targets on one line, one cargo run
  building them in parallel), and a module's own tests when a file it compiles in changed
  (`include_str!`, `include_bytes!` or `include!` outside a `#[test]` function: web/buildpage's pages
  in coord/mod.rs, #969), and a crate's whole unit tests when its source compiles the file in from
  another crate by `#[path]` (agentcore's tests, which run on the plan, after an edit of pipeline's
  agent/build.rs, steps.rs, idents.rs, theplan.rs or tiles.rs: they reach it through
  `seam::for_tests`, not by name). A moved file counts at its old path too (`--no-renames`, as in
  check.sh's own triggers). Run those, or narrower: the same `-p` and target, each
  filter starting with one it prints. No Rust change, nothing to run. `affected-tests.sh <commit>`
  or `<a>..<b>` shows the set for past work.
- Scan tests (#662): an integration test that reads the repo's own files (write_sets,
  inputs_accessor, cli, source_names …, through `common::root()`) names them on a
  `//! scans: <path prefix> …` line; a change to any file under one (any kind, deleted too) runs
  it whole. guards.sh refuses such a test without the line. (#660 changed agent/room.rs; check
  passed and publish failed on write_sets, which nothing mapped to it.)
- The tracing goes one level: `crate::<mod>`, the parent naming it, children using `super::`,
  the crate's own binaries, its `tests/`; other crates' modules naming `<crate>::<mod>`, or
  `crate::<mod>` where their root re-exports it (`pub use <crate>::…`) or a module of theirs does by
  glob (`pub use <crate>::<mod>::*;`), and so on through their own re-exports (none is left between
  the workspace's crates since #1084), their binaries and their integration tests.
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
  - first, on the publishing Mac, `crates/server/tests/serves.rs` against the release server it
    publishes (#1114: the test's binary, built as the suite builds it, run with `CARGO_BIN_EXE_server`
    naming `target/release/server`; cargo and nextest set that variable themselves as they run a test);
  - `cargo nextest run --workspace`: every workspace crate's libraries, binaries and integration
    tests (det, roadcore, pipeline, server, store, names, pyport, codecs, jobrt, buildstore, coordwire, offload, landmarks, mapview, inputs, terrain, trees, roads, bld, agentcore, webpenc), on the test Mac;
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
(`-D clippy::disallowed_methods`: processes only through `store::spawn`) on the crates the change
affects (#1123: the packages of the tests' set below and those holding a changed file; every crate
when a manifest, Cargo.lock, clippy.toml, the toolchain, `.cargo/`, a crate outside the workspace or
rustflags in the environment changed; none, skipped, when no crate did: the lint flags a call where
it's written, and a user of a changed crate no longer compiling is the tests' build's to find, or
publish's whole clippy line's), `tools/app/guards.sh`,
when web/ or a `tools/check/*.test.mjs` changed: `node --test` of every `web/test/*.test.{ts,mjs}` and
`tools/check/*.test.mjs` (globbed, so a new test file runs without editing a list), the web typecheck and build (after `tools/dev/web-deps.sh`, which gives a fresh worktree its `web/node_modules`: `pnpm install --frozen-lockfile --offline` from this Mac's shared pnpm store, only when node_modules is missing or not a finished install of this lockfile (its `node_modules/.web-deps-lock` stamp); a package the store lacks is an error naming the one online install that fills it; publish.sh runs it with `--online`), `node --test` of tools/devtools/test when
tools/devtools or the build page's timeline modules it serves changed (#672), `tools/dev/test/devjob.zsh` when the
dev jobs' recorder changed (#946), `tools/app/test/shell.zsh` (publish.sh's shell
steps in `tools/app/lib.zsh`, tested in a scratch folder) when tools/app changed, `tools/dev/test/test-guard.zsh` when the test guard changed, `tools/dev/test/git-guard.zsh` when the git guard (below) changed, `tools/dev/test/check-record.zsh` (check.sh itself on a toy repo: what it records) when check.sh changed, the Swift build and self-test when tools/status changed,
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
Clippy runs after the other parts, just before the tests; when both run and the tests run on this Mac
(the test Mac itself), they take one turn with the shared build folder (#1123, below: a hold, let
go as the first test starts), so the tests' build doesn't queue again behind other agents (10 Nov:
clippy waited 118 s, then the tests' build another 86 s).
`tools/dev/test/check-reuse.zsh` runs when the reuse rule's files (check.sh, check-reuse.py,
affected-tests.sh, merge-ok.sh) or the test changed.
One PASS/FAIL/REUSE line per part with its wall and CPU time here (the tests part also the test Mac's);
whole outputs in ~/Library/Logs/scenic/check-<time>/. A pass on a clean checkout (nothing
untracked either) records HEAD in `.git/check-passed` with its tree key (`tree` HEAD's tree, `base`
the merge-base's tree), `at` the merge-base, `tests` the hash of its test commands and a `reused
<part>=<commit>` for each part it took from an earlier pass. A record names the tree it checked
(#980): check.sh notes HEAD, the tree key and whether the checkout is clean when it starts, and
records nothing (`PASS, but NOT recorded: …`, exit 1) when HEAD moved during the run (a commit, a
reset, a checkout), the environment sets how the tests build (`CARGO_PROFILE_*`, `RUSTFLAGS`: no tree
says what it held, and such a run takes no reuse either), the checkout has untracked files or
uncommitted edits at the start or the end,
or a file of the checkout (tracked, or untracked and not ignored) was written after it started (an
edit undone before the end, a reset away and back): on 6 Nov a check ran on while another session
reset its worktree, and recorded the new HEAD as passed. A later run on a clean checkout with
the same tree key passes at once, recording its HEAD with that pass's fields and `reused
all=<commit>` (an amend of the message, a cherry-pick onto the same base: nothing a part reads changed).

A rebased branch (#950), or one edited since a pass (#969): on a clean checkout (nothing
untracked), check.sh asks `tools/dev/check-reuse.py` whether an earlier pass P still holds for
clippy and for the tests part, the two that take minutes (fmt, guards and the rest are never
reused: each runs by its own rule, as without a pass). It weighs X, the files differing between
P's tree and HEAD's: each is main's (changed by main since P's base, HEAD holding main's version)
or the branch's own (edited since P: on 5 Nov, 56 of 61 passes reused nothing, every check after
an agent's own edit running its whole set). Of the newest 60 passes recorded (any branch's), the
one with the fewest own files, then the fewest in X. Neither part is reused when a file in X is:
- of no known kind: a top-level entry outside the script's `KNOWN`;
- for clippy, any Rust build input; for the tests, one, main's or the branch's own, other than a
  crate's source (a `.rs` file under `crates/<crate>/src`, its root too, not a build script or a
  proc macro's), an integration test's (under `crates/<crate>/tests`), an example (no test builds or
  runs one; clippy checks it), a file `include_str!`d or `include_bytes!`d, or one a crate's source
  declares a module by `#[path]` (a source of that crate too: the plan's files are agentcore's tests'
  as well as pipeline's); not one `include!`d or named in a macro, nor a crate `[patch]` names (its users' manifests name a version,
  not its path). The inputs: `crates/`, Cargo.toml and Cargo.lock, rust-toolchain, mise.toml,
  clippy.toml, rustfmt.toml, `.cargo/`, `.config/`; the package folders (Cargo.toml's members and
  exclude, and the paths the manifests name: path dependencies, `[patch]`); any file a crate's
  source `include_*!`s or `#[path]`s, names as a macro's or attribute's first `"../"` literal, or a
  build script or proc macro reads; the scripts running the parts (`RUNNER`: check.sh,
  check-reuse.py, affected-tests.sh, tools/dev/bin, tools/app/remote-tests.sh, and the recorder
  wrapping them, tools/dev/devjob.zsh and devjob.py).
For the tests part, what HEAD's commands run again:
- the tests P's commands didn't run (each pass keeps its commands in `.git/check-tests/<hash>`, 30 days);
- every test in a program linking a changed source's crate, main's or the branch's own: that crate's
  targets, and those of the crates reaching it through their dependencies (read from the manifests:
  a program holds its crate's dependencies of any kind, and theirs but for their dev-dependencies).
  A file compiled in outside a `#[test]` function counts as the source compiling it (web/buildpage's
  pages in coord/mod.rs). Nothing nearer can be shown: the one-level tracing above bounds what a
  branch's first check runs, but a test's result can change through a user's user, a trait's impl,
  a macro or a re-export, none naming the file changed;
- of the rest, what changes without a program changing: `affected-tests.sh P..HEAD`'s integration
  tests changed and scan tests whose `//! scans:` line covers a file in X (that line trusted for what
  the test reads; guards.sh refuses a `root.join("…")` or `repo().join("…")` under no prefix of it), a
  `#[test]` function's own `include_str!` or `include_bytes!` of a changed file (that test), and a source's
  `CARGO_MANIFEST_DIR` or `".."` path inside a `#[test]` function over a file in X (that test). A path whose base
  can't be told (`join("..")`, `"{}/../x"`, a `CARGO_MANIFEST_DIR` whose parent is taken) is the
  whole repo; one leaving the repo from the crate's folder is its tail under any folder. Such a read
  outside a `#[test]` function, inside a `macro_rules!`, or in a crate whose targets aren't cargo's
  defaults for its folder (its name, no `[lib]`/`[[bin]]`/`[[test]]`, not both lib.rs and main.rs)
  runs the whole part.
So a test reused is one in a program no changed source is linked into and no changed file is read by.
Both parts run on a doubt: a symlink anywhere in the repo, an `include!` or
`#[path]` outside the package folders, a source reading the environment as it compiles (`env!` or
`option_env!` other than cargo's own `CARGO_*`),
`.cargo/config` naming paths. A part reused prints `REUSE`
with P and why it holds (main's changed files and the own ones, which tests ran again); a part
run says why it wasn't reused (no such pass, a lookup failing, the file that is an input). Its test,
`tools/dev/test/check-reuse.zsh`, runs the rule on toy repos (docs only, dev tools only, a file a
test reads, a module's change on main, a new file no module declares, the crate's root changed on
main, a Cargo.lock change, a file the root includes, a file a module compiles in, a binary's root,
an example, a module two levels from the test, a crate depending on the changed one directly, through
others or by a dev-dependency, a dev-dependency of a dependency, a crate linking nothing changed, a file
another crate's source names by `#[path]`, a binary's source and the integration test running it, a file an integration test compiles in, a
patched crate, a source reading the environment, the runner changed, an unknown
file, the branch changed after the rebase, its own edit of a module, of the crate's root, its
manifest, an included file or an integration test, fewer or more tests than the pass ran, the pass
with the fewest own changes, no earlier pass, an untracked file, and the reads above
that need a further look: `join("..")`, a deeper base, a symlink, a `#[path]` outside, a macro's
test, lib and main, a test folder, a renamed package, a `[patch]` path, a macro attribute's
folder) and checks the lists: every top-level entry of the repo is in `KNOWN`, and the runner
scripts' code names no repo script outside `RUNNER`.

publish.sh doesn't reuse: its suite is whole, the one run with nothing traced, which every lighter
rule above leans on. (#969 tried taking a passed publish's suite when no Rust build input differed;
4 of 27 publishes qualified on 6 Nov, and it would have trusted the `//! scans:` lines at the
one gate that doesn't: `cli`'s left out README.md, which it reads.)

Enforced twice: `tools/dev/report-guard.sh` (a SubagentStop hook) keeps a worktree agent with
commits beyond main from finishing until its HEAD passed, and the PM runs
`tools/dev/merge-ok.sh <commit>…` before cherry-picking: ok when a pass is recorded at the commit or
a later commit of its branch and holds, saying what that pass reused and from which commit; a pass
holds when it names its commit's tree and every pass it reused from is recorded and holds in turn
(the reason given when none does). report-guard.sh, check-reuse.py and check.sh's own reuse of a
tree checked before likewise take only a pass naming its commit's tree; passes recorded before
#980 without one (a dirty checkout's, or one whose HEAD moved) vouch for nothing.

## Whose checkout: git writes (#980, 6 Nov)

A session writes with git only in its own checkouts. `tools/dev/git-guard.py`, run by the
PreToolUse hook `tools/dev/shell-guard.sh` (so no settings change), refuses a Bash command whose git
write (reset, checkout, switch, restore, commit, rebase, merge, cherry-pick, revert, am, pull,
clean, add, rm, mv, apply, stash but its list/show/drop/clear/create/store, worktree remove/move,
read-tree, update-index, checkout-index) goes to a checkout of this repo that isn't the session's.
That checkout is the one `git -C`, `--work-tree`/`GIT_WORK_TREE` or `--git-dir`/`GIT_DIR` names (on
the command or `export`ed earlier on the line), else where an earlier `cd`, `pushd`/`popd` or
`env -C` of the command line went, else the cwd; for `worktree remove|move`, its path. Whose:
- a subagent's worktree `.claude/worktrees/agent-<id>` (Claude Code made it): agent `<id>`'s (the
  hook input's `agent_id`) while it runs (on 6 Nov a reviewer started without isolation in another
  agent's worktree reset it while that worktree's check ran). Once Claude Code's
  `subagents/agent-<id>.meta.json` (in the project's transcripts) says the agent finished, it is
  whoever's writes there next: a reviewer told to work on that branch, or the PM. A subagent's write
  claims it (`.git/worktree-owners/`, under the path's hash) until that subagent finishes in turn;
  meanwhile no other session writes there. Only the PM (or the agent itself) removes or moves one.
- a worktree a subagent made (a `git worktree add <path>` the guard saw, of a path not there yet,
  recorded in `.git/worktree-owners/`): that subagent's while it runs.
- every other checkout of the repo (the main checkout, the PM's `int-*` worktrees, those made
  before #980 too): the main session's. Main sessions are told from subagents only, not by session
  id, so a resumed, forked or compacted PM keeps its worktrees.
To look at or test a running agent's commit, make a worktree of your own (`git worktree add <scratch
folder> <commit>`). A write whose folder the guard can't tell (a variable not set on the line, a
substitution other than `$(pwd)`, `cd -`, a `popd` past the line's `pushd`s) is refused: write the
path out. Git commands inside `$( … )`, `<( … )`, `>( … )` and backticks, `sh -c`/`bash -c`/`zsh -c` and `eval` are
checked; reads (log, diff, status, show, rev-parse, merge-base, branch, worktree list), other repos
and quoted or heredoc text pass. Not seen: what a script the command runs does with git
(publish.sh's snapshot worktree is its own), git in `python -c`, and a `cd` inside a
subshell `( … )`, which the guard takes the line's later commands to be in too. Test:
`tools/dev/test/git-guard.zsh`.

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
  set as a test runs name clones too, so a test reads its program's path from them
  (`std::env::var("CARGO_BIN_EXE_…")`), never `env!`'s compiled-in path, which names the shared
  folder's program (tools/app/guards.sh refuses it). `cargo nextest run` runs its test binaries from such clones,
  made as it lists them; the lock goes when its first test starts, whatever the binaries (#1108:
  a binary with `env!("CARGO_BIN_EXE_…")`, pipeline's and server's `tests/cli.rs` then, kept it to
  the run's end, so on 10 Nov a hung test held every worktree's builds 35 min). A signal to the shim (a `kill` of
  the pid `ps` shows as cargo) goes on to the cargo or program it waits for, and the shim keeps the
  lock until that is gone (the build scripts and rustc of a killed cargo still run on, as they
  would from a plain cargo killed by its pid). Kept under
  the lock: doctests (rustdoc compiles and runs them in one go) and programs for another target
  (wasm32). On 4 Nov a `cargo run --release --example` held the
  lock 24 minutes while two check.sh runs waited; with a 60 s `cargo run` and two toy checks
  started 2 s after it, the checks waited 58 and 59 s before and 1 and 0 s after (M4, 4 Nov).
  A clones folder of a gone process is deleted by the next such run.
- A hold (#1123): check.sh's clippy and its tests' build take one turn. `cargo --scenic-hold
  <ready file>`, started by check.sh, takes the lock in line as any cargo, writes the ready file and
  holds it while check.sh lives; the holder's line names check.sh's pid (its processes are what a
  long hold's report shows) and the hold's own, `(hold <pid>)`. A cargo with `SCENIC_CARGO_HOLD`
  naming a hold the holder's line still names builds without taking the lock; where it would let
  the lock go (nextest's first test, a test's or run's programs cloned) it lets the hold go (USR1).
  remote-tests.sh lets it go when the tests go to the test Mac, check.sh after its tests part (or
  as it exits); programs never get the variable. A waiter behind a hold waits through clippy and the
  tests' build, which before queued apart (clippy waited 118 s on 10 Nov, then the tests' build 86 s
  more behind two agents); a long test run still holds no one up.
- Shared across worktrees, correctly (#1123): a workspace crate's artifact names don't depend on
  the checkout's path, so worktree B can use what worktree A built. Cargo judges freshness here by
  the sources' checksums (`-Z checksum-freshness`, unstable in cargo 1.98: the shim takes cargo's
  channel for a nightly's, `__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS`, and makes
  `tools/dev/bin/rustc-checksums` the rustc wrapper, which gives `RUSTC_BOOTSTRAP=1` to the compiles
  carrying cargo's `-Z checksum-hash-algorithm` alone, not to a build script's version or feature
  probes, then runs the wrapper cargo's config names, sccache). By times, a fresh worktree's files
  are newer than any build and B's older sources passed for A's build, so until #1123 every
  workspace crate's fingerprint was deleted whenever another worktree had built last, and every
  crate compiled again (113 s in a check's tests part, 10 Nov). Cargo checks a source by a path
  relative to its package, or by a full path outside it, so a unit is tied to the checkout it was
  built in when its record names another checkout: it compiles in a file outside its package
  (agentcore's `include_bytes!` of web/buildpage) that differs from this checkout's (cargo would
  check A's copy), it was built with a variable naming another checkout (`env!("CARGO_MANIFEST_DIR")`,
  whose value cargo doesn't check), a build script's output names one, its build script watches a
  file by `rerun-if-changed` (judged by time, below: this checkout's copy, older than A's run,
  passed for A's), or it was built without checksums (an older shim, or a cargo not honouring the
  flag: then every unit is, and the folder rebuilds as before #1123). When the folder's last build
  (`.scenic-built-by`, `checksums <checkout>`) came from another checkout, the shim looks at every
  local unit's records (the workspace's and path packages outside it, geos-src; rustc's dep-info,
  cargo's own, a build script's output) under the lock and deletes the tied units' fingerprints,
  and a build script run's output too: cargo passes a unit's staleness on to its users by what it
  finds on disk, not by a missing fingerprint (#1123 review: a program including what a tied run
  wrote kept the other checkout's). They and their users compile again. (A shim from before #1123
  reads another checkout in `.scenic-built-by` whatever built last, and so deletes every workspace
  fingerprint before it builds, as it did.) A unit kept that compiles in another checkout's file (the same bytes then) is listed in
  `.scenic-elsewhere` and looked at again before each build, since cargo goes on checking that
  checkout's copy, not this one's. `.scenic-tied` lists the last look's units, with why. So tests
  read their files as they run, from their package's folder (cargo and nextest run a test there:
  `std::path::absolute("../../tools/status/test")`), never by `env!("CARGO_MANIFEST_DIR")`, which
  ties the unit (store::testkit's tied every test build). A build script's `rerun-if-changed`
  paths are judged by time even so, so such a script runs again in each checkout: agentcore's and
  geos-src's say `rerun-if-env-changed` of a variable set by nothing (cargo runs a script it
  compiled anew by itself, judged by content), as `rerun-if-changed=build.rs` ran GEOS's CMake
  build and every crate linking it again in each new worktree. geos-src's compiles in GEOS's
  `source/Version.txt`, so a new GEOS runs it again; an edit in `source/` keeping the version, only
  with `GEOS_SRC_RERUN` set to a new value. Measured (M4, load 55-85, 11 Nov; the test build of agentcore, pipeline, store,
  terrain, jobrt and buildstore in a scratch shared folder): a new worktree's first build after
  another's, the same sources, 139 s and 20 workspace crates compiled before, 1 s and none after;
  back in the first worktree, 140 s before, 0 s after. Test: tools/dev/test/shared-target.zsh
  (check.sh): the same sources in two checkouts fresh, a differing file outside the package and an
  `env!` path compiled again with the crate using it, a build script watching a file older than the
  other checkout's run run again with the program including its output, back and forth; in one
  checkout, a source edited to the same size and dated back compiled again.
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
  refills third-party crates after a sweep. It can't share the workspace's crates across worktrees
  (#1123): it passes incremental compiles (the workspace's, in dev and test) and binaries (every
  test program) through uncached, and hashes every `CARGO_*` variable, `CARGO_MANIFEST_DIR` (the
  checkout's path) among them; its Rust hit rate on the M4 was 4 % (17 of 426, 10 Nov).
- Room (#954): tools/dev/sweep-targets.sh (launchd, every 20 min) keeps the Mac's room target free
  for the pool. Once free disk falls below the room target (`room-target.json` in the agent's
  folder, the build page's "Disk room target") plus the sweep's margin (`sweep-margin.json` beside
  it, set on the build page's Machines tab, Settings, "Dev sweep margin"; none set:
  `SWEEP_MARGIN_DEFAULT_GB` in crates/agentcore/src/agent/settings.rs, which the sweep reads from that
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
every cargo build through tools/dev/bin/cargo (in a worktree: its wait for the shared folder, a
check's hold of it (a `cargo` span of its own, its wait and its `held` time), the reset of the units
tied to another checkout, the build with each crate compiled from cargo's `--timings` report, which
the shim adds and moves out of the shared folder, each program run: a test binary a suite; a
measurement run, an example or a release run, and a backtest, `--replay` or a backtest test, so
named; in the main checkout: the build and its crates). A span started inside another's (a cargo in a check's part) is part of it: each job exports
its id as `SCENIC_DEVJOB_PARENT`. A run of its own says who ran it: its checkout, branch, commit,
task (`SCENIC_TASK`, else the latest `#<n>` in the branch's commits since main) and whether an agent
did (Claude Code's `CLAUDECODE`). A span is on the Mac it ran on (#1086): remote-tests.sh's run on
the test Mac and its compiles, suites and tests are that Mac's; the job says the test Mac's name
(`ran_on`) as soon as its run starts there.
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
  no terminal, so cargo prints no colour there. A name past the app's 2000 characters is cut there
  (devjob.py's `NAME_MAX`, devjob.zsh's `dj_name`, #1123): a cargo's name is its command line, and
  the app refused check.sh's nextest (4 KB with its filterset), losing its span and every span under
  it, the tests part's wait for the shared folder with them (45 refusals on 10 Nov).
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
  folders, are deleted there. Once the run has a slot, that copy goes into the slot's build folder
  (below), whatever else was there deleted. Both by content (`--checksum`), times not kept (#983): a file written is
  dated when it arrives, after every build in that folder, so cargo rebuilds it; one whose
  content is there already is left alone, its crate not rebuilt. (Kept times let a file edited
  during a run arrive at the next dated before that run's build: cargo took it for built and the
  next run tested the old binary.) Costs ~0.1-0.5 s a run over `-a` (the 46 MB tree, M1
  to M4), ~0.3-0.8 s on a cold copy (~4 s with `-a`). Tested by
  `tools/app/test/remote-tests.zsh`: a toy crate, an edit dated back, and a real `cargo test` of a crate over a dependency, whose test
  passes only on its own tree's value, in one folder: two trees taking turns, a dependency edited keeping its size, then its date too,
  a file back to an earlier version with an older date, a module moved, a source a test overwrote. The same turns with times kept fail it (#983's bug).
- The build folders are the slots', not the keys' (#969): `~/scenic-check/.build/<slot>/{src,target}`,
  one per test slot, the same paths whichever tree comes, kept between runs. A key takes the slot it
  last used if it is free, else the least recently used. rustc's incremental state holds only for
  the folder it was made in, so a folder per key compiled every workspace crate anew at a key's first run;
  here a run compiles what differs from the slot's last tree, publish's too. The target is set by
  `~/scenic-check/.build/<slot>/.cargo/config.toml`, not CARGO_TARGET_DIR (sccache hashes every CARGO_*
  variable, so a value per folder missed the cache: 0 % Rust hits on the mini, #887); rustc
  through the test Mac's sccache, third-party crates for an emptied folder. A folder held 7.1-7.9 GB
  through the runs measured below, with a variant of the crates a package set without pipeline changes the
  features of. Cargo never deletes what it built, so it can grow as dependencies change (a key's folder kept for
  publishes reached 14 GB); only the room rule below removes one, and never the one a run is building in.
- A slot is its build folder's lock: mkdir takes it, and it's freed only once nothing the run started is
  left. As a run ends or is stopped, its process group goes first (TERM, 15 s, then KILL; nextest stops the tests, each in a group of its own, within 10 s); a run killed outright has
  its watcher do that, and the next run takes the slot only when the group is gone. A run whose slot isn't its own at the end gives no result.
  Tests: `remote-tests.zsh`, `remote_main` itself in a fake home (each of these, five runs at once on three slots). The dev job sampler
  leaves the group as it starts (it's the Mac's, not the run's).
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
  for an approval for ever. The probe (a fresh connection) has 20 s (`SCENIC_TEST_PROBE_S`, a test's shorter), the lock calls 60 s, and the run
  must print its first line (`remote-tests: connected to the test Mac`, at once) within 120 s; past a bound it runs here, saying why.
- Runs here instead: on the test Mac itself (the M4 since 31 Oct; publish runs on the mini, so its suite goes
  to the M4); when the test Mac is down or the copy fails; when the tree
  has `data/build` (ignored test data that tests skip without, which the copy wouldn't carry); with
  `SCENIC_CHECK_LOCAL=1` (also `SCENIC_TESTS_LOCAL=1`).
- Disk on the test Mac (#912): before a run, other keys' folders go until the test Mac has its
  agent's room target free (`room-target.json` in its agent's folder, set from the menu bar) plus
  what the run needs (its build folder; an empty one, the largest there): first those
  whose worktree is gone (a worktree's key records its origin, the calling Mac and repo; each run
  sends that repo's worktrees), then the least recently used, then the other slots' build folders,
  least recently used first. Never a locked key (a lock younger
  than 6 h) nor a slot in use; a folder is removed by taking its lock or slot first. The sweep also drops a removed
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
  They poll every 10 ms under `WATCHDOG` (2 min), which only turns a hang into a failure. Never a
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
- **A hung NAS: the agent's `nasread::fake::hang(prefix)`**, every call under it held, each given
  up at its real deadline (`nasread::DEADLINE`, 20 s). Where the claim isn't how long the loop is
  stuck (a pause answered by control's watcher with its sweep off, a job's end woken by the
  reaper), `.deadline(d)` gives the held calls a short one, test builds only (#969:
  `a_job_is_reaped_while_the_nas_hangs` waited out 225 s of them); "it made no NAS call" is
  `fake::held_here()` unchanged across it, not a time.
- **"It measured at least this much": make the thing measured last that long by construction.**
  A sleep inside a wait is at least the sleep; two waits at once are held open by events (one
  begins, signals, and ends only when the other has ended), never two sleeps hoped to overlap; CPU
  time is spun until the process's CPU clock has moved, never for a wall-clock span a loaded Mac
  may not give it (tools/check/timings_py.py). A load can then only make it longer. "It costs little"
  is CPU work per call, by this thread's CPU clock (`testkit::thread_cpu_ns`), never a wall-clock loop.
- **Process-wide settings:** pass the value in. Where the code reads an environment variable, a
  test sets it with `EnvVar::set(k, v)` (restored on drop, tests taking turns), and only for a
  variable no other test in that binary reads. A static a test changes is thread-local where
  it can be (the agent's `TEST_ROOT`), so tests in one process can't see
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

### Every test bounded (#1108, 10 Nov; #1112, 11 Nov)

`WATCHDOG` is checked between looks: a look that never returns (a call blocked in the kernel, a
lock never let go) defeats it. So nextest bounds every test itself (`.config/nextest.toml`, read by
check.sh's and publish's `cargo nextest run`, here and on the test Mac): SLOW each minute, ended
(SIGTERM, 10 s, SIGKILL) and failed as TIMEOUT at 3 min, over `WATCHDOG` (2 min), so a waiting test
fails itself first, saying what for. That bound is the guard on the budget below: a test over it
fails, and is never given more. Two tests are still bounded at 20 min, listed there with why
(terrain_task's z6 tiles, over the budget, left for #1096's change of the terrain steps). `cargo
test` runs have no such bound.
- **The budget (owner, 10 Nov): every test under 10 s alone on the M4 unloaded.** The bound is
  that with room for the loaded test Mac (three suites at nice 20 beside builds): over 8-10 Nov's
  runs (the dev jobs' test spans) a test took up to 4 times its time alone at the median, 7 times at
  the 90th percentile. A test past the budget is made smaller, never given time: one that reruns a
  whole job per safe point stops at a few (the first, the last, midway, a seeded sample); one on a
  full-size fixture takes a small or mostly empty one reaching the same code; one at live scale
  asserts the same properties on a small synthetic input, and its live-scale run is a benchmark,
  `#[ignore]`d, run by hand (`-- --ignored`); one waiting on a timer it doesn't claim (a beat's
  period, a deadline, a poll) is given a test's own short one (`beat::Handle::tick_every`,
  `nasread::fake::Hung::deadline`, a coordinator's `Shared::headers_max_ms`, `SCENIC_TEST_PROBE_S`),
  every timer scaled together, so the margin between what is measured and what would show a
  failure stays as it was (the beat's tests: a gap allowed of 1.5 s against a hung call held 2.5 s,
  as 11.5 s against 20 s before; a 1-s hold under a 3.5-s gap allowed had proved nothing). Every assertion stays: what the smaller test still proves, and how, is said in its comment.
- **A stopped coordinator refuses at once** in a test program too: the listeners it keeps for the
  next coordinator on the port (`coord::KEPT`) take each connection and close it, as a port nothing
  listens on refuses it (#1112: its clients had waited out their 15-s timeout in the backlog, 15-60
  s of each test asking a stopped lead).
- **No signal ends a process stuck in the kernel** (`ps` state U, then E as it exits): nextest
  waits on it, its run with it. A test's run never holds the shared build folder's lock (above),
  so such a hang delays only its own run, never another worktree's build.
- **What a test calls must not block in the kernel on the NAS.** On 10 Nov
  `agent::takeover`'s test, through `jobrt::sys::openers` (libproc's
  `proc_listpidspath(PROC_ALL_PIDS, …)`, which resolves the path of every open file of every
  process on the Mac), sat in state U for 7 min while the owner's work stalled the NAS's SMB share,
  then would not exit; the scan is gone (the agent's lock names its holder, docs/surface.md). Reading
  one process by its pid (`proc_pidinfo`, `proc_pidpath`) touches none of its open files. `lsof`
  without `-p` walks them all the same way (one sat 16 min in check.sh's shell part, 10 Nov, asking
  who held a port). A server a test starts takes a free port and says which (`--port 0`, its
  listening line: crates/server/tests/serves.rs), so no test asks who holds one. (`netstat -anv -p
  tcp` isn't a stand-in: started from mise's Python, as check.sh's parts are through devjob.py, it
  printed no Internet sockets at all.)

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
(hand-made), `~/Library/Logs/scenic/`, `.git/check-passed`, `.git/check-tests/`.

## The build page on a phone: touch-test.mjs and safari-sim.sh (#614, 23 Oct)

Every brief that changes web/buildpage runs both and looks at the screenshots. Neither presses a button
that changes anything: the page opens as `?view` from a read-only server (`touch-test.mjs --serve`,
port 8097) that serves web/buildpage and forwards only the reading calls (swarm, steps, regions,
settings with `{}`, history, live) to this Mac's agent on 127.0.0.1:8090, refusing the rest.
- `node tools/dev/touch-test.mjs [--out DIR]` (35–55 s on the M1): headless Chrome as a 390×844 phone (DPR 3,
  mobile, 5 touch points, iPhone user agent), real touches through CDP. Each tab: no sideways
  scroll (else the elements past the edge), no console errors, a screenshot (`chrome-<tab>.png`,
  1 px a CSS px). Then: a card pins on a tap and unpins on a tap elsewhere; a tap on the
  map pins the stack under it, each tap its own, and one off it pins nothing (#615); the world map zooms
  with a pinch (its touch-action turns `none`, its zoomed-in sign) and pans
  with a drag (pixels changed once it held still), the page staying put; the Jobs tab's stages counting done and left, no job squares; no share of a whole in the header, the Overview or the legend; a region row lighting its region on the map (a mouse); the panels' split (#685) stacked at phone width
  and side by side 2:1 at 1280 px, its bar dragged, a double click back to 2:1; the schedule's timeline (#787) pans with one finger sideways and zooms with a pinch, no dropdown; settings fields focus on a
  tap without a jump (never Set); the map (#685: in its own panel, published tiles off at first, a step's legend item soloing it while hovered and holding it on a click or tap, its published tiles painted by age; the age controls' timeline, mark labels never overlapping, a mark hovered lighting its tiles, the range inputs' swap and one-span rule; `--catalogs FILE` answers `/api/catalogs` from `scenic catalogues > FILE`, `--width W --scheme dark|light --only-map` for its screenshots). At 720 px or wider (`--width`) the "no text cut off" check is skipped: a desktop keeps labels on one line, ellipsis and all (#617). `--agent H:P` reads another Mac's agent (the lead's, for its forecast's queue: an agent before #685 is asked a Mac at a time). `--member NAME --tocome FILE` checks a member's queued tiles (#622). One PASS/FAIL line a check, exit 1 on a FAIL. The other Macs'
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

## Where test time goes (#969, measured 5 Nov)

From the dev job dashboard's 5 Nov records (48 check.sh runs, 4 publishes) and the last 9 publish
suites' nextest times (M4, loaded: three runs at nice 20 beside builds), all before #969's changes
(the single nextest run above, the shorter agent timer tests, the one live-scale forecast run, the
one source_names scan):
- **check.sh**: p50 986-2100 s a run, nearly all of it the tests part (p50 1115 s, p90 2048 s); clippy
  p50 42 s, fmt/guards 3 s each, web 11 s when it runs. In the tests part: compiles 40-60 % (each
  `cargo test` line built under its own feature set, a server line recompiling store, codecs,
  buildstore and pipeline, ~170 s), the suites 30-40 % (pipeline's lib, p50 316 s, p90 753 s, is
  nearly all of it), waits for a test-Mac slot 10-20 % (p50 188 s, p90 838 s).
- **Selection**: 84 checks ran tests, most with 100-170 filters: a change to `agent/mod.rs` or another
  widely used module reaches most of pipeline's lib one level out; any change under `crates/` runs
  the scan tests (pipeline's integration tests). The heavy modules were in most: agent::tests 63,
  agent::forecast 57, agent::build 56, depth 33 of the 84.
- **Reuse** (#950): 5 of 61 passes on 5 Nov reused a part; the rest said "no earlier pass whose tree
  plus main's changes is this tree": reuse holds only after a rebase with no own change since, so
  an agent's every check after an edit runs its whole affected set.
- **Publish** runs its whole suite whatever check.sh passed (by design: its tree is the merged one):
  "Rust tests" p50 1754 s (compile 4-14 min, then the run 700-860 s, bounded by its slowest test).
  An idler run (5 Nov 17:00, `--workspace`, 6bd14bb2): compile 247 s, run 306 s, 4135 s of CPU.
- **The slowest tests** (median of 9 publish suites, s): depth's z6 tile paused at every safe point
  718; agent::build a_key_names_exactly_its_reads 581; agent::forecast's live-scale run 425;
  polar's block paused at every safe point 289; depth's run stopped after some blocks 288;
  slope_pack's z6 tile down to its deep terrain 280; agent::tests a_job_is_reaped_while_the_nas_hangs
  225 (waiting out the NAS deadlines); terrain_task's group tiles 219; agent::tests
  beat_steady_while_planning 182 (a 3-min plan); terrain_task's z6 stopped and resumed 162;
  beat_steady_while_the_nas_hangs 143; slope_pack's 2 threads and 8 125; terrain_task's subtrees
  108 and paused mid-sampling 106; depth's fine source 104; trees' z3 pieces 64; agent::tests'
  hung NAS while planning, cancel while planning and pause on a hung NAS 62 each; source_names'
  two scans 62 and 60; offload's coordinator gone 61; trees' z3 run 59; treepacks 56; depth's
  rivers 50; trees' room making 49; depth's resumed z6 48; slope_pack's assembly 46; polar's block
  in progress 43; pool::sim's cache filled in parts 43. The top 10 are 45 % of the suite's test
  time, the top 30 70 %; the 1,909 tests under a second, 3 %.

#969's second part, measured 5 Nov on the M4 under load 120-190 (each test alone, before and after
interleaved, 3 runs each, median wall, CPU in brackets): the hung-NAS fake's short deadline, the
reaped job 223 → 13 s, the pause on a hung NAS 61 → 11 s, the hung NAS while planning 61 → 8 s;
the pause while planning (an 8-s plan) 31 → 9 s; depth's z6 tile paused at its safe points 80 → 5 s
(75 → 7 s), its run stopped 220 → 6 s (a missing grid's open no longer waited out to the end of its
retries), its resumed tile's test cut (11 s); a key names exactly its reads 57 → 20 s (50 → 20 s);
terrain_task's z6 tile stopped and resumed 25 → 15 s (159 → 95 s); slope_pack's deep tile and its
2-and-8-threads test merged (24 + 10 s, 19 + 7 s → 29 s, 22 s). The suites in the publish records
above ran three at once beside builds, their tests' walls 3 to 15 times these. The same affected
set as check.sh's (1,261 tests; 1,265 before), on main's code and on the branch's, at once: the
tests' run 658 → 343 s, CPU (with each build) 3,993 → 3,169 s. A re-check after a one-line edit of
terrain_task.rs since that pass ran 275 of the 1,261 again (its module's users,
agent::tests among them, and the scan tests): the test Mac's CPU 3,169 → 2,283 s, its wall about
the same, bound by the pipeline's compile after any Rust edit (11 min) and a flaky test's 300-s
watchdog (a_slow_plan_delays_no_grant_pause_or_heartbeat: 2 of its last 61 runs failed so; its
cause, a hold ask taken and dropped by the taker it wasn't for, fixed since: `control::take_hold_request`).

#969's third part, measured from the dev job records and `.git/check-passed` to 6 Nov 22:30 (M1's
clock), the tests parts run on the M4 only, against the same selection of 5 Nov's:
- **The tests part**: p50 1,187 → 576 s, p90 2,059 → 1,645 s (50 runs before the first part, 98 since the second
  part; every Mac's parts by the dev job records' `stats`, 4 Nov to 5 Nov 21:36 against 6 Nov 01:40 to 22:30: p50 920 → 469 s, p90 2,048 → 1,574 s). Compiles 49 → 58 % of it, waits for a slot or the key 17 → 18 %, the rest, the run, 33 → 24 %:
  compiling is now what bounds it. A new key's first build (34 of the 98) takes 664 s p50, a key's
  later ones 193 s: 60 % of all compile time.
- **Cloning a new key's build folder from another key's** was tried and dropped: third-party crates
  are cheap through sccache, and the tree's crates must compile again for its path. The same
  build-only run of the usual targets: empty, 2,351 s of CPU; cloned, 2,278 s (-3 %).
- **Reuse**: 7 of 97 passes reused tests before the second part, 59 of 119 since. Of the 92 checks since then
  running their whole set, replayed with check-reuse.py as it stood at each commit: main's Rust
  changes 40, a page coord/mod.rs compiles in 10, a crate's main.rs alone 4, an example alone 1, the rest
  manifests, Cargo.lock, runner scripts, a `#[path]` module. Under #969's first tracing of main's changes
  (one level of naming, since replaced: it could skip a test whose result changed) 60 of the 92 ran part of
  their set, their test time 157,978 → 133,920 s (-15 %). Under today's rule, each test in a program linking
  a change, 51 checks of 5 to 7 Nov sampled and replayed at their commits: 32 run part of their set and 19 all
  of it (29 and 22 under the first tracing, as today's takes a crate's root as any source), and of the 29 running part under
  both, 27 % more filters. The 7 of 97 and 59 of 119 above are under the first tracing's rule or before. Their compiles stay.
- **The costliest tests** by total time (154 runs): the forecast's live-scale run 12,355 s, terrain_task's
  groups 10,627, a key names exactly its reads 10,164, polar's paused block 9,447, slope_pack's z6
  tile 9,325, depth's fine source 9,148, terrain_task's stopped and paused tiles 8,357 and 8,110. Of
  all test time the top 10 are 28 %, the top 40 55 %. terrain_task's paused tile (four pause points)
  and groups test (two block sizes fewer), alone, interleaved, load ~130: wall 59-69 → 41-51 s and
  80-96 → 69-79 s, the two's CPU 359 → 250 s. What they give up: the tile has 18 samplings and its
  log holds a block only from the 13th, so of the seven pauses before (after 0, 1, 2, 3, 5, 8, 13)
  the cut 2, 3, 5 and 8 were each the state 0, 1 and 9 still are, nothing kept, and 13 stays the one
  with a block kept (asserted); neither list pauses with two kept (17). Blocks 1 and 3 levels up
  differ from 0 and 2 only in `blocks`' shift.
- **Publish** "Rust tests", the dev job records' passed runs: p50 1,184 s, p90 2,122 s (6, 4 and 5 Nov) → p50 802 s, p90 1,478 s (28, 6 Nov).

#969's fourth part, measured 7 Nov on the M4:
- **The tests part before it**, the dev job records of 6 Nov 01:30 to 7 Nov 01:00: the 123 passed, p50 429 s, p90 1,340 s (all 146: p50 457 s, p90 1,432 s),
  the M4 less loaded than the third part's; the 4 since each test in a program linking a change runs again: p50 327 s.
- **Why a new key rebuilt**: of a new key's 2,647 s of compiles, 2,042 s (77 %) were workspace crates (47 units; 296 third-party
  units 606 s, 155 of it GEOS's build script), every one anew as rustc's incremental state
  doesn't hold across working directories. So the build folders are the slots', above. The usual
  check set, build only, beside others' runs: a new key's folder 576 s, CPU 2,674 s (load ~100); an empty slot's 300 s, 2,391 s (load ~50-85),
  which only a first use or a removal for room meets; an older commit's tree (15 workspace crates changed or depending on one) in a slot
  last used by another's 131 s, 599 s (load ~50); back again, with terrain's test edits, 101 s, 715 s (load ~40-70). Of those 101 s,
  pipeline's lib and its tests are 150 s of the 296 s of compiles: the split (#916) is what shortens that.
- **Features**: a package set with pipeline but not server (58 of 170 check.sh runs) built tower-http without
  server's features, and so all of pipeline a second time; pipeline's manifest now names them, and the two
  sets build the same units. A set without pipeline (7 of 170, each a second run before pipeline's) still
  has its own variants of a few crates (serde_core, syn, the `testkit` features), kept in the folder after the first.
  `resolver.feature-unification` would unify them, but cargo 1.98.1 ignores it without `-Z`.
- **Tried and left**: roadcore, codecs and det at opt-level 3 gave terrain's slowest tests nothing (rivers and narrow water 5.7 s, 6.8 s of CPU, either way).
- **Terrain's tests** (each alone, interleaved, wall and CPU): slope_pack's z6 tile 21.4 → 6.5 s, 20.4 → 5.4 s, its fixture's z12 tiles 64 → 16:
  full parents at z11 and z10 as before, and a partial one at z9, all it had at a coarser level. polar's paused block 14.7 → 5.4 s, on the block with the fewest z8 tiles
  over a chunk (two stops, not four): a stop with a chunk kept, one with all kept and the run after, the states the four gave; its progress test 3.4 → 0.9 s,
  a level fewer (the same counts asserted). depth's fine source 12.1 → 6.8 s, 20.1 → 9.8 s, the survey 5 z14 tiles across, not 15:
  each level's mean checked at the same point, at z12 and z14 as before. slope_pack's pyramid 2.2 → 0.6 s, 24.0 → 6.1 s, z0-4 not z0-5: three runs and the tops as before.
- **Not touched**: the live-scale forecast (with #993) and a key names exactly its reads (agent/build.rs, with #916 6f2), the costliest left beside terrain_task's.

#1123, measured 11 Nov on the M4 (load 40-85), in a scratch shared folder, the test build of
agentcore, pipeline, store, terrain, jobrt and buildstore (340 units) with its workspace units'
fingerprints and incremental state deleted, then #1123's affected tests (1,385 + 79):
- **Opt-level 0 for the workspace's crates in tests, dependencies at 3: tried and left.** The build
  117 and 129 s → 58 and 77 s (cargo's units 334 → 153 s of compile); the tests' run 122 s → 643 s,
  25 tests slow past a minute and 4 of depth's past nextest's 3-min bound. `[profile.test]` stays at 1.
- **Fewer test binaries (pipeline's 13 integration tests in one or two): not done.** Their compile
  and link, 15 programs, 15 s of the 334 s, each 0.4-1.6 s but cli's 4.6 s, built beside the rest
  (the critical path is agentcore's lib and its unit tests, 43 + 65 s: #1025's split is the lever);
  merging them would trade per-file `//! scans:` lines, affected-tests.sh's `--test` targets and
  check-reuse.py's per-test reads for a few seconds.
