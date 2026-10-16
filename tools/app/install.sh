#!/bin/zsh
# Install the published app on this Mac (once; after that the server updates itself): copy
# app/<current version>/ from the NAS, point ~/Library/Application Support/scenic/app/current at it,
# write the launcher's run files for the server and the agent, and restart their login items.
# Every Mac's agent is a member of the pool, which leads or works as the terms say (docs/pool.md).
#
#   tools/app/install.sh
set -euo pipefail
while (( $# )); do
  case $1 in
    # (Options of before the pool was always on: every Mac runs the agent now.)
    --agent|--helper) echo "($1 is no longer needed: every Mac runs the agent)" >&2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
NAS=/Volumes/personal/projects/scenic-roads
HOME_S="$HOME/Library/Application Support/scenic"
version=$(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")
mkdir -p "$HOME_S/app" "$HOME_S/run"
if [[ ! -d "$HOME_S/app/$version" ]]; then
  rsync -a "$NAS/app/$version/" "$HOME_S/app/$version.tmp/"
  for b in server scenic scenic-build extract tile scenic-metrics elev areaflags landcover railfreq trees; do
    if [[ -f "$HOME_S/app/$version.tmp/$b" ]]; then chmod +x "$HOME_S/app/$version.tmp/$b"; fi
  done
  mv "$HOME_S/app/$version.tmp" "$HOME_S/app/$version"
fi
# -h: replace the link itself, not something inside the folder it points to.
ln -sfh "$version" "$HOME_S/app/current"
# The mirror's reserve (the free space downloads leave) is the Regions panel's setting, kept in the
# app's folder (reserve.json; 30 GB unless set), not the run file's.
printf '%s\n' "$HOME_S/app/current/server" --web "$HOME_S/app/current/web" --fonts "$HOME_S/app/current/fonts" > "$HOME_S/run/server"
launchctl kickstart -k gui/$(id -u)/local.scenic.server
{
  # The agent's jobs run osmium (Homebrew), Planetiler (Java 21) and the Python steps (uv, wherever
  # the login shell finds it: Homebrew, ~/.local/bin, a mise or asdf shim).
  uv=$(zsh -lc 'command -v uv' 2>/dev/null || true)
  if [[ -z $uv ]]; then
    echo "uv not found: the agent's unit builds need it (https://docs.astral.sh/uv/)" >&2
    exit 1
  fi
  run=("$HOME_S/app/current/scenic" agent)
  printf '%s\n' /usr/bin/env "PATH=${uv:h}:/opt/homebrew/opt/openjdk@21/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    "${run[@]}" > "$HOME_S/run/agent"
  plist="$HOME/Library/LaunchAgents/local.scenic.agent.plist"
  if [[ -f $plist ]]; then
    launchctl kickstart -k gui/$(id -u)/local.scenic.agent
  else
    mkdir -p "$HOME/Library/Logs/scenic" "$HOME/Library/LaunchAgents"
    plutil -create xml1 "$plist"
    plutil -insert Label -string local.scenic.agent "$plist"
    plutil -insert ProgramArguments -array "$plist"
    plutil -insert ProgramArguments.0 -string "$HOME_S/bin/scenic-launcher" "$plist"
    plutil -insert ProgramArguments.1 -string agent "$plist"
    plutil -insert RunAtLoad -bool true "$plist"
    plutil -insert KeepAlive -bool true "$plist"
    plutil -insert ProcessType -string Standard "$plist"
    plutil -insert StandardOutPath -string "$HOME/Library/Logs/scenic/agent.log" "$plist"
    plutil -insert StandardErrorPath -string "$HOME/Library/Logs/scenic/agent.log" "$plist"
    launchctl bootstrap gui/$(id -u) "$plist"
  fi
  echo "the agent runs; see: $HOME_S/app/current/scenic status (and scenic pool status)"
}
# The menu bar item (tools/status, both Macs): the launcher runs Scenic.app from `current`, under
# its own login item.
"${0:A:h}/../status/install.sh"
echo "installed $version; the map is at http://localhost:7311"
