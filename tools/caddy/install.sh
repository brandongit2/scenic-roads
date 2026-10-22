#!/bin/sh
# Installs Caddy's LaunchAgent on this Mac (the M6): https://<name> for the tailnet, with the owner's
# certificate (acme.sh renews it and restarts the agent), proxying / to the map (:7311), /build to
# the build page (the agent, :8090) and /task-board to the board (:8077); the Caddyfile beside this
# script, log ~/Library/Logs/scenic/caddy.log. It also gives the map's server and the agent the name
# (~/.config/scenic/names, pipeline::net::names). Re-run after moving the checkout, Caddy or the
# certificate. Needs `brew install caddy`, and `tailscale serve` off port 443 (docs/macs.md).
# Usage: install.sh [name] [cert] [key]
#   (default map.brandontsang.net ~/certs/map.crt ~/certs/map.key)
set -eu
usage="usage: tools/caddy/install.sh [name] [cert] [key]"
case "${1:-}" in
  -h|--help) echo "$usage"; exit 0 ;;
  -*) printf '%s\n%s\n' "install.sh: unexpected $1" "$usage" >&2; exit 2 ;;
esac
[ $# -le 3 ] || { printf '%s\n%s\n' "install.sh: too many arguments" "$usage" >&2; exit 2; }
dir=$(cd "$(dirname "$0")" && pwd)
name=${1:-map.brandontsang.net}
cert=${2:-$HOME/certs/map.crt}
key=${3:-$HOME/certs/map.key}
caddy=$(command -v caddy || echo /opt/homebrew/bin/caddy)
[ -x "$caddy" ] || { echo "no caddy (brew install caddy)" >&2; exit 1; }
[ -r "$cert" ] && [ -r "$key" ] || { echo "can't read $cert and $key" >&2; exit 1; }
SCENIC_MAP_NAME=$name SCENIC_CERT=$cert SCENIC_KEY=$key "$caddy" validate --config "$dir/Caddyfile" --adapter caddyfile >/dev/null

mkdir -p "$HOME/.config/scenic"
printf '# Written by tools/caddy/install.sh: the names Caddy serves this Mac under.\n%s\n' "$name" > "$HOME/.config/scenic/names"

agents=$HOME/Library/LaunchAgents
mkdir -p "$agents" "$HOME/Library/Logs/scenic"
domain=gui/$(id -u)
label=local.scenic.caddy
plist=$agents/$label.plist
launchctl bootout "$domain/$label" 2>/dev/null || true
# bootout returns before the job is gone; a bootstrap before then fails and leaves it unloaded.
i=0
while launchctl print "$domain/$label" >/dev/null 2>&1; do
  i=$((i + 1))
  [ $i -le 100 ] || { echo "$label still loaded after 10 s" >&2; exit 1; }
  sleep 0.1
done
sed -e "s|@CADDY@|$caddy|g" -e "s|@DIR@|$dir|g" -e "s|@NAME@|$name|g" -e "s|@CERT@|$cert|g" \
  -e "s|@KEY@|$key|g" -e "s|@HOME@|$HOME|g" "$dir/caddy.plist.in" > "$plist"
launchctl bootstrap "$domain" "$plist"
echo "loaded $label: https://$name"
