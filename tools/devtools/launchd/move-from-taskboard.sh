#!/bin/zsh
# One-off (#946), on the M6: the task board's server (tools/taskboard, LaunchAgents
# local.scenic.taskboard and -backup) becomes the dev tools app (tools/devtools, local.scenic.devtools
# and -backup), and Caddy's /task-board becomes /dev. Delete this script once the move has held.
#
#   cd ~/projets/scenic-roads && git pull --ff-only && tools/devtools/launchd/move-from-taskboard.sh
#   tools/devtools/launchd/move-from-taskboard.sh --rollback      (back to the task board's server)
#
# Run it right after the pull: the pull deletes tools/taskboard's code, which the old server (loaded
# already) doesn't need until it restarts, and its nightly backup (03:30) does.
#
# The move: checks first (node, the Caddyfile), then stops the old jobs, copies the board's data with
# SQLite's .backup (its -wal with it) to tools/devtools/taskboard.db, checks the copy (quick_check,
# the same rows in each table), puts the old LaunchAgents' plists aside (so a login doesn't load
# them), installs the app's and Caddy's, and checks each answers. The old data stays where it was,
# untouched (tools/taskboard/ is ignored by git now), with a copy on the NAS
# (state/taskboard/pre-946-<time>.db); the old nightly copies stay in state/taskboard/.
#
# The rollback: stops the app's jobs, puts their plists aside; the board's data as it is now goes back
# to tools/taskboard/taskboard.db (the files there moved to tools/taskboard/pre-rollback-<time>/, and
# the app's to tools/devtools/rolled-back-<time>/, so a later move starts clean); the old server's
# code and Caddyfile from the commit before #946 (git archive: the board's to tools/taskboard, an
# ignored folder; Caddy's to ~/.config/scenic/pre-946/caddy, run from there); their install.sh run.
set -euo pipefail
usage="usage: tools/devtools/launchd/move-from-taskboard.sh [--rollback]"
case $#:${1:-} in
  1:-h|1:--help) print -r -- $usage; exit 0 ;;
  0:|1:--rollback) ;;
  *) print -ru2 -- "move-from-taskboard.sh: unexpected ${1:-}"$'\n'"$usage"; exit 2 ;;
esac
zmodload zsh/datetime
path=(/opt/homebrew/bin $path)  # (mise and caddy over ssh, whose PATH is the system's)
cd ${0:A:h}/../../..
repo=$PWD
old=$repo/tools/taskboard new=$repo/tools/devtools
domain=gui/$(id -u) agents=~/Library/LaunchAgents aside=~/.config/scenic/pre-946
nas=/Volumes/personal/projects/scenic-roads/state/taskboard
url=http://127.0.0.1:8077
# (The name Caddy serves, as tools/caddy/install.sh wrote it: its first line not a comment.)
name=$(grep -v '^#' ~/.config/scenic/names 2>/dev/null | head -1) || name=
site=https://$name
stamp=$(strftime %Y%m%d-%H%M%S $EPOCHSECONDS)
say() { print -r -- "move-from-taskboard: $*" }
die() { say "$*"; exit 1 }

