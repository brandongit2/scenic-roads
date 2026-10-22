#!/bin/sh
# A windowed Chrome (the display's own pacing, 120 Hz) for timing, CDP on 5293, its own profile:
#   sh chrome-win.sh <profile dir>   (a fresh, empty directory for a clean browser)
case "$#:${1:-}" in
  1:-h|1:--help) echo "usage: chrome-win.sh <profile dir>"; exit 0 ;;
  1:-*|1:|0:*|[2-9]*) echo "usage: chrome-win.sh <profile dir>" >&2; exit 2 ;;
esac
exec /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --remote-debugging-port=5293 \
  --user-data-dir="$1" \
  --use-angle=metal --enable-gpu --ignore-gpu-blocklist --no-first-run --no-default-browser-check \
  --window-position=0,0 --window-size=1440,900 about:blank
