#!/bin/zsh
# The PM's check before cherry-picking (task #273): exits 0 when tools/dev/check.sh passed on
# <commit> or on a later commit of the same branch (one that has <commit> as an ancestor);
# otherwise says so and exits 1. A pass that reused parts of an earlier one (#950: clippy or the
# tests, tools/dev/check-reuse.py) says which and from where, and holds only while each pass it
# reused from is recorded too.
#   tools/dev/merge-ok.sh <commit>...
case ${1:-} in
  -h|--help) print "usage: tools/dev/merge-ok.sh <commit>..."; exit 0 ;;
  ""|-*) print -u2 "merge-ok.sh: which commits? (merge-ok.sh <commit>...)"; exit 2 ;;
esac
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
lines=( ${(f)"$(cat $repo/.git/check-passed 2>/dev/null)"} )
passed=( ${lines%% *} )
# What a pass reused, "clippy from abc1234, tests from def5678 (partly run again)"; fails when a
# pass it reused from isn't recorded.
reuses() {
  local l=${lines[(R)$1 *]} t=() said=() i src what
  t=( ${=l} )
  for (( i = 3; i < ${#t}; i++ )); do
    [[ ${t[i]} == reused ]] || continue
    what=${t[i+1]%%=*} src=${${t[i+1]#*=}%/partial}
    (( ${passed[(Ie)$src]} )) || { print -r -- "$what reused from ${src:0:7}, which has no pass recorded"; return 1 }
    said+="$what from ${src:0:7}"
    [[ ${t[i+1]} == */partial ]] && said[-1]+=" (partly run again)"
  done
  print -r -- ${(j:, :)said}
}
rc=0
for c in "$@"; do
  full=$(git -C $repo rev-parse --verify -q "$c^{commit}") || { print "$c: no such commit"; rc=1; continue; }
  ok=
  for p in ${(Oa)passed}; do
    [[ $p == $full ]] && { ok=1; break }
    git -C $repo merge-base --is-ancestor $full $p 2>/dev/null && { ok=1; break }
  done
  if [[ -z $ok ]]; then print "$c: check.sh never passed on it: don't merge; have its agent run tools/dev/check.sh"; rc=1; continue; fi
  if r=$(reuses $p); then print "$c: ok (check.sh passed at ${p:0:7}${r:+; reused: $r})"
  else print "$c: check.sh's pass at ${p:0:7} doesn't hold: $r: have its agent run tools/dev/check.sh"; rc=1; fi
done
exit $rc
