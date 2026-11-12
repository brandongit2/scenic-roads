# Macs

Each Mac's agent, the map server and its mirror (on the Mac mini alone), and the devices the map opens on. The pool itself is `docs/pool.md`; tasks for any worker, `docs/coordinator.md`.

**Each Mac's agent.** Every Mac runs the agent (`scenic agent`, set up by `tools/app/install.sh`), a
member of the pool (docs/building.md §2, The pool): the Mac mini's leads (the declared lead,
docs/pool.md §6.1), every other takes work from the lead's coordinator. Below, the M4 (48 GB) unless said.
- It runs under the launcher as a LaunchAgent with `ProcessType Interactive` (else launchd throttles
  it).
- **Several jobs at once** (docs/building.md §2, Jobs at once). Each job is a child process group at utility
  priority (`taskpolicy -c utility`; `-b` would confine it to the efficiency cores), its Rust steps
  on the threads its grant gives it.
- **Power:** jobs run at any charge. Each holds `caffeinate -i -s -w <pid>` (no idle sleep; no system
  sleep on mains), dropped while paused. A Mac that runs down mid-job loses only the target under
  way.
- **Away from home** the agent mounts the share by the NAS's bare name through Tailscale (~12 MB/s)
  when the Keychain has its password; every job runs, slowly. Home again, with no job running, it
  remounts by the LAN name.
