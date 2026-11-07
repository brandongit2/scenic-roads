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
for s in ${(f)"$(grep -E '^\s*(if .*then )?part (--parse [^ ]+ )?(clippy|tests) ' $repo/tools/dev/check.sh | grep -oE 'tools/[A-Za-z0-9_./-]+')"}; do
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
mod m;
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
w crates/a/src/m.rs 'pub const Y: &[u8] = include_bytes!("../../../web/y.txt");
#[cfg(test)]
mod tests {
    #[test]
    fn t() {}
    #[test]
    fn reads_c() {
        let _ = include_str!("../../../docs/c.md");
    }
}'
w crates/a/tests/scan.rs '//! scans: docs/a.md
#[test]
fn t() {}'
for f in docs/a.md docs/b.md docs/c.md tools/dev/x.sh tools/data/y tools/helper/z web/x.txt web/y.txt README.md; do w $f 1; done
g add -A; g commit -m b0; b0=$(git rev-parse HEAD)
g checkout -q -b br
print '// branch' >> crates/a/src/lib.rs; g commit -am branch; P=$(git rev-parse HEAD)
print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- m:: tests::' > $tmp/tests
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
# (#969: main's Rust changes since the pass traced as the branch's own are.)
edit() { print '// 2' >> crates/a/src/m.rs }
case_ "a module's change on main" run partial 'cargo test -p a --lib -- m::'
edit() { w crates/a/src/b.rs 'pub fn b() {}' }
case_ "a new file on main no module declares" run reuse
edit() { { print '// 2'; cat crates/a/src/lib.rs } > $tmp/lib.rs && mv $tmp/lib.rs crates/a/src/lib.rs }   # (at its top: the branch's edit is at its end)
case_ "the crate's root changed on main" run run
edit() { print v2 > Cargo.lock }
case_ "a Cargo.lock change on main" run run
edit() { print 2 >> web/x.txt }
case_ "a file the crate's root includes" run run
edit() { print 2 >> web/y.txt }
case_ "a file a module compiles in" run partial 'cargo test -p a --lib -- m::'
edit() { print 2 >> tools/dev/check.sh }
case_ "the script running the parts" run run
edit() { w newthing/z 1 }
case_ "a file of no known kind" run run
edit() { print 2 >> docs/b.md }; after='print 3 >> docs/a.md'
case_ "the branch changed after the rebase: a file a scan test reads" reuse partial 'cargo test -p a --test scan'
# (#969: the branch's own edits since the pass; the tests they affect run again.)
edit() { print 2 >> docs/b.md }; after='print "// 2" >> crates/a/src/m.rs'
case_ "the branch's own edit of a module since the pass" run partial 'cargo test -p a --lib -- m::'
edit() { print 2 >> docs/b.md }; after='print "// 2" >> crates/a/src/lib.rs'
case_ "the branch's own edit of the crate's root" run run
edit() { print 2 >> docs/b.md }; after='print "# 2" >> crates/a/Cargo.toml'
case_ "the branch's own manifest edit" run run
edit() { print 2 >> docs/b.md }; after='print 2 >> web/x.txt'
case_ "the branch's own edit of a file the build includes" run run
edit() { print 2 >> docs/b.md }; after='print 2 >> docs/c.md'
case_ "the branch's own edit of a file a #[test] function includes" run partial 'cargo test -p a --lib -- m::tests::reads_c'
edit() { print 2 >> docs/b.md }; after='print 2 >> crates/a/tests/scan.rs'
case_ "the branch's own edit of an integration test" run partial 'cargo test -p a --test scan'
edit() { print 2 >> docs/b.md }
print -l 'cargo test -p a --test scan' > $tmp/tests
case_ "other tests to run here, the pass's not kept" reuse run
mkdir -p $tmp/check-tests; print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- m:: tests::' > $tmp/check-tests/$hash
case_ "fewer tests to run here than the pass ran" reuse reuse
print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- m:: n:: tests::' 'cargo test -p a --bin x' > $tmp/tests
case_ "more tests to run here than the pass ran" reuse partial $'cargo test -p a --bin x\ncargo test -p a --lib -- n::'
# (Its file cut short, as a reader sees one half copied: its last filter, `n`, would match n::.)
print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- m:: n' > $tmp/check-tests/$hash
print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- m:: n::' > $tmp/tests
case_ "the pass's kept commands not their hash" reuse run
rm -r $tmp/check-tests
print -l 'cargo test -p a --test scan' 'cargo test -p a --lib -- m:: tests::' > $tmp/tests
# (Another pass beside P, of main's tree before the branch: the branch's change its own since; P is weighed.)
print -l -- $rec "$b0 2026-11-04T01:00:00Z tree $(git rev-parse "$b0^{tree}") base $(git rev-parse "$b0^{tree}") at $b0 tests $hash" > $tmp/passed
edit() { print 2 >> docs/b.md }
case_ "the pass with the fewest own changes since" reuse reuse
print -r -- $rec > $tmp/passed
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

