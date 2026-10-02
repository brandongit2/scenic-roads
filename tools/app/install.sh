#!/bin/zsh
# Install the published app on this Mac (once; after that the server updates itself): copy
# app/<current version>/ from the NAS, point ~/Library/Application Support/scenic/app/current at it,
# write the launcher's run file for the server, and restart the login item.
set -euo pipefail
NAS=/Volumes/personal/projects/scenic-roads
HOME_S="$HOME/Library/Application Support/scenic"
version=$(python3 -c "import json;print(json.load(open('$NAS/app/current.json'))['version'])")
mkdir -p "$HOME_S/app" "$HOME_S/run"
if [[ ! -d "$HOME_S/app/$version" ]]; then
  rsync -a "$NAS/app/$version/" "$HOME_S/app/$version.tmp/"
  chmod +x "$HOME_S/app/$version.tmp/server"
  mv "$HOME_S/app/$version.tmp" "$HOME_S/app/$version"
fi
# -h: replace the link itself, not something inside the folder it points to.
ln -sfh "$version" "$HOME_S/app/current"
printf '%s\n' "$HOME_S/app/current/server" --web "$HOME_S/app/current/web" --fonts "$HOME_S/app/current/fonts" > "$HOME_S/run/server"
launchctl kickstart -k gui/$(id -u)/local.scenic.server
echo "installed $version; the map is at http://localhost:8080"
