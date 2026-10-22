#!/bin/zsh
# Installs the sweep's launchd job (every 20 min) for this Mac: fills local.scenic.sweep.plist's
# @REPO@ with the main checkout's path and loads it. Run once per Mac.
repo=${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}
dst=~/Library/LaunchAgents/local.scenic.sweep.plist
mkdir -p ${dst:h}
sed "s|@REPO@|$repo|" ${0:A:h}/local.scenic.sweep.plist > $dst || exit 1
launchctl bootout gui/$UID/local.scenic.sweep 2>/dev/null
launchctl bootstrap gui/$UID $dst && print "installed $dst (sweep of $repo)"
