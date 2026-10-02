#!/bin/zsh
# Copy a local folder into the NAS project folder and verify every file by SHA-256, computed
# locally and on the NAS itself (over SSH), so the comparison never trusts the SMB read path.
#
#   tools/nas/move-to-nas.sh <local_dir> <dest_rel> [--copy-only]
#   tools/nas/move-to-nas.sh --verify-from <ssh host> <remote local_dir> <dest_rel>
#
# The second form verifies a copy another Mac made (its files hashed there over SSH, the NAS's
# from here), for a Mac that can't reach the NAS over SSH unattended.
# Prints "VERIFIED <n> files" and exits 0 only when every file matches; the local copy is left in
# place (delete it separately once verified). Re-running resumes: rsync skips files already there.
set -euo pipefail
ROOT_SMB=/Volumes/personal/projects/scenic-roads
ROOT_NAS=/volume1/personal/projects/scenic-roads
NAS=(ssh -o ControlMaster=auto -o 'ControlPath=~/.ssh/cm-%r@%h:%p' -o ControlPersist=12h -o LogLevel=ERROR brandontsang@fishandchips.local)
LIST='find . -type f ! -name .DS_Store ! -path "*/@eaDir/*" -print0 | sort -z'
tmp=$(mktemp -d)
compare() { # $1 label
  if diff -q $tmp/local $tmp/nas >/dev/null; then
    echo "VERIFIED $(wc -l < $tmp/local | tr -d ' ') files ($1)"
  else
    echo "MISMATCH ($1):"; diff $tmp/local $tmp/nas | head -20; exit 1
  fi
}
if [[ ${1:-} == --verify-from ]]; then
  host=$2; src=$3; rel=$4
  ssh $host "cd '$src' && $LIST | xargs -0 shasum -a 256" > $tmp/local
  "${NAS[@]}" "cd '$ROOT_NAS/$rel' && $LIST | xargs -0 -r sha256sum" > $tmp/nas
  compare "$host:$src -> $rel"
  exit 0
fi
src=${1:A}; rel=$2
[[ -d $src ]] || { echo "no such folder: $src"; exit 2; }
mount | grep -q " on /Volumes/personal (smbfs" || { echo "NAS not mounted at /Volumes/personal"; exit 2; }
dst=$ROOT_SMB/$rel
mkdir -p $dst
rsync -a --exclude '.DS_Store' "$src/" "$dst/"
if [[ ${3:-} == --copy-only ]]; then
  echo "COPIED $src -> $rel"
  exit 0
fi
(cd $src && eval $LIST | xargs -0 shasum -a 256) > $tmp/local
"${NAS[@]}" "cd '$ROOT_NAS/$rel' && $LIST | xargs -0 -r sha256sum" > $tmp/nas
compare "$src -> $rel"
