#!/bin/zsh
# Gives an agent worktree a build folder seeded from the main checkout's: an APFS clone (cp -c, no
# bytes copied; blocks shared until either side rewrites them) of the compiled third-party crates
# (target/debug/{deps,build,.fingerprint} entries of non-workspace packages). The worktree then
# compiles only the workspace's own crates. Workspace crates are never copied: their artifact names
# don't depend on the checkout's path, and cargo judges freshness by mtime, so main's newer build
# of a crate would pass as fresh for the worktree's older source (shown on a toy workspace, 18 Oct).
# For the same reason the worktrees don't share one CARGO_TARGET_DIR / build-dir: two worktrees'
# copies of a crate would overwrite each other and one could run the other's code.
#
# Usage: seed-target.sh <worktree>. Also a PreToolUse hook on Bash (.claude/settings.local.json):
# reads the call's JSON on stdin and seeds the worktree it runs in, once, when the command runs cargo.
setopt extendedglob
case $#:${1:-} in
  1:-h|1:--help) print "usage: tools/dev/seed-target.sh <worktree>   (no argument: a hook, the call's JSON on stdin)"; exit 0 ;;
  0:|1:[^-]*) ;;
  *) print -u2 "seed-target.sh: unexpected arguments: $* (seed-target.sh --help)"; exit 2 ;;
esac
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}   # the main checkout, found from this script (also from a worktree copy)
log=$repo/.claude/sweep-targets.log
if [[ -n $1 ]]; then wt=${1:A}
else
  input=$(cat); [[ $input == *cargo* ]] || exit 0
  wt=$(print -r -- "$input" | python3 -c '
import json, re, sys
d = json.load(sys.stdin)
if d.get("tool_name") != "Bash" or not re.search(r"\bcargo\b", (d.get("tool_input") or {}).get("command") or ""): sys.exit()
print(d.get("cwd") or "")' 2>/dev/null)
  [[ $wt == $repo/.claude/worktrees/agent-* ]] || exit 0
  wt=$repo/.claude/worktrees/${${wt#$repo/.claude/worktrees/}%%/*}
fi
[[ -d $wt && $wt != $repo ]] || exit 0
dst=$wt/target/debug src=$repo/target/debug
# .seeding marks a seed under way: a hook killed part way (its timeout) or a failed cp leaves it, so
# the next hook seeds again; a deps folder without it is the worktree's own build, left alone.
[[ -e $dst/.seeded || ! -d $src/deps ]] && exit 0
[[ -d $dst/deps && ! -e $dst/.seeding ]] && exit 0
# Workspace names (packages and targets, - and _ forms), from the worktree's manifests.
names=$(cd $wt && cargo metadata --no-deps --format-version 1 --offline 2>/dev/null | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    for n in [p["name"]] + [t["name"] for t in p["targets"]]: print(n); print(n.replace("-", "_"))')
[[ -n $names ]] || exit 0
typeset -A ws; for x in ${(f)names}; do ws[$x]=1; done
mkdir -p $dst/deps $dst/build $dst/.fingerprint || exit 0
touch $dst/.seeding
# One cp per folder (#439): a cp per entry took 3 min at load 240, past the hook's 120 s timeout,
# which left half-seeded worktrees (no .fingerprint) that were never seeded again.
# 500 names a cp, under ARG_MAX however many entries main's target holds; a failed cp leaves
# .seeding in place (the next hook tries again) and says why in the log.
n=0; failed=0
for d in deps build .fingerprint; do
  typeset -a fs=()
  for f in $src/$d/*(DN); do
    b=${f:t}; b=${b#lib}; b=${b%%-[0-9a-f](#c16)*}
    (( ${+ws[$b]} )) || fs+=$f
  done
  for (( i = 1; i <= $#fs; i += 500 )); do
    chunk=(${fs[i,i+499]})
    if cp -cRp $chunk $dst/$d/ 2>>$log; then (( n += $#chunk )); else failed=1; fi
  done
done
if (( failed )); then
  print "$(date '+%F %T') seeding ${wt:t}/target failed part way ($n clones): not marked seeded" >> $log
  exit 0
fi
touch $dst/.seeded; rm -f $dst/.seeding
(( n > 0 )) || exit 0
print "$(date '+%F %T') seeded ${wt:t}/target from main ($n clones)" >> $log
exit 0
