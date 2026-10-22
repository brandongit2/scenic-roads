#!/bin/sh
# Nightly: a consistent copy of the board's database to the NAS, the newest 14 kept.
set -eu
board=$(cd "$(dirname "$0")/.." && pwd)
dest=/Volumes/personal/projects/scenic-roads/state/taskboard
[ -d /Volumes/personal/projects ] || { echo "$(date -u +%FT%TZ) taskboard backup: NAS not mounted" >&2; exit 1; }
mkdir -p "$dest"
day=$(date +%F)
# .backup to a local file, then one copy: the NAS takes few small writes well (docs/macs.md).
tmp=$(mktemp -t taskboard)
/usr/bin/sqlite3 "$board/taskboard.db" ".backup '$tmp'"
cp "$tmp" "$dest/$day.db.part"
mv "$dest/$day.db.part" "$dest/$day.db"
rm -f "$tmp"
ls -1 "$dest"/*.db | sort -r | tail -n +15 | while read -r old; do rm -f "$old"; done
echo "$(date -u +%FT%TZ) taskboard backup: $dest/$day.db"
