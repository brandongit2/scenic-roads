#!/bin/zsh
# The PM's check before cherry-picking (task #273): exits 0 when tools/dev/check.sh passed on
# <commit> or on a later commit of the same branch (one that has <commit> as an ancestor);
# otherwise says so and exits 1.
#   tools/dev/merge-ok.sh <commit>...
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
passed=( ${(f)"$(cut -d' ' -f1 $repo/.git/check-passed 2>/dev/null)"} )
rc=0
for c in "$@"; do
  full=$(git -C $repo rev-parse --verify -q "$c^{commit}") || { print "$c: no such commit"; rc=1; continue; }
  ok=
  for p in $passed; do
    [[ $p == $full ]] && { ok=1; break }
    git -C $repo merge-base --is-ancestor $full $p 2>/dev/null && { ok=1; break }
  done
  if [[ -n $ok ]]; then print "$c: ok (check.sh passed at ${p:0:7})"
  else print "$c: check.sh never passed on it: don't merge; have its agent run tools/dev/check.sh"; rc=1; fi
done
exit $rc
