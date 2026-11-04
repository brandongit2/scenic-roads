#!/bin/zsh
# tools/dev/bin/cargo (#915, #937) on a toy workspace: two worktrees build into one shared folder,
# each runs its own code (worktree b's sources older than a's build), a second cargo waits for the
# first, saying so, and programs and tests run outside the lock, from clones, with cargo's
# environment and folder. Run by check.sh when tools/dev/bin or this file changes.
zmodload zsh/system
sh=${0:A:h:h}/bin/cargo
s=$(mktemp -d); trap 'rm -rf $s' EXIT
w=$s/.claude/worktrees; mkdir -p $w
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
        // (TOY_LOCKED: run while the lock is held, where a cargo it started must pass through.)
        let locked = std::env::var("TOY_LOCKED").is_ok();
        assert_eq!(std::env::var("SCENIC_CARGO_SHARED").is_ok(), locked, "SCENIC_CARGO_SHARED");
    }
    #[test]
    fn fails_when_asked() {
        assert!(std::env::var("TOY_FAIL").is_err());
    }
}
EOF
  # CARGO_BIN_EXE_toy names the shared folder's program: this binary runs under the lock.
  cat > $w/$x/tests/cli.rs <<'EOF'
#[test]
fn starts_the_program() {
    assert_eq!(std::env::var("SCENIC_CARGO_SHARED").as_deref(), Ok("1"), "not under the lock");
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_toy")).output().unwrap();
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
[[ $(cat $s/shared/.scenic-built-by) == $w/a ]] || fail "built-by not recorded"
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
# (The program replaced the shim, so its clones stay until the next run sweeps them.)
d=${${out[2]#exe }:h:h:h}; [[ -d $d ]] || fail "no clones folder $d"
[[ $(cd $w/a && $sh run -q -- 'exit' 2>/dev/null) == a$'\n''arg "exit"' ]] || fail "a second run"
[[ ! -e $d ]] || fail "the first run's clones were not swept: $d"

# cargo test: the lib's tests outside the lock (env checks the folder, the environment and the
# clone), cli under it (CARGO_BIN_EXE_toy), the doctest by rustdoc; another build meanwhile.
(cd $w/a && TOY_HOLD=$s/go2 $sh test -q --lib > $s/test.out 2>$s/test.err) & held=$!
started $s/go2 $s/test.err
free || fail "the lock is held while the tests run"
: > $s/go2; wait $held || fail "a's lib tests: $(cat $s/test.out $s/test.err)"
at a test || fail "a's tests: $(cat $s/err)"
grep -q 'Running debug/deps/cli-[0-9a-f]* (under the build lock' $s/err || fail "cli not under the lock: $(cat $s/err)"
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

# A TERM to the shim reaches the program it runs, and the shim outlives it (it held the lock).
(cd $w/a && TOY_HOLD=$s/go5 exec $sh test -q --lib --test plain > $s/term.out 2>&1) & held=$!
started $s/go5 $s/term.out
p=$(pgrep -f "/.scenic-runs/$held/bin/deps/toy-") || fail "no test program of $held"
kill -TERM $held; wait $held; rc=$?
(( rc == 143 )) || fail "a TERM'd shim exited $rc: $(cat $s/term.out)"
kill -0 $p 2>/dev/null && { kill $p; fail "the test program outlived the shim"; }
grep -q 'Running debug/deps/plain-' $s/term.out && fail "ran on after a TERM: $(cat $s/term.out)"

# cargo nextest run: the clones run once nextest starts testing, the lock let go then (kept when a
# binary names the shared folder's programs).
(cd $w/a && TOY_HOLD=$s/go3 $sh nextest run --lib > $s/nx.out 2>&1) & held=$!
started $s/go3 $s/nx.out
freed || fail "the lock is held while nextest's tests run"  # (the shim looks every 0.2 s)
: > $s/go3; wait $held || fail "nextest: $(cat $s/nx.out)"
(cd $w/a && TOY_HOLD=$s/go4 TOY_LOCKED=1 $sh nextest run > $s/nx.out 2>&1) & held=$!
started $s/go4 $s/nx.out
sleep 1  # (absence: five of the shim's looks)
free && fail "the lock was let go with cli (CARGO_BIN_EXE_toy) in the run"
: > $s/go4; wait $held || fail "nextest with cli: $(cat $s/nx.out)"

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
[[ $(cat $s/shared/.scenic-built-by) == $w/a ]] || fail "two shims: built-by not recorded"

# Outside .claude/worktrees: plain cargo, the shared folder untouched.
mkdir -p $s/plain/src; : > $s/plain/.git; cp $w/a/Cargo.toml $s/plain/; cp -R $w/a/src $s/plain/
(cd $s/plain && $sh build -q) || fail "plain build"
[[ -d $s/plain/target ]] || fail "outside worktrees: not its own target/"
print "shared-target: ok"
