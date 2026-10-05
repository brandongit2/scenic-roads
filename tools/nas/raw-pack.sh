#!/bin/zsh
# Pack the NAS's loose raw terrain tiles into its archives (pipeline::rawpack: grouped as the terrain
# is, named in sources/aws-terrarium/packs/index.json). The NAS lists them (one walk), this Mac puts
# the list in the areas' order, and the NAS's own tar sends them in that order over SSH: the build
# Mac packs an area at a time, putting each archive on the NAS whole (a large file each, where a tile
# at a time over SMB the NAS takes ~23 a second), with a GB or two on its own disk at most. The run
# fails unless the stream carried every file listed. The list leaves out the tiles the archives hold
# already, so a run stopped partway, run again, streams only what's left ("every loose tile on the
# NAS is in the archives" once none is). Then `scenic-build raw-pack --check --every 50` reads
# every archive back, matched against its name, and a tile in 50 against its loose copy. The loose
# tiles stay: deleting them, hundreds of thousands of files with Synology's metadata beside each, is
# the owner's to do. Needs SSH to the NAS (the 1Password agent unlocked).
#
#   tools/nas/raw-pack.sh [scenic-build]
set -euo pipefail
NAS=brandontsang@fishandchips.local
STORE=/volume1/personal/projects/scenic-roads/sources/aws-terrarium
ROOT=/Volumes/personal/projects/scenic-roads
BUILD=${1:-"$HOME/Library/Application Support/scenic/app/current/scenic-build"}
work=$(mktemp -d)
trap 'rm -rf $work' EXIT
# (Synology's @eaDir folders, and the archives themselves, passed over.)
ssh -o BatchMode=yes $NAS "cd $STORE && find . -name @eaDir -prune -o -name packs -prune -o -type f \( -name '*.png' -o -name '*.none' \) -print" > $work/found
if [[ ! -s $work/found ]]; then
  echo "raw-pack: no tiles on the NAS under $STORE" >&2
  exit 1
fi
"$BUILD" raw-pack --order --root "$ROOT" --scratch "$work" < $work/found > $work/list
n=$(wc -l < $work/list | tr -d ' ')
if (( n == 0 )); then
  echo "raw-pack: every loose tile on the NAS is in the archives" >&2
  exit 0
fi
echo "raw-pack: $n files to pack" >&2
ssh -o BatchMode=yes $NAS "cd $STORE && tar -cf - -T -" < $work/list \
  | "$BUILD" raw-pack --root "$ROOT" --scratch "$work" --cache "$work/aws-terrarium" --from-tar - --expect $n
