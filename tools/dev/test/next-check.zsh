#!/bin/zsh
# Tests of tools/dev/next-check.zsh (health.sh's list step, #1551) on a scratch folder: the list stays
# in place and is copied; an item appended keeps the others; items gone unmarked are flagged, once;
# items marked done, or edited past their first word, aren't. Run by check.sh when it changed:
#   zsh tools/dev/test/next-check.zsh
set -u
here=${0:A:h}
d=$(mktemp -d); trap 'rm -rf $d' EXIT
fail=0
ok() { print "PASS  $1" }
no() { print "FAIL  $1:\n$2"; fail=1 }
step() { zsh $here/../next-check.zsh $d 2>&1 }
newest() { local f=( $d/next-check-done/*.md(On[1]) ); print -r -- "$(<$f)" }
copies() { print -l $d/next-check-done/*.md(N) | wc -l | tr -d ' ' }
age() { local f; for f in $d/next-check-done/*.md(N); do touch -t $(date -v-1M +%Y%m%d%H%M) $f; mv $f ${f:h}/0-${f:t}; done }  # (older copies sort first)

out=$(step)
[[ $out == *"nothing listed"* && $out != *GONE* ]] && ok "no list, no copy before it" || no "no list, no copy before it" "$out"
age
print -l -- '- #1 first: watch a' '- #2 second' '- #3 third' > $d/next-check.md
out=$(step)
[[ -f $d/next-check.md && $out == *"  - #2 second"* && $out != *GONE* ]] && ok "the list shown and left in place" || no "the list shown and left in place" "$out"
[[ $(newest) == $(<$d/next-check.md) ]] && ok "copied whole" || no "copied whole" "$(ls $d/next-check-done)"
age
print -r -- '- #4 appended' >> $d/next-check.md
out=$(step)
[[ $out != *GONE* && $(newest) == *'#1 first'*'#4 appended'* ]] && ok "a line appended keeps the earlier items" || no "a line appended keeps the earlier items" "$out"
age
# #1 edited past its key, #2 marked done, #3 and #4 dropped unmarked.
print -l -- '- #1 first: watch a, then b' '- [x] #2 second' > $d/next-check.md
out=$(step)
[[ $out == *"ITEMS GONE"*"    - #3 third"* && $out != *"    - #1"* && $out != *"    - #2"* && $out == *"    - #4 appended"* ]] \
  && ok "a list shrunk unmarked is flagged, by the items gone" || no "a list shrunk unmarked is flagged, by the items gone" "$out"
age
out=$(step)
[[ $out != *GONE* ]] && ok "flagged once" || no "flagged once" "$out"
age
# Marked done at the last copy, then deleted: not flagged; the list emptied: what was open, flagged.
print -l -- '- #1 first: watch a, then b' > $d/next-check.md
out=$(step)
[[ $out != *GONE* ]] && ok "an item marked done, then deleted" || no "an item marked done, then deleted" "$out"
age
: > $d/next-check.md
out=$(step)
[[ $out == *"nothing listed"*"ITEMS GONE"*"    - #1 first"* ]] && ok "a list emptied" || no "a list emptied" "$out"
age
out=$(step)
[[ $out != *GONE* ]] && ok "an empty list after an empty copy" || no "an empty list after an empty copy" "$out"
# Two items for one task: one gone is flagged.
age
print -l -- '- #5 a' '- #5 b' > $d/next-check.md; step >/dev/null; age
print -l -- '- #5 b' > $d/next-check.md
out=$(step)
[[ $out == *"ITEMS GONE"*"    - #5"* ]] && ok "one of two items for a task gone" || no "one of two items for a task gone" "$out"
# Copies older than 2 days go.
age
touch -t $(date -v-3d +%Y%m%d%H%M) $d/next-check-done/0-*.md(N[1])
n=$(copies); step >/dev/null
(( $(copies) == n )) && ok "an old copy deleted as one is made" || no "an old copy deleted as one is made" "$n, then $(copies)"
exit $fail
