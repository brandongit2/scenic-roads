#!/bin/zsh
# tools/dev/test-guard.py's verdicts on a subagent's commands in this worktree: each case below is a
# command and what the guard must say ("ok": nothing, or only that it is outside the affected set;
# else a text its refusal holds). Run by check.sh when the guard or shellwords.py changed.
# The cases run in one python process (#1003), each through the guard's whole code as the hook runs
# it (its stdin, its exit); the processes it starts (affected-tests.sh, git) run once for all cases,
# their answers kept, as the tree doesn't change meanwhile. (A process per case ran affected-tests.sh
# for each of the ~80 cases it reached: on the M4, with 6f4's 130 Rust files changed, 893 s, 2,060 s
# in 7 Nov's checks; now 11 s, 10 s of it the one run.) "ok" also fails on a crash, which passed.
set -u
here=${0:A:h}; wt=$(git -C $here rev-parse --show-toplevel)
[[ $wt == */.claude/worktrees/* ]] || { print "skip: not in a worktree"; exit 0 }
main=${wt%%/.claude/worktrees/*}; rel=${wt#$HOME/}
# (A crate with folder modules: agentcore, the runtime, since #916's 6f4 left pipeline none.)
crate=agentcore
mod=$(cd $wt/crates/$crate/src && for d in */; do [[ -f $d/mod.rs ]] && { print ${d%/}; break }; done)
[[ -n $mod ]] || { print "no <mod>/mod.rs in $crate"; exit 1 }
typeset -a cases   # expectation, cwd, command; a case after another
check() {  # [-C <cwd>] expectation (ok | a text the refusal has), command; cwd: the worktree
  local cwd=$wt; [[ $1 == -C ]] && { cwd=$2; shift 2 }
  cases+=( "$1" "$cwd" "$2" )
}

# The worktree a command goes into, from the main checkout: by ~/…, $HOME/… or its full path, a filter
# for a module in <mod>/mod.rs is a module path (task #458 review: `cd ~/…` was cut at its first `/`
# and every module refused).
for dir in "~/$rel" "\$HOME/$rel" "$wt"; do check -C $main ok "cd $dir && cargo test -p $crate --lib -- $mod::"; done

# Redirects and what they name are not cargo's arguments (task #938: `cargo test -p det > log 2>&1`
# refused the log's path as a filter), nor is what a pipe feeds.
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
check ok "cargo test -p $crate "$'\\\n  -- '"$f > /p/log"   # a line continuation is no word ($'…' expands no $crate)
check "--release" "cargo test -p $crate "$'\\\n  --release -- '$f
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

python3 - $here/../test-guard.py "${cases[@]}" <<'PY'
import io, json, os, subprocess, sys, traceback
from contextlib import redirect_stderr, redirect_stdout
guard, args = sys.argv[1], sys.argv[2:]
sys.path.insert(0, os.path.dirname(os.path.abspath(guard)))   # (its shellwords, as when run as a script)
code = compile(open(guard, encoding="utf-8").read(), guard, "exec")
def once(f):   # a process's answer (or its error), kept for the same call
    kept = {}
    def call(*a, **kw):
        k = repr((a, sorted(kw.items())))
        if k not in kept:
            try: kept[k] = (f(*a, **kw), None)
            except Exception as e: kept[k] = (None, e)
        r, e = kept[k]
        if e: raise e
        return r
    return call
subprocess.run, subprocess.check_output = once(subprocess.run), once(subprocess.check_output)
def verdict(cwd, cmd):   # what the hook would show: the guard's output, or its traceback
    sys.stdin, out = io.StringIO(json.dumps({"tool_name": "Bash", "tool_input": {"command": cmd}, "cwd": cwd, "agent_id": "t"})), io.StringIO()
    try:
        with redirect_stdout(out), redirect_stderr(out): exec(code, {"__name__": "__main__", "__file__": guard})
    except SystemExit: pass
    except Exception: out.write(traceback.format_exc())
    return out.getvalue().strip()
fail = 0
for i in range(0, len(args), 3):
    want, cwd, cmd = args[i:i + 3]
    out = verdict(cwd, cmd)
    good = (not out or out.startswith("This cargo test is outside the affected set")) if want == "ok" else want in out
    print("ok (%s): %s" % (want, cmd) if good else "FAIL (%s): %s\n  %s" % (want, cmd, out.replace("\n", "\n  ")))
    fail |= not good
sys.exit(fail)
PY
fail=$?

# The hook itself (test-guard.sh, as Claude Code runs it): exit 2 with the reason, or 0 and nothing.
hook() { python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":sys.argv[1]},"cwd":sys.argv[2],"agent_id":"t"}))' "$1" $wt | zsh $here/../test-guard.sh 2>&1 }
out=$(hook "cargo test -p $crate -- bare"); rc=$?
if (( rc == 2 )) && [[ $out == *"isn't a module path"* ]]; then print "ok (hook refuses): cargo test -p $crate -- bare"
else print "FAIL (hook refuses, exit $rc): cargo test -p $crate -- bare\n  $out"; fail=1; fi
out=$(hook "ls"); rc=$?
if (( rc == 0 )) && [[ -z $out ]]; then print "ok (hook passes): ls"; else print "FAIL (hook passes, exit $rc): ls\n  $out"; fail=1; fi
exit $fail
