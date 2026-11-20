#!/bin/zsh
# The PM's list for the next health check (tools/dev/health.sh's first part, #1551):
#   tools/dev/next-check.zsh [<folder>]     the folder holding next-check.md (default: the main checkout's .claude)
# Shows <folder>/next-check.md, then copies it to <folder>/next-check-done/<time>.md (an empty or
# missing list too, as an empty copy); the list stays in place, the PM's to edit. A line starting
# "- " is an item; one starting "- [x] " (or "- [X] ") is an item marked done, "- [ ] " one not.
# Copies older than 2 days are deleted.
# Flagged ("ITEMS GONE"), once, by the check that first sees it: an item of the last copy, not marked
# done there, that the list no longer holds. An item is known by its key: its leading task number
# (`#1505`) when it starts with one, else its first word less trailing punctuation, after any
# checkbox. So "#1505 (live 2342): …" edited to "#1505: …" is the same item. An item is finished by
# marking it done before a check and deleting it after that check, as the output says.
set -uo pipefail
case $#:${1:-} in
  1:-h|1:--help) print "usage: tools/dev/next-check.zsh [<folder>]"; exit 0 ;;
  0:|1:[^-]*) ;;
  *) print -u2 "next-check.zsh: unexpected arguments: $*"; exit 2 ;;
esac
dir=${1:-${$(git -C ${0:A:h} rev-parse --path-format=absolute --git-common-dir):h}/.claude}
list=$dir/next-check.md done=$dir/next-check-done
setopt extendedglob
# key <line>: an item's key (above), or nothing (status 1) for a line that isn't an item.
key() {
  [[ $1 == '- '* ]] || return 1
  local w=${1#- }
  w=${w#\[[ xX]\] }
  if [[ $w == (#b)(\#[0-9]##)* ]]; then print -r -- $match[1]; return 0; fi
  local -a ws=( ${=w} )   # (an array: a lone word's [1] would be its first letter)
  w=${ws[1]:-}
  while [[ $w == *[[:punct:]] ]]; do w=${w%?}; done
  print -r -- $w
}
# done_item <line>: whether it is an item marked done.
done_item() { [[ $1 == '- ['[xX]'] '* ]] }
print "attend to first (.claude/next-check.md; finish an item by marking it \"- [x]\" before a check, then delete it after that check):"
if [[ -s $list ]]; then sed 's/^/  /' $list; else print "  nothing listed"; fi
mkdir -p $done
last=( $done/*.md(NOn[1]) )
if (( ${#last} )); then
  # (Keys as many times as they're there: two items for one task are two.)
  typeset -A now; typeset -a gone
  if [[ -f $list ]]; then
    while IFS= read -r l || [[ -n $l ]]; do k=$(key "$l") && now[$k]=$(( ${now[$k]:-0} + 1 )); done < $list
  fi
  while IFS= read -r l || [[ -n $l ]]; do
    k=$(key "$l") || continue
    if (( ${now[$k]:-0} > 0 )); then now[$k]=$(( ${now[$k]} - 1 ))
    elif ! done_item "$l"; then gone+=( "$l" ); fi
  done < $last[1]
  if (( ${#gone} )); then
    print "  ITEMS GONE since the last copy (${last[1]:t}) without being marked done (- [x]): put them back, or mark them done first  <-- REVIEW"
    print -rl -- "${(@)gone/#/    }"
  fi
fi
# (An empty or missing list is copied too, empty: a list emptied is flagged at one check, not at each.)
{ cat $list 2>/dev/null || : } > $done/$(date +%Y%m%d-%H%M%S).md
find $done -name '*.md' -mtime +2 -delete 2>/dev/null
exit 0
