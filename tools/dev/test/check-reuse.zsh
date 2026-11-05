#!/bin/zsh
# tools/dev/check-reuse.py's rule (#950) on a toy repo: a branch passed at P, main moves on, the
# branch is rebased; which of clippy and tests the pass at P still vouches for. And the lists it
# keeps: every top-level entry of this repo is a known kind, and the scripts running clippy and the
# tests run no repo script outside RUNNER. Run by check.sh.
set -u
here=${0:A:h}; reuse=$here/../check-reuse.py; repo=$(git -C $here rev-parse --show-toplevel)
fail=0
ok() { print "ok: $1" }
bad() { print "FAIL: $1"; fail=1 }

# -- the lists, against this repo
known=( ${(f)"$(python3 -c 'import importlib.util,sys; s=importlib.util.spec_from_file_location("r",sys.argv[1]); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print("\n".join(sorted(m.KNOWN))); print("\n".join("RUNNER " + r for r in m.RUNNER))' $reuse)"} )
runner=( ${${(M)known:#RUNNER *}#RUNNER } ); known=( ${known:#RUNNER *} )
for t in ${(u)${(f)"$(git -C $repo ls-files | cut -d/ -f1)"}}; do
  (( ${known[(Ie)$t]} )) || bad "top-level $t isn't in check-reuse.py's KNOWN: say whether the Rust build reads it (BUILD) or not"
done
for r in $runner; do [[ -e $repo/$r ]] || bad "RUNNER's $r doesn't exist"; done
# (The scripts' code lines: a repo script they name is one they run.)
for r in tools/dev/affected-tests.sh tools/dev/bin/cargo tools/app/remote-tests.sh tools/dev/check-reuse.py; do
  for s in ${(u)${(f)"$(grep -v '^\s*#' $repo/$r | grep -oE 'tools/[A-Za-z0-9_./-]+\.(sh|zsh|py|mjs)')"}}; do
    (( ${runner[(Ie)$s]} )) || [[ $s == tools/dev/test/* ]] || bad "$r names $s, not in check-reuse.py's RUNNER"
  done
done
for s in ${(f)"$(grep -E '^\s*(if .*then )?part (clippy|tests) ' $repo/tools/dev/check.sh | grep -oE 'tools/[A-Za-z0-9_./-]+')"}; do
  (( ${runner[(Ie)$s]} )) || bad "check.sh's clippy or tests line names $s, not in RUNNER"
done
grep -q '^rt=$root/tools/app/remote-tests.sh$' $repo/tools/dev/check.sh || bad "check.sh's tests run through something other than tools/app/remote-tests.sh"
(( fail )) || ok "KNOWN covers the repo; clippy and tests run only RUNNER's scripts"

# -- the rule, on a toy repo
tmp=$(mktemp -d); trap 'rm -rf $tmp' EXIT
mkdir $tmp/r; cd $tmp/r
g() { git -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@" >/dev/null 2>&1 }
w() { mkdir -p ${1:h}; print -r -- "$2" > $1 }
g init -q -b main .
w Cargo.toml '[workspace]
members = ["crates/a"]'
w Cargo.lock 'v1'
w crates/a/Cargo.toml '[package]
name = "a"'
w crates/a/src/lib.rs 'pub const X: &str = include_str!("../../../web/x.txt");
#[cfg(test)]
mod tests {
    #[test]
    fn reads_data() {
        let _ = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/data");
        helper();
    }
    fn helper() {
        let _ = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/helper");
    }
}'
w crates/a/tests/scan.rs '//! scans: docs/a.md
#[test]
fn t() {}'
for f in docs/a.md docs/b.md tools/dev/x.sh tools/data/y tools/helper/z web/x.txt README.md; do w $f 1; done
g add -A; g commit -m b0; b0=$(git rev-parse HEAD)
g checkout -q -b br
print '// branch' >> crates/a/src/lib.rs; g commit -am branch; P=$(git rev-parse HEAD)
print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- tests::' > $tmp/tests
hash=$(shasum $tmp/tests | cut -d' ' -f1)
rec="$P 2026-11-04T00:00:00Z tree $(git rev-parse 'HEAD^{tree}') base $(git rev-parse "$b0^{tree}") at $b0 tests $hash"

# case <name> <expected clippy> <expected tests> [<expected cmd>]: main gets the edits made by
# the function `edit`, the branch is rebased onto it, check-reuse.py is asked.
case_() {
  local name=$1 wc=$2 wt=$3 wcmd=${4:-} out c t cmd
  g checkout -q main; g reset -q --hard $b0
  edit; g add -A; g commit -m main
  g checkout -q br; g reset -q --hard $P; g rebase -q main
  [[ -n ${after:-} ]] && { eval $after; g commit -am after }
  out=$(python3 $reuse $tmp/passed $tmp/tests 2>&1)
  c=$(print -r -- $out | awk -F'\t' '$1=="clippy"{print $2}')
  t=$(print -r -- $out | awk -F'\t' '$1=="tests" && $2!="cmd"{print $2}')
  cmd=$(print -r -- $out | awk -F'\t' '$2=="cmd"{print $4}')
  if [[ $c == $wc && $t == $wt && $cmd == $wcmd ]]; then ok "$name: clippy $c, tests $t${cmd:+ ($cmd)}"
  else bad "$name: wanted clippy $wc, tests $wt${wcmd:+ ($wcmd)}; got:"; print -r -- $out | sed 's/^/    /'; fi
  # Every reuse names the pass and says why.
  print -r -- $out | awk -F'\t' '$2=="reuse" || $2=="partial"' | while IFS=$'\t' read -r _ _ from why; do
    [[ $from == $P && $why == *"plus main's changes since its base ${b0:0:7}"* ]] || bad "$name: a reuse not naming $P and why: $from $why"
  done
  after=
}
print -r -- $rec > $tmp/passed

edit() { print 2 >> docs/b.md }
case_ "docs only, no test reads it" reuse reuse
edit() { print 2 >> docs/a.md; print 2 >> README.md }
case_ "docs only, a scan test reads it" reuse partial 'cargo test -p a --test scan'
edit() { print 2 >> tools/dev/x.sh }
case_ "dev tools only" reuse reuse
edit() { print 2 >> tools/data/y }
case_ "a file a #[test] function reads" reuse partial 'cargo test -p a --lib -- tests::reads_data'
edit() { print 2 >> tools/helper/z }
case_ "a file read outside a #[test] function" reuse run
edit() { w crates/a/src/b.rs 'pub fn b() {}' }
case_ "a Rust change on main" run run
edit() { print v2 > Cargo.lock }
case_ "a Cargo.lock change on main" run run
edit() { print 2 >> web/x.txt }
case_ "a file the Rust build includes" run run
edit() { print 2 >> tools/dev/check.sh }
case_ "the script running the parts" run run
edit() { w newthing/z 1 }
case_ "a file of no known kind" run run
edit() { print 2 >> docs/b.md }; after='print 3 >> docs/a.md'
case_ "the branch changed after the rebase" run run
edit() { print 2 >> docs/b.md }
print -l 'cargo test -p a --test scan' > $tmp/tests
case_ "other tests to run here" reuse run
print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- tests::' > $tmp/tests
: > $tmp/passed
case_ "no earlier pass" run run
print -r -- ${rec/ at $b0/} > $tmp/passed
case_ "a pass recorded without its base" run run
print -r -- ${rec/at $b0/at 0123456789012345678901234567890123456789} > $tmp/passed
case_ "a pass whose base can't be found" run run
print -r -- $rec > $tmp/passed
edit() { print 2 >> docs/b.md }
case_ "docs only, again" reuse reuse
print x > untracked
out=$(python3 $reuse $tmp/passed $tmp/tests 2>&1)
[[ $out == *$'clippy\trun'* ]] && ok "an untracked file: run" || bad "an untracked file: $out"
rm untracked

# -- merge-ok.sh says what a pass reused, and refuses one reusing an unrecorded pass
mkdir -p tools/dev; cp $here/../merge-ok.sh tools/dev/; N=$(git rev-parse HEAD)
merge() {  # merge <want status> <text it says> <check-passed lines...>
  local want=$1 text=$2 out rc; shift 2
  print -l -- "$@" > .git/check-passed
  out=$(zsh tools/dev/merge-ok.sh $N 2>&1); rc=$?
  if [[ $rc == $want && $out == *"$text"* ]]; then ok "merge-ok: $out"; else bad "merge-ok ($want, $text): $rc $out"; fi
}
merge 0 "ok (check.sh passed at ${N:0:7})" "$N t"
merge 0 "reused: clippy from ${P:0:7}, tests from ${P:0:7} (partly run again)" "$P t" "$N t tree x reused clippy=$P reused tests=$P/partial"
merge 1 "clippy reused from ${b0:0:7}, which has no pass recorded" "$N t reused clippy=$b0"
merge 1 "never passed" "$P t"
exit $fail
