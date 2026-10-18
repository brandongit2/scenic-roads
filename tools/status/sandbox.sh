#!/bin/zsh
# A sandboxed Scenic app for testing the app on a Mac whose Scenic is live (task #194): "Scenic
# Sandbox.app" (tools/status/bundle.sh --sandbox) with its own bundle id, launchd labels
# (local.scenic-sandbox.build, local.scenic-sandbox.server) and home, so the live app, services
# (local.scenic.*) and ~/Library/Application Support/scenic are never touched. Its build service
# runs a stand-in agent (it reports a job and quits as the app asks, as crates/pipeline/src/control.rs
# Quit says), its map server a stand-in on port 17399.
#
#   tools/status/sandbox.sh setup <dir>     builds v1 (and v2, for the update) and installs v1 at
#                                           <dir>/Applications, the stand-ins, the map server's job
#   tools/status/sandbox.sh update <dir>    points the sandbox's app/current at v2 (the app replaces
#                                           itself within a minute)
#   tools/status/sandbox.sh clean <dir>     the app quit, its services unregistered and unloaded,
#                                           the map server's plist and <dir> removed
#
# The stand-in agent: a job while <dir>/home/job exists (its seconds left in it, counting down); no
# job otherwise. Its log: <dir>/home/build.log.
set -euo pipefail
cmd=${1:?setup|update|clean} dir=${2:?<dir>}
dir=${dir:A}
home=$dir/home
src=${0:A:h}
server_plist=~/Library/LaunchAgents/local.scenic-sandbox.server.plist
case $cmd in
setup)
  mkdir -p $home/run $home/agent $home/app $home/www/api $dir/Applications
  for v in v1 v2; do
    mkdir -p $home/app/$v
    zsh $src/bundle.sh $home/app/$v $v - --sandbox $home >/dev/null
  done
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
  # The stand-in map server, a launchd job of its own.
  print -r -- '{"local": true, "now": 0}' > $home/www/api/build
  print -r -- 'ok' > $home/www/api/ping
  mkdir -p ${server_plist:h}
  rm -f $server_plist
  plutil -create xml1 $server_plist
  plutil -insert Label -string local.scenic-sandbox.server $server_plist
  plutil -insert ProgramArguments -array $server_plist
  plutil -insert ProgramArguments.0 -string /usr/bin/python3 $server_plist
  plutil -insert ProgramArguments.1 -string -m $server_plist
  plutil -insert ProgramArguments.2 -string http.server $server_plist
  plutil -insert ProgramArguments.3 -string 17399 $server_plist
  plutil -insert ProgramArguments.4 -string --bind $server_plist
  plutil -insert ProgramArguments.5 -string 127.0.0.1 $server_plist
  plutil -insert ProgramArguments.6 -string --directory $server_plist
  plutil -insert ProgramArguments.7 -string $home/www $server_plist
  plutil -insert RunAtLoad -bool true $server_plist
  plutil -insert KeepAlive -bool true $server_plist
  plutil -insert StandardErrorPath -string $home/server.log $server_plist
  launchctl bootout gui/$(id -u)/local.scenic-sandbox.server 2>/dev/null || true
  launchctl bootstrap gui/$(id -u) $server_plist
  echo "open \"$dir/Applications/Scenic Sandbox.app\""
  ;;
update)
  ln -sfh v2 $home/app/current
  echo "current: v2"
  ;;
clean)
  pkill -f "$dir/Applications/Scenic Sandbox.app/Contents/MacOS/scenic-status" 2>/dev/null || true
  [[ -x "$dir/Applications/Scenic Sandbox.app/Contents/MacOS/scenic-status" ]] && "$dir/Applications/Scenic Sandbox.app/Contents/MacOS/scenic-status" --unregister || true
  launchctl bootout gui/$(id -u)/local.scenic-sandbox.build 2>/dev/null || true
  launchctl bootout gui/$(id -u)/local.scenic-sandbox.server 2>/dev/null || true
  rm -f $server_plist
  rm -rf $dir
  echo "cleaned"
  ;;
*) echo "setup|update|clean" >&2; exit 2 ;;
esac
