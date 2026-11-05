#!/bin/sh
# Installs the dev tools app's LaunchAgents on this Mac (the M6): the server on every address, IPv4
# and IPv6 (`--host ::`), port 8077 (no sign-in: the home network and tailnet may use it), and the
# nightly backup of its data to the NAS (backup.sh). Re-run after moving the checkout or Node.
# Usage: install.sh [path/to/node]   (default: `mise which node` in the checkout, mise.toml's pin)
set -eu
usage="usage: tools/devtools/launchd/install.sh [path/to/node]"
case "${1:-}" in
  -h|--help) echo "$usage"; exit 0 ;;
  -*) printf '%s\n%s\n' "install.sh: unexpected $1" "$usage" >&2; exit 2 ;;
esac
[ $# -le 1 ] || { printf '%s\n%s\n' "install.sh: too many arguments" "$usage" >&2; exit 2; }
app=$(cd "$(dirname "$0")/.." && pwd)
node=${1:-$(cd "$app" && mise which node)}
case $node in */shims/*) echo "$node is a shim; pass the real node" >&2; exit 1;; esac
[ -x "$node" ] || { echo "no node at $node" >&2; exit 1; }
"$node" -e 'require("node:sqlite")' 2>/dev/null || { echo "$node lacks node:sqlite (needs 22.5+)" >&2; exit 1; }
agents=$HOME/Library/LaunchAgents
mkdir -p "$agents" "$HOME/Library/Logs/scenic"
domain=gui/$(id -u)
for job in devtools:local.scenic.devtools backup:local.scenic.devtools-backup; do
  src=${job%%:*}
  label=${job#*:}
  plist=$agents/$label.plist
  launchctl bootout "$domain/$label" 2>/dev/null || true
  # bootout returns before the job is gone; a bootstrap before then fails and leaves it unloaded.
  i=0
  while launchctl print "$domain/$label" >/dev/null 2>&1; do
    i=$((i + 1))
    [ $i -le 100 ] || { echo "$label still loaded after 10 s" >&2; exit 1; }
    sleep 0.1
  done
  sed -e "s|@NODE@|$node|g" -e "s|@APP@|$app|g" -e "s|@HOME@|$HOME|g" "$app/launchd/$src.plist.in" > "$plist"
  launchctl bootstrap "$domain" "$plist"
  echo "loaded $label"
done
