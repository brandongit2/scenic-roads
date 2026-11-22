#!/bin/zsh
# Tests of tools/app/services.zsh, the services a release ships and its agent installs (the dev
# tools app and the map's site in the Mac's Caddy), in a sandbox (SCENIC_SERVICES_SANDBOX): real
# LaunchAgents of this user's, labelled local.scenic-sandbox.<folder>.*, loaded from the sandbox and
# removed after; the dev tools on a free port with data of their own; the Mac's Caddy a stand-in (a
# Python HTTPS server the test runs, answering with the mark of the sites it last loaded; its
# `validate` refuses a site marked BROKEN, its `reload` one marked NORUN, keeping what it had).
# Nothing of this Mac's own services is touched. Run by tools/dev/check.sh when tools/app changes,
# and by publish's other tests:
#   zsh tools/app/test/services.zsh
# Prints PASS or FAIL per test; exits non-zero when one failed.
repo=${0:A:h:h:h:h}
source $repo/tools/app/lib.zsh
box=$(mktemp -d "${TMPDIR:-/tmp}/svc.XXXXXX") || exit 2
box=${box:A}
prefix=local.scenic-sandbox.${box:t}
standin=
cleanup() {
  launchctl bootout gui/$UID/$prefix.devtools 2>/dev/null
  [[ -n $standin ]] && kill $standin 2>/dev/null
  rm -rf $box
}
trap cleanup EXIT
trap 'exit 143' TERM; trap 'exit 130' INT
failed=0
ok() { print "PASS  $1" }
no() { print "FAIL  $1: $2"; failed=1 }
export SCENIC_SERVICES_SANDBOX=$box
H=$box/services

node=$(node -p process.execPath) || { no setup "no node"; exit 1 }
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])' }
port=$(free_port) hport=$(free_port)
mkdir -p $box/bin
/opt/homebrew/bin/openssl req -x509 -newkey rsa:2048 -nodes -subj /CN=svc.test -days 1 \
  -keyout $box/key.pem -out $box/cert.pem >/dev/null 2>&1 || { no setup "openssl"; exit 1 }
# The stand-in Caddy: `validate`, `reload` and `run`, each `--config F`, F's sites the files in
# sites/ beside it (as the Mac's Caddyfile imports them). What it loaded is in F's folder: `loaded`
# (the sites' mark, which `run` answers with), each reload a line in `reloads`.
print -r -- '#!/usr/bin/python3
import glob, http.server, os, ssl, sys
cmd, conf = sys.argv[1], sys.argv[sys.argv.index("--config") + 1]
d = os.path.dirname(conf)
text = "".join(open(f).read() for f in sorted(glob.glob(d + "/sites/*.caddy")))
def load():
    mark = [l for l in text.splitlines() if l.startswith("# mark ")]
    open(d + "/loaded.tmp", "w").write(mark[0][7:] if mark else "none")
    os.replace(d + "/loaded.tmp", d + "/loaded")
if cmd == "validate": sys.exit(1 if "BROKEN" in text else 0)
if cmd == "reload":
    if "NORUN" in text: sys.exit(1)
    load(); open(d + "/reloads", "a").write("reload\n"); sys.exit(0)
load()
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(open(d + "/loaded").read().encode())
s = http.server.HTTPServer(("127.0.0.1", int(os.environ["HTTPS_PORT"])), H)
c = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); c.load_cert_chain(os.environ["CERT"], os.environ["KEY"])
s.socket = c.wrap_socket(s.socket, server_side=True)
s.serve_forever()' > $box/bin/caddy
chmod +x $box/bin/caddy

