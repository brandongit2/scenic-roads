#!/bin/zsh
# Builds Scenic.app (tools/status: the menu bar item, the app's window, its build service and the map
# server's controls) into <folder>: the app (main.swift, service.swift), its launcher (tools/launcher
# /launcher.c, which runs the build service), the build service's launchd plist (build-service.plist,
# registered by the app with SMAppService), the version it's published as (Contents/Resources/
# version: the app replaces itself when the installed version's differs), signed with <identity>
# ("-": ad hoc). tools/app/publish.sh runs it for every publish.
#
#   tools/status/bundle.sh <folder> <version> <identity>
#   tools/status/bundle.sh <folder> <version> <identity> --sandbox <home>
#
# --sandbox builds "Scenic Sandbox.app", for testing on a Mac whose Scenic is live: its own bundle id
# (local.scenic-sandbox.app), build service (local.scenic-sandbox.build) and map server
# (local.scenic-sandbox.server) labels, its own home in place of ~/Library/Application Support/scenic
# (its run files, its agent folder, its installed versions), its server's port 17399, never added
# to the login items. It never touches the live app, services or home. (tools/status/sandbox.sh)
set -euo pipefail
src=${0:A:h}
out=$1 version=$2 sign_id=$3
sandbox=
if [[ ${4:-} == --sandbox ]]; then sandbox=${5:?--sandbox <home>}; fi
name=Scenic id=local.scenic.status label=local.scenic.build
if [[ -n $sandbox ]]; then name="Scenic Sandbox" id=local.scenic-sandbox.app label=local.scenic-sandbox.build; fi
app="$out/$name.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Library/LaunchAgents"
swiftc -O -swift-version 5 -target arm64-apple-macos26.0 -o "$app/Contents/MacOS/scenic-status" "$src/main.swift" "$src/service.swift"
xcrun clang -O2 -Wall -target arm64-apple-macos26.0 -o "$app/Contents/MacOS/scenic-launcher" "$src/../launcher/launcher.c"
cp -X "$src/Info.plist" "$app/Contents/Info.plist"
info="$app/Contents/Info.plist"
plist="$app/Contents/Library/LaunchAgents/$label.plist"
sed -e "s/LABEL/$label/" -e "s/BUNDLE_ID/$id/" "$src/build-service.plist" > "$plist"
if [[ -n $sandbox ]]; then
  plutil -replace CFBundleIdentifier -string $id "$info"
  plutil -replace CFBundleName -string "$name" "$info"
  plutil -replace CFBundleDisplayName -string "$name" "$info"
  plutil -replace ScenicBuildService -string $label "$info"
  plutil -replace ScenicServerLabel -string local.scenic-sandbox.server "$info"
  plutil -replace ScenicUpdateFrom -string "$name.app" "$info"
  plutil -replace ScenicOpenAtLogin -bool false "$info"
  plutil -insert ScenicLegacyAgent -string local.scenic-sandbox.agent "$info"
  plutil -insert ScenicHome -string "$sandbox" "$info"
  plutil -insert ScenicRunDir -string "$sandbox/run" "$info"
  plutil -insert ScenicServer -string http://127.0.0.1:17399 "$info"
  plutil -replace EnvironmentVariables.SCENIC_LAUNCHER_LOG -string "$sandbox/build.log" "$plist"
  plutil -insert EnvironmentVariables.SCENIC_RUN_DIR -string "$sandbox/run" "$plist"
fi
plutil -lint -s "$info" "$plist"
print -r -- $version > "$app/Contents/Resources/version"
xattr -cr "$app"
# (The launcher and the app under fixed identifiers: what the owner allows them, the NAS among it,
# outlives an update when they're signed with the owner's certificate.)
codesign -s "$sign_id" -i ${id%.app}.launcher --force "$app/Contents/MacOS/scenic-launcher"
codesign -s "$sign_id" -i $id --force "$app"
print -r -- "$app"
