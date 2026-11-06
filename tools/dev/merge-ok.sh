#!/bin/zsh
# The PM's check before cherry-picking (task #273): exits 0 when tools/dev/check.sh passed on
# <commit> or on a later commit of the same branch (one that has <commit> as an ancestor);
# otherwise says so and exits 1. A pass that reused parts of an earlier one (#950: clippy or the
# tests, tools/dev/check-reuse.py) says which and from where, and holds only while each pass it
# reused from is recorded and holds in turn. A pass holds only when it names the commit's tree
# (`tree <hash>`, #980): check.sh records nothing for a checkout that changed during its run.
#   tools/dev/merge-ok.sh <commit>...
case ${1:-} in
  -h|--help) print "usage: tools/dev/merge-ok.sh <commit>..."; exit 0 ;;
  ""|-*) print -u2 "merge-ok.sh: which commits? (merge-ok.sh <commit>...)"; exit 2 ;;
esac
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
lines=( ${(f)"$(cat $repo/.git/check-passed 2>/dev/null)"} )
passed=( ${lines%% *} )
# holds <commit> [<seen>...]: one of the passes recorded for <commit> holds: each pass it reused
# from (#950: clippy or the tests, tools/dev/check-reuse.py; all, a tree checked before) is recorded
# and holds in turn. REPLY: what it reused ("clippy from abc1234, tests from def5678 (partly run
# again)"), or why none holds.
holds() {
  local c=$1 l i src what bad tree; local -a t said; shift
  REPLY="${c:0:7} has no pass recorded"
  tree=$(git -C $repo rev-parse -q --verify "$c^{tree}")
  for l in ${(M)lines:#$c *}; do
    t=( ${=l} ) said=() bad=
    # (A pass names the tree it checked, #980: one that doesn't, or names another, vouches for nothing.)
    if [[ -z $tree || ${t[(ie)tree]} -ge ${#t} || ${t[${t[(ie)tree]}+1]} != $tree ]]; then
      REPLY="${c:0:7}'s pass doesn't name its tree ${tree:0:7} (recorded by a check.sh before #980, or while its checkout changed)"; continue
    fi
    for (( i = 3; i < ${#t}; i++ )); do
      [[ ${t[i]} == reused ]] || continue
      what=${t[i+1]%%=*} src=${${t[i+1]#*=}%/partial}
      if (( ! ${passed[(Ie)$src]} )); then bad="$what reused from ${src:0:7}, which has no pass recorded"
      elif [[ $src == $c ]] || (( ${@[(Ie)$src]} )); then bad="$what reused from ${src:0:7}, in a loop"
      elif ! holds $src $c "$@"; then bad="$what reused from ${src:0:7}, whose pass doesn't hold ($REPLY)"; fi
      [[ -n $bad ]] && break
      said+="$what from ${src:0:7}"
      [[ ${t[i+1]} == */partial ]] && said[-1]+=" (partly run again)"
    done
    if [[ -z $bad ]]; then REPLY=${(j:, :)said}; return 0; fi
    REPLY=$bad
  done
  return 1
}
rc=0
for c in "$@"; do
  full=$(git -C $repo rev-parse --verify -q "$c^{commit}") || { print "$c: no such commit"; rc=1; continue; }
  ok= why=
  for p in ${(Oa)passed}; do
    [[ $p == $full ]] || git -C $repo merge-base --is-ancestor $full $p 2>/dev/null || continue
    if holds $p; then ok=1; break; fi
    [[ -n $why ]] || why="check.sh's pass at ${p:0:7} doesn't hold: $REPLY"
  done
  if [[ -n $ok ]]; then print "$c: ok (check.sh passed at ${p:0:7}${REPLY:+; reused: $REPLY})"
  elif [[ -n $why ]]; then print "$c: $why: have its agent run tools/dev/check.sh"; rc=1
  else print "$c: check.sh never passed on it: don't merge; have its agent run tools/dev/check.sh"; rc=1; fi
done
exit $rc
