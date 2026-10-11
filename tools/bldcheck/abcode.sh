#!/bin/sh
# The committed renderer before the darkening work (7e46ba2) against the current one, A B A B, with
# the gated time.mjs (whole frames on − off) on three views; the file swapped between runs.
S=/private/tmp/claude-501/-Users-brandontsang-repos-scenic-roads/10eba93e-3572-4e06-9dab-eecc1cf564b8/scratchpad
W=/Users/brandontsang/repos/scenic-roads/.claude/worktrees/agent-a76f8d7bd5ded89b1
cd "$W"
python3 -c "
import json; v=json.load(open('tools/bldcheck/views/views-T.json')); json.dump([x for x in v if x['id'] in ('t-manhattan-p70','t-kanto-p70','t-tokyo-z11')], open('$S/views-AB.json','w'))"
cp web/src/bldfield.ts "$S/bldfield-cur.ts"
git show 7e46ba2:web/src/bldfield.ts > "$S/bldfield-7e46.ts"
for round in 1 2; do
  for v in 7e46 cur; do
    cp "$S/bldfield-$v.ts" web/src/bldfield.ts
    sleep 4
    echo "### $v round $round"
    BLD_MAX_LOAD=5 node tools/bldcheck/time.mjs "$S/views-AB.json" "$S/ab-$v-$round.json" --frames 300 > "$S/ab-$v-$round.log" 2>&1
    grep -E "load|busy" "$S/ab-$v-$round.log" | head -6
    python3 "$S/tsum.py" "$S/ab-$v-$round.json"
  done
done
cp "$S/bldfield-cur.ts" web/src/bldfield.ts
