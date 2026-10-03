#!/bin/zsh
# Install the published app on this Mac (once; after that the server updates itself): copy
# app/<current version>/ from the NAS, point ~/Library/Application Support/scenic/app/current at it,
# write the launcher's run file for the server, and restart the login item.
#
#   tools/app/install.sh           the map's server
#   tools/app/install.sh --agent   also the build agent (the build Mac only)
set -euo pipefail
NAS=/Volumes/personal/projects/scenic-roads
HOME_S="$HOME/Library/Application Support/scenic"
version=$(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")
mkdir -p "$HOME_S/app" "$HOME_S/run"
if [[ ! -d "$HOME_S/app/$version" ]]; then
  rsync -a "$NAS/app/$version/" "$HOME_S/app/$version.tmp/"
  for b in server scenic scenic-build extract tile scenic-metrics; do
    [[ -f "$HOME_S/app/$version.tmp/$b" ]] && chmod +x "$HOME_S/app/$version.tmp/$b"
  done
  mv "$HOME_S/app/$version.tmp" "$HOME_S/app/$version"
fi
# -h: replace the link itself, not something inside the folder it points to.
ln -sfh "$version" "$HOME_S/app/current"
printf '%s\n' "$HOME_S/app/current/server" --web "$HOME_S/app/current/web" --fonts "$HOME_S/app/current/fonts" > "$HOME_S/run/server"
launchctl kickstart -k gui/$(id -u)/local.scenic.server
if [[ ${1:-} == --agent ]]; then
  # The agent's jobs run osmium (Homebrew), Planetiler (Java 21) and the Python steps (uv, wherever
  # the login shell finds it: Homebrew, ~/.local/bin, a mise or asdf shim).
  uv=$(zsh -lc 'command -v uv' 2>/dev/null || true)
  if [[ -z $uv ]]; then
    echo "uv not found: the agent's unit builds need it (https://docs.astral.sh/uv/)" >&2
    exit 1
  fi
  printf '%s\n' /usr/bin/env "PATH=${uv:h}:/opt/homebrew/opt/openjdk@21/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    "$HOME_S/app/current/scenic" agent > "$HOME_S/run/agent"
  launchctl kickstart -k gui/$(id -u)/local.scenic.agent
  echo "the build agent runs; see: $HOME_S/app/current/scenic status"
fi
echo "installed $version; the map is at http://localhost:8080"
