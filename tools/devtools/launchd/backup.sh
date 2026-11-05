#!/bin/sh
# Nightly: a consistent copy of the dev tools app's databases (the task board's, the dev jobs') to
# the NAS, state/devtools/<name>-<date>.db, the newest 14 of each kept.
set -eu
app=$(cd "$(dirname "$0")/.." && pwd)
dest=/Volumes/personal/projects/scenic-roads/state/devtools
[ -d /Volumes/personal/projects ] || { echo "$(date -u +%FT%TZ) devtools backup: NAS not mounted" >&2; exit 1; }
mkdir -p "$dest"
day=$(date +%F)
tmp=$(mktemp -t devtools)
trap 'rm -f "$tmp" "$dest"/*.db.part' EXIT
for name in taskboard jobs; do
  [ -f "$app/$name.db" ] || continue
  # .backup to a local file, checked, then one copy: the NAS takes few small writes well (docs/macs.md).
  /usr/bin/sqlite3 "$app/$name.db" ".backup '$tmp'"
  ok=$(/usr/bin/sqlite3 "$tmp" "PRAGMA quick_check")
  [ "$ok" = ok ] || { echo "$(date -u +%FT%TZ) devtools backup: $name: quick_check: $ok" >&2; exit 1; }
  cp -X "$tmp" "$dest/$name-$day.db.part"   # -X: the NAS refuses macOS's provenance xattrs (as publish.sh)
  mv "$dest/$name-$day.db.part" "$dest/$name-$day.db"
  ls -1 "$dest/$name"-*.db | sort -r | tail -n +15 | while read -r old; do rm -f "$old"; done
  echo "$(date -u +%FT%TZ) devtools backup: $dest/$name-$day.db"
done