rows() {  # rows <db>: each table's row count, one line
  /usr/bin/sqlite3 $1 "select 'items '||(select count(*) from items)||', events '||(select count(*) from events)||', meta '||(select count(*) from meta)||', blobs '||(select count(*) from blobs)"
}
copy() {  # copy <from db> <to db>: a consistent copy (its -wal read with it), checked
  [[ ! -e $2 ]] || die "$2 is there already"
  /usr/bin/sqlite3 $1 ".backup '$2'"
  [[ $(/usr/bin/sqlite3 $2 "PRAGMA quick_check") == ok ]] || die "the copy $2 fails quick_check"
  [[ $(rows $1) == $(rows $2) ]] || die "the copy $2 has $(rows $2), not $(rows $1)"
  say "copied $1 to $2 ($(rows $2))"
}
unload() {  # unload <label>…: each stopped, waited for until launchd has let it go
  local l i
  for l; do
    launchctl bootout $domain/$l 2>/dev/null || true
    for (( i = 0; i < 100; i++ )); do launchctl print $domain/$l >/dev/null 2>&1 || break; sleep 0.1; done
    launchctl print $domain/$l >/dev/null 2>&1 && die "$l still loaded after 10 s"
    if [[ -e $agents/$l.plist ]]; then mkdir -p $aside; mv -f $agents/$l.plist $aside/$l.plist.$stamp; fi
    say "stopped $l (its plist in $aside)"
  done
  # (The socket table, never lsof: it walks every process's open files and blocks in the kernel
  # while the NAS's SMB share stalls, #1108.)
  local on
  for (( i = 0; i < 100; i++ )); do
    on=$(netstat -anv -p tcp 2>/dev/null | awk '$6 == "LISTEN" && $4 ~ /[.]8077$/')
    [[ -z $on ]] && return 0; sleep 0.1
  done
  die "something still listens on 8077: $on"
}
answers() {  # answers <items' count>: the app answers the board's API with that many items
  local i n
  for (( i = 0; i < 100; i++ )); do
    n=$(curl -sf --max-time 2 "$url/api/c/items?limit=1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["total"])' 2>/dev/null) && break
    sleep 0.2
  done
  [[ ${n:-} == $1 ]] || die "$url/api/c/items: ${n:-no answer}, not $1 items"
  curl -sf --max-time 5 $url/api/c/meta/status >/dev/null || die "$url/api/c/meta/status: no answer"
  say "$url answers: $n items, the status line"
}
node=$(cd $new && mise which node) || die "no node from mise in $new"
items() { /usr/bin/sqlite3 $1 "select count(*) from items" }

if [[ ${1:-} == --rollback ]]; then
  [[ -f $new/taskboard.db ]] || die "no $new/taskboard.db: nothing to roll back"
  rev=$(git rev-list -n 1 HEAD -- tools/taskboard/server.mjs)  # (#946's first commit: it deleted it)
  [[ -n $rev ]] || die "no commit deleted tools/taskboard/server.mjs"
  unload local.scenic.devtools local.scenic.devtools-backup
  if [[ -e $old/taskboard.db ]]; then
    mkdir -p $old/pre-rollback-$stamp && mv $old/taskboard.db*(N) $old/pre-rollback-$stamp/
  fi
  copy $new/taskboard.db $old/taskboard.db
  n=$(items $old/taskboard.db)
  mkdir -p $new/rolled-back-$stamp && mv $new/taskboard.db*(N) $new/rolled-back-$stamp/
  git archive $rev^ tools/taskboard | tar -x -C $repo
  rm -rf $aside/caddy; mkdir -p $aside/caddy
  git archive $rev^ tools/caddy | tar -x -C $aside/caddy --strip-components 2
  $old/launchd/install.sh $node
  answers $n
  $aside/caddy/install.sh $name
  say "rolled back: the board's server from ${rev:0:8}^ on :8077, Caddy's /task-board (from $aside/caddy)"
  exit 0
fi

[[ -f $old/taskboard.db ]] || die "no $old/taskboard.db (moved already?)"
[[ ! -e $new/taskboard.db ]] || die "$new/taskboard.db is there already: move it aside first"
"$node" -e 'require("node:sqlite")' 2>/dev/null || die "$node lacks node:sqlite"
caddy=$(command -v caddy || echo /opt/homebrew/bin/caddy)
[[ -n $name ]] || die "no name in ~/.config/scenic/names (tools/caddy/install.sh writes it)"
SCENIC_MAP_NAME=$name SCENIC_CERT=~/certs/map.crt SCENIC_KEY=~/certs/map.key \
  $caddy validate --config $repo/tools/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 || die "the Caddyfile doesn't validate"
trap '(( $? )) && say "stopped part way: tools/devtools/launchd/move-from-taskboard.sh --rollback puts the old server back"' EXIT
unload local.scenic.taskboard local.scenic.taskboard-backup
copy $old/taskboard.db $new/taskboard.db
# (One copy to the NAS, made here: SMB takes few small writes well, docs/macs.md.)
if [[ -d ${nas:h} ]]; then
  mkdir -p $nas && cp -X $new/taskboard.db $nas/pre-946-$stamp.db.part && mv $nas/pre-946-$stamp.db.part $nas/pre-946-$stamp.db
  say "copied to $nas/pre-946-$stamp.db"
else say "the NAS isn't mounted: no copy there (the old data stays in $old)"; fi
$new/launchd/install.sh $node
answers $(items $old/taskboard.db)
$repo/tools/caddy/install.sh $name
for (( i = 0; i < 50; i++ )); do curl -sf --max-time 2 $site/dev/api/c/meta/status >/dev/null && break; sleep 0.2; done
curl -sf --max-time 5 $site/dev/api/c/meta/status >/dev/null || die "$site/dev/api/c/meta/status: no answer"
[[ $(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' $site/task-board/709) == "308 $site/dev/task-board/709" ]] \
  || die "$site/task-board/709 isn't sent to /dev/task-board/709"
say "moved: the dev tools app on :8077 and $site/dev/; the old data stays in $old"
