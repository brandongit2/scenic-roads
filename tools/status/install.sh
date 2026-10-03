#!/bin/zsh
# The menu bar item's login item (once per Mac; tools/app/install.sh runs it): the launcher runs
# Scenic.app from the installed app's `current` (~/Library/LaunchAgents/local.scenic.status.plist,
# run/status), so each published app's item takes over when it's installed.
set -euo pipefail
HOME_S="$HOME/Library/Application Support/scenic"
[[ -x "$HOME_S/app/current/Scenic.app/Contents/MacOS/scenic-status" ]] || { echo "the installed app has no Scenic.app yet"; exit 1; }
mkdir -p "$HOME_S/run" "$HOME/Library/Logs/scenic" "$HOME/Library/LaunchAgents"
printf '%s\n' "$HOME_S/app/current/Scenic.app/Contents/MacOS/scenic-status" > "$HOME_S/run/status"
plist="$HOME/Library/LaunchAgents/local.scenic.status.plist"
if [[ -f $plist ]]; then
  launchctl kickstart -k gui/$(id -u)/local.scenic.status
  echo "restarted the menu bar item"
  exit 0
fi
plutil -create xml1 "$plist"
plutil -insert Label -string local.scenic.status "$plist"
plutil -insert ProgramArguments -array "$plist"
plutil -insert ProgramArguments.0 -string "$HOME_S/bin/scenic-launcher" "$plist"
plutil -insert ProgramArguments.1 -string status "$plist"
plutil -insert RunAtLoad -bool true "$plist"
plutil -insert KeepAlive -bool true "$plist"
plutil -insert ProcessType -string Interactive "$plist"
plutil -insert LimitLoadToSessionType -string Aqua "$plist"
plutil -insert StandardOutPath -string "$HOME/Library/Logs/scenic/status.log" "$plist"
plutil -insert StandardErrorPath -string "$HOME/Library/Logs/scenic/status.log" "$plist"
launchctl bootstrap gui/$(id -u) "$plist"
echo "the menu bar item runs (it asks once to send notifications)"
