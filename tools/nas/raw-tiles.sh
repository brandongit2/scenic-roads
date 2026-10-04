#!/bin/zsh
# Copy the build Mac's raw terrain tiles that the NAS's store lacks into it (docs/plan.md §3,
# Downloads): AWS's tiles are kept in the agent's cache as terrain jobs fetch them, and reach the
# store in bulk, one tar stream unpacked on the NAS itself (a tile at a time over SMB, the NAS's
# small-file writes run at ~25 a second; this, thousands). Then every tile is checked to be there at
# the same size. Re-running copies only what's still missing. Needs SSH to the NAS (the 1Password
# agent unlocked).
#
#   tools/nas/raw-tiles.sh
set -euo pipefail
CACHE="$HOME/Library/Application Support/scenic/agent/cache/aws-terrarium"
NAS=brandontsang@fishandchips.local
STORE=/volume1/personal/projects/scenic-roads/sources/aws-terrarium
work=$(mktemp -d)
trap 'rm -rf $work' EXIT
remote_list() {
  ssh -o BatchMode=yes $NAS "cd $STORE && find . -type f \( -name '*.png' -o -name '*.none' \) -printf '%P %s\n' | sort"
}
# (Files at least a minute old: a running job's newest may still be written.)
(cd "$CACHE" && find . -type f \( -name '*.png' -o -name '*.none' \) -mmin +1 -print0 | xargs -0 stat -f '%N %z' | sed 's|^\./||' | sort) > $work/local
remote_list > $work/nas
join -v1 $work/local $work/nas | cut -d' ' -f1 > $work/missing
echo "$(wc -l < $work/local | tr -d ' ') tiles here, $(wc -l < $work/nas | tr -d ' ') on the NAS; copying $(wc -l < $work/missing | tr -d ' ')"
if [[ -s $work/missing ]]; then
  COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -C "$CACHE" -cf - -T $work/missing \
    | ssh -o BatchMode=yes $NAS "mkdir -p $STORE && cd $STORE && tar --skip-old-files -xf -"
fi
# Every tile here on the NAS at the same size; a NAS copy of another size is reported, not replaced
# (a reader checks it whole and takes it again from the next source).
remote_list > $work/nas
missing=$(join -v1 $work/local $work/nas | wc -l | tr -d ' ')
differ=$(join $work/local $work/nas | awk '$2 != $3' | wc -l | tr -d ' ')
echo "after: $missing missing, $differ of another size"
[[ $missing == 0 ]]
