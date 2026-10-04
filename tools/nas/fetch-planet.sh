#!/bin/bash
# Fetches the OpenStreetMap planet onto the NAS for the scenic-roads map. DSM's Task Scheduler runs
# this daily as brandontsang. It does nothing unless the newest planet here is six months old, or a
# file named fetch-now sits next to this script. An interrupted download resumes from where it
# stopped; the file is checked against its MD5, then moved to sources/osm/<date>/planet.osm.pbf.
# One run at a time; the log is state/logs/fetch-planet.log.
# The repository's tools/nas/fetch-planet.sh is the source: tools/app/publish.sh copies it to the
# NAS's nas/ folder (docs/plan.md §6, The OSM pass).
set -u
ROOT=/volume1/personal/projects/scenic-roads
LIST=https://planet.openstreetmap.org/pbf/
MIRRORS="https://ftpmirror.your.org/pub/openstreetmap/pbf https://ftp.fau.de/osm-planet/pbf https://ftp5.gwdg.de/pub/misc/openstreetmap/planet.openstreetmap.org/pbf https://planet.openstreetmap.org/pbf"
UA='road-elevations/0.1 (personal offline map)'
mkdir -p "$ROOT/state/logs" "$ROOT/sources/osm"
exec >>"$ROOT/state/logs/fetch-planet.log" 2>&1
log() { echo "$(date '+%F %T') $*"; }

LOCK="$ROOT/state/fetch-planet.lock"
if ! mkdir "$LOCK" 2>/dev/null; then
  if [ -n "$(find "$LOCK" -maxdepth 0 -mmin +2880 2>/dev/null)" ]; then rmdir "$LOCK"; mkdir "$LOCK" || exit 0; else exit 0; fi
fi
trap 'rmdir "$LOCK"' EXIT

newest=$(ls -d "$ROOT"/sources/osm/20[0-9][0-9]-[0-9][0-9]-[0-9][0-9] 2>/dev/null | sort | tail -1)
if [ -n "$newest" ] && [ -f "$newest/planet.osm.pbf" ] && [ ! -e "$ROOT/nas/fetch-now" ]; then
  age=$(( ( $(date +%s) - $(date -d "$(basename "$newest")" +%s) ) / 86400 ))
  [ "$age" -lt 182 ] && exit 0
fi

# The newest dated planet whose MD5 is published (so the file is complete).
name=$(curl -fsS -m 60 -A "$UA" "$LIST" | grep -oE 'planet-[0-9]{6}\.osm\.pbf\.md5' | sort -u | tail -1)
name=${name%.md5}
[ -n "$name" ] || { log "no planet listed at $LIST"; exit 1; }
d=${name#planet-}; d=${d%.osm.pbf}
date="20${d:0:2}-${d:2:2}-${d:4:2}"
if [ -f "$ROOT/sources/osm/$date/planet.osm.pbf" ]; then rm -f "$ROOT/nas/fetch-now"; exit 0; fi

part="$ROOT/sources/osm/$name.part"
for m in $MIRRORS; do
  curl -fsI -m 60 -A "$UA" "$m/$name" >/dev/null 2>&1 || continue
  want=$(curl -fsS -m 60 -A "$UA" "$m/$name.md5" | cut -d' ' -f1)
  [ -n "$want" ] || continue
  log "fetching $name from $m"
  curl -f -sS -L -A "$UA" --retry 20 --retry-delay 60 --retry-all-errors -C - -o "$part" "$m/$name" || { log "download stopped: $?"; continue; }
  got=$(md5sum "$part" | cut -d' ' -f1)
  if [ "$got" != "$want" ]; then log "MD5 mismatch for $name ($got, want $want); starting over"; rm -f "$part"; continue; fi
  mkdir -p "$ROOT/sources/osm/$date"
  mv "$part" "$ROOT/sources/osm/$date/planet.osm.pbf"
  echo "$want  planet.osm.pbf" > "$ROOT/sources/osm/$date/planet.osm.pbf.md5"
  rm -f "$ROOT/nas/fetch-now"
  log "done: sources/osm/$date/planet.osm.pbf"
  exit 0
done
log "no mirror finished $name; will retry next run"
exit 1
