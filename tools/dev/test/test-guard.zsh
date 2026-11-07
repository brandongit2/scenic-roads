#!/bin/zsh
# tools/dev/test-guard.py's worktree lookup: a filter for a module in <mod>/mod.rs passes when the
# command goes into the worktree by ~/…, $HOME/… or its full path, from the main checkout (task
# #458 review: `cd ~/…` was cut at its first `/` and every module refused). Run by check.sh.
set -u
here=${0:A:h}; wt=$(git -C $here rev-parse --show-toplevel)
[[ $wt == */.claude/worktrees/* ]] || { print "skip: not in a worktree"; exit 0 }
main=${wt%%/.claude/worktrees/*}; rel=${wt#$HOME/}
# (A crate with folder modules: agentcore, the runtime, since #916's 6f4 left pipeline none.)
crate=agentcore
mod=$(cd $wt/crates/$crate/src && for d in */; do [[ -f $d/mod.rs ]] && { print ${d%/}; break }; done)
[[ -n $mod ]] || { print "no <mod>/mod.rs in $crate"; exit 1 }
fail=0
for dir in "~/$rel" "\$HOME/$rel" "$wt"; do
  cmd="cd $dir && cargo test -p $crate --lib -- $mod::"
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
  if [[ $1 == ok ]]; then [[ $out == *"isn't a module path"* || $out == *"No full test suites"* || $out == *"--release"* || $out == *"can't be checked"* ]] && bad=1
  else [[ $out == *"$1"* ]] || bad=1; fi
  if (( bad )); then print "FAIL ($1): $2\n  $out"; fail=1; else print "ok ($1): $2"; fi
}
f="$mod::x"
for r in '> /p/log' '>/p/log' '>> /p/log' '>>/p/log' '2> /p/log' '2>/p/log' '2>>/p/log' '&> /p/log' '&>/p/log' '&>> /p/log' '>& /p/log' '>&/p/log' '< /p/in' '</p/in' '> "/p/my log"' "> '/p/my log'" '>"/p/my log"' '2>&1' '> /p/log 2>&1' '1>&2 >/p/log' '>| /p/log' '<<< x' '>! /p/log' '>>|/p/log' '&>>! /p/log' '<> /p/f' '2>&-' '>&2x'; do
  check ok "cargo test -p $crate $r -- $f"
  check ok "cargo test -p $crate -- $f $r"
  check "isn't a module path" "cargo test -p $crate $r -- bare"
  check "No full test suites" "cargo test -p $crate $r"
  check "--release" "cargo test -p $crate --release $r -- $f"
done
check ok "cargo test -p $crate 2>&1 -- $f"
check ok "cargo test -p $crate 2>&1 --lib -- $f"
check ok "cargo test -p $crate -- $f 2>&1 | tee /p/log"
check ok "cargo test -p $crate -- $f 2>&1 | tee -a /p/log"
check ok "cargo test -p $crate -- $f > /p/log 2>&1 && echo done"
check ok "cargo nextest run -p $crate $f > /p/log 2>&1"
check ok "zsh -c 'cargo test -p $crate -- $f > /p/log 2>&1'"
check ok "sh -c \"cargo test -p $crate -- $f 2>&1 | tee /p/log\""
check "isn't a module path" "cargo test -p $crate -- bare 2>&1 | tee /p/log"
check "isn't a module path" "cargo nextest run -p $crate bare > /p/log 2>&1"
check "isn't a module path" "zsh -c 'cargo test -p $crate -- bare > /p/log 2>&1'"
check "No full test suites" "cargo test -p $crate 2>&1 | tee /p/log"
check "No full test suites" "sh -c 'cargo test -p $crate > /p/log'"
check "--release" "cargo test -p $crate --release > /p/log 2>&1"
check "--release" "cargo test -p $crate --release -- $f 2>&1 | tee /p/log"
check ok "cargo test -p $crate --release -- $f > /p/log 2>&1 #perf"
check "isn't a module path" "cargo test -p $crate -- '>x'"   # quoted: an argument, not a redirect
check "isn't a module path" "cargo test -p $crate -- $f '2'>/p/log"   # a quoted 2 is an argument, not the fd
check "isn't a module path" "cargo test -p $crate -- $f \\2>/p/log"
check ok "cargo test -p $crate -- $f 2>&1--release"   # >& to the file 1--release, not fd 1 and a flag
check ok $'cargo test -p $crate \\\n  -- '"$f > /p/log"   # a line continuation is no word
check "--release" $'cargo test -p $crate \\\n  --release -- '$f
check "isn't a module path" "cargo test -p $crate -- bare"
check "No full test suites" "cargo test -p $crate"
check "--release" "cargo test -p $crate --release -- $f"
check "--workspace" "cargo test --workspace 2>&1 | tee /p/log"
# A substitution stays with its command (it was cut at the "(" and the rest left the cargo command), and a
# test run whose arguments hold one is refused: the guard can't see what it expands to.
S="can't be checked (the guard can't see"
check "$S" "cargo test -p $crate -- ${f}\$(:) --release"
check "$S" "cargo test -p $crate \$(echo --release) -- $f"
check "$S" "cargo test -p $crate -- $f \`echo --release\`"
check "$S" "cargo test -p $crate -- $f <(:)"
check "$S" "cargo test -p $crate -- $f >(:)"
check "$S" "cargo test -p $crate -- $f \"\$(:)\""
check "$S" "cargo nextest run -p $crate $f\$(:)"
check "$S" "cargo \$(echo --config x) test -p pipeline -- $f"
check "$S" "cargo \$(echo test) -p pipeline --release -- $f"
check ok "cargo build -p pipeline 2>&1 | tail -\$(echo 3)"
check ok "X=\$(echo a b) cargo test -p $crate -- $f"   # an assignment isn't split into cargo's words
check ok "cargo test -p $crate -- $f > /p/log-\$(date +%s)"   # a redirect's target isn't an argument
check ok "cargo test -p $crate -- $f   # checks \`Room::passes_over\`"   # nor a comment
check "--release" "x=\$(echo \")\"; cargo test -p $crate --release -- $f)"   # a quoted ) doesn't end it
check "--release" "x=\$(echo ')'; cargo test -p $crate --release -- $f)"
check "--release" "echo \`cargo test -p $crate --release -- $f\`"
# A command that runs what a substitution writes: a cargo test written there can't be read.
B="writes out a cargo test"
check "$B" "sh -c \"\$(echo cargo test -p $crate --release -- $f)\""
check "$B" "eval \"\$(echo cargo test -p $crate --release -- $f)\""
check "$B" "\$(echo cargo test) -p pipeline --release -- $f"
check ok "\$(git rev-parse --show-toplevel)/tools/dev/affected-tests.sh; x=\$(cargo test -p $crate -- $f 2>&1)"
check ok "eval \"\$(git rev-parse --show-toplevel)\" && cargo test -p $crate -- $f"
check "built from variables or substitutions" "sh -c 'cargo test -p $crate -- $f\$(:)'"
check "--release" "echo \$(:) && cargo test -p $crate --release -- $f"
check "--release" "echo \$(echo a; echo b) && cargo test -p $crate --release -- $f"
check "No full test suites" "x=\$(cargo test -p $crate)"   # a run inside a substitution is found
check "--release" "echo \"\$(cargo test -p $crate --release -- $f)\""
check ok "cargo test -p $crate -- $f --skip '\$(:)'"   # single-quoted: text, not a substitution
check ok "cargo test -p $crate -- $f > /p/log && echo \$(date)"
check ok "echo \$(date) | cargo test -p $crate -- $f"
exit $fail
