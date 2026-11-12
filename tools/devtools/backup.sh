#!/bin/sh
# Nightly (its LaunchAgent, from tools/app/services.zsh setup devtools): a consistent copy of the dev
# tools app's databases in <data> (the task board's, the dev jobs') to the NAS,
# state/devtools/<name>-<date>.db, the newest 14 of each kept.
#   backup.sh <data folder>
set -eu
data=${1:?usage: backup.sh <data folder>}
dest=/Volumes/personal/projects/scenic-roads/state/devtools
[ -d /Volumes/personal/projects ] || { echo "$(date -u +%FT%TZ) devtools backup: NAS not mounted" >&2; exit 1; }
mkdir -p "$dest"
day=$(date +%F)
tmpd=$(mktemp -d -t devtools)
tmp=$tmpd/copy.db
trap 'rm -rf "$tmpd"; rm -f "$dest"/*.db.part' EXIT
for name in taskboard jobs; do
  [ -f "$data/$name.db" ] || continue
  # A local copy, checked, then one copy: the NAS takes few small writes well (docs/macs.md). VACUUM
  # INTO reads the database in one transaction (.backup starts again at each write by the server).
  rm -f "$tmp"
  /usr/bin/sqlite3 "$data/$name.db" "VACUUM INTO '$tmp'"
  ok=$(/usr/bin/sqlite3 "$tmp" "PRAGMA quick_check")
  [ "$ok" = ok ] || { echo "$(date -u +%FT%TZ) devtools backup: $name: quick_check: $ok" >&2; exit 1; }
  cp -X "$tmp" "$dest/$name-$day.db.part"   # -X: the NAS refuses macOS's provenance xattrs (as publish.sh)
  mv "$dest/$name-$day.db.part" "$dest/$name-$day.db"
  ls -1 "$dest/$name"-*.db | sort -r | tail -n +15 | while read -r old; do rm -f "$old"; done
  echo "$(date -u +%FT%TZ) devtools backup: $dest/$name-$day.db"
done