# Versions, each an app folder holding services/ (services_stage of this checkout), changed so each
# can be told apart: the board page's index carries the version, the site file a mark.
n=0
version() {  # version <name> [devtools: ok|exit|migrate|late|same] [caddy: ok|BROKEN|NORUN]: its services.zsh's path
  # (late: migrates, then ends 3 s later, not answering; same: the running version's dev tools.)
  local v=20260101-$(( ++n ))000-$1 s
  s=$box/app/$v/services
  services_stage $repo $s >/dev/null || { no "stage $1" "services_stage failed"; exit 1 }
  print -r -- "<!-- $v -->" >> $s/devtools/tools/devtools/public/index.html
  case ${2:-ok} in
    exit) print -r -- 'process.exit(3);' > $s/devtools/tools/devtools/server.mjs ;;
    migrate) print -r -- "import { DatabaseSync } from 'node:sqlite';
const d = process.argv[process.argv.indexOf('--data') + 1];
new DatabaseSync(d + '/jobs.db').exec('PRAGMA user_version = 99');
process.exit(3);" > $s/devtools/tools/devtools/server.mjs ;;
    late) print -r -- "import { DatabaseSync } from 'node:sqlite';
const d = process.argv[process.argv.indexOf('--data') + 1];
new DatabaseSync(d + '/jobs.db').exec('PRAGMA user_version = 99');
setTimeout(() => process.exit(3), 3000);" > $s/devtools/tools/devtools/server.mjs ;;
    same) rm -rf $s/devtools && cp -R $H/devtools/$(readlink $H/devtools/current) $s/devtools ;;
  esac
  { print -r -- "# mark $1"; [[ ${3:-ok} == ok ]] || print -r -- "# ${3}"; cat $repo/tools/caddy/scenic.caddy } > $s/caddy/scenic.caddy.new
  mv $s/caddy/scenic.caddy.new $s/caddy/scenic.caddy
  REPLY=$s/services.zsh
}
B=http://127.0.0.1:$port
page() { curl -sf --max-time 2 $B/ | tail -1 }
mark() { curl -sk --max-time 2 --resolve svc.test:$hport:127.0.0.1 https://svc.test:$hport/ }
pid() { launchctl print gui/$UID/$prefix.$1 2>/dev/null | awk '$1 == "pid" && $2 == "=" { print $3; exit }' }
items() { curl -sf --max-time 2 "$B/api/c/items?limit=1" | python3 -c 'import json, sys; print(json.load(sys.stdin)["total"])' }
uv() { /usr/bin/sqlite3 $data/jobs.db 'PRAGMA user_version' }
# (The form this checkout's dev tools make and keep jobs.db at: jobs.mjs's FORM.)
jform=$($node --input-type=module -e "import('$repo/tools/devtools/jobs.mjs').then((m) => console.log(m.FORM))") || { no setup "jobs.mjs's FORM"; exit 1 }
# (The script run as a program, as by hand and by the agent: services_stage ships it executable.)
run() { out=$("$@" 2>&1); rc=$? }

# ---- setup ------------------------------------------------------------------------------------------
# As on the Mac mini: the dev tools run from a git checkout by a LaunchAgent of the same label, the
# board's data in the checkout's tools/devtools; setup takes what it ran (its node, port and data
# folder) and replaces it. The Mac's Caddy runs, its Caddyfile importing sites/, another project's
# site there already.
co=$box/checkout
services_stage $repo $co >/dev/null || { no setup "services_stage failed"; exit 1 }
data=$co/devtools/tools/devtools
mkdir -p $box/caddy/sites
print -r -- 'import sites/*.caddy' > $box/caddy/Caddyfile
print -r -- '# another project' > $box/caddy/sites/other.caddy
HTTPS_PORT=$hport CERT=$box/cert.pem KEY=$box/key.pem $box/bin/caddy run --config $box/caddy/Caddyfile > $box/caddy.log 2>&1 &
standin=$!
reloads() { wc -l < $box/caddy/reloads 2>/dev/null | tr -d ' ' || print 0 }
mkdir -p $box/LaunchAgents
old() {  # old <label> <working folder> <program> <arg>…: a LaunchAgent as the checkout's install.sh wrote it
  local f=$box/LaunchAgents/$1.plist i=0 a
  plutil -create xml1 $f; plutil -insert Label -string $1 $f; plutil -insert ProgramArguments -array $f
  for a in ${@:3}; do plutil -insert ProgramArguments.$i -string $a $f; (( ++i )); done
  plutil -insert WorkingDirectory -string $2 $f
  plutil -insert RunAtLoad -bool true $f; plutil -insert KeepAlive -bool true $f
  REPLY=$f
}
old $prefix.devtools $data $node $data/server.mjs --host :: --port $port
launchctl bootstrap gui/$UID $REPLY
for (( i = 0; i < 50; i++ )); do [[ -n $(items 2>/dev/null) && $(mark) == none ]] && break; sleep 0.2; done
curl -sf -X POST $B/api/c/items -H 'content-type: application/json' -d '{"text": "kept"}' >/dev/null || no "board" "can't add a task to the checkout's"
[[ $(items) == 1 ]] || no "the checkout's dev tools" "items $(items 2>&1)"

version one; v1=$REPLY
# (Task #1214: staged executable, so the documented `…/services/services.zsh setup devtools` runs.)
[[ -x $v1 && $(head -1 $v1) == "#!/bin/zsh" ]] && ok "services_stage: the script is executable" \
  || no "services_stage: the script is executable" "$(ls -l $v1)"
run $v1 setup devtools
if (( rc )); then no "setup devtools" "exit $rc: $out"
elif [[ $(page) != "<!-- ${v1:h:h:t} -->" ]]; then no "setup devtools" "the page isn't one's: $(page)"
elif [[ $(items) != 1 ]]; then no "setup devtools" "items $(items), not the checkout's 1"
elif [[ $(plutil -extract ProgramArguments json -o - $box/LaunchAgents/$prefix.devtools.plist) != "[\"${node//\//\\/}\",\"${${H//\//\\/}}\\/devtools\\/current\\/tools\\/devtools\\/server.mjs\",\"--host\",\"::\",\"--port\",\"$port\",\"--data\",\"${data//\//\\/}\"]" ]]; then
  no "setup devtools" "the plist: $(plutil -extract ProgramArguments json -o - $box/LaunchAgents/$prefix.devtools.plist)"
else ok "setup devtools: from the checkout's, its data kept where it is"; fi
[[ $(uv) == $jform ]] || no "jobs.db" "user_version $(uv), not $jform"

print -rl -- '# an older names file' other.test > $box/names
csetup=(--caddy $box/bin/caddy --name svc.test --cert $box/cert.pem --key $box/key.pem --https-port $hport)
run $v1 setup caddy --config $box/nowhere/Caddyfile $csetup
(( rc == 1 )) && [[ $out == *"isn't set up"* && ! -f $H/caddy/setup ]] && ok "setup caddy refuses a Mac without its Caddy" \
  || no "setup caddy refuses a Mac without its Caddy" "exit $rc: $out"
run $v1 setup caddy $csetup
if (( rc )); then no "setup caddy" "exit $rc: $out"
elif [[ $(mark) != one ]]; then no "setup caddy" "serves $(mark), not one"
elif [[ $(grep -v '^#' $box/names) != $'svc.test\nother.test' ]]; then no "setup caddy" "names (its first, the others kept): $(<$box/names)"
elif [[ $(<$H/version) != ${v1:h:h:t} ]]; then no "setup caddy" "version $(<$H/version)"
elif ! grep -qxF "https://svc.test:$hport {" $box/caddy/sites/scenic.caddy || grep -q '@[a-z_]*@' $box/caddy/sites/scenic.caddy; then
  no "setup caddy" "the site file isn't filled in: $(head -12 $box/caddy/sites/scenic.caddy)"
elif [[ $out != *"devtools: at "*" already"* ]]; then no "setup caddy" "the dev tools restarted too: $out"
else ok "setup caddy: the site in the Mac's Caddy, the dev tools left running"; fi
run $v1 setup caddy --bogus x
(( rc == 2 )) && ok "setup refuses an unknown option" || no "setup refuses an unknown option" "exit $rc"
# (A setup again takes its settings before as its defaults.)
run $v1 setup caddy
(( rc == 0 )) && [[ $(mark) == one ]] && grep -qxF "https://svc.test:$hport {" $box/caddy/sites/scenic.caddy \
  && ok "setup caddy again: its settings before kept" || no "setup caddy again: its settings before kept" "exit $rc: $out"

# ---- update -----------------------------------------------------------------------------------------
version two; v2=$REPLY
run $v2 update
order=$(grep -E '^services: [0-9:]+ (devtools|caddy): ' <<< $out | head -1)
if (( rc )); then no "update" "exit $rc: $out"
elif [[ $(page) != "<!-- ${v2:h:h:t} -->" || $(mark) != two ]]; then no "update" "page $(page), mark $(mark)"
elif [[ $(items) != 1 ]]; then no "update" "items $(items), not 1"
elif [[ $order != *devtools* ]]; then no "update" "Caddy before the dev tools: $out"
elif [[ $(<$H/version) != ${v2:h:h:t} ]]; then no "update" "version $(<$H/version)"
else ok "update: the dev tools, then Caddy, the data kept"; fi
p0=$(pid devtools) r0=$(reloads)
run $v2 update
(( rc == 0 )) && [[ $(pid devtools) == $p0 && $(reloads) == $r0 ]] && ok "update at the version already restarts nothing" \
  || no "update at the version already restarts nothing" "exit $rc, pid $p0 → $(pid devtools), reloads $r0 → $(reloads): $out"

# The lock: another run holding it, this one stops at once.
touch $H/lock; zsh -c "zmodload zsh/system; zsystem flock -f fd $H/lock; sleep 3" &
holder=$!
sleep 0.5
run $v2 update
kill $holder 2>/dev/null; wait $holder 2>/dev/null
(( rc == 1 )) && [[ $out == *"another run holds"* ]] && ok "one run at a time" || no "one run at a time" "exit $rc: $out"

# ---- failures ----------------------------------------------------------------------------------------
version three exit; v3=$REPLY
run $v3 update
if (( rc != 1 )); then no "a dev tools that doesn't answer" "exit $rc: $out"
elif [[ $(page) != "<!-- ${v2:h:h:t} -->" || $(items) != 1 ]]; then no "a dev tools that doesn't answer" "page $(page), items $(items)"
elif [[ $(<$H/failed) != ${v3:h:h:t} || $(<$H/version) != ${v2:h:h:t} ]]; then no "a dev tools that doesn't answer" "failed/version: $(<$H/failed) $(<$H/version)"
elif [[ $(readlink $H/devtools/current) != ${v2:h:h:t} ]]; then no "a dev tools that doesn't answer" "current $(readlink $H/devtools/current)"
else ok "a dev tools that doesn't answer: the one before back"; fi

version four migrate; v4=$REPLY
run $v4 update
if (( rc != 1 )); then no "a failed migration" "exit $rc: $out"
elif [[ $(uv) != $jform ]]; then no "a failed migration" "jobs.db's user_version $(uv), not put back: $out"
elif [[ $(page) != "<!-- ${v2:h:h:t} -->" || $(items) != 1 ]]; then no "a failed migration" "page $(page), items $(items)"
elif [[ $out != *"jobs.db put back"* || $out == *"taskboard.db put back"* ]]; then no "a failed migration" "only jobs.db put back: $out"
else ok "a failed migration: its database put back"; fi

# (Caddy's update goes on past the dev tools' failures: it's at four's.)
[[ $(mark) == four ]] || no "Caddy beside a failed dev tools" "mark $(mark), not four"
site_is() { [[ $(head -1 $box/caddy/sites/scenic.caddy) == "# mark $1" ]] }
version five ok BROKEN; v5=$REPLY
run $v5 update
if (( rc != 1 )); then no "a site that doesn't validate" "exit $rc: $out"
elif [[ $(mark) != four || $(page) != "<!-- ${v5:h:h:t} -->" ]]; then no "a site that doesn't validate" "mark $(mark), page $(page)"
elif ! site_is four; then no "a site that doesn't validate" "the site file: $(head -1 $box/caddy/sites/scenic.caddy)"
else ok "a site that doesn't validate: the one before back"; fi

version six ok NORUN; v6=$REPLY
run $v6 update
if (( rc != 1 )); then no "a site Caddy won't reload with" "exit $rc: $out"
elif [[ $(mark) != four ]] || ! site_is four; then no "a site Caddy won't reload with" "mark $(mark): $out"
else ok "a site Caddy won't reload with: the one before back"; fi

# Kept: the current version's copy and the one before.
kept=(${(f)"$(ls $H/devtools | grep -E '^[0-9]+-')"})
[[ ${#kept} == 2 && ${kept[(Ie)${v6:h:h:t}]} != 0 ]] && ok "two versions' copies kept" || no "two versions' copies kept" "${kept[*]}"

# A job gone (unloaded) is loaded again by the next update.
launchctl bootout gui/$UID/$prefix.devtools
version seven; v7=$REPLY
run $v7 update
(( rc == 0 )) && [[ $(page) == "<!-- ${v7:h:h:t} -->" && $(mark) == seven && $(items) == 1 ]] && ok "an unloaded job loaded again" \
  || no "an unloaded job loaded again" "exit $rc, page $(page), mark $(mark): $out"

# The dev tools unchanged since the running version's: left running, Caddy updated.
version eight same; v8=$REPLY
p0=$(pid devtools)
run $v8 update
(( rc == 0 )) && [[ $(pid devtools) == $p0 && $(readlink $H/devtools/current) == ${v7:h:h:t} && $(mark) == eight && $(<$H/version) == ${v8:h:h:t} ]] \
  && ok "dev tools unchanged: not restarted" \
  || no "dev tools unchanged: not restarted" "exit $rc, pid $p0 → $(pid devtools), current $(readlink $H/devtools/current), mark $(mark): $out"

# The databases not copied (their folder not writable): nothing switched.
version nine; v9=$REPLY
chmod a-w $H/devtools/backup
run $v9 update
chmod u+w $H/devtools/backup
if (( rc != 1 )); then no "no copy of the databases" "exit $rc: $out"
elif [[ $(pid devtools) != $p0 || $(readlink $H/devtools/current) != ${v7:h:h:t} || $(page) != "<!-- ${v7:h:h:t} -->" ]]; then
  no "no copy of the databases" "pid $p0 → $(pid devtools), current $(readlink $H/devtools/current): $out"
else ok "no copy of the databases: nothing switched"; fi

# A run cut short after its switch (killed while the new version, which changed jobs.db's form,
# starts): the next run goes on from it, back to the version before, the copy taken before put back.
version ten late; v10=$REPLY
$v10 update > $box/cut.log 2>&1 &
cut=$!
for (( i = 0; i < 100; i++ )); do [[ $(uv) == 99 ]] && break; sleep 0.1; done
kill $cut 2>/dev/null; wait $cut 2>/dev/null
if [[ $(uv) != 99 || ! -f $H/devtools/backup/pending ]]; then no "a run cut short" "not cut after the switch: uv $(uv): $(<$box/cut.log)"
else
  run $v10 update
  if (( rc != 1 )); then no "a run cut short" "exit $rc: $out"
  elif [[ $(uv) != $jform || $(readlink $H/devtools/current) != ${v7:h:h:t} || -f $H/devtools/backup/pending ]]; then
    no "a run cut short" "uv $(uv), current $(readlink $H/devtools/current): $out"
  else
    for (( i = 0; i < 50; i++ )); do [[ $(page) == "<!-- ${v7:h:h:t} -->" ]] && break; sleep 0.2; done
    [[ $(page) == "<!-- ${v7:h:h:t} -->" && $(items) == 1 ]] && ok "a run cut short: the next goes back to the version before it, its database put back" \
      || no "a run cut short" "page $(page), items $(items): $out"
  fi
fi

[[ $(<$box/caddy/sites/other.caddy) == '# another project' ]] && ok "another project's site left as it was" \
  || no "another project's site left as it was" "$(<$box/caddy/sites/other.caddy)"

exit $failed
