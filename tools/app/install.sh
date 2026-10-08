#!/bin/zsh
# Install the published app on this Mac (once; after that the server updates itself): copy
# app/<current version>/ from the NAS, point ~/Library/Application Support/scenic/app/current at it,
# write the launcher's run file for the server, and restart the login item.
#
#   tools/app/install.sh                            the map's server
#   tools/app/install.sh --agent [--seed-cache DIR]  also the build agent (the build Mac only);
#       DIR (today's data/cache: canopy files, the per-vertex elevation cache) moves into the
#       agent's cache, so the first builds reuse it
#   tools/app/install.sh --helper                   also a helper agent (the other Mac: it asks the
#       build Mac's coordinator for the jobs that fit it of terrain, slope, tree cover, units and the
#       landmarks' candidates and peaks, else units' last steps; docs/plan.md §8, Two Macs)
set -euo pipefail
agent=0 helper=0 seed=""
while (( $# )); do
  case $1 in
    --agent) agent=1 ;;
    --helper) helper=1 ;;
    --seed-cache) seed=$2; shift ;;
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
if (( agent && helper )); then
  echo "--agent or --helper, not both" >&2
  exit 2
fi
# The build Mac keeps room for its builds (the OSM pass starts with 80 GB free, the pack cache holds
# the base packs): its mirror fills only what's left past 150 GB. (Apart from the agent's disk room
# target, `scenic room`: the mirror never frees for it, nor the agent for the mirror; docs/plan.md §8.)
reserve=50
if (( agent )); then reserve=150; fi
printf '%s\n' "$HOME_S/app/current/server" --web "$HOME_S/app/current/web" --fonts "$HOME_S/app/current/fonts" --reserve-gb $reserve > "$HOME_S/run/server"
launchctl kickstart -k gui/$(id -u)/local.scenic.server
if (( agent || helper )); then
  if [[ -n $seed ]]; then
    # Same disk: moves are instant. What the agent's cache already has stays.
    mkdir -p "$HOME_S/agent/cache"
    for f in "$seed"/*(N); do
      [[ -e "$HOME_S/agent/cache/${f:t}" ]] || mv "$f" "$HOME_S/agent/cache/"
    done
    echo "seeded the agent's cache from $seed"
  fi
  # The agent's jobs run osmium (Homebrew), Planetiler (Java 21) and the Python steps (uv, wherever
  # the login shell finds it: Homebrew, ~/.local/bin, a mise or asdf shim).
  uv=$(zsh -lc 'command -v uv' 2>/dev/null || true)
  if [[ -z $uv ]]; then
    echo "uv not found: the agent's unit builds need it (https://docs.astral.sh/uv/)" >&2
    exit 1
  fi
  run=("$HOME_S/app/current/scenic" agent)
  if (( helper )); then run+=(--helper); fi
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
  if (( helper )); then
    echo "the helper agent runs; its status: /Volumes/personal/projects/scenic-roads/state/helpers/$(scutil --get LocalHostName 2>/dev/null || hostname -s).json"
  else
    echo "the build agent runs; see: $HOME_S/app/current/scenic status"
  fi
fi
# The menu bar item (tools/status, both Macs): the launcher runs Scenic.app from `current`, under
# its own login item.
"${0:A:h}/../status/install.sh"
echo "installed $version; the map is at http://localhost:8080"
