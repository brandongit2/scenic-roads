#!/bin/zsh
# Install the published app on this Mac (once; after that it updates itself): copy app/<current
# version>/ from the NAS, point ~/Library/Application Support/scenic/app/current at it, set up the
# map server (its run file; its launchd job started), and copy Scenic.app to /Applications.
# Opening Scenic then starts this Mac's build service (its agent, a member of the pool:
# docs/pool.md), which it registers as its own login item; Scenic replaces itself on each update.
# The menu bar item's and the agent's login items from before Scenic was an app
# (local.scenic.status, local.scenic.agent) are removed: the switch-over (docs/plan.md §8, Scenic).
#
#   tools/app/install.sh            the app (the switch-over, on a Mac that ran the login items)
#   tools/app/install.sh --legacy   the login items as before Scenic was an app, Scenic.app
#                                   unregistered and removed: the way back, until both Macs have
#                                   switched over
set -euo pipefail
legacy=0
case ${1:-} in
  "") ;;
  --legacy) legacy=1 ;;
  *) echo "tools/app/install.sh [--legacy]" >&2; exit 2 ;;
esac
NAS=/Volumes/personal/projects/scenic-roads
HOME_S="$HOME/Library/Application Support/scenic"
uid=$(id -u)
# The agent's jobs run osmium (Homebrew), Planetiler (Java 21) and the Python steps (uv, wherever
# the login shell finds it: Homebrew, ~/.local/bin, a mise or asdf shim). Scenic writes the agent's
# run file with it when it opens; checked here so a Mac without it says so now.
uv=$(zsh -lc 'command -v uv' 2>/dev/null || true)
if [[ -z $uv ]]; then
  echo "uv not found: the agent's unit builds need it (https://docs.astral.sh/uv/)" >&2
  exit 1
fi
version=$(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")
mkdir -p "$HOME_S/app" "$HOME_S/run" "$HOME/Library/Logs/scenic" "$HOME/Library/LaunchAgents"
if [[ ! -d "$HOME_S/app/$version" ]]; then
  rsync -a "$NAS/app/$version/" "$HOME_S/app/$version.tmp/"
  for b in server scenic scenic-build extract tile scenic-metrics elev areaflags landcover railfreq trees; do
    if [[ -f "$HOME_S/app/$version.tmp/$b" ]]; then chmod +x "$HOME_S/app/$version.tmp/$b"; fi
  done
  mv "$HOME_S/app/$version.tmp" "$HOME_S/app/$version"
fi
# -h: replace the link itself, not something inside the folder it points to.
ln -sfh "$version" "$HOME_S/app/current"
# The map server: a service of its own (Scenic shows it, and starts or stops it when asked): its
# run file, its job restarted, or loaded if it was stopped (Scenic's Stop unloads it).
# The mirror's reserve (the free space downloads leave) is the Regions panel's setting, kept in the
# app's folder (reserve.json; 30 GB unless set), not the run file's.
printf '%s\n' "$HOME_S/app/current/server" --web "$HOME_S/app/current/web" --fonts "$HOME_S/app/current/fonts" > "$HOME_S/run/server"
server_plist="$HOME/Library/LaunchAgents/local.scenic.server.plist"
if launchctl print gui/$uid/local.scenic.server >/dev/null 2>&1; then
  launchctl kickstart -k gui/$uid/local.scenic.server
elif [[ -f $server_plist ]]; then
  launchctl bootstrap gui/$uid "$server_plist"
else
  echo "no map server job ($server_plist): it was set up when this Mac was first installed" >&2
fi
# A running Scenic ended before it's replaced or removed (SIGTERM: it quits without asking, its build
# service left as it is).
pkill -f /Applications/Scenic.app/Contents/MacOS/scenic-status 2>/dev/null || true
if (( legacy )); then
  # The way back: Scenic's build service and login item unregistered, the app removed, and the two
  # login items as they were (the launcher running the agent, and the menu bar item from `current`).
  if [[ -x /Applications/Scenic.app/Contents/MacOS/scenic-status ]]; then
    /Applications/Scenic.app/Contents/MacOS/scenic-status --unregister || true
    rm -rf /Applications/Scenic.app
  fi
  rm -f "$HOME_S/agent/quit-request.json"
  printf '%s\n' /usr/bin/env "PATH=${uv:h}:/opt/homebrew/opt/openjdk@21/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    "$HOME_S/app/current/scenic" agent > "$HOME_S/run/agent"
  printf '%s\n' "$HOME_S/app/current/Scenic.app/Contents/MacOS/scenic-status" > "$HOME_S/run/status"
  for name in agent status; do
    plist="$HOME/Library/LaunchAgents/local.scenic.$name.plist"
    if [[ ! -f $plist ]]; then
      plutil -create xml1 "$plist"
      plutil -insert Label -string local.scenic.$name "$plist"
      plutil -insert ProgramArguments -array "$plist"
      plutil -insert ProgramArguments.0 -string "$HOME_S/bin/scenic-launcher" "$plist"
      plutil -insert ProgramArguments.1 -string $name "$plist"
      plutil -insert RunAtLoad -bool true "$plist"
      plutil -insert KeepAlive -bool true "$plist"
      if [[ $name == agent ]]; then
        plutil -insert ProcessType -string Standard "$plist"
      else
        plutil -insert ProcessType -string Interactive "$plist"
        plutil -insert LimitLoadToSessionType -string Aqua "$plist"
      fi
      plutil -insert StandardOutPath -string "$HOME/Library/Logs/scenic/$name.log" "$plist"
      plutil -insert StandardErrorPath -string "$HOME/Library/Logs/scenic/$name.log" "$plist"
    fi
    launchctl bootout gui/$uid/local.scenic.$name 2>/dev/null || true
    launchctl bootstrap gui/$uid "$plist"
  done
  echo "installed $version the old way: the agent and the menu bar item as login items"
  exit 0
fi
# The login items Scenic replaces: stopped and removed (a job the old agent runs given back).
for old in local.scenic.status local.scenic.agent; do
  if [[ -f "$HOME/Library/LaunchAgents/$old.plist" ]] || launchctl print gui/$uid/$old >/dev/null 2>&1; then
    launchctl bootout gui/$uid/$old 2>/dev/null || true
    rm -f "$HOME/Library/LaunchAgents/$old.plist"
    echo "removed the old $old login item"
  fi
done
# The app.
rm -rf /Applications/Scenic.app
ditto "$HOME_S/app/current/Scenic.app" /Applications/Scenic.app
open /Applications/Scenic.app
echo "installed $version; Scenic is open (its build service starting); the map is at http://localhost:7311"
