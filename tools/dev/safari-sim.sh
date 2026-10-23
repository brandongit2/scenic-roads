#!/bin/zsh
# The build page in Mobile Safari (#614): boots an iOS Simulator, opens each tab from
# touch-test.mjs's read-only server and saves a screenshot a tab, then shuts the simulator and the
# server down (by their own ids). Touch by hand or with the Simulator control tools meanwhile:
# SCENIC_SIM_HOLD=<seconds> waits that long before shutting down.
#   tools/dev/safari-sim.sh [out-dir] [device name]     (default: iPhone 18 Pro)
set -u
here=${0:A:h}
out=${1:-${TMPDIR:-/tmp}/scenic-safari}
dev=${2:-iPhone 18 Pro}
port=8097
mkdir -p "$out"
udid=$(xcrun simctl list devices available | sed -n "s/^ *${dev} (\([0-9A-F-]*\)).*/\1/p" | head -1)
[[ -n $udid ]] || { echo "FAIL no simulator named '$dev' (xcrun simctl list devices)"; exit 1; }

node "$here/touch-test.mjs" --serve --port $port >"$out/server.log" 2>&1 &
srv=$!
booted=0
xcrun simctl list devices | grep -q "$udid) (Booted)" || { xcrun simctl boot "$udid" && booted=1; }
cleanup() { [[ $booted = 1 ]] && xcrun simctl shutdown "$udid"; kill -TERM $srv 2>/dev/null; }
trap cleanup EXIT INT TERM
xcrun simctl bootstatus "$udid" -b >/dev/null
for tab in overview jobs machines regions activity details; do
  xcrun simctl openurl "$udid" "http://localhost:$port/work/?view#$tab"
  sleep ${SCENIC_SIM_WAIT:-6}
  xcrun simctl io "$udid" screenshot "$out/safari-$tab.png" >/dev/null 2>&1 && echo "saved $out/safari-$tab.png"
done
# (Waited on in the background, so a kill of this script shuts down at once, not after the hold.)
sleep ${SCENIC_SIM_HOLD:-0} & wait $!
