#!/bin/zsh
# tools/dev/test-guard.py's worktree lookup: a filter for a module in <mod>/mod.rs passes when the
# command goes into the worktree by ~/…, $HOME/… or its full path, from the main checkout (task
# #458 review: `cd ~/…` was cut at its first `/` and every module refused). Run by check.sh.
set -u
here=${0:A:h}; wt=$(git -C $here rev-parse --show-toplevel)
[[ $wt == */.claude/worktrees/* ]] || { print "skip: not in a worktree"; exit 0 }
main=${wt%%/.claude/worktrees/*}; rel=${wt#$HOME/}
mod=$(cd $wt/crates/pipeline/src && for d in */; do [[ -f $d/mod.rs ]] && { print ${d%/}; break }; done)
[[ -n $mod ]] || { print "no <mod>/mod.rs in pipeline"; exit 1 }
fail=0
for dir in "~/$rel" "\$HOME/$rel" "$wt"; do
  cmd="cd $dir && cargo test -p pipeline --lib -- $mod::"
  # Only the module check matters here: the affected-set check may refuse it, but not as "isn't a module".
  out=$(python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":sys.argv[1]},"cwd":sys.argv[2],"agent_id":"t"}))' $cmd $main | python3 $here/../test-guard.py 2>&1)
  if [[ $out == *"isn't a module path"* ]]; then print "FAIL: $cmd\n  $out"; fail=1; else print "ok: $cmd"; fi
done
exit $fail
