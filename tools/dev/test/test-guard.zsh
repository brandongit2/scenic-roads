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
# Which Mac the guard takes this one for (SCENIC_THIS_MAC, tools/app/test-mac.zsh): the test Mac, but in the cases
# that say -M <name> [<VAR=value>...]: that Mac, with those variables in the hook's own environment (Claude
# Code's, as the PM's session has them; an agent's command can't set them). No PM's marker but the cases' own
# (SCENIC_TESTS_LOCAL_FILE: a file that isn't there, or $marker), whatever this Mac's .claude/tests-local says.
source $here/../../app/test-mac.zsh
marker=$(mktemp); trap 'rm -f $marker' EXIT
export SCENIC_THIS_MAC=$(test_mac_name) SCENIC_TESTS_LOCAL_FILE=$marker.none; unset SCENIC_TESTS_LOCAL SCENIC_CHECK_LOCAL
check() {  # [-C <cwd>] [-M <mac> [VAR=value...]] expectation (ok | a text the refusal has), command; cwd: the worktree
  local cwd=$wt mac=; [[ $1 == -C ]] && { cwd=$2; shift 2 }
  [[ $1 == -M ]] && { mac=$2; shift 2; while [[ $1 == *=* ]]; do mac+=,$1; shift; done; cwd+="@mac=$mac" }
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

# Where the run goes (#1398; agent-rules.md, "Where things run"): a cargo test or nextest run an agent types is
# refused on any Mac but the test Mac, saying what to run instead; the test Mac, the hook's own environment's
# SCENIC_TESTS_LOCAL or SCENIC_CHECK_LOCAL and the PM's marker let it through, and so does the remote runner.
W="Tests run on the test Mac"
for cmd in "cargo test -p $crate -- $f" "cargo nextest run -p $crate $f" "cargo t -p $crate -- $f" "cd $wt && cargo test -p $crate -- $f 2>&1 | tee /p/log" "zsh -c 'cargo test -p $crate -- $f'" "cargo test -p $crate --release -- $f #perf" "cargo test -p $crate #repro" \
    "cargo  test -p $crate -- $f" "command cargo test -p $crate -- $f" "\\cargo test -p $crate -- $f" "~/.cargo/bin/cargo test -p $crate -- $f" "/opt/x/bin/cargo test -p $crate -- $f" \
    "cargo +nightly test -p $crate -- $f" "cargo --locked test -p $crate -- $f" "env FOO=1 cargo test -p $crate -- $f" "time nice -n 5 cargo test -p $crate -- $f" \
    "echo x | xargs cargo test -p $crate -- $f" "(cd /tmp && cargo test -p $crate -- $f)" "cargo-nextest nextest run -p $crate $f" "cargo nextest r -p $crate $f"; do
  check -M another-mac "$W" "$cmd"
  check -M another-mac "tools/dev/check.sh" "$cmd"
  check -M another-mac "tools/app/remote-tests.sh <a key" "$cmd"
  check ok "$cmd"
  check -M another-mac SCENIC_TESTS_LOCAL=1 ok "$cmd"
  check -M another-mac SCENIC_CHECK_LOCAL=1 ok "$cmd"
  check -M another-mac SCENIC_TESTS_LOCAL_FILE=$marker ok "$cmd"
done
check -M another-mac SCENIC_TESTS_LOCAL= "$W" "cargo test -p $crate -- $f"   # an empty value asks for nothing
check -M another-mac ok "cargo test -p $crate --no-run"   # a build
# The override is the PM's: an agent's command that sets it (or names another Mac as this one, or the test Mac)
# or writes the PM's marker is refused, on the test Mac too, saying only the PM sets it.
P="Only the PM lets tests run off the test Mac"
for cmd in "SCENIC_TESTS_LOCAL=1 cargo test -p $crate -- $f" "env SCENIC_CHECK_LOCAL=1 cargo test -p $crate -- $f" "SCENIC_CHECK_LOCAL=1 tools/dev/check.sh" \
    "export SCENIC_TESTS_LOCAL=1; cargo test -p $crate -- $f" "SCENIC_TESTS_LOCAL=1" "typeset -x SCENIC_CHECK_LOCAL=y" "SCENIC_THIS_MAC=Brandons-MacBook-Pro cargo test -p $crate -- $f" \
    "SCENIC_TEST_MAC=x tools/app/remote-tests.sh k . 'cargo test'" "SCENIC_TESTS_LOCAL_FILE=/tmp/m zsh tools/dev/check.sh" "zsh -c 'SCENIC_TESTS_LOCAL=1 tools/dev/check.sh'" \
    "eval SCENIC_TESTS_LOCAL=1 tools/dev/check.sh" "launchctl setenv SCENIC_TESTS_LOCAL 1" "touch $main/.claude/tests-local" "print away > .claude/tests-local" "cd ~/x && echo y >> ../../.claude/tests-local" \
    "cp /tmp/a $main/.claude/tests-local"; do
  check "$P" "$cmd"
  check -M another-mac "$P" "$cmd"
done
# Text that only mentions a run, the override or the marker is no run and sets nothing.
for cmd in 'grep -rn "cargo test" tools/dev' "echo 'cargo test -p x'" 'git commit -m "cargo test now refused off the M4; SCENIC_TESTS_LOCAL=1 is the PM'"'"'s"' \
    'git log --grep "cargo test"' "rg 'cargo nextest run' docs" $'cat > /tmp/x.md <<EOF\nrun cargo test -p x, or SCENIC_TESTS_LOCAL=1 cargo test\nEOF' \
    'grep -n SCENIC_TESTS_LOCAL tools/app/test-mac.zsh' 'ls -la .claude/tests-local' 'cat .claude/tests-local' 'git commit -m "touch .claude/tests-local > tests-local"'; do
  check -M another-mac ok "$cmd"
done
# A run through the remote runner holds its cargo in one quoted argument (never found), as an ssh to the test Mac does.
check -M another-mac ok "tools/dev/check.sh"
check -M another-mac ok "tools/app/remote-tests.sh k $wt 'cargo nextest run -p $crate $f'"
check -M another-mac ok "tools/app/remote-tests.sh k . 'cargo test -p $crate -- $f' 2>&1 | tail -5"
check -M another-mac ok "ssh m4 'cd x && cargo test -p $crate -- $f'"
check -M another-mac ok "cargo clippy -p $crate && cargo build -p $crate && cargo check -p $crate"   # builds and lints aren't test runs
check -M another-mac ok "cargo fmt --check"
# (The checks of a run that is let through still apply: here, a filter that isn't a module path.)
check -M another-mac SCENIC_TESTS_LOCAL=1 "isn't a module path" "cargo test -p $crate -- bare"

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
    cwd, _, spec = cwd.partition("@mac=")   # (a case's Mac and variables: SCENIC_THIS_MAC=<mac>, VAR=value)
    env = dict(os.environ)
    for k, kv in enumerate(spec.split(",") if spec else []):
        if k == 0: os.environ["SCENIC_THIS_MAC"] = kv
        else: os.environ[kv.split("=")[0]] = kv.split("=", 1)[1]
    try: return verdict_in(cwd, cmd)
    finally: os.environ.clear(); os.environ.update(env)
def verdict_in(cwd, cmd):
    sys.stdin, out = io.StringIO(json.dumps({"tool_name": "Bash", "tool_input": {"command": cmd}, "cwd": cwd, "agent_id": "t"})), io.StringIO()
    try:
        with redirect_stdout(out), redirect_stderr(out): exec(code, {"__name__": "__main__", "__file__": guard})
    except SystemExit as e:   # (sys.exit("text") prints the text, as a process would to its stderr)
        if e.code is not None and not isinstance(e.code, int): out.write(str(e.code))
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
# The hook off the test Mac: refused with where tests run; the override lets it through to the checks that follow.
out=$(SCENIC_THIS_MAC=another-mac hook "cargo test -p $crate -- $f"); rc=$?
if (( rc == 2 )) && [[ $out == *"Tests run on the test Mac"*"tools/dev/check.sh"* ]]; then print "ok (hook off the test Mac): cargo test -p $crate -- $f"
else print "FAIL (hook off the test Mac, exit $rc): cargo test -p $crate -- $f\n  $out"; fail=1; fi
out=$(SCENIC_THIS_MAC=another-mac SCENIC_TESTS_LOCAL=1 hook "cargo test -p $crate -- bare"); rc=$?
if (( rc == 2 )) && [[ $out == *"isn't a module path"* ]]; then print "ok (hook, override): cargo test -p $crate -- bare"
else print "FAIL (hook, override, exit $rc): cargo test -p $crate -- bare\n  $out"; fail=1; fi
out=$(hook "ls"); rc=$?
if (( rc == 0 )) && [[ -z $out ]]; then print "ok (hook passes): ls"; else print "FAIL (hook passes, exit $rc): ls\n  $out"; fail=1; fi
# affected-tests.sh failing (here rg off the PATH, as in a plain ssh shell) refuses as that, not as "nothing affected".
out=$(PATH=${(j.:.)${path:#${commands[rg]:h}}} hook "cargo test -p $crate -- $f"); rc=$?
if (( rc == 2 )) && [[ $out == *"affected-tests.sh failed"*"'rg'"* ]]; then print "ok (hook, no rg): cargo test -p $crate -- $f"
else print "FAIL (hook, no rg, exit $rc): cargo test -p $crate -- $f\n  $out"; fail=1; fi
# The PM's Agent briefs (no agent_id): each names the standing rules (tools/dev/agent-rules.md), but
# a brief to an agent that never touches the repo; a subagent's own Agent call isn't checked.
brief() { python3 -c 'import json,sys; ti={"prompt":sys.argv[1]}; ti.update({"subagent_type":sys.argv[2]} if sys.argv[2] else {}); d={"tool_name":"Agent","tool_input":ti}; d.update({"agent_id":"t"} if sys.argv[3] else {}); print(json.dumps(d))' "$1" "$2" "$3" | zsh $here/../test-guard.sh 2>&1 }
for want type agent prompt in \
    refuse "" "" "Look at the pool's logs." \
    refuse general-purpose "" "Fix the docs. Follow tools/dev/testing.md for running tests." \
    pass "" "" "Fix the docs. Follow the standing rules in tools/dev/agent-rules.md." \
    pass web-fetch "" "Read https://example.org/ and summarize it." \
    pass "" t "Look at the pool's logs."; do
  out=$(brief "$prompt" "$type" "$agent"); rc=$?
  if [[ $want == refuse ]]; then
    (( rc == 2 )) && [[ $out == *"tools/dev/agent-rules.md"* ]] && print "ok (brief refused): $prompt" || { print "FAIL (brief refused, exit $rc): $prompt\n  $out"; fail=1 }
  else
    (( rc == 0 )) && [[ -z $out ]] && print "ok (brief passes): $type $prompt" || { print "FAIL (brief passes, exit $rc): $type $prompt\n  $out"; fail=1 }
  fi
done
exit $fail
