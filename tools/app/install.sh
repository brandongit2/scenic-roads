#!/bin/zsh
# Install the published app on this Mac (once; after that it updates itself): copy app/<current
# version>/ from the NAS, point ~/Library/Application Support/scenic/app/current at it, set up the
# map server (its run file; its launchd job restarted), and copy Scenic.app to /Applications.
# Opening Scenic then starts this Mac's build service (its agent, a member of the pool:
# docs/pool.md), which it registers as its own login item; Scenic replaces itself on each update.
# The menu bar item's and the agent's login items from before Scenic was an app
# (local.scenic.status, local.scenic.agent) are removed.
#
#   tools/app/install.sh
set -euo pipefail
(( $# == 0 )) || { echo "tools/app/install.sh takes no options" >&2; exit 2; }
NAS=/Volumes/personal/projects/scenic-roads
HOME_S="$HOME/Library/Application Support/scenic"
version=$(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")
mkdir -p "$HOME_S/app" "$HOME_S/run" "$HOME/Library/Logs/scenic"
if [[ ! -d "$HOME_S/app/$version" ]]; then
  rsync -a "$NAS/app/$version/" "$HOME_S/app/$version.tmp/"
  for b in server scenic scenic-build extract tile scenic-metrics elev areaflags landcover railfreq trees; do
    if [[ -f "$HOME_S/app/$version.tmp/$b" ]]; then chmod +x "$HOME_S/app/$version.tmp/$b"; fi
  done
  mv "$HOME_S/app/$version.tmp" "$HOME_S/app/$version"
fi
# -h: replace the link itself, not something inside the folder it points to.
ln -sfh "$version" "$HOME_S/app/current"
# The map server: a service of its own (Scenic shows it, and starts or stops it when asked).
# The mirror's reserve (the free space downloads leave) is the Regions panel's setting, kept in the
# app's folder (reserve.json; 30 GB unless set), not the run file's.
printf '%s\n' "$HOME_S/app/current/server" --web "$HOME_S/app/current/web" --fonts "$HOME_S/app/current/fonts" > "$HOME_S/run/server"
launchctl kickstart -k gui/$(id -u)/local.scenic.server
# The login items Scenic replaces: stopped and removed.
for old in local.scenic.status local.scenic.agent; do
  if [[ -f "$HOME/Library/LaunchAgents/$old.plist" ]]; then
    launchctl bootout gui/$(id -u)/$old 2>/dev/null || true
    rm -f "$HOME/Library/LaunchAgents/$old.plist"
    echo "removed the old $old login item"
  fi
done
# The app.
rm -rf /Applications/Scenic.app
ditto "$HOME_S/app/current/Scenic.app" /Applications/Scenic.app
open /Applications/Scenic.app
echo "installed $version; Scenic is open (its build service starting); the map is at http://localhost:7311"
