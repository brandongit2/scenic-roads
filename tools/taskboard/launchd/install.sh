#!/bin/sh
# Installs the board's LaunchAgents on this Mac (the M6): the server on 0.0.0.0:8077 (no sign-in:
# the home network and tailnet may use it), and the nightly backup to the NAS (backup.sh).
# Re-run after moving the checkout or Node. Usage: install.sh [path/to/node]
set -eu
board=$(cd "$(dirname "$0")/.." && pwd)
node=${1:-$(command -v node || echo "$HOME/.local/share/mise/installs/node/latest/bin/node")}
[ -x "$node" ] || { echo "no node at $node; pass its path" >&2; exit 1; }
"$node" -e 'require("node:sqlite")' 2>/dev/null || { echo "$node lacks node:sqlite (needs 22.5+)" >&2; exit 1; }
agents=$HOME/Library/LaunchAgents
mkdir -p "$agents" "$HOME/Library/Logs/scenic"
for job in board:local.scenic.taskboard backup:local.scenic.taskboard-backup; do
  src=${job%%:*}
  label=${job#*:}
  plist=$agents/$label.plist
  launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
  sed -e "s|@NODE@|$node|g" -e "s|@BOARD@|$board|g" -e "s|@HOME@|$HOME|g" "$board/launchd/$src.plist.in" > "$plist"
  launchctl bootstrap "gui/$(id -u)" "$plist"
  echo "loaded $label"
done
