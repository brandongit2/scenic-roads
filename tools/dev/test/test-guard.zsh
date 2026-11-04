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

# Redirects and what they name are not cargo's arguments (task #938: `cargo test -p det > log 2>&1`
# refused the log's path as a filter), nor is what a pipe feeds. "ok" = none of the guard's argument
# refusals (module path, full suite, --release); the affected-set check may still say what it likes.
run() { python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":sys.argv[1]},"cwd":sys.argv[2],"agent_id":"t"}))' "$1" $wt | python3 $here/../test-guard.py 2>&1 }
check() {  # expectation (ok | a text the refusal has), command
  local out=$(run "$2") bad=0
  if [[ $1 == ok ]]; then [[ $out == *"isn't a module path"* || $out == *"No full test suites"* || $out == *"--release"* ]] && bad=1
  else [[ $out == *"$1"* ]] || bad=1; fi
  if (( bad )); then print "FAIL ($1): $2\n  $out"; fail=1; else print "ok ($1): $2"; fi
}
f="$mod::x"
for r in '> /p/log' '>/p/log' '>> /p/log' '>>/p/log' '2> /p/log' '2>/p/log' '2>>/p/log' '&> /p/log' '&>/p/log' '&>> /p/log' '>& /p/log' '>&/p/log' '< /p/in' '</p/in' '> "/p/my log"' "> '/p/my log'" '>"/p/my log"' '2>&1' '> /p/log 2>&1' '1>&2 >/p/log' '>| /p/log' '<<< x'; do
  check ok "cargo test -p pipeline $r -- $f"
  check ok "cargo test -p pipeline -- $f $r"
  check "isn't a module path" "cargo test -p pipeline $r -- bare"
  check "No full test suites" "cargo test -p pipeline $r"
  check "--release" "cargo test -p pipeline --release $r -- $f"
done
check ok "cargo test -p pipeline 2>&1 -- $f"
check ok "cargo test -p pipeline 2>&1 --lib -- $f"
check ok "cargo test -p pipeline -- $f 2>&1 | tee /p/log"
check ok "cargo test -p pipeline -- $f 2>&1 | tee -a /p/log"
check ok "cargo test -p pipeline -- $f > /p/log 2>&1 && echo done"
check ok "cargo nextest run -p pipeline $f > /p/log 2>&1"
check ok "zsh -c 'cargo test -p pipeline -- $f > /p/log 2>&1'"
check ok "sh -c \"cargo test -p pipeline -- $f 2>&1 | tee /p/log\""
check "isn't a module path" "cargo test -p pipeline -- bare 2>&1 | tee /p/log"
check "isn't a module path" "cargo nextest run -p pipeline bare > /p/log 2>&1"
check "isn't a module path" "zsh -c 'cargo test -p pipeline -- bare > /p/log 2>&1'"
check "No full test suites" "cargo test -p pipeline 2>&1 | tee /p/log"
check "No full test suites" "sh -c 'cargo test -p pipeline > /p/log'"
check "--release" "cargo test -p pipeline --release > /p/log 2>&1"
check "--release" "cargo test -p pipeline --release -- $f 2>&1 | tee /p/log"
check ok "cargo test -p pipeline --release -- $f > /p/log 2>&1 #perf"
check "isn't a module path" "cargo test -p pipeline -- '>x'"   # quoted: an argument, not a redirect
check "isn't a module path" "cargo test -p pipeline -- bare"
check "No full test suites" "cargo test -p pipeline"
check "--release" "cargo test -p pipeline --release -- $f"
check "--workspace" "cargo test --workspace 2>&1 | tee /p/log"
exit $fail
