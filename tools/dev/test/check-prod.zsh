#!/bin/zsh
# tools/dev/check.sh's prod part (#1133) on a toy workspace, with the real cargo: an item that
# exists only under the testkit (a feature only dev-dependencies turn on), used in a library's or a
# program's production code, passes clippy --all-targets (the dev-dependencies turn the testkit on)
# and fails prod, as #1097's StopCarry passed check.sh and failed publish's release build. Gated
# as the item is, it passes; prod is skipped when no crate changed, and reused with clippy. The
# other parts are stubs (cargo but check and clippy, guards.sh, affected-tests.sh, check-reuse.py).
# Run by check.sh when it or this file changes.
set -u
here=${0:A:h}
fail=0
ok() { print "ok: $1" }
bad() { print "FAIL: $1"; fail=1 }
tmp=$(mktemp -d); trap 'rm -rf $tmp' EXIT
mkdir -p $tmp/r $tmp/home; cd $tmp/r
g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null 2>&1 }
w() { mkdir -p ${1:h}; print -r -- "$2" > $1 }
# (rustup's cargo, at the repo's toolchain; its own build folder, not the shared one.)
real=~/.cargo/bin/cargo
[[ -x $real ]] || { print "FAIL: no cargo at $real"; exit 1 }
cp $here/../../../rust-toolchain.toml .
export CARGO_TARGET_DIR=$tmp/target
unset SCENIC_CARGO_HOLD SCENIC_CARGO_SAY SCENIC_DEVJOB_PARENT
g init -q -b main .
mkdir -p tools/dev tools/app
cp $here/../check.sh $here/../devjob.zsh $here/../devjob.py tools/dev/
w tools/dev/affected-tests.sh '#!/bin/zsh'
# (check-reuse.py's verdict: $TOY_VERDICT's, none without it.)
w tools/dev/check-reuse.py 'import os; os.environ.get("TOY_VERDICT") and print(os.environ["TOY_VERDICT"])'
w tools/app/guards.sh '#!/bin/zsh'
w tools/dev/bin/cargo "#!/bin/zsh
[[ \$1 == (check|clippy) ]] && exec $real \"\$@\"
exit 0"
chmod +x tools/dev/affected-tests.sh tools/app/guards.sh tools/dev/bin/cargo
w Cargo.toml '[workspace]
resolver = "2"
members = ["crates/a", "crates/b"]'
# a: the testkit's item (as jobrt's STOP_AFTER), and `run` using it only where it exists.
w crates/a/Cargo.toml '[package]
name = "a"
version = "0.1.0"
edition = "2021"
[features]
testkit = []'
lib_ok='#[cfg(any(test, feature = "testkit"))]
pub fn stop_after() -> Option<usize> {
    Some(1)
}
pub fn run() -> Option<usize> {
    #[cfg(any(test, feature = "testkit"))]
    return stop_after();
    #[cfg(not(any(test, feature = "testkit")))]
    None
}'
w crates/a/src/lib.rs $lib_ok
# b: a program, whose tests turn a's testkit on (as pipeline's dev-dependencies do).
w crates/b/Cargo.toml '[package]
name = "b"
version = "0.1.0"
edition = "2021"
[dependencies]
a = { path = "../a" }
[dev-dependencies]
a = { path = "../a", features = ["testkit"] }'
main_ok='fn main() {
    println!("{:?}", a::run());
}
#[cfg(test)]
mod tests {
    #[test]
    fn stops() {
        assert_eq!(a::stop_after(), Some(1));
    }
}'
w crates/b/src/main.rs $main_ok
$real generate-lockfile -q || { print "FAIL: cargo generate-lockfile"; exit 1 }
w README.md 1
g add -A; g commit -m b0
g checkout -q -b br
passed=$tmp/r/.git/check-passed

# run <name> <rc: 0 or 1> <texts the output has>…
run() {
  local name=$1 want=$2 out rc t; shift 2
  HOME=$tmp/home TOY_VERDICT=${verdict:-} SCENIC_DEVJOB=0 SCENIC_CHECK_LOCAL=1 zsh tools/dev/check.sh > $tmp/out 2>&1; rc=$?
  out=$(<$tmp/out)
  for t in "$@"; do
    [[ $out == *"$t"* ]] || { bad "$name: wanted \"$t\" (exit $rc):"; print -r -- $out | tail -30 | sed 's/^/    /'; return }
  done
  if (( (rc != 0) == want )); then ok "$name: ${(j:; :)@}"
  else bad "$name: wanted exit ${want/1/!= 0}, got $rc:"; print -r -- $out | tail -30 | sed 's/^/    /'; fi
}

w docs/a.md 1; g add -A; g commit -m docs
run "no crate changed" 0 "skip  prod  (no workspace crate changed)" "skip  clippy"
w crates/a/src/lib.rs '#[cfg(any(test, feature = "testkit"))]
pub fn stop_after() -> Option<usize> {
    Some(1)
}
pub fn run() -> Option<usize> {
    stop_after()
}'
# (b changed too, as #1097's commit changed StopCarry's users: clippy's -p a -p b turns a's testkit
# on through b's dev-dependency, and it passes.)
w crates/b/src/main.rs "$main_ok
// uses a::run"
g add -A; g commit -m lib
run "a library using a testkit-only item" 1 "PASS  clippy" "FAIL  prod" 'cannot find function `stop_after`'
w crates/a/src/lib.rs $lib_ok
w crates/b/src/main.rs 'fn main() {
    println!("{:?}", a::stop_after());
}'
g add -A; g commit -m bin
run "a program using one" 1 "PASS  clippy" "FAIL  prod" 'cannot find function `stop_after`'
w crates/b/src/main.rs $main_ok
w crates/a/src/lib.rs "$lib_ok
// gated"
g add -A; g commit -m gated
run "gated" 0 "PASS  clippy" "PASS  prod" "PASS: recorded"
# (Reused with clippy: check-reuse.py's verdict, here the stub's, for a clean checkout.)
P=$(git rev-parse HEAD)
w docs/a.md 2; g add -A; g commit -m reuse
verdict="clippy"$'\t'"reuse"$'\t'"$P"$'\t'"stub" run "clippy reused" 0 "REUSE clippy" "REUSE prod" "PASS: recorded"
[[ $(tail -1 $passed) == *" reused clippy=$P reused prod=$P" ]] && ok "the record names both reuses" ||
  bad "the record: $(tail -1 $passed)"
exit $fail