- **Downloads are kept on the NAS, each made once** (`sources/`): Meta's canopy squares, AWS's raw
  terrain tiles, GLO-30's tiles (all of them, fetched once by `scenic-build store-fetch`; builds
  only read them: docs/inputs.md §6.8), the leaf-type sources, Overture's buildings, Wikipedia's monthly
  pageviews (`sources/pageviews/<month>.tsv.zst`, every article of the map's languages counted, so a
  dump is streamed once). A Mac's copy is a cache filled from the NAS; one the NAS lacks is uploaded
  the next time it's looked up.
  - **AWS's raw terrain tiles** are packed onto the NAS (`buildstore::rawpack`,
    `sources/aws-terrarium/packs/`, listed in `index.json`) in archives grouped as the terrain's
    packs are (z9–12 by z6 tile, z3–8 by z3 tile, z0–2 together). A job's new tiles go up at its end
    as one archive beside its shard's others (the NAS's small-file writes run at ~23 a second). A
    shard's archives are each kept more than twice the size of all those after it, else merged, so
    a shard has a dozen at most. Archives leave the index through its `gone` list and are deleted a
    day later, so a reader of the old index stays right. Only the lead changes the index; a job
    puts its archives there unlisted and hands them off. A tile is read by range for a shard's first
    16, then each archive is copied whole once and checked against its name.
  - **GSI's elevation tiles** (the pyramids read tile by tile, `[data] encoding = gsi-png`) are
    packed the same way (`terrain::pyramids`, `sources/pyramids/<host-path>/packs/`, one store a
    URL template, a z14/z15 tile in its z6 tile's shard), by their own step, the lead's
    (`pyramids-fill`, a job per z6 tile a declared pyramid meets, one at a time so GSI sees one
    process's requests to it, held to its host's limit (buildstore::fetch, store::aimd), tasks #810, #823 and #1096; its cold start granted a thread, task #896): it fetches each tile
    a z6 tile's terrain-hi run reads (under its z9–12 pixels, the sea's too, and the whole of
    its terrain-deep targets' base tiles; inside the
    dataset's cover; and the ring round them its blends' bands reach) that the store lacks, once,
    and packs it (a 404 kept as an empty entry; any other failure leaves the z6 tile not done, so
    its next try fetches those). The z6 tile's terrain-hi and terrain-deep wait for it and read
    the store (task #975). As every job's, its archives go onto
    the NAS unlisted and into its hand-off as its store's (`<host-path>/<shard>`), and the lead
    names them in that store's index. Each z6 tile starts only while the NAS has more room past
    the owner's reserve (`scenic nas-room`) than the most one z6 tile has put there (6/56/23:
    1.97 GB of 170,612 tiles, 148,038 of them 404s, in 83 min on the M4); a pause stops it before the next tile it would ask (the fetches under way finish, task #885), what's fetched handed off. It says how far it is
    (`progress: <asked>/<to fetch> elevation tiles`, of all its layers' tiles the store lacks,
    each chunk of `pyramids::CHUNK`, 4,096 tiles: the page's of-the-job and time left, task #830),
    and notes the tiles it asked of GSI (`fetched`), which weigh its time (docs/pool.md). It's filled again when
    its key changes (docs/steps.md §12). Terrain-hi and terrain-deep read the store first, each
    tile by range from the NAS's archive; a tile it lacks is fetched as before, never written
    back. `scenic-build pyramids-fill <6/x/y …> --into <dir>` fills a scratch store by hand
    (`terrain-same --pyramids <dir>` reads one); `--sizes` says each store's size.
  - **The Wikidata and Wikipedia answers** the items job and the heritage chain got for a pass
    (`agentcore::answers`) are kept as one archive per pass (`sources/items/<date>/answers.tar.zst`,
    `heritage.tar.zst`), written as the step starts (when the NAS lacks what the Mac has) and as it
    ends. The two sides are merged by key where both changed; a file deleted by hand stays deleted;
    an archive that doesn't read whole is moved aside, not written over. A new pass's answers start
    from the last pass's.
  - **Whole:** each copy is written by a temporary name, flushed and its length checked before the
    rename, and checked whole when read (`codecs::whole`; written whole by `store::whole`). One that isn't is taken
    again from the NAS, else the source.
  - **"Not there"** (a 404, or S3's 403) is remembered only once the source says so twice.
  - **What's fetched again** is only new data (a planet, Wikidata facts and pageviews, timetables, an
    Overture release) or a window not yet covered (DEM points not sampled before, WorldCover tiles
    the packs lack).
- **Caches** (`~/Library/Application Support/scenic/agent/cache`): what room-making or a clear empties is a
  copy of what the NAS keeps, or made from it, refilled from the NAS, never the internet; or a
  range a job fetched (`blobs/net/`, below), fetched again only as a job reads it.
  - **The cheap ones** (emptied for room, cleared; kept after a build while the disk has room): canopy squares
    (`chm10/`), raw terrain tiles and archive copies (`aws-terrarium/`),
    copies of record files staging reads (`blobs/`: hi packs, roadside buildings' z8 tiles; with the
    basemap, the lakes' fill and the depth sources' packs the depth's and water's steps read, the stored GLO-30 tiles
    and fill masks a terrain or polar terrain job reads half or more of, task #689: buildstore::localread; and the
    canopy rows the trees read of the NAS's squares, `trees::squares::RowsCache`; and the ranges the
    fetcher fetched of remote files, `blobs/net/<host>/<path>/<length>.<hash of URL and version>/<start>-<end>.<hash of bytes>`,
    by the file's version as its server names it (none named: not kept), kept only while the disk has them free past the room
    target, not counted as a job's disk, checked against their hash as they're read, and read by the jobs after on this Mac rather than fetched again: buildstore::fetch::kept,
    task #1099), pageview month indexes (`items/months/`).
  - **Cleared too, on the owner's ask:** the pack cache (`base/`, the base packs' way indexes for
    pack(T) and lo), the
    retired caches, local copies the summits and peaks jobs
    read, and the heritage job's clip of the pass's `whs` set (made again by osmium).
  - **Deleted always** (`room::retire`, at the agent's start and daily, room needed or not): caches no
    code reads any more (`room::RETIRED`: the old DEM cache, `dem-cache.*`; the worldwide z8
    terrain's copies, `sources-terrain-z8-*`), and local copies of NAS files (`sources-*`, `work-*`,
    `layers-*`: the summits' and peaks' terrain lo packs) a day old whose logical name the build
    manifest no longer names (an older pass's). Without the NAS or a readable manifest, only the retired caches;
    not while a job an earlier agent left runs. A file a job holds stays.
  - **Kept:** the answers (`items/`, `heritage-<date>/`), the trains' stop pairs (`rail/`), the z6 tile
    stages' timings (`unit-stages.json`), what a z6 tile kept that isn't on the NAS yet, and the
    coverage as last read (`coverage/<hash16>.<format>.cov`, by the regions' accepted version and the
    file's format). Each read of it there deletes the folder's other files no process holds: another
    version's or an older format's once the last job mapping it lets go, a `.making` lock whose
    maker is gone, a dead writer's temporary file; a newer app's format is left (docs/inputs.md
    §6.1.4).
  - Nothing is deleted through a link, nor anything in the NAS's project folder.
- **No installed map server** (task #874): the devices' map is the Mac mini's (Devices). An installed server started
  on any other Mac retires itself (`crates/server/src/retire.rs`): it removes its LaunchAgent
  (`local.scenic.server`) and its run file (`run/server`), and unloads itself. The Mac's offline map is left as it was (`mirror/`, `idx/`, `downloads.json`,
  `~/Library/Caches/scenic/map` and the rest of the server's files): nothing deletes it. It retires
  only when it's sure it isn't the map's Mac: no name in `~/.config/scenic/names` (the file
  without one, or not there; one it can't read keeps it serving), and the pool's declared lead
  (`state/build/lead-member.json`) read and naming another member than this Mac's (`agent/member`,
  read). Without the NAS, or with either unread, it serves.
- **The offline map, on demand** (#882 step 7a, docs/roles.md §8.3): the menu bar's Open the Map on
  This Mac opens http://localhost:7311 in the browser; where no server answers there and none is
  installed (a laptop), it first starts the app's `server --on-demand` (its output appended to
  `~/Library/Logs/scenic/server.log`) and waits for it, up to two minutes. That server answers this
  Mac's loopback alone (127.0.0.1 and `::1`), is never retired, follows the map's log and copies and
  Updates the downloads while it runs (from the NAS's snapshot and its saved map with the Mac mini
  away), and quits 30 minutes after its last use (a request, or a copy of the mirror's under way).
  It keeps its map in `~/Library/Caches/scenic/map` each time the map moves (a resident server:
  every 10 minutes at most), so a start away from the Mac mini and the NAS serves the map its
  downloads were copied for. Away from the NAS it serves what's downloaded and nothing else (#985,
  Mirror below): inside a downloaded region every feature works (the place search, the terrain and
  its shading, the basemap, at every zoom), outside the downloads each answers empty at once, and
  with nothing downloaded the search finds nothing and no terrain is drawn (docs/roles.md §8.3). It
  runs no app updater (its next start runs the newest app), and
  ends at once when another server holds 127.0.0.1:7311. Nothing runs while it's closed, so the
  downloads are as current as the last time it ran.
- **An OSM pass** starts with 80 GB free (the pack cache counting as free) and copies the planet to
  the SSD when there's room, else reads it from the NAS.
- **What a z6 tile keeps for its later runs** is on the NAS (`cache/`), shared by every Mac: its canopy
  and view results (`scenic-units/`, docs/steps.md §6). A z6 tile's build
  folders go once it's built.

**The M1 (16 GB)** is a member: it builds what the lead's coordinator
gives it (docs/building.md §2, The pool): any shared step's jobs that fit the memory it has available above its memory floor
(`scenic memory`, 2 GB by default, its running jobs' growth reserved: docs/pool.md §7.1) and its disk (its room target kept free past each job's measured disk; a task 5 GB, tree cover's rows and terrain's subtrees 1 GB), a run from the front of the plan's order
(docs/pool.md §7.2), else z6 tiles' tails (`docs/coordinator.md`). A z6 tile's predicted peak is the most its job's
processes held together last time, else about ten times its extract, never under 3.7 GB nor below its
floor (docs/pool.md §7.2). A member's status goes to the lead in its beat (docs/coordination.md step 3), the lead listing
the members' in its own (`state/status.json`, `helpers`).

**The Mac mini (M6, 32 GB, 1 TB)** is always on, at home and headless: the pool's lead, fixed
(docs/pool.md §6, task #881), with room for more at once (its memory floor 4 GB by default). It also hosts the dev tools app, the task board and the dev job dashboard
(`tools/devtools`, http://mac-mini-de-brandon:8077, its LaunchAgent and nightly NAS backup from
`tools/devtools/launchd/install.sh`) and is the map's address for the phone, the iPad and devices
away from home: https://map.brandontsang.net (Devices). It publishes the app (`ssh m6 'cd
~/projets/scenic-roads && tools/app/publish.sh'`, docs/building.md, App publishing) and is the NAS's
Mac every publish installs through (in this checkout, whose git checks the order); its publishes' tests
run on the test Mac (`tools/app/remote-tests.sh`: known by its LocalHostName, `SCENIC_TEST_MAC`, default the M4's, on which the tests run in place with no ssh; reached from another Mac by `SCENIC_TEST_HOST`, an ssh alias, default `m4`). Its toolchain: brew's mise
and rustup (`/opt/homebrew/bin`, `~/.cargo/bin`, which publish.sh puts on the PATH itself) and,
from `mise install` in the checkout, everything mise.toml and rust-toolchain.toml pin (Rust,
Node, pnpm, uv, cargo-nextest, CMake); Xcode for swiftc
and codesign.

**A new (headless) Mac**, in this order:
1. `sudo scutil --set LocalHostName <name>` first: the pool's identity (a member's status file is
   named by it).
2. Automatic login (FileVault off), so the LaunchAgents and Scenic start after a restart; start up
   after a power failure (`sudo pmset -a autorestart 1`); never sleep on mains (`sudo pmset -c sleep
   0`); Screen Sharing on, to reach it.
3. Mount the NAS share at `/Volumes/personal`, its password saved to the login Keychain.
4. Homebrew, then `brew install osmium-tool openjdk@21 uv git`.
5. Tailscale; SSH keys both ways with the other Macs.
   SSH to the NAS too (the OSM pass's filters stream big inputs over it, `osmpass::streamed`):
   this Mac's public key in `~/.ssh/authorized_keys` of `brandontsang@fishandchips.local`, and the
   NAS's host key accepted once by hand (`ssh brandontsang@fishandchips.local true`, answering
   yes), since the build's `ssh` runs in batch mode and refuses an unknown host. Check:
   `ssh -o BatchMode=yes brandontsang@fishandchips.local ls /volume1/personal/projects/scenic-roads`.
   The share `/Volumes/personal` is `/volume1/personal` on the NAS.
6. `tools/app/install.sh`, then open Scenic.app once. On the map's Mac (the one
   `tools/caddy/install.sh` gave the map's name, a name in `~/.config/scenic/names`, or the one
   `state/build/lead-member.json` declares the lead: either) it also creates the map server's
   LaunchAgent; on any other it removes one, leaving the Mac's offline map.
7. Allow `scenic` (and on the map's Mac `server`) through the firewall (pool.md §6.8).
8. `scenic memory` says the memory floor, the memory the Mac keeps available (an eighth of its
   memory unless set; available counts what macOS frees by compressing idle memory with its
   pressure normal, docs/pool.md §7.1), the only memory setting; the room target is the owner's
   choice.

**App Macs (both laptops).**
- **The launcher** (`tools/launcher/launcher.c`) runs the command in `~/Library/Application
  Support/scenic/run/<name>` and restarts it when it exits: `bin/scenic-launcher server` (the map
  server's LaunchAgent, on the Mac mini alone), and Scenic.app's own copy running `agent` (the build service; there an
  agent exiting with `QUIT_EXIT` ends it cleanly, so launchd leaves it stopped). macOS's Local
  Network privacy counts the agent as the app's launcher (its launchd program), not as the app
  (the app's own entry stays undecided while the agent connects): the launcher carries an embedded
  Info.plist (`tools/status/bundle.sh`: CFBundleIdentifier its signing identifier
  `local.scenic.status.launcher`, NSLocalNetworkUsageDescription) so macOS can ask for the grant,
  listed as "Scenic build service". macOS keeps the grant by the launcher's signing identifier and
  path, so a launcher rebuilt or signed otherwise keeps it. Without the grant the agent's probe of
  `fishandchips.local:445` fails (the log's `nas: at home: no; … (errno 65)`, EHOSTUNREACH) while
  `nc` from a shell and the mount work (shells and ssh are exempt).
  A launcher whose signature changed no longer matches the build service's launch constraint
  (LWCR, launchd: spawn failed, exit 78): Scenic registers the service again when it opens with a
  new launcher or finds the job refused (docs/surface.md). The test, in a sandbox: `tools/status/
  sandbox.sh setup <dir>` (v2's launcher is re-signed with the hardened runtime: another cdhash),
  open `<dir>/Applications/Scenic Sandbox.app`, check `launchctl print gui/$UID/local.scenic-
  sandbox.build` runs, `sandbox.sh update <dir>`; within about a minute the app replaces itself
  and reopens, update.log says the new launcher's cdhash and "registering the build service
  again", and the job runs (no "needs LWCR update"; the first registration after the swap can keep the old
  constraint, the next, 30 s later, takes); `sandbox.sh clean <dir>`. A server started
  any other way (such as the desktop app's preview) waits on macOS's network-volume prompt; test
  servers run from the shell.
- **The updater** (`jobrt::appupdate`): every Mac's agent checks the NAS's `app/current.json`
  every five minutes, copies a newer app locally checking every file's SHA-256, points
  `app/current` at it and restarts into it by its update drain; on the Mac mini the map server
  checks too (so the map updates with the agent stopped), one check at a time (`app/update.lock`),
  and restarts into the new version once the map has been idle a minute. Each copier copies into a
  folder and a link of its own (`<version>.<pid>.tmp`, `current.<pid>.tmp`), so an updater that
  doesn't know the lock (a server from before #874, during the rollout) never writes into its copy;
  whichever finishes first is used. Pruning keeps the current version, the checker's, every version
  a process on the Mac runs from (the map server waiting for an idle map, an older server), the
  agent's and its jobs', and one to roll back to.
- The app reads the current and the previous form of every file kind.

**Server** (the Mac mini alone: a plain LaunchAgent, `local.scenic.server`, no window or menu;
installed by `tools/app/install.sh`, updated by the updater).
- **Finding the NAS:** `getfsstat(MNT_NOWAIT)`, never `statfs`. At home (`fishandchips.local`
  answers on 445) it mounts the share with `osascript` and the Keychain, at most every five minutes,
  by the LAN name when the Keychain has it, else the bare (Tailscale) name. The per-user
  `~/Library/Preferences/nsmb.conf` makes it a soft mount (the server adds its sections at start,
  `store::nas::ensure_nsmb_conf`, docs/formats.md); no Mac has an `/etc/nsmb.conf`.
- **Short NAS outages** (a dropped SMB session, the NAS restarting): a NAS read, stat or listing
  (store::nasfs, docs/nas-access.md) that fails for a moment (EIO, ENXIO, ETIMEDOUT, ESTALE…) is
  tried again for up to ~3.5 minutes in a build job or a thread of the agent's own (5 s in the
  server; never in the agent's loop, the pool or the coordinator, whose next beat reads again),
  the share's mount looked at before its last try, instead of failing the job (`store::retry`,
  docs/pool.md §3).
- **Stale SMB handles** (docs/pool.md §3): a Mac that dies with files open on the share (kill -9,
  Migration Assistant, a power cut) leaves its session's handles on the NAS until the server drops
  the session. Writers rename a held file aside meanwhile (`store::whole`). The build agent heals
  its own session (`crate::agent::heal`). A NAS call busy past whole's retries (a read for 5 s, a
  rename aside or into place) is noted with its file and logged (`whole: <path> stays busy …`):
  the agent's own in the process, its Rust jobs' appended to `nas-stuck.log` in the agent's folder
  (`SCENIC_NAS_STUCK`; the Python steps don't report). Three within 10 minutes start a drain: no new
  job is taken or granted on this Mac (heartbeats and leases go on), shown as "draining for a NAS
  remount (calls stuck busy on <path>)". Once the slots and the agent's own NAS threads (cache
  trims and freeing, the answers' seeding, the forecast, unpublishing) are done, the share is
  unmounted forced under the pool's lock (no tick mid-write) and mounted again in the same round.
  A drain waits 30 minutes at most (jobs running, or the pool's lock never free), then is given
  up without cutting a job and the interval doubles. Drains come at most
  every 30 minutes; when calls stay busy after a remount, the status warns naming the file (another
  Mac's dead session, most likely: its agent drops it when it starts again; else disconnect it in
  DSM) and the interval doubles, up to 4 hours, back to 30 minutes after 10 quiet ones. A remount
  ends only this Mac's sessions. (The "home again" remount from the tunnel to the LAN name still
  waits for idle, with no drain: the tunnel works meanwhile, only slower, so stopping new work for
  it isn't worth it.)
- **Dropping this Mac's dead sessions in DSM** (`crate::agent::dsm`, task #567): the agent ends
  its own Mac's stale SMB sessions on the NAS through DSM's web API (`SYNO.API.Auth` login and
  logout, `SYNO.Core.CurrentConnection` list and kick_connection; HTTPS on 5001, the certificate
  checked against the NAS's tailnet name `fishandchips.tail23e9f2.ts.net`, reached at home by the
  NAS's LAN address and away through Tailscale; every request a POST, logged out after each use).
  The certificate is Let's Encrypt's for that name (90 days; the current one ends 14 Nov 2026),
  installed and renewed on the NAS by the owner, not by the repo; once it lapses or doesn't match,
  each drop is a warning saying the TLS failed and naming the certificate, and nothing is sent (there
  is no unchecked fallback). The account is
  `NAS_DSM_USER` / `NAS_DSM_PASS` in `inputs/keys.env` (`scenic-sessions`: in administrators,
  which kicking another user's connection takes, with only the DSM application allowed); never
  printed or logged. A session is this Mac's when it is SMB, of the share's user (`brandontsang`,
  every Mac's), holds the `personal` share and not Time Machine (DSM lists one row per client
  connection with every share it holds, and a kick ends the whole connection; Time Machine's own
  connection is separate, so never touched), and comes from one of this Mac's addresses: those
  it has now, or, away from home, the LAN addresses it had there (`home-addresses.json` in the
  agent's folder, written while at home), for 24 hours at most and only for sessions that last
  connected a share before the Mac was last seen at home, so an address the LAN has since given
  another Mac is never taken for this one's. A session through Tailscale comes from this Mac's
  tailnet address (the NAS's Tailscale is a TUN device, and DSM lists a tunnelled client by that
  address: seen 11 Nov), one of its addresses at home and away. A tailnet address is one device's
  for as long as it stays in the tailnet, so every one the Mac has had in its agent's run counts as
  its own, with no time limit. A live session is never dropped: the newest of the Mac's is kept,
  and the newest of each route its mounts of the NAS hold as the drop is made (a mount by the LAN
  name, from a LAN address; one by the bare name, Finder's say, through Tailscale, from the tailnet
  address). The map server, Finder and the agent's own beats keep or reopen files on the share
  within seconds, so a kick would only swap it for a fresh one while their open handles fail.
  When:
  - at the agent's start, once the share answers: the sessions of its addresses now that last
    connected a share before the start, but the newest (the live one: a dead session stopped before
    the one replacing it began; DSM's time is when a connection last connected a share, not when
    it began, which keeps that order). Two live connections from one address holding the share
    (two mounts of it) would lose the older;
  - when the agent sees it has left home (at home: yes, then no): those of the addresses it had at
    home. Dropping beats unmounting here: by then the LAN is gone, so a forced unmount ends nothing
    on the NAS (no word reaches it), and the share is mounted again through Tailscale at once; only
    DSM, reached through Tailscale, can end the session the NAS still holds. A drop that doesn't
    reach DSM (no network yet) is tried again each minute for half an hour;
  - once the share is mounted by its LAN name after it was mounted through Tailscale (home again,
    the "home again" remount done): the same as at the start, as of then, with every tailnet
    address it has had. The remount's forced unmount usually ends the tunnelled session (the
    tunnel still works at home), but one it didn't end, or one a sleep or a dropped tunnel left
    behind while away, holds its leases just as a LAN session left behind on leaving does. The new
    LAN session is kept as its route's newest;
  - with the self-heal's remount: the same as at the start, as of the remount (another address's,
    one a reconnect left behind); the remount itself ends the live one.

  Pausing from the menu bar drops nothing: the session left behind on leaving is the live one,
  which the drop on leaving ends; a Mac carried away asleep keeps its session on the NAS until it
  wakes and its agent sees it has left. A failure (DSM not reached, no keys, refused) is a warning
  in the agent's log and stops nothing. By hand still: a Mac that doesn't come back, and the
  certificate's renewal on the NAS.
- **Settings that shorten a dead client's hold** (the owner applies them; nothing in the repo
  changes system settings). They are server-wide trade-offs:
  - Synology, DSM Control Panel → File Services → SMB → Advanced settings:
    - "SMB durable handles": with them on, a disconnected client's handles are kept for it to
      reclaim after a network blip. Turning them off makes a dead client's handles close with its
      session, but **keep them on if Time Machine or other macOS clients back up or work on the
      NAS** (they rely on reconnecting across blips); then a held file waits for the DSM disconnect
      or the server's own timeout.
    - "SMB2 lease" and "opportunistic locking": optional to turn off. A lease or oplock held by a
      dead client makes others wait for a break it never answers; off, every client loses its
      read caching on every share (slower, mostly for small-file reads).
    - Either way the server holds a dead session until it finds its TCP connection dead (its
      keepalive, minutes), which DSM doesn't expose; until then only the DSM disconnect frees it
      (the agents' own, above, for their Macs).
  - Each Mac's SMB client (`man nsmb.conf`; applies to mounts made afterwards). What every Mac
    has is the per-user file's `[FISHANDCHIPS:PERSONAL]` and `[FISHANDCHIPS.LOCAL:PERSONAL]`, both
    `soft=yes`. On a soft mount, once the client is reconnecting, SMB 2/3 calls fail with
    ETIMEDOUT instead of waiting, any still waiting fail after 12 s of reconnecting, and the share is
    given up as dead after 60 s (a hard mount: 10 minutes). The risk is for a program writing a
    file in place, which a failed call can leave part-written (the pipeline's writes go to a
    temporary name and rename, so a cut write never takes the name).
    - `notify_off=yes` (the owner's suggestion; allowed in a `[SERVER:SHARE]` section, so it can
      go in those two, whose other lines the server keeps): drops change notifications, one fewer
      long-lived handle per open folder, but Finder windows on the share no longer refresh by
      themselves.
    - `max_resp_timeout` is left at its default, 30 s: when a call has waited that long and the
      NAS has sent nothing at all for as long, the client drops the connection and reconnects (a
      call answered by nothing for 2 minutes while the NAS answers others fails alone with
      ETIMEDOUT). It is the NAS's whole connection that it times, not one call. A shorter value
      makes those reconnects come sooner, and with `soft=yes` each fails the calls in flight; the
      NAS's own log showed the mini's client closing connections mid-transfer on large files while
      the NAS was 85-90 % busy (11 Nov), the likely cause of task #1140's damaged uploads.

    These keep this Mac from hanging on the NAS; they can't shorten what the server keeps for a
    dead Mac. A Mac's agent drops its own dead sessions (above); one still held by a Mac that
    doesn't come back: DSM → Resource Monitor → Connections, disconnect that Mac (its LAN or
    tailnet address); the agent's warning names the file.
- **NAS access** goes through a bounded I/O pool with timeouts. An overrun trips the breaker (offline
  banner) only when a quick probe also fails. Files are read in 1 MB pieces with handles kept open,
  never mmapped; base packs and hidata in 256 KB pages, or whole sections for scans.
- **Failures are never cached:** an unanswerable read is a 503; the client retries after 2 s,
  doubling to a minute, and at once when the NAS is back.
- **Memory:** caches keyed by content name, with budgets (512 MB base-pack views, 1 GB whole
  sectioned files, 384 MB pages, 1.5 GB whole sections, 128 open NAS files).
- **Place search** (`crates/server/src/places.rs`, `web/src/ui/search.ts`; `/` opens it): the
  labels layer's places (every label of the map's areas, worldwide what shows by zoom 8), by any
  word of the shown name, own name or English, ignoring accents, case and punctuation (inside CJK
  names too, not Thai). Whole name first, then prefix, then inner word; then importance, then
  distance. Return or a click flies there and marks it. The index is built on this Mac at the first
  search after the labels or translations change, on three threads, and dropped after half an hour
  unused (its arrays mapped, so the memory goes back). Unreadable packs are passed over and the
  index rebuilt later.
- **Offline start:** the map kept on the Mac (below) and the pack indexes read so far stay local.
- **In use** means any request in the last ten minutes except the status polls (`/api/catalog`,
  `/api/ping`, `/api/downloads`) and the search's `poll=1`. A mirror's copy stopped by a new map
  resumes under it (copies are kept by content name). The share is mounted again, while it's away,
  every 30 s.
- **The map's log** (docs/maplog.md; `crates/server/src/maplog.rs`): the map served is the lead's
  log's, nothing else (no catalogue is read). A thread follows the log over HTTP (`GET
  /map/changes`, a 30-s long poll, its cursor `server/<host>`), applying each answer's batches to
  its own map (checked by their digest; the map takes ~0.5–1.5 GB) and the lead's snapshot on a
  reset; with the lead away, the NAS's newest `map/<seq>.json.zst` when it holds none or an older
  one. Each map is handed to the server as one catalogue, swapped whole, never an older one than
  served. Its map is kept in `~/Library/Caches/scenic/map/current.json.zst` (at most every 10
  minutes) and read at a start, before the server answers: a start away from the lead and the NAS
  serves it (the laptops' offline map, kept: owner's decision, 1 Nov; their server runs on demand,
  #882 step 7a: Mac apps, above). The lead's contact is kept
  beside it (`lead.json`, mode 600, the build's key in it, as on the NAS); losing the folder means a
  cold start (the NAS's snapshot). A server started on a folder laid out as the NAS's (`--root`)
  serves that folder's newest snapshot.
- **URLs and ETags:** data URLs carry a content version (`?v=`): a tile's is its pack's (meta
  `packs`), so a new catalogue re-fetches only the changed packs' tiles; a versioned response is
  `immutable` while current. ETags are content hashes, for named tiles combined with the versions of
  the languages spoken there and the spoken-languages raster (docs/names.md). The basemap's come from the
  catalogue, so its 304s read nothing.

**Mirror, the Mac mini's** (`store::mirror`, `store::pieces`, `crates/server/src/downloads.rs`).
- **Only what's downloaded.** The map reads a file from the Mac when it's there, else from the NAS
  while the NAS is read (mounted, and its I/O pool's breaker closed); away from it, from the Mac
  alone: a file not downloaded is none, as if the map had none there (`Data::content_in`, the one
  place the server chooses its sources), so every feature answers from the downloads, empty at
  once outside them, with no NAS read and no timeout. A share found mounted is read only once it
  has answered a probe, made at once (`IoPool::probe_first`): a mount left from home never holds
  a request; at home the map is the downloads until it answers (milliseconds; seconds while the
  NAS wakes), and an open page, asking for the catalogue every 5 s while the server says it's away
  (every minute else), takes the NAS's map within 5 s of it. Each change of whether the NAS is read
  drops what was made from the other side's reads (the catalogue generation); away, every version
  the app's URLs carry names where the downloads reach (`Data::reach_token`) and a 204 ("not
  downloaded") is never cached for good, so nothing answered away is reused at home. What's downloaded is this Mac's alone (`downloads.json` in its
  home).
- **The World, zoomed out:** the build's worldwide files (`global/`: only those a step makes, the
  catalogue lists no other), the Canadian lake fill
  (`work/lakefill/<date>`, read by the water's deeper zooms anywhere in Canada), every layer's root
  and lo packs (z0–8), markdata, ovdata, and the basemap's zooms 0–10: 19.3 GB on map 1658
  (6 Nov), the basemap's piece 1.6 GB of it. Not the pass's outlines. Away from the NAS it is the
  whole world to zoom 8 (the basemap to 10) and the place search's places that show by zoom 8,
  everywhere.
- **A region:** what the map reads over it at every zoom, so away from the NAS every feature works
  inside it without the World. Every layer's hi pack, the base pack and road values, the hi data,
  landmark points (markdata) and area details (ovdata, by z3 tile) of each z6 tile within 2 km of
  it, and the base pack and road values of the owners of the ways those tiles' hi data list whose boxes
  meet the region within 2 km (read once per hi data and region, from the Mac's copy or the NAS;
  a downloaded region's size counts them once read, a region not downloaded doesn't), so a way
  drawn in it that starts far outside has its info; the basemap's zooms 11–14 over those tiles; the worldwide `global/` files (those by z6 tile, such as the
  roads' English names and ids, only for its tiles); the Canadian lake fill when it meets Canada.
  Zoomed out (#985), only what lies over its z6 tiles of the files that cover far more, as pieces
  (below): of every layer's root (z0–2) and lo packs (z3–8), each tile of zooms 0–5 over its z6
  tiles and their zooms 6–8; of a polar layer's root, its tiles over the region's blocks; and of
  the basemap, the same at zooms 0–10. What a terrain or shading tile without its own is made
  from, and the labels the place search reads there. On map 1668 they add 28 MB to Jamaica (25 MB
  of the packs, 2.8 MB of the basemap; the whole root and lo packs over it and the basemap's
  zooms 0–10 were 2.6 GB), 29 MB to Prince Edward Island, and 2.4 GB to Canada's 14 regions
  together (9,592 pieces: 2.0 GB of the packs, 0.4 GB of the basemap); a piece shared with
  another region is copied once. A tile of zooms 0–5 still reaches past the region, as do the
  landmark points' one bundle and the area details' z3 files: away from the NAS, every tile shows
  only what lies inside the downloaded regions' z6 tiles (`Data::reaches`, `Data::cut`, at every
  zoom; the World's reach everywhere). One only partly over them is cut to them as it's served
  (crates/server clip.rs): a raster's pixels outside take the value the layer has where it has
  nothing (the terrain 0 m, the depth's no data, the shading flat, the water and classes none), a
  vector tile's points outside go and its lines and areas are cut at their edges (the basemap,
  labels, stations, areas, roads and rails, landmarks, ferries; a ferry leaving and
  entering them keeps its longest stretch inside); its ETag names where the downloads reach. So a small region stays alone when zoomed
  out, with nothing around it. The place search, a tile made from a coarser one (terrain,
  shading, water, depth), the lists of a view (drives, rides, rail lines: the hi data), the
  viewshed and the landmark points keep to them too, the area details (looked up by
  id, a z3 tile's file) to the z3 tiles meeting them; outside them every layer is none (the
  terrain and its shading flat). A region downloaded before its pieces (its
  list naming the whole root and lo packs over it, or the basemap's worldwide zooms 0–10) takes
  them at the catalogue served at its next round with the NAS, without an Update; the rest stays
  at its catalogue. Its whole files and zooms 0–10 piece here stay, and are what's read, until
  every piece replacing them is here (copied and checked); then they go at once (`bridge` in
  crates/server downloads.rs). Cut off at any point (the NAS gone mid-copy, the server stopped),
  away from the NAS it's its old form or its new one, whole. Away from the NAS, a
  whole road (hover, profile) leaving it is cut where this Mac's files end: the ways joined end to
  end through the one asked for, the profile marked cut short. While a downloaded region's hi data
  can't be read for its owners, no base pack or road values here go (a hi data that stays
  unreadable keeps them all).
- **A region's terrain detail** (an add-on to the region, which it needs): the terrain's deep packs
  and their shading's (z13 and finer, by z10 tile) within 2 km of it, where the terrain goes past
  z12; the panel says
  the share of the region they cover and their size first. Downloaded only while the region is;
  copied after everything else.
- **Pieces** (`store::pieces`): the parts of a worldwide file a download takes, each a file of the
  same kind of its own, shared by every download naming it. The basemap's
  (`mirror/.basemap/<archive hash>/`), each a PMTiles archive: `lo` (zooms 0–10, the World's),
  `z-x-y` (one tile of zooms 0–5), `mid-6-x-y` (a z6 tile's zooms 6–10) and `6-x-y` (its 11–14). On
  PMTiles' Hilbert curve a z6 tile's tiles at each zoom are one run, so a piece is a few directory
  runs and mostly sequential reads. A root or lo pack's (`mirror/.packs/<pack hash>/`), each an
  RDPACK of its tiles alone, as the server reads any pack: `z-x-y` and `mid-6-x-y` (a z6 tile's
  zooms 6–8). The server reads a pack's tile from the pack when it's here whole, else from its
  piece here (`Data::pack_key`), and the app's URLs version it by what it's read from. Sizes are
  known ahead from the archive's directory or the pack's index (`sizes.json`); a piece cut short
  resumes; each tile's gzip checksum (the basemap's) or XXH3 (a pack's) is checked. Pieces rather
  than a sparse copy: removed by deleting, sized as packs are, shared, opened as any archive or
  pack.
- **Updated only when asked:** each download (the World, a region, a region's terrain detail) stays
  at the catalogue it was downloaded or last updated at (`downloads.json`); a new catalogue copies
  nothing. Its row in the Regions panel then says it's out of date, with what changed and the size
  to copy, and offers **Update**. Meanwhile the map reads the download's own older files first, the
  NAS for what it lacks. An update copies the catalogue served's files beside the old ones (so the
  region keeps working offline throughout), then moves the download to that catalogue, and the
  files only the old one named go. **Whole tiles:** an update moves every z6 tile the download has,
  including those it shares with another download (a neighbouring region's tiles, or its ways'
  owners'), and copies the other download's files of those tiles too (base pack, hi packs, lists,
  road values, basemap piece), so the map never reads one tile from two catalogues. The other
  download keeps its other tiles at its own catalogue, still out of date for those alone (its size
  to copy counts only them). A road crossing from its old tile into a moved one may lack its info at
  the crossing until it's updated too (a way not found, never a wrong one). The World moves no
  tile: its per-tile files (roads' English names, road ids) index no base pack, so updating it copies
  none of a region's. A download moves on by
  itself only when that copies nothing (all an update would copy is here: its files are the same in
  the new catalogue, or other downloads brought them; or its catalogue didn't have it yet), or when it has
  nothing whole to keep (not all here when the new catalogue came: the map never mixes a partial
  download's older files with the NAS's newer ones). Each download keeps its own
  list of its files at its catalogue (`downloads.json`), so what changed and the size to copy come
  from its list against the catalogue served, however far behind it is; no older catalogue is kept
  for it. Removing a download deletes what no other
  download names. Nothing else goes, ever.
- **Copies:** one file or piece at a time, the World first, then small worldwide files, root and lo
  packs, hi data and road values, base packs, hi packs, buildings, basemap pieces. They run whenever
  the NAS is reachable, from home or not (no rule holds them back away from home). While the build
  runs (the lead's heartbeat says a job runs) copies keep to 20 MB/s. Each copy is checked before
  it's moved into place. Each downloading or updating row shows its own progress bar.
- **Its cursor in the map's log** (`mirror/<host>`, `crate::maplog::MirrorLink`, docs/maplog.md
  §6.2): each round with the NAS, before it copies, it tells the lead the oldest map whose files it
  still copies (the one served; an Update's older one until its files are here), so GC keeps them,
  and the map its round copies and the last it got through at, which the lead notes in the history
  (`mirror-start`, `mirror-end`, for `scenic timings publish`). The cursor is made each round from
  `downloads.json` and the files here, so a restart says the same one. One the lead resets (older
  than its log, or not held by GC: new, or unheard or at one map `CURSOR_TTL_S`, 6 hours) moves
  each Update of an older map on to the one served, whose files the NAS has, once the server has
  caught up to the lead's head (a start or a wake serves the map it kept until then).
- **Room:** a per-Mac reserve ("Keep free", 30 GB unless set; `reserve.json`, `PUT
  /api/downloads/reserve`), applied at once. When this Mac's build agent's room target (`scenic
  room`, docs/building.md §2) is higher, the mirror keeps that instead (seen within 5 s). A
  download that wouldn't fit is refused; when the disk fills later, what doesn't fit waits, and nothing downloaded goes for it.
- **The agent's jobs** don't need the mirror, but read some files from it where present: pack and
  lo their owners' base packs and road values (whole, mapped, in place of their part reads), the marks and overlay conversions the heritage
  job's files. Each is read under a shared lock (`store::cachefile`). The mirror deletes a file
  only under an exclusive lock taken without waiting, so a file a job holds stays until a later
  sync.
- **Pack indexes** are cached as they're read (`idx/`), for offline starts.
- `/api/downloads` gives the panel its state.

**Devices: an iPhone, an iPad, the other Macs.** The map is the M6's (always on), over the
tailnet: **https://map.brandontsang.net**, or `tailscale serve`'s
`https://mac-mini-de-brandon.<tailnet>.ts.net:8443`. No other Mac runs a map server (task #874).
- **map.brandontsang.net** (`tools/caddy`): its DNS record (Cloudflare, DNS only) is the M6's
  Tailscale address, so only the tailnet reaches it. Caddy on the M6 (`brew install caddy`; the
  LaunchAgent `local.scenic.caddy` from `tools/caddy/install.sh`, log
  `~/Library/Logs/scenic/caddy.log`) serves it on port 443 with the owner's Let's Encrypt
  certificate (`~/certs/map.crt`, `map.key`; acme.sh with Cloudflare DNS renews it and restarts
  Caddy, the owner's token never in the repo): `/` the map (:7311), `/build` the build page (the
  agent, :8090, the page at its root: `/build/`), `/dev` the dev tools app (:8077: `/dev/task-board/` the
  board, `/dev/jobs/` the dev job dashboard; `/task-board/…` goes to `/dev/task-board/…`). The macOS firewall
  must let Caddy in (`socketfilterfw --add` its Cellar binary, again after `brew upgrade caddy`).
  `tailscale serve` holds port 443 on the tailnet address whatever the name asked (it ends any
  other name's TLS), so the M6's own `tailscale serve` of the agent is on 8444 instead (pool.md §14).
- **Who's answered:** the server answers only this Mac, its LAN and the tailnet, also through a
  proxy on this Mac (`tailscale serve`, Caddy) from those alone (`coordwire::net::reached`); anything
  else is 403, Funnel included. No key.
- **A page elsewhere is never the map's:** the `Host` must be an address, localhost, a name only a
  tailnet or local network resolves, or a name the owner gave this Mac (`~/.config/scenic/names`, a
  name a line, written by `tools/caddy/install.sh`; `coordwire::net::names`, read again each minute,
  so a new name is taken within a minute, no restart): DNS rebinding refused.
  An `Origin` must be the map's own page or this Mac's; CORS only to this Mac's own pages.
- **The address** is `<home>/map-page` (`crates/server/src/remote.rs`): the first name in
  `~/.config/scenic/names` (https), else HTTPS where `tailscale serve --bg --https=8443
  http://127.0.0.1:7311` proxies the server, else the tailnet address. `scenic status` on the
  mini gives it. Only a proxy that says so (`X-Forwarded-For`) keeps its
  requests from passing for this Mac's own.
- **An app** (`web/public/manifest.webmanifest`): Add to Home Screen. Its service worker
  (`web/public/sw.js`, HTTPS only, never on localhost) keeps the page, its scripts and the catalogue's
  metadata, plus the fonts, icons and `immutable` data as viewed (12,000 files; 3D buildings' tiles
  in a cache of their own, 2,000). A Mac that doesn't answer within 4 s is taken as away for a
  minute.
- **Touch** (`web/src/trackpad.ts`, `web/src/ui/touch.ts`): one finger pans; two pinch, rotate and
  tilt; double tap zooms; a long press offers Street View, Google Maps and OpenStreetMap. Popups
  stay on screen.
- **A phone's layout:** settings fold into a panel; lists become a bottom sheet (a card on its
  side). On a tablet the panel stays docked. Touch screens get bigger controls.
