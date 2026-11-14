#!/bin/zsh
# tools/dev/bin/cargo (#915, #937, #941) on a toy workspace: two worktrees build into one shared
# folder, each runs its own code (worktree b's sources older than a's build), a second cargo waits
# for the first, saying so, and programs and tests run outside the lock, from clones, with cargo's
# environment and folder; what builds nothing passes the lock, waiters get it in arrival order, and
# a long hold is reported, and what it does is recorded for the dev job dashboard. Run by check.sh
# when tools/dev/bin or this file changes.
zmodload zsh/system; zmodload -F zsh/stat b:zstat
sh=${0:A:h:h}/bin/cargo
# (A kept folder, never removed here: the toy's programs run from it, tools/dev/ran.zsh, #1131.)
source ${0:A:h:h}/ran.zsh
s=$(ran_dir shared-target)
w=$s/.claude/worktrees; mkdir -p $w
# The shim's records (tools/dev/devjob.zsh) go to a scratch spool, sent nowhere (a closed port).
export SCENIC_DEVJOB_DIR=$s/devjobs SCENIC_BOARD=http://127.0.0.1:9; unset SCENIC_DEVJOB SCENIC_DEVJOB_PARENT
# (The toy cargos' waits are this test's, not its caller's: check.sh's terminal doesn't get them.)
unset SCENIC_CARGO_SAY
# (These toys run tests as an agent worktree does, so this Mac is the test Mac for them: the shim refuses a
# worktree's test run elsewhere, which "Tests refused off the test Mac" below checks.)
source ${0:A:h:h:h}/app/test-mac.zsh
# (No PM's marker but this test's own, whatever this Mac's .claude/tests-local says.)
export SCENIC_THIS_MAC=$(test_mac_name) SCENIC_TESTS_LOCAL_FILE=$s/tests-local; unset SCENIC_TESTS_LOCAL SCENIC_CHECK_LOCAL
dj=$SCENIC_DEVJOB_DIR
quiet() {  # the sampler gone (it ends once nothing is live and every report is read)
  local i f; for (( i = 0; i < 600; i++ )); do
    zsystem flock -t 0 -f f $dj/sampler.lock 2>/dev/null && { zsystem flock -u $f; return 0 }; sleep 0.1
  done
  fail "the sampler never ended"
}
# The repo's nextest (its mise.toml's line), which the shim's `mise env` finds for the toy.
grep -E '^"[^"]*cargo-nextest" *=' ${0:A:h:h:h:h}/mise.toml | sed '1s/^/[tools]\n/' > $s/mise.toml
export MISE_TRUSTED_CONFIG_PATHS=$s
for x in a b; do
  mkdir -p $w/$x/src/bin $w/$x/tests; : > $w/$x/.git
  printf '[package]\nname = "toy"\nversion = "0.1.0"\nedition = "2021"\ndefault-run = "toy"\n[workspace]\n' > $w/$x/Cargo.toml
  # The program: its worktree's name, then what each argument asks (after a hold, see lib.rs).
  cat > $w/$x/src/main.rs <<EOF
fn main() {
    toy::hold();
    println!("$x");
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "exe" => println!("exe {}", std::env::current_exe().unwrap().display()),
            "helper" => println!("helper {}", std::env::current_exe().unwrap().with_file_name("helper").exists()),
            "shared" => println!("shared {:?}", std::env::var("SCENIC_CARGO_SHARED").ok()),
            "cwd" => println!("cwd {}", std::env::current_dir().unwrap().display()),
            x => println!("arg {x:?}"),
        }
    }
}
EOF
  print 'fn main() { println!("helper"); }' > $w/$x/src/bin/helper.rs
  # TOY_HOLD=<file>: says it started (<file>.started) and waits for <file>.
  cat > $w/$x/src/lib.rs <<'EOF'