# -- reads and inputs placed only by looking further (review of #950): each on a toy repo of its
# own, the branch passed at P, main changing docs/a.md (or as said), the branch rebased.
fresh() {  # fresh <name> <base setup> <main's edit> <tests commands, |-separated> <want clippy> <want tests> [<want cmd>]
  local name=$1 d=$(mktemp -d) out c t cmd
  ( mkdir $d/r && cd $d/r && g init -q -b main . && w Cargo.toml '[workspace]
members = ["crates/a"]' && w crates/a/Cargo.toml '[package]
name = "a"' && w crates/a/src/lib.rs '' && w docs/a.md 1 && w tools/shared/sub.rs 1
    $2; g add -A; g commit -m b0; local b=$(git rev-parse HEAD)
    g checkout -q -b br; w docs/branch.md 1; g add -A; g commit -m branch; local p=$(git rev-parse HEAD)
    print -l -- ${(s:|:)4} > $d/tests
    print -r -- "$p t tree $(git rev-parse 'HEAD^{tree}') base $(git rev-parse "$b^{tree}") at $b tests $(shasum $d/tests | cut -d' ' -f1)" > $d/passed
    g checkout -q main; $3; g add -A; g commit -m main; g checkout -q br; g rebase -q main
    python3 $reuse $d/passed $d/tests 2>&1 ) > $d/out
  out=$(<$d/out); rm -rf $d
  c=$(print -r -- $out | awk -F'\t' '$1=="clippy"{print $2}')
  t=$(print -r -- $out | awk -F'\t' '$1=="tests" && $2!="cmd"{print $2}')
  cmd=$(print -r -- $out | awk -F'\t' '$2=="cmd"{print $4}')
  if [[ $c == $5 && $t == $6 && $cmd == ${7:-} ]]; then ok "$name: clippy $c, tests $t${cmd:+ ($cmd)}"
  else bad "$name: wanted clippy $5, tests $6${7:+ ($7)}; got:"; print -r -- $out | sed 's/^/    /'; fi
}
docs() { print 2 >> docs/a.md }
s() { w crates/a/src/lib.rs '#[test]
fn t() { let _ = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("docs"); }' }
fresh 'join("..") from the crate' s docs 'cargo test -p a --lib -- t' reuse partial 'cargo test -p a --lib -- t'
s() { w crates/a/src/lib.rs '#[test]
fn t() { let _ = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("../../../docs/a.md"); }' }
fresh '"../" from a deeper folder' s docs 'cargo test -p a --lib -- t' reuse partial 'cargo test -p a --lib -- t'
s() { w crates/a/src/lib.rs '#[test]
fn t() { let _ = std::fs::read(std::path::Path::new("..").join("..").join("docs/a.md")); }' }
fresh 'Path::new("..")' s docs 'cargo test -p a --lib -- t' reuse partial 'cargo test -p a --lib -- t'
s() { mkdir -p crates/a; ln -s ../../docs crates/a/data; w crates/a/src/lib.rs 'pub const X: &str = include_str!("../data/a.md");' }
fresh "a symlink into docs/, included" s docs 'cargo test -p a --lib -- t' run run
s() { w crates/a/src/lib.rs '#[path = "../../../tools/shared/mod.rs"] mod shared;'; w tools/shared/mod.rs 'mod sub;' }
m() { print '// 2' >> tools/shared/sub.rs }
fresh "a #[path] module outside the packages (its own module changed)" s m 'cargo test -p a --lib -- t' run run
s() { w crates/a/src/lib.rs 'mod m;
mod b { m!(); }'; w crates/a/src/m.rs '#[macro_export] macro_rules! m { () => { #[test] fn reads() { let _ = std::fs::read("../../docs/a.md"); } } }' }
fresh "a test a macro makes elsewhere" s docs 'cargo test -p a --lib -- b::' reuse run
s() { w crates/a/src/main.rs '#[test]
fn t() { let _ = std::fs::read("../../docs/a.md"); }
fn main() {}' }
fresh "a library and a main.rs" s docs 'cargo test -p a --bin a' reuse run
s() { w crates/a/tests/sub/main.rs '#[test]
fn t() { let _ = std::fs::read("../../docs/a.md"); }' }
fresh "a test as a folder (tests/sub/main.rs)" s docs 'cargo test -p a --test sub' reuse partial 'cargo test -p a --test sub'
s() { w crates/a/Cargo.toml '[package]
name = "alpha"'; w crates/a/src/lib.rs '#[test]
fn t() { let _ = std::fs::read("../../docs/a.md"); }' }
fresh "a package not named as its folder" s docs 'cargo test -p alpha --lib -- t' reuse run
s() { w Cargo.toml '[workspace]
members = ["crates/a"]
[patch.crates-io]
x = { path = "tools/x" }'; w tools/x/src/lib.rs 1 }
m() { print 2 >> tools/x/src/lib.rs }
fresh "a [patch] path in the root manifest" s m 'cargo test -p a --lib -- t' run run
s() { w crates/a/src/lib.rs '#[derive(Embed)] #[folder = "../../web/dist"] struct S;'; w web/dist/i.html 1 }
m() { print 2 >> web/dist/i.html }
fresh "a folder a macro attribute names" s m 'cargo test -p a --lib -- t' run run
s() { rm crates/a/src/lib.rs; w crates/a/src/main.rs '#[test]
fn t() {}
fn main() {}'; w crates/a/tests/i.rs '#[test]
fn i() {}' }
m() { print '// 2' >> crates/a/src/main.rs }
fresh "a binary's root changed on main: its crate's every target" s m 'cargo test -p a --bin a --test i' run partial 'cargo test -p a --bin a --test i'
s() { w crates/a/examples/e.rs 'fn main() {}' }
m() { print '// 2' >> crates/a/examples/e.rs }
fresh "an example changed on main: no test builds it" s m 'cargo test -p a --lib -- t' run reuse

# -- affected-tests.sh --nextest: check.sh's tests lines as one nextest run (#969), a lib's unit
# tests built only for the packages asking for them
w crates/b/src/main.rs 'fn main() {}'; w crates/c/src/lib.rs ''; w crates/c/tests/scan.rs ''
nextest() {  # nextest <want, lines joined by |> <cargo test lines...>
  local want=$1 out; shift
  print -l -- "$@" > $tmp/nx
  out=$(zsh $here/../affected-tests.sh --nextest $tmp/nx 2>&1)
  out=${out//cargo nextest run --no-fail-fast --hide-progress-bar --no-tests=warn /}
  if [[ ${(j:|:)${(f)out}} == $want ]]; then ok "--nextest: $want"; else bad "--nextest wanted $want; got:"; print -r -- $out | sed 's/^/    /'; fi
}
nextest "-p a -p b --lib --bin b -E '(binary_id(=a) & (test(~tests::x))) | (binary_id(=b::bin/b) & (test(~m::) | test(~n::)))'" \
  'cargo test -p a --lib -- tests::x' 'cargo test -p b --bin b -- n:: m::'
nextest "-p a --lib -E '(binary_id(=a) & (test(~t::)))'|-p c -p b --bin b --test scan -E 'binary_id(=c::scan) | binary_id(=b::bin/b)'" \
  'cargo test -p a --lib -- t::' 'cargo test -p c --test scan' 'cargo test -p b --bin b'
nextest "-p a --lib --test scan -E 'binary_id(=a::scan) | binary_id(=a)'" 'cargo test -p a --test scan' 'cargo test -p a --lib -- tests::' 'cargo test -p a --lib'

# -- merge-ok.sh says what a pass reused, and refuses one reusing an unrecorded pass
mkdir -p tools/dev; cp $here/../merge-ok.sh tools/dev/; N=$(git rev-parse HEAD)
merge() {  # merge <want status> <text it says> <check-passed lines...>
  local want=$1 text=$2 out rc; shift 2
  print -l -- "$@" > .git/check-passed
  out=$(zsh tools/dev/merge-ok.sh $N 2>&1); rc=$?
  if [[ $rc == $want && $out == *"$text"* ]]; then ok "merge-ok: $out"; else bad "merge-ok ($want, $text): $rc $out"; fi
}
n="$N t tree $(git rev-parse "$N^{tree}")" p="$P t tree $(git rev-parse "$P^{tree}")"   # (a pass names its tree, #980)
merge 0 "ok (check.sh passed at ${N:0:7})" "$n"
merge 0 "reused: clippy from ${P:0:7}, tests from ${P:0:7} (partly run again)" "$p" "$n reused clippy=$P reused tests=$P/partial"
merge 1 "clippy reused from ${b0:0:7}, which has no pass recorded" "$n reused clippy=$b0"
merge 1 "never passed" "$p"
merge 1 "whose pass doesn't hold (clippy reused from ${b0:0:7}, which has no pass recorded)" "$p reused clippy=$b0" "$n reused all=$P"
merge 0 "ok (check.sh passed at ${N:0:7})" "$n" "$n reused all=$N"   # (a tree checked twice: not a loop)
merge 1 "doesn't name its tree" "$N t"   # (before #980, or a checkout changed during its run)
merge 1 "doesn't name its tree" "$N t tree $(git rev-parse "$P^{tree}")"
merge 1 "whose pass doesn't hold (${P:0:7}'s pass doesn't name its tree" "$P t" "$n reused clippy=$P"
exit $fail
