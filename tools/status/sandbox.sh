#!/bin/zsh
# A sandboxed Scenic app for testing the app on a Mac whose Scenic is live (task #194): "Scenic
# Sandbox.app" (tools/status/bundle.sh --sandbox) with its own bundle id, launchd labels
# (local.scenic-sandbox.build, local.scenic-sandbox.page) and home, so the live app, services
# (local.scenic.*) and ~/Library/Application Support/scenic are never touched. Its build service
# runs a stand-in agent (it reports a job and quits as the app asks, as crates/pipeline/src/control.rs
# Quit says), each installed version's `scenic` a stand-in answering `scenic menu`, and a stand-in
# build page on port 17399 serves the panel (the app's ScenicPage, never this Mac's live agent's
# port 8090).
#
#   tools/status/sandbox.sh setup <dir>     builds v1 (and v2, for the update) and installs v1 at
#                                           <dir>/Applications, the stand-ins, the page's job
#   tools/status/sandbox.sh update <dir>    points the sandbox's app/current at v2 (the app replaces
#                                           itself within a minute)
#   tools/status/sandbox.sh clean <dir>     the app quit, its services unregistered and unloaded,
#                                           the page's plist and <dir> removed
#
# The stand-in agent: a job while <dir>/home/job exists (its seconds left in it, counting down); no
# job otherwise. Its log: <dir>/home/build.log.
set -euo pipefail
usage="usage: tools/status/sandbox.sh setup|update|clean <dir>"
case $#:${1:-} in
  1:-h|1:--help) print -r -- $usage; exit 0 ;;
  2:setup|2:update|2:clean) ;;
  *) print -ru2 -- "sandbox.sh: unexpected arguments: $*"$'\n'"$usage"; exit 2 ;;
esac
cmd=$1 dir=$2
dir=${dir:A}
home=$dir/home
src=${0:A:h}
page_plist=~/Library/LaunchAgents/local.scenic-sandbox.page.plist
case $cmd in
setup)
  mkdir -p $home/run $home/agent $home/app $home/www $dir/Applications
  for v in v1 v2; do
    mkdir -p $home/app/$v
    zsh $src/bundle.sh $home/app/$v $v - --sandbox $home >/dev/null
    # (The stand-in `scenic menu`: the app's status, its own agent's.)
    print -r -- '#!/bin/sh' "echo '{\"local\": true, \"now\": 0}'" > $home/app/$v/scenic
    chmod +x $home/app/$v/scenic
  done
  # (v2's launcher signed otherwise, with the hardened runtime: another cdhash, as an update that
  # changes the launcher brings, so the update tests the build service registered again.)
  v2="$home/app/v2/Scenic Sandbox.app"
  codesign -s - -i local.scenic-sandbox.launcher -o runtime --force "$v2/Contents/MacOS/scenic-launcher"
  codesign -s - -i local.scenic-sandbox.app --force "$v2"
  ln -sfh v1 $home/app/current
  rm -rf "$dir/Applications/Scenic Sandbox.app"
  ditto "$home/app/v1/Scenic Sandbox.app" "$dir/Applications/Scenic Sandbox.app"
  # The stand-in agent.
  cat > $home/stub-agent.sh <<EOF
#!/bin/zsh
home=$home
echo "stub agent \$\$ started"
while true; do
  now=\$(date +%s)
  q=\$home/agent/quit-request.json
  if [[ -f \$home/job ]]; then
    left=\$(cat \$home/job)
    job="{\"id\": \"unit 6/1/1\", \"what\": \"Building unit 6/1/1\", \"started\": \$now, \"progress\": {\"done\": 1, \"total\": 4, \"unit\": \"parts\", \"eta_s\": \$left}}"
  else
    job=null
  fi
  print -r -- "{\"host\": \"sandbox\", \"beat\": \$now, \"job\": \$job}" > \$home/agent/status.json
  if [[ -f \$q ]]; then
    if grep -q '"now"' \$q; then
      echo "stub agent: quit now: the job given back"; rm -f \$home/job \$q; exit 76
    fi
    if [[ ! -f \$home/job ]]; then
      echo "stub agent: quit, no job running"; rm -f \$q; exit 76
    fi
  fi
  if [[ -f \$home/job ]]; then
    left=\$(( \$(cat \$home/job) - 2 ))
    if (( left <= 0 )); then rm -f \$home/job; echo "stub agent: the job ended"; else print \$left > \$home/job; fi
  fi
  sleep 2
