#!/bin/sh
# A windowed Chrome (the display's own pacing, 120 Hz) for timing, CDP on 5293, its own profile.
exec /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --remote-debugging-port=5293 \
  --user-data-dir=/private/tmp/claude-501/-Users-brandontsang-repos-scenic-roads/10eba93e-3572-4e06-9dab-eecc1cf564b8/scratchpad/chrome-win \
  --use-angle=metal --enable-gpu --ignore-gpu-blocklist --no-first-run --no-default-browser-check \
  --window-position=0,0 --window-size=1440,900 about:blank
