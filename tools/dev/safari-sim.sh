#!/bin/zsh
# The build page in Mobile Safari (#614): boots an iOS Simulator, opens each tab from
# touch-test.mjs's read-only server and saves a screenshot a tab, then shuts the simulator and the
# server down (by their own ids). Touch by hand or with the Simulator control tools meanwhile:
# SCENIC_SIM_HOLD=<seconds> waits that long before shutting down; SCENIC_SIM_PORT=<n> serves on another
# port than 8097 (another agent's run holding it).
#   tools/dev/safari-sim.sh [out-dir] [device name]     (default: iPhone 18 Pro)
set -u
case ${1:-} in
  -h|--help) print "usage: tools/dev/safari-sim.sh [out-dir] [device name]"; exit 0 ;;
  -*) print -u2 "safari-sim.sh takes no options: $1 (--help)"; exit 2 ;;
esac
(( $# <= 2 )) || { print -u2 "safari-sim.sh takes at most two arguments (--help)"; exit 2 }
here=${0:A:h}
out=${1:-${TMPDIR:-/tmp}/scenic-safari}
dev=${2:-iPhone 18 Pro}
port=${SCENIC_SIM_PORT:-8097}
mkdir -p "$out"
udid=$(xcrun simctl list devices available | sed -n "s/^ *${dev} (\([0-9A-F-]*\)).*/\1/p" | head -1)
[[ -n $udid ]] || { echo "FAIL no simulator named '$dev' (xcrun simctl list devices)"; exit 1; }

# (Set before anything starts, so every exit, an error, Ctrl-C or a kill, stops what did; once.)
srv= booted=0
cleanup() {
  [[ $booted = 1 ]] && xcrun simctl shutdown "$udid"; booted=0
  [[ -n $srv ]] && kill -TERM $srv 2>/dev/null; srv=
}
trap cleanup EXIT
trap 'exit 130' INT TERM
node "$here/touch-test.mjs" --serve --port $port >"$out/server.log" 2>&1 &
srv=$!
# (A port already held, someone else's server: their page would be the one shown.)
sleep 1; kill -0 $srv 2>/dev/null || { srv=; echo "FAIL the server didn't start on $port ($out/server.log); SCENIC_SIM_PORT=<n> for another"; exit 1; }
xcrun simctl list devices | grep -q "$udid) (Booted)" || { booted=1; xcrun simctl boot "$udid" || exit 1; }
xcrun simctl bootstatus "$udid" -b >/dev/null
for tab in overview jobs machines regions nas dropboxes activity details; do
  xcrun simctl openurl "$udid" "http://localhost:$port/work/?view#$tab"
  sleep ${SCENIC_SIM_WAIT:-6}
  xcrun simctl io "$udid" screenshot "$out/safari-$tab.png" >/dev/null 2>&1 && echo "saved $out/safari-$tab.png"
done
# (Waited on in the background, so a kill of this script shuts down at once, not after the hold.)
sleep ${SCENIC_SIM_HOLD:-0} & wait $!