done
EOF
  chmod +x $home/stub-agent.sh
  printf '%s\n' "# scenic: keep (the sandbox's stand-in agent)" $home/stub-agent.sh > $home/run/agent
  # The panel's build page, a stand-in (the app's ScenicPage, never the live agent's 8090), a
  # launchd job of its own.
  mkdir -p $home/www/work
  print -r -- '<!doctype html><meta charset="utf-8"><body style="font: 13px system-ui; padding: 16px">The sandbox'"'"'s build page (a stand-in)</body>' > $home/www/work/index.html
  mkdir -p ${page_plist:h}
  rm -f $page_plist
  plutil -create xml1 $page_plist
  plutil -insert Label -string local.scenic-sandbox.page $page_plist
  plutil -insert ProgramArguments -array $page_plist
  plutil -insert ProgramArguments.0 -string /usr/bin/python3 $page_plist
  plutil -insert ProgramArguments.1 -string -m $page_plist
  plutil -insert ProgramArguments.2 -string http.server $page_plist
  plutil -insert ProgramArguments.3 -string 17399 $page_plist
  plutil -insert ProgramArguments.4 -string --bind $page_plist
  plutil -insert ProgramArguments.5 -string 127.0.0.1 $page_plist
  plutil -insert ProgramArguments.6 -string --directory $page_plist
  plutil -insert ProgramArguments.7 -string $home/www $page_plist
  plutil -insert RunAtLoad -bool true $page_plist
  plutil -insert KeepAlive -bool true $page_plist
  plutil -insert StandardErrorPath -string $home/page.log $page_plist
  launchctl bootout gui/$(id -u)/local.scenic-sandbox.page 2>/dev/null || true
  launchctl bootstrap gui/$(id -u) $page_plist
  echo "open \"$dir/Applications/Scenic Sandbox.app\""
  ;;
check)
  # The transition guard: a copy inside an installed version manages nothing; the installed copy
  # manages nothing while the old agent's login item is there (a stand-in plist, never loaded).
  inside="$home/app/v1/Scenic Sandbox.app/Contents/MacOS/scenic-status"
  installed="$dir/Applications/Scenic Sandbox.app/Contents/MacOS/scenic-status"
  legacy=~/Library/LaunchAgents/local.scenic-sandbox.agent.plist
  fail=0
  "$inside" --plan | tee /dev/stderr | grep -q "manages nothing: run from inside" || fail=1
  "$installed" --plan | tee /dev/stderr | grep -q "manages the build service" || fail=1
  plutil -create xml1 $legacy
  "$installed" --plan | tee /dev/stderr | grep -q "manages nothing: Switch over" || fail=1
  rm -f $legacy
  (( fail == 0 )) && echo "check: ok" || { echo "check: FAILED"; exit 1; }
  ;;
update)
  ln -sfh v2 $home/app/current
  echo "current: v2"
  ;;
clean)
  pkill -f "$dir/Applications/Scenic Sandbox.app/Contents/MacOS/scenic-status" 2>/dev/null || true
  [[ -x "$dir/Applications/Scenic Sandbox.app/Contents/MacOS/scenic-status" ]] && "$dir/Applications/Scenic Sandbox.app/Contents/MacOS/scenic-status" --unregister || true
  launchctl bootout gui/$(id -u)/local.scenic-sandbox.build 2>/dev/null || true
  launchctl bootout gui/$(id -u)/local.scenic-sandbox.page 2>/dev/null || true
  rm -f $page_plist
  rm -rf $dir
  echo "cleaned"
  ;;
esac
