#!/bin/zsh
# Gives this checkout's web/ its packages (task #893): pnpm install --frozen-lockfile from this Mac's
# shared pnpm store (hard links: seconds, almost no disk), only when web/node_modules is missing or
# installed from another lockfile (pnpm keeps its copy in node_modules/.pnpm/lock.yaml).
#   tools/dev/web-deps.sh            offline: a package missing from the store is an error
#   tools/dev/web-deps.sh --online   fetches what the store lacks (publish.sh)
# check.sh's web part and publish.sh call it; run it by hand before node or vite in a fresh worktree.
set -uo pipefail
mode=--offline
case $#:${1:-} in
  0:) ;;
  1:--online) mode=--prefer-offline ;;
  *) print -u2 "web-deps.sh [--online]"; exit 2 ;;
esac
web=${0:A:h:h:h}/web
cmp -s $web/pnpm-lock.yaml $web/node_modules/.pnpm/lock.yaml && exit 0
cd $web || exit 1
# (mise.toml's pnpm and Node, also from a shell without mise on its PATH, such as ssh's.)
path+=(/opt/homebrew/bin)
eval "$(mise env -s zsh 2>/dev/null)" 2>/dev/null
command -v pnpm >/dev/null || { print -u2 "web-deps: no pnpm (mise.toml pins it: mise install)"; exit 1; }
if ! out=$(pnpm install --frozen-lockfile $mode --reporter=append-only 2>&1); then
  print -r -- $out
  if [[ $mode == --offline ]]; then
    print -u2 "web-deps: pnpm's store ($(pnpm store path 2>/dev/null)) lacks a package web/pnpm-lock.yaml names (above)."
    print -u2 "  Fill it once on this Mac (needs the network): cd $web && pnpm install --frozen-lockfile"
  else
    print -u2 "web-deps: pnpm install failed (above)"
  fi
  exit 1
fi