/// ```
/// assert_eq!(toy::two(), 2);
/// ```
pub fn two() -> u32 {
    2
}
pub fn hold() {
    if let Ok(p) = std::env::var("TOY_HOLD") {
        std::fs::write(format!("{p}.started"), "").unwrap();
        let t = std::time::Instant::now();
        while !std::path::Path::new(&p).exists() {
            assert!(t.elapsed().as_secs() < 120, "never let go");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn env() {
        super::hold();
        assert!(std::path::Path::new("Cargo.toml").exists(), "not in the package's folder");
        assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml").exists());
        assert_eq!(std::env::var("CARGO_PKG_NAME").unwrap(), "toy");
        let exe = std::env::current_exe().unwrap();
        assert!(exe.to_string_lossy().contains("/.scenic-runs/"), "not a clone: {}", exe.display());
        // (In a folder named `deps`, as store::spawn::in_test_program reads it.)
        assert_eq!(exe.parent().and_then(|d| d.file_name()).unwrap(), "deps", "{}", exe.display());
        // (Outside the lock: a cargo it started would take it.)
        assert!(std::env::var("SCENIC_CARGO_SHARED").is_err(), "SCENIC_CARGO_SHARED");
        // (Nor under a hold: a cargo it started would take it, #1123.)
        assert!(std::env::var("SCENIC_CARGO_HOLD").is_err(), "SCENIC_CARGO_HOLD");
    }
    #[test]
    fn fails_when_asked() {
        assert!(std::env::var("TOY_FAIL").is_err());
    }
}
EOF
  # cli starts the program as the test runs (TOY_HOLD's hold is its own: it says it started, after
  # the lock went): from a clone, outside the lock, as every test binary (#1108).
  cat > $w/$x/tests/cli.rs <<'EOF'
#[test]
fn starts_the_program() {
    toy::hold();
    assert!(std::env::var("SCENIC_CARGO_SHARED").is_err(), "under the lock");
    let p = std::env::var("CARGO_BIN_EXE_toy").expect("CARGO_BIN_EXE_toy");
    assert!(p.contains("/.scenic-runs/"), "not a clone: {p}");
    let o = std::process::Command::new(&p).output().unwrap();
    assert!(o.status.success() && !o.stdout.is_empty());
}
EOF
  print '#[test]\nfn plain() {}' > $w/$x/tests/plain.rs
  # Cargo sets CARGO_BIN_EXE_* as it runs a test too: they name clones (a name with a - included,
  # which zsh would drop from its environment).
  print 'fn main() { println!("two"); }' > $w/$x/src/bin/two-words.rs
  cat > $w/$x/tests/binexe.rs <<'EOF2'
#[test]
fn names_a_clone() {
    let p = std::env::var("CARGO_BIN_EXE_two-words").expect("CARGO_BIN_EXE_two-words");
    assert!(p.contains("/.scenic-runs/"), "not a clone: {p}");
    assert_eq!(std::process::Command::new(&p).output().unwrap().stdout, b"two\n");
}
EOF2
done
touch -t 202601010000 $w/b/src/main.rs $w/b/Cargo.toml
export SCENIC_SHARED_TARGET=$s/shared
fail() { print -u2 "FAIL: $*"; exit 1; }
at() { local x=$1; shift; (cd $w/$x && $sh "$@" 2>$s/err) }
run() { at $1 run -q }
free() { zsystem flock -t 0 -f f $s/shared/.scenic-lock 2>/dev/null && zsystem flock -u $f }
freed() { local i; for (( i = 0; i < 1200; i++ )); do free && return 0; sleep 0.1; done; return 1 }
started() {  # started <file> <log>: waits for TOY_HOLD's <file>.started while $held runs (2 min at most)
  local i; for (( i = 0; i < 1200; i++ )); do
    [[ -e $1.started ]] && return 0; kill -0 $held 2>/dev/null || break; sleep 0.1
  done
  fail "never started: ${1:t} ($(cat $2))"
}
[[ $(run a) == a ]] || fail "a's first build: $(cat $s/err)"
[[ $(run b) == b ]] || fail "b after a ran a's build"
[[ $(run a) == a ]] || fail "a after b ran b's build"
[[ $(cat $s/shared/.scenic-built-by) == "checksums $w/a" ]] || fail "built-by not recorded"
# The lock: a holder that sleeps 3 s; the second cargo says it waits and then runs.
( zsystem flock -f fd $s/shared/.scenic-lock; print "test holder" > $s/shared/.scenic-holder; sleep 3 ) &
sleep 0.5
out=$(run b) || fail "b behind the lock: $(cat $s/err)"
[[ $out == b ]] || fail "b behind the lock ran $out"
grep -q 'waiting for the shared build folder .*held by: test holder' $s/err || fail "no waiting line: $(cat $s/err)"
wait

# cargo run: the program runs outside the lock, from a clone with the programs beside it, in the
# caller's folder, with its arguments as given; another worktree builds meanwhile without waiting.
mkdir -p $w/a/src/sub
at a build -q --bins || fail "a's programs: $(cat $s/err)"
(cd $w/a/src/sub && TOY_HOLD=$s/go1 $sh run -q -- exe helper shared cwd 'x y' '' > $s/run.out 2>$s/run.err) & held=$!
started $s/go1 $s/run.err
free || fail "the lock is held while cargo run's program runs"
at b build -q || fail "b's build beside a's program: $(cat $s/err)"
grep -q waiting $s/err && fail "b waited for a's program: $(cat $s/err)"
: > $s/go1; wait $held || fail "a's program: $(cat $s/run.err)"
out=( ${(f)"$(<$s/run.out)"} )
[[ $out[1] == a ]] || fail "a's program after b's build ran ${out[1]} (b's build replaced it)"
[[ $out[2] == "exe $s/shared/.scenic-runs/"<->/bin/debug/toy ]] || fail "not run from a clone: $out[2]"
[[ $out[3] == 'helper true' ]] || fail "the programs beside it weren't cloned: $out[3]"
[[ $out[4] == 'shared None' ]] || fail "SCENIC_CARGO_SHARED outside the lock: $out[4]"
[[ $out[5] == "cwd ${w:A}/a/src/sub" ]] || fail "not in the caller's folder: $out[5]"
[[ $out[6] == 'arg "x y"' && $out[7] == 'arg ""' ]] || fail "arguments: $out[6,7]"
# (Its clones stay once it ends, #1131: the next run marks them seen gone, and a run removes them
# once marked 10 minutes, tools/dev/ran.zsh's ran_clean, as a gone PID's set-aside folder.)
d=${${out[2]#exe }:h:h:h}; [[ -e $d/bin/debug/toy ]] || fail "the run's clones went as it ended: $d"
old=$(date -v-11M +%Y%m%d%H%M.%S)
mkdir -p $s/shared/.scenic-runs/99999999.1 $s/shared/.scenic-runs/99999999.2
touch $s/shared/.scenic-runs/99999999.1/.scenic-gone; touch -t $old $s/shared/.scenic-runs/99999999.1/.scenic-gone
touch -t $old $d $s/shared/.scenic-runs/99999999.2
[[ $(cd $w/a && $sh run -q -- 'exit' 2>/dev/null) == a$'\n''arg "exit"' ]] || fail "a second run"
[[ -e $d/bin/debug/toy && -e $d/.scenic-gone ]] || fail "a gone run's clones, 11 min old, not kept and marked at the next run"
[[ ! -e $s/shared/.scenic-runs/99999999.1 ]] || fail "a set-aside folder seen gone 11 min ago stayed"
[[ -e $s/shared/.scenic-runs/99999999.2/.scenic-gone ]] || fail "a set-aside folder first seen gone went, or wasn't marked"
touch -t $old $d/.scenic-gone
[[ $(cd $w/a && $sh run -q -- 'exit' 2>/dev/null) == a$'\n''arg "exit"' ]] || fail "a third run"
[[ ! -e $d ]] || fail "a gone run's clones, seen gone 11 min ago, stayed"

# cargo test: the tests outside the lock (env checks the folder, the environment and the clone;
# cli starts the program's clone), the doctest by rustdoc; another build meanwhile.
(cd $w/a && TOY_HOLD=$s/go2 $sh test -q --lib --test cli > $s/test.out 2>$s/test.err) & held=$!
started $s/go2 $s/test.err
free || fail "the lock is held while the tests run"
: > $s/go2; wait $held || fail "a's lib and cli tests: $(cat $s/test.out $s/test.err)"
at a test || fail "a's tests: $(cat $s/err)"
grep -q 'Running debug/deps/cli-[0-9a-f]*$' $s/err || fail "cli not run outside the lock: $(cat $s/err)"
grep -q 'Running debug/deps/plain-[0-9a-f]*$' $s/err || fail "plain not run outside the lock: $(cat $s/err)"
grep -q 'Running debug/deps/binexe-[0-9a-f]*$' $s/err || fail "binexe not run outside the lock: $(cat $s/err)"
grep -q 'Doc-tests toy' $s/err || fail "no doctests: $(cat $s/err)"
# A failure: stops there with the test's status, unless --no-fail-fast.
TOY_FAIL=1 at a test --lib --test plain; rc=$?
(( rc == 101 )) || fail "a failing test exited $rc: $(cat $s/err)"
grep -q 'error: test failed: debug/deps/toy-' $s/err || fail "no failure line: $(cat $s/err)"
grep -q 'Running debug/deps/plain-' $s/err && fail "ran on after a failure: $(cat $s/err)"
TOY_FAIL=1 at a test --no-fail-fast --lib --test plain; rc=$?
(( rc == 101 )) && grep -q 'Running debug/deps/plain-' $s/err || fail "--no-fail-fast: $rc, $(cat $s/err)"

# A TERM or INT to the shim reaches the program it runs, and the shim outlives it (it held the
# lock). (The shim as a terminal starts it: INT not ignored, as this script's & would.)
for sig in TERM INT; do
  rm -f $s/go5*(N)
  (trap - INT; cd $w/a && TOY_HOLD=$s/go5 exec $sh test -q --lib --test plain > $s/term.out 2>&1) & held=$!
  started $s/go5 $s/term.out
  p=$(pgrep -f "/.scenic-runs/$held/bin/deps/toy-") || fail "no test program of $held"
  kill -$sig $held; wait $held; rc=$?
  (( rc == 128 + $signals[(Ie)$sig] - 1 )) || fail "a $sig'd shim exited $rc: $(cat $s/term.out)"
  kill -0 $p 2>/dev/null && { kill $p; fail "the test program outlived the shim ($sig)"; }
  grep -q 'Running debug/deps/plain-' $s/term.out && fail "ran on after a $sig: $(cat $s/term.out)"
done

# cargo nextest run: the clones run once nextest starts testing, the lock let go then, whatever
# the binaries (cli starts the program through CARGO_BIN_EXE_toy: #1108's run held it to the end).
(cd $w/a && TOY_HOLD=$s/go3 $sh nextest run > $s/nx.out 2>&1) & held=$!
started $s/go3 $s/nx.out
freed || fail "the lock is held while nextest's tests run"  # (the shim looks every 0.2 s)
: > $s/go3; wait $held || fail "nextest: $(cat $s/nx.out)"

# This shim and another checkout's both on the PATH (check.sh puts its own before the main checkout's):
# neither is taken for the real cargo, so a command passed straight through (version) and the
# shim's own metadata (the last build another worktree's) end (4 Nov: they exec'd each other for
# ever; #941: a metadata under the lock waited on it).
mkdir -p $s/main/tools/dev/bin; cp $sh $s/main/tools/dev/bin/cargo
tree() { local c; for c in $(pgrep -P $1); do tree $c; done; print $1 }  # tree <pid>: it and its descendants
within() {  # within <secs> <command>…: its status, or fails (its processes killed) when it outlasts <secs>
  local secs=$1; shift; "$@" & local p=$! i
  for (( i = 0; i < secs * 10; i++ )); do kill -0 $p 2>/dev/null || { wait $p; return }; sleep 0.1; done
  local t; for (( i = 0; i < 50; i++ )); do t=( $(tree $p) ); kill $t 2>/dev/null || break; sleep 0.1; done
  fail "still running after $secs s: $*"
}
print other > $s/shared/.scenic-built-by
both=${sh:h}:$s/main/tools/dev/bin:$PATH
PATH=$both within 60 at a version || fail "version with two shims: $(cat $s/err)"
PATH=$both within 120 at a build -q || fail "build with two shims: $(cat $s/err)"
[[ $(cat $s/shared/.scenic-built-by) == "checksums $w/a" ]] || fail "two shims: built-by not recorded"
# A mise shim ahead of the real cargo, outside any checkout: mise has no cargo there, so its shim
# runs the first other cargo on the PATH, the shim (#1108 review: they ran each other ~2,900
# times). The stand-in mise does as mise does: `which` fails, and as cargo it runs the PATH's next.
mkdir -p $s/mise/shims $s/nowhere
cat > $s/mise/mise <<'EOF'
#!/bin/zsh
[[ $1 == which ]] && exit 1
for c in $(whence -ap cargo); do [[ ${c:h} == ${0:h} ]] || exec $c "$@"; done
exit 127
EOF
chmod +x $s/mise/mise; ln -s ../mise $s/mise/shims/cargo
PATH=${sh:h}:$s/mise/shims:$PATH within 60 zsh -fc "cd $s/nowhere && $sh --version" > $s/out 2>$s/err || fail "a mise shim outside a checkout: $(cat $s/err)"
grep -q '^cargo ' $s/out || fail "a mise shim outside a checkout: not the real cargo: $(cat $s/out $s/err)"

# #941: what builds nothing passes a held lock; the rest wait in arrival order, a gone waiter's
# ticket holds no one up, and a long hold is reported with what its holder is doing.
# Every built-in of cargo --list is classed (a cargo-* on the PATH is a plug-in: unknown is locked).
eval "$(grep -E '^(builds|reads)=' $sh)"
for c in ${${(f)"$(cargo --list)"}[2,-1]}; do
  c=${${=c}[1]}; whence -p cargo-$c >/dev/null && continue
  (( !$builds[(Ie)$c] != !$reads[(Ie)$c] )) || fail "cargo $c is in neither or both of the shim's builds and reads"
done
q=$s/shared/.scenic-queue
hold() {  # hold <file>: holds the build lock as "cargo" pid <its pid>, until <file> exists
  rm -f $1 $1.held
  ( zsystem flock -f fd $s/shared/.scenic-lock
    print -r -- "test pid $sysparams[pid] since now: cargo build" > $s/shared/.scenic-holder; : > $1.held
    while [[ ! -e $1 && -d $s ]]; do sleep 0.1; done ) & holding=$!  # (and ends with the test)
  until [[ -e $1.held ]]; do sleep 0.05; done
}
until_in() {  # until_in <file> <pattern>: waits (1 min at most) for a line of <file> to match
  local i; for (( i = 0; i < 600; i++ )); do grep -qE -- $2 $1 2>/dev/null && return 0; sleep 0.1; done
  fail "never said /$2/: $(cat $1)"
}
hold $s/let1
for c in 'fmt --check' '--color never metadata --no-deps --format-version 1' 'build --help'; do
  within 60 at a ${=c} >/dev/null; (( $? <= 1 )) || fail "cargo $c behind a held lock: $(cat $s/err)"
  grep -q waiting $s/err && fail "cargo $c waited for the lock: $(cat $s/err)"
done
# The order: a cargo that writes its name (the shim's "real" cargo; the folder already a's).
# (It answers the shim's question for the configured rustc wrapper, `config get`, with none.)
mkdir -p $s/fake; print -rl -- '#!/bin/sh' 'case " $* " in *" config "*) exit 101 ;; esac' 'echo "$TOY_NAME" >> "$TOY_ORDER"' > $s/fake/cargo; chmod +x $s/fake/cargo
export TOY_ORDER=$s/order; rm -f $TOY_ORDER
waiter() {  # waiter <name>: a build behind the lock, its pid in $waiting, its stderr in $s/<name>.err
  # (zsh -f: a ~/.zshenv putting rustup's cargo first on the PATH, as the M1's does, hid the fake.)
  (cd $w/a && TOY_NAME=$1 PATH=$s/fake:$PATH exec zsh -f $sh build 2>$s/$1.err) & waiting=$!
}
pids=()
for x in 1 2 3; do waiter $x; pids+=( $waiting ); until_in $s/$x.err "place $x in line; held by: test pid $holding "; done
: > $s/let1; wait $holding
for x in 1 2 3; do wait $pids[x] || fail "waiter $x: $(cat $s/$x.err)"; done
[[ $(<$TOY_ORDER) == $'1\n2\n3' ]] || fail "not in arrival order: $(<$TOY_ORDER)"
grep -q 'got the shared build folder after' $s/3.err || fail "no got line: $(cat $s/3.err)"
# A waiter killed (no cleanup) leaves its ticket; the next is first in line, and gets the lock.
hold $s/let2; rm -f $TOY_ORDER
waiter dead; dead=$waiting; until_in $s/dead.err 'place 1 in line'
kill -KILL $dead; wait $dead
gone=( $q/<->(N) ); (( $#gone == 1 )) || fail "no ticket left by the killed waiter: $(ls -a $q)"
waiter next; until_in $s/next.err 'place 1 in line'
[[ ! -e $gone[1] ]] || fail "the killed waiter's ticket is still there"
: > $s/let2; wait $holding; wait $waiting || fail "the waiter after a killed one: $(cat $s/next.err)"
[[ $(<$TOY_ORDER) == next ]] || fail "after a killed waiter: $(<$TOY_ORDER)"
# The report: holds of 0 s recorded, so the first waiting line names the holder's pid, its
# processes and their CPU.
print '0 build' > $s/shared/.scenic-holds
hold $s/let3
waiter report; until_in $s/report.err 'using (no )?CPU'
grep -q "the folder has been held [0-9]* s (9 holds in 10 of the last 1 took ≤ 0 s); its holder, pid $holding:\$" $s/report.err || fail "no report line: $(cat $s/report.err)"
grep -qE "^ +$holding [0-9.]+% cpu, .*cpu time: " $s/report.err || fail "the holder's process not shown: $(cat $s/report.err)"
: > $s/let3; wait $holding; wait $waiting || fail "the reported waiter: $(cat $s/report.err)"
(( $(wc -l < $s/shared/.scenic-holds) == 2 )) || fail "the hold not recorded: $(cat $s/shared/.scenic-holds)"
# Tickets are numbered past the last one, whatever .next says (emptied: a cargo killed as it wrote
# it). A whole cargo clean empties the folder but keeps the lock and queue: a cargo waiting behind
# it still gets the lock, the same file (cargo's own clean deleted the folder, the lock with it).
hold $s/let4; rm -f $TOY_ORDER; : > $q/.next; : > $q/40
zstat -A ino +inode $s/shared/.scenic-lock
(cd $w/a && exec $sh clean 2>$s/clean.err) & cleaning=$!
until_in $s/clean.err 'place 1 in line'
[[ -e $q/41 ]] || fail "not numbered past the last ticket: $(ls -a $q)"
waiter after; until_in $s/after.err 'place 2 in line'
: > $s/let4; wait $holding
wait $cleaning || fail "the clean: $(cat $s/clean.err)"
wait $waiting || fail "the build after a clean: $(cat $s/after.err)"
[[ $(<$TOY_ORDER) == after ]] || fail "after a clean: $(<$TOY_ORDER)"
[[ ! -e $s/shared/debug ]] || fail "the clean left debug/: $(ls -a $s/shared)"
zstat -A ino2 +inode $s/shared/.scenic-lock && (( ino2[1] == ino[1] )) || fail "the clean replaced the lock"
[[ -e $s/shared/.scenic-holds ]] || fail "the clean deleted the holds"
unset TOY_ORDER

# #1123: a hold (check.sh's clippy, then its tests' build): taken in line as any cargo, its line
# naming the process it holds for and itself; a cargo under it (SCENIC_CARGO_HOLD) builds without
# waiting while another worktree's waits; a nextest run under it lets it go as its first test
# starts, and the waiter gets the folder then; a cargo not under it (a stale SCENIC_CARGO_HOLD) waits.
rm -f $s/ready
(cd $w/a && exec $sh --scenic-hold $s/ready 2>$s/hold.err) & hp=$!
for (( i = 0; i < 600; i++ )); do [[ -e $s/ready ]] && break; sleep 0.1; done
[[ -e $s/ready ]] || fail "the hold never held: $(cat $s/hold.err)"
grep -q "^a pid $$ since .*: check.sh's clippy, then its tests' build (hold $hp)\$" $s/shared/.scenic-holder || fail "the hold's line: $(cat $s/shared/.scenic-holder)"
(cd $w/b && exec $sh build -q 2>$s/hw.err) & wp=$!
until_in $s/hw.err "place 1 in line; held by: a pid $$ .*\(hold $hp\)"
SCENIC_CARGO_HOLD=$hp within 120 at a build -q --tests || fail "a build under the hold: $(cat $s/err)"
grep -q waiting $s/err && fail "a build under the hold waited: $(cat $s/err)"
SCENIC_CARGO_HOLD=1 within 60 at a version >/dev/null || fail "version"
(cd $w/a && SCENIC_CARGO_HOLD=$hp TOY_HOLD=$s/go6 exec $sh nextest run > $s/nx6.out 2>&1) & held=$!
started $s/go6 $s/nx6.out
for (( i = 0; i < 600; i++ )); do kill -0 $hp 2>/dev/null || break; sleep 0.1; done
kill -0 $hp 2>/dev/null && fail "the hold outlived the first test"
wait $wp || fail "the waiter behind the hold: $(cat $s/hw.err)"
: > $s/go6; wait $held || fail "nextest under the hold: $(cat $s/nx6.out)"
[[ ! -e $s/shared/.scenic-holder ]] || fail "a holder's line left: $(cat $s/shared/.scenic-holder)"

# Outside .claude/worktrees: plain cargo, the shared folder untouched.
mkdir -p $s/plain/src; : > $s/plain/.git; cp $w/a/Cargo.toml $s/plain/; cp -R $w/a/src $s/plain/
(cd $s/plain && $sh build -q) || fail "plain build"
[[ -d $s/plain/target ]] || fail "outside worktrees: not its own target/"
quiet
grep -h '"op":"start"' $dj/s*.ndjson(N) | grep -q '"checkout":"plain"' || fail "outside worktrees: the build not recorded"
[[ -z $(print $s/plain/target/cargo-timings/cargo-timing-*.html(N)) ]] || fail "outside worktrees: the timing report left"
# The dev job dashboard's records of a cargo test: the cargo, its build (the toy's crates, from
# cargo's timing report, which the sampler reads), each test binary's run as a suite, how it ended;
# a failing one's, failed.
quiet; rm -rf $dj
touch $w/a/src/lib.rs
at a test -q --lib || fail "a recorded test: $(cat $s/err)"
TOY_FAIL=1 at a test -q --lib; (( $? == 101 )) || fail "a recorded failing test: $(cat $s/err)"
quiet
python3 - $dj <<'PY' || fail "the shim's records"
import glob, json, os, sys
ev = [json.loads(l) for f in sorted(glob.glob(os.path.join(sys.argv[1], "s*.ndjson"))) for l in open(f)]
starts = [e for e in ev if e["op"] == "start" and e["kind"] == "cargo"]
assert [e["name"] for e in starts] == ["cargo test -q --lib"] * 2, [e["name"] for e in starts]
def under(p, kind): return [e for e in ev if e["op"] == "span" and e.get("parent") == p and e["kind"] == kind]
end = {e["id"]: e for e in ev if e["op"] == "end"}
ok, bad = starts[0]["id"], starts[1]["id"]
b = under(ok, "build")
assert len(b) == 1 and b[0]["status"] == "ok", b
crates = under(b[0]["id"], "crate")
assert any(c["name"].startswith("toy") and c["t1"] > c["t0"] for c in crates), ("no toy crate from the timing report", crates)
assert [(x["name"], x["status"]) for x in under(ok, "suite")] == [("toy", "ok")], under(ok, "suite")
assert (end[ok]["status"], end[ok]["attrs"]["exit"]) == ("ok", "0"), end[ok]
assert [(x["name"], x["status"]) for x in under(bad, "suite")] == [("toy", "fail")], under(bad, "suite")
assert (end[bad]["status"], end[bad]["attrs"]["exit"]) == ("fail", "101"), end[bad]
PY
[[ -z $(print $s/shared/cargo-timings/cargo-timing-*.html(N)) ]] || fail "timing reports left in the shared folder: $(ls $s/shared/cargo-timings)"

# #1123: a crate whose sources are the bytes another worktree built is fresh, whatever its files'
# times (c and d are two checkouts of one workspace, d's files newer than c's build); a crate
# compiling in a file outside its package (cargo checks it by c's full path) compiles again where
# that file differs, and one built with env!("CARGO_MANIFEST_DIR") compiles again in each checkout;
# a build script watching a file (rerun-if-changed, judged by time) runs again in each checkout,
# though d's file, its bytes not c's, is dated before c's build (#1123 review: it had passed for
# c's); within a checkout, a source edited to the same size and dated back compiles again.
for x in c d; do
  mkdir -p $w/$x/crates/{plain,inc,dir,gen,dlib,user}/src $w/$x/shared; : > $w/$x/.git
  printf '[workspace]\nmembers = ["crates/plain", "crates/inc", "crates/dir", "crates/gen", "crates/dlib", "crates/user"]\nresolver = "2"\n' > $w/$x/Cargo.toml
  for k in plain inc dir gen dlib user; do
    printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n' $k > $w/$x/crates/$k/Cargo.toml
  done
  print 'pub fn two() -> u32 { 2 }' > $w/$x/crates/plain/src/lib.rs
  print 'fn main() { print!("{}", include_str!("../../../shared/note.txt")); }' > $w/$x/crates/inc/src/main.rs
  print 'fn main() { print!("{}", env!("CARGO_MANIFEST_DIR")); }' > $w/$x/crates/dir/src/main.rs
  print same > $w/$x/shared/note.txt
  cat > $w/$x/crates/gen/build.rs <<'EOF'
fn main() {
    println!("cargo::rerun-if-changed=data.txt");
    let d = std::fs::read_to_string("data.txt").unwrap();
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("d.txt"), d.trim()).unwrap();
}
EOF
  print 'fn main() { print!("{}", include_str!(concat!(env!("OUT_DIR"), "/d.txt"))); }' > $w/$x/crates/gen/src/main.rs
  print $x-data > $w/$x/crates/gen/data.txt
  # (A crate using a tied one compiles again with it: user inlines dlib's constant.)
  print 'pub const D: &str = env!("CARGO_MANIFEST_DIR");' > $w/$x/crates/dlib/src/lib.rs
  print 'dlib = { path = "../dlib" }' | sed '1s/^/[dependencies]\n/' >> $w/$x/crates/user/Cargo.toml
  print 'fn main() { print!("{}", dlib::D); }' > $w/$x/crates/user/src/main.rs
done
touch -t 202001010000 $w/d/crates/gen/data.txt
built() {  # built <checkout> <cargo build args>…: the packages it compiled (not fresh), sorted
  (cd $w/$1 && $sh build --message-format json "${@:2}" 2>$s/err) | python3 -c '
import json, sys
print(" ".join(sorted({json.loads(l)["package_id"].split("#")[0].rsplit("/", 1)[-1] for l in sys.stdin
    if l.startswith("{") and json.loads(l).get("reason") == "compiler-artifact" and not json.loads(l)["fresh"]})))'
}
[[ $(built c) == "dir dlib gen inc plain user" ]] || fail "c's first build: $(cat $s/err)"
touch $w/d/crates/*/src/*.rs(N) $w/d/shared/note.txt
b=$(built d); [[ $b == "dir dlib gen user" ]] || fail "d, the same sources as c's but gen's data, compiled other than dir and dlib (env!), user (dlib's) and gen: $b; $(cat $s/err; cat $s/shared/.scenic-tied)"
grep -q 'dir-.*CARGO_MANIFEST_DIR names another checkout' $s/shared/.scenic-tied || fail "no tied dir: $(cat $s/shared/.scenic-tied)"
grep -q 'gen-.*rerun-if-changed' $s/shared/.scenic-tied || fail "no tied gen: $(cat $s/shared/.scenic-tied)"
[[ $(cd $w/d && $sh run -q -p gen 2>$s/err) == d-data ]] || fail "d's gen ran with c's data.txt: $(cat $s/err)"
[[ $(cd $w/d && $sh run -q -p user 2>$s/err) == ${w:A}/d/crates/dlib ]] || fail "d's user ran with another checkout's dlib: $(cat $s/err)"
print 'pub fn two() -> u32 { 3 }' > $w/d/crates/plain/src/lib.rs; touch -t 202001010000 $w/d/crates/plain/src/lib.rs
[[ $(built d) == plain ]] || fail "d's plain, edited to the same size and dated back, not compiled again: $(cat $s/err)"
[[ $(cd $w/d && $sh run -q -p dir 2>$s/err) == ${w:A}/d/crates/dir ]] || fail "d's dir ran with another checkout's path: $(cat $s/err)"
print other > $w/d/shared/note.txt
[[ $(built d) == inc ]] || fail "d's note.txt changed: not inc alone: $(built d)"
[[ $(cd $w/d && $sh run -q -p inc 2>$s/err) == other ]] || fail "d's inc ran with c's note.txt: $(cat $s/err)"
[[ $(built c) == "dir dlib gen inc plain user" ]] || fail "back in c: not dir, dlib, gen, inc, plain and user again: $(built c)"
[[ $(cd $w/c && $sh run -q -p gen 2>$s/err) == c-data ]] || fail "c's gen ran with d's data.txt: $(cat $s/err)"
[[ $(cd $w/c && $sh run -q -p inc 2>$s/err) == same ]] || fail "c's inc ran with d's note.txt: $(cat $s/err)"
# Tests refused off the test Mac (#1398): a test or nextest run from a worktree on another Mac runs nothing and
# says where tests run; SCENIC_TESTS_LOCAL (or SCENIC_CHECK_LOCAL), the PM's marker or the test Mac itself lets it
# through, and what isn't a test run (a build, a test's --no-run) is never refused.
for sub in "test -q --lib" "t -q --lib" "nextest run"; do
  out=$(cd $w/a && SCENIC_THIS_MAC=another-mac $sh ${=sub} 2>&1); rc=$?
  (( rc == 2 )) && [[ $out == *"tests run on the test Mac"*"tools/dev/check.sh"*"remote-tests.sh"* ]] || fail "$sub off the test Mac: exit $rc: $out"
done
for e in SCENIC_TESTS_LOCAL SCENIC_CHECK_LOCAL; do
  (cd $w/a && env SCENIC_THIS_MAC=another-mac $e=1 $sh test -q --lib >/dev/null 2>&1) || fail "$e=1: test run refused"
done
: > $SCENIC_TESTS_LOCAL_FILE
(cd $w/a && SCENIC_THIS_MAC=another-mac $sh test -q --lib >/dev/null 2>&1) || fail "the PM's marker: test run refused"
rm $SCENIC_TESTS_LOCAL_FILE
(cd $w/a && SCENIC_THIS_MAC=another-mac $sh build -q 2>&1) || fail "a build off the test Mac refused"
(cd $w/a && SCENIC_THIS_MAC=another-mac $sh test -q --lib --no-run 2>&1) || fail "a test's --no-run off the test Mac refused"

print "shared-target: ok"
