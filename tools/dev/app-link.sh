#!/bin/zsh
# Makes ~/scenic a link to ~/Library/Application Support/scenic, the app's home (#1579): a path
# without a space for agents' shell commands, which Claude Code's worktree check refuses with
# `Application\ Support` in them. Run by install-sweep.sh, this Mac's dev setup; safe to run again.
# A link it made (to the app's home) is kept; anything else there, a link elsewhere, a file or a
# folder, is left as it is, with a warning. Made atomically: a temporary link, then a rename.
set -uo pipefail
case $#:${1:-} in
  0:) ;;
  1:-h|1:--help) print "usage: tools/dev/app-link.sh   (no arguments)"; exit 0 ;;
  *) print -u2 "app-link.sh takes no arguments: $*"; exit 2 ;;
esac
link=$HOME/scenic app="$HOME/Library/Application Support/scenic"
if [[ -L $link ]]; then
  [[ $(readlink $link) == $app ]] && exit 0
  print -u2 "app-link.sh: WARNING: $link is a link to $(readlink $link), not to $app: left as it is"
  exit 0
fi
if [[ -e $link ]]; then
  print -u2 "app-link.sh: WARNING: $link is there and not a link: left as it is"
  exit 0
fi
tmp=$link.tmp-$$
ln -s $app $tmp || exit 1
# (rename(2): the link itself put in place, never moved into a folder made there since, which it
# refuses.)
python3 -c 'import os, sys; os.rename(sys.argv[1], sys.argv[2])' $tmp $link || { rm -f $tmp; exit 1 }
print "app-link.sh: $link made, a link to $app"
