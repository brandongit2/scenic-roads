# Macs

Each Mac's agent, server and mirror, and the devices the map opens on. The pool itself is `docs/pool.md`; tasks for any worker, `docs/workers.md`.

**Each Mac's agent.** Every Mac runs the agent (`scenic agent`, set up by `tools/app/install.sh`), a
member of the pool (docs/building.md §2, The pool): it leads while its Mac holds the term, else takes work from the
lead's coordinator. Below, the M4 (48 GB) unless said.
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
  - **AWS's raw terrain tiles** are packed onto the NAS (`pipeline::rawpack`,
    `sources/aws-terrarium/packs/`, listed in `index.json`) in archives grouped as the terrain's
    packs are (z9–12 by z6 tile, z3–8 by z3 tile, z0–2 together). A job's new tiles go up at its end
    as one archive beside its shard's others (the NAS's small-file writes run at ~23 a second). A
    shard's archives are each kept more than twice the size of all those after it, else merged, so
    a shard has a dozen at most. Archives leave the index through its `gone` list and are deleted a
    day later, so a reader of the old index stays right. Only the lead changes the index; a job
    puts its archives there unlisted and hands them off. A tile is read by range for a shard's first
    16, then each archive is copied whole once and checked against its name.
  - **The Wikidata and Wikipedia answers** the items job and the heritage chain got for a pass
    (`pipeline::answers`) are kept as one archive per pass (`sources/items/<date>/answers.tar.zst`,
    `heritage.tar.zst`), written as the step starts (when the NAS lacks what the Mac has) and as it
    ends. The two sides are merged by key where both changed; a file deleted by hand stays deleted;
    an archive that doesn't read whole is moved aside, not written over. A new pass's answers start
    from the last pass's.
  - **Whole:** each copy is written by a temporary name, flushed and its length checked before the
    rename, and checked whole when read (`pipeline::whole`). One that isn't is taken
    again from the NAS, else the source.
  - **"Not there"** (a 404, or S3's 403) is remembered only once the source says so twice.
  - **What's fetched again** is only new data (a planet, Wikidata facts and pageviews, timetables, an
    Overture release) or a window not yet covered (DEM points not sampled before, WorldCover tiles
    the packs lack).
- **Caches** (`~/Library/Application Support/scenic/agent/cache`): what room-making or a clear empties is a
  copy of what the NAS keeps, or made from it, refilled from the NAS, never the internet.
  - **The cheap ones** (emptied for room, cleared; kept after a build while the disk has room): canopy squares
    (`chm10/`), raw terrain tiles and archive copies (`aws-terrarium/`),
    copies of record files staging reads (`blobs/`: hi packs, roadside buildings' z8 tiles, z6 tiles'
    kept DEM samples), pageview month indexes (`items/months/`).
  - **Cleared too, on the owner's ask:** the pack cache (`base/`, the base packs' way indexes for
    pack(T) and lo), the
    per-vertex DEM seed (only while the NAS has it whole), local copies the summits and peaks jobs
    read, and the heritage job's clip of the pass's `whs` set (made again by osmium).
  - **Kept:** the answers (`items/`, `heritage-<date>/`), the trains' stop pairs (`rail/`), the z6 tile
    stages' timings (`unit-stages.json`), what a z6 tile kept that isn't on the NAS yet, and the
    coverage as last read (`coverage/<hash16>.cov`, by the regions' accepted version).
  - Nothing is deleted through a link, nor anything in the NAS's project folder.
- **Its own map** is served from the NAS and what it has downloaded (Mirror).
- **An OSM pass** starts with 80 GB free (the pack cache counting as free) and copies the planet to
  the SSD when there's room, else reads it from the NAS.
- **What a z6 tile keeps for its later runs** is on the NAS (`cache/`), shared by every Mac: its DEM
  samples (`dem-units/`) and canopy and view results (`scenic-units/`, docs/steps.md §6). A z6 tile's build
  folders go once it's built.

**The M1 (16 GB)** leads when it holds the term; as a member it builds what the lead's coordinator
gives it (docs/building.md §2, The pool): any shared step's jobs that fit the memory its owner gives the build there
(`scenic memory`, within the memory guard's limit: docs/pool.md §7.1) and its disk (its room target kept free past each job's measured disk; a task 5 GB, tree cover's rows and terrain's subtrees 1 GB), from the far end of the list (terrain from the
near end), else z6 tiles' tails (`docs/workers.md`). A z6 tile's predicted peak is the most its job's
processes held together last time, else about ten times its extract, never under 3.7 GB nor below its
floor (docs/pool.md §7.2). A member's status is `state/helpers/<host>.json`, the lead's the heartbeat
(`state/status.json`).

**The Mac mini (M6, 32 GB, 1 TB)** is always on, at home and headless: a member of the pool like any
other Mac, with room for more at once (build memory 28 GB by default). It also hosts the task board
(`tools/taskboard`, http://mac-mini-de-brandon:8077, its LaunchAgent and nightly NAS backup from
`tools/taskboard/launchd/install.sh`) and is the map's address for the phone, the iPad and devices
away from home: https://map.brandontsang.net (Devices). It publishes the app (`ssh m6 'cd
~/projets/scenic-roads && tools/app/publish.sh'`, docs/building.md, App publishing), its tests
run on the test Mac (`SCENIC_TEST_HOST`, default `m6`: itself, so locally; `tools/app/remote-tests.sh`). Its toolchain: brew's mise
and rustup (`/opt/homebrew/bin`, `~/.cargo/bin`, which publish.sh puts on the PATH itself) and,
from `mise install` in the checkout, everything mise.toml and rust-toolchain.toml pin (Rust with
its wasm32-wasip1 target, the wasi-sdk, Node, pnpm, uv, cargo-nextest, CMake); Xcode for swiftc
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
6. `tools/app/install.sh` (it also creates the map server's LaunchAgent), then open Scenic.app once.
7. Allow `scenic` and `server` through the firewall (pool.md §6.8).
8. `scenic memory` says the build's memory (total − 4 GB unless set); the room target is the
   owner's choice.

**App Macs (both laptops).**
- **The launcher** (`tools/launcher/launcher.c`) runs the command in `~/Library/Application
  Support/scenic/run/<name>` and restarts it when it exits: `bin/scenic-launcher server` (the map
  server's LaunchAgent), and Scenic.app's own copy running `agent` (the build service; there an
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
- **The updater** checks the NAS's `app/current.json` every five minutes, copies a newer app locally
  checking every file's SHA-256, and restarts into it when the map is idle. It keeps the current
  version, its own, the agent's and one to roll back to.
- The app reads the current and the previous form of every file kind.

**Server.**
- **Finding the NAS:** `getfsstat(MNT_NOWAIT)`, never `statfs`. At home (`fishandchips.local`
  answers on 445) it mounts the share with `osascript` and the Keychain, at most every five minutes,
  by the LAN name when the Keychain has it, else the bare (Tailscale) name. `nsmb.conf` makes it a
  soft mount.
- **Stale SMB handles** (docs/pool.md §3): a Mac that dies with files open on the share (kill -9,
  Migration Assistant, a power cut) leaves its session's handles on the NAS until the server drops
  the session. Writers rename a held file aside meanwhile (`crate::whole`). The build agent heals
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
  Mac's dead session, most likely: disconnect it in DSM) and the interval doubles, up to 4 hours,
  back to 30 minutes after 10 quiet ones. A remount ends only this Mac's session. (The "home
  again" remount from the tunnel to the LAN name still waits for idle, with no drain: the tunnel
  works meanwhile, only slower, so stopping new work for it isn't worth it.)
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
      keepalive, minutes), which DSM doesn't expose; until then only the DSM disconnect frees it.
  - Each Mac, `/etc/nsmb.conf` (system-wide; `sudo`, applies to mounts made afterwards), scoped to
    the NAS (a section per name it's mounted by), not `[default]`:

    ```
    [fishandchips]
    soft=yes
    max_resp_timeout=20
    notify_off=yes

    [fishandchips.local]
    soft=yes
    max_resp_timeout=20
    notify_off=yes
    ```

    `soft=yes` fails calls on a dead or unreachable NAS instead of hanging them (the agent already
    keeps it in the per-user `~/Library/Preferences/nsmb.conf`); the risk is for a program writing a
    file in place, which can be left part-written by a failed call (the pipeline's writes go to a
    temporary name and rename, so a cut write never takes the name). `max_resp_timeout` (seconds)
    shortens how long a call waits for an answer. `notify_off=yes` drops change notifications, one
    fewer long-lived handle per open folder, but Finder windows on the share no longer refresh by
    themselves. These keep this Mac from hanging on the NAS; they can't shorten what the server keeps
    for a dead Mac. If a session still holds files after a crash: DSM → Control Panel → File
    Services → SMB (or Resource Monitor → Connections), disconnect that Mac; the agent's warning
    names the file.
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
- **Offline start:** the last catalog and the pack indexes read so far stay local.
- **In use** means any request in the last ten minutes except the status polls (`/api/catalog`,
  `/api/ping`, `/api/build`, `/api/downloads`) and the search's `poll=1`. It checks for a newer
  catalog every 10 s while the lead's heartbeat (`state/status.json`, read at most every 30 s)
  says a build runs, else every 30 s in use and every 10 minutes otherwise; each check is one
  listing of `catalog/`. On the lead's Mac the agent also POSTs `/api/catalog/refresh` (this Mac's
  own requests alone) after writing a catalog, so it is adopted at once. A mirror's copy stopped by a
  new catalog resumes under it (copies are kept by content name).
- **URLs and ETags:** data URLs carry a content version (`?v=`): a tile's is its pack's (meta
  `packs`), so a new catalog re-fetches only the changed packs' tiles; a versioned response is
  `immutable` while current. ETags are content hashes, for named tiles combined with the versions of
  the languages spoken there and the spoken-languages raster (docs/names.md). The basemap's come from the
  catalog, so its 304s read nothing.

**Mirror, per Mac** (`store::mirror`, `store::pieces`, `crates/server/src/downloads.rs`).
- **Only what's downloaded.** The map reads a file from the Mac when it's there, else from the NAS.
  What's downloaded is this Mac's alone (`downloads.json` in its home).
- **The World, zoomed out:** the build's worldwide files (`global/`: only those a step makes, the
  catalog lists no other), the Canadian lake fill
  (`work/lakefill/<date>`, read by the water's deeper zooms anywhere in Canada), every layer's root
  and lo packs (z0–8), markdata, ovdata, and the basemap's zooms 0–10: about 10 GB. Not the pass's
  outlines.
- **A region:** every layer's hi pack and the base pack and road values of each z6 tile
  within 2 km of it; the basemap's zooms 11–14 over those tiles; the terrain's and grids' hi packs
  within 25 km (the viewshed's reach); the hi data, landmark points (markdata) and area details
  (ovdata, by z3 tile) within 50 km; the worldwide `global/` files (those by z6 tile, such as the
  roads' English names and ids, only for its tiles); the Canadian lake fill when it meets Canada.
  So a region works zoomed in without the World; only zoomed out is missing (but task #457).
- **A region's fine terrain** (an add-on to the region, which it needs): the terrain's fine packs
  (z13 and finer, by z10 tile) within 2 km of it, where the terrain has finer data; the panel says
  the share of the region they cover and their size first. Downloaded only while the region is;
  copied after everything else.
- **The basemap's pieces** (`mirror/.basemap/<archive hash>/`): each a PMTiles archive of its own,
  `lo` (zooms 0–10) and `6-x-y` (a z6 tile's zooms 11–14, shared by every download meeting it). On
  PMTiles' Hilbert curve a z6 tile's tiles at each zoom are one run, so a piece is four directory
  runs and mostly sequential reads. Sizes are known ahead (`sizes.json`); a piece cut short resumes;
  each tile's gzip checksum is checked. Pieces rather than a sparse copy: removed by deleting, sized
  as packs are, shared, opened as any archive.
- **Kept current:** each new catalog's files a download names are copied, and what no download
  names any more goes. Nothing else goes, ever.
- **Copies:** one file or piece at a time, the World first, then small worldwide files, root and lo
  packs, hi data and road values, base packs, hi packs, buildings, basemap pieces. While the build
  runs (the lead's heartbeat says a job runs) copies keep to 20 MB/s. Each copy is checked before
  it's moved into place.
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
- `/api/downloads` gives the panel its state; the menu bar says what's downloaded.

**Devices: an iPhone, an iPad.** Any Mac's map opens on them over the tailnet; the address they keep
is the M6's (always on): **https://map.brandontsang.net**. Each Mac keeps its own map server for
itself.
- **map.brandontsang.net** (`tools/caddy`): its DNS record (Cloudflare, DNS only) is the M6's
  Tailscale address, so only the tailnet reaches it. Caddy on the M6 (`brew install caddy`; the
  LaunchAgent `local.scenic.caddy` from `tools/caddy/install.sh`, log
  `~/Library/Logs/scenic/caddy.log`) serves it on port 443 with the owner's Let's Encrypt
  certificate (`~/certs/map.crt`, `map.key`; acme.sh with Cloudflare DNS renews it and restarts
  Caddy, the owner's token never in the repo): `/` the map (:7311), `/build` the build page (the
  agent, :8090; the page at `/build/work/`), `/task-board` the board (:8077). The macOS firewall
  must let Caddy in (`socketfilterfw --add` its Cellar binary, again after `brew upgrade caddy`).
  `tailscale serve` holds port 443 on the tailnet address whatever the name asked (it ends any
  other name's TLS), so the M6's own `tailscale serve` of the agent is on 8444 instead (pool.md §14).
- **Who's answered:** the server answers only this Mac, its LAN and the tailnet, also through a
  proxy on this Mac (`tailscale serve`, Caddy) from those alone (`pipeline::net::reached`); anything
  else is 403, Funnel included. No key.
- **A page elsewhere is never the map's:** the `Host` must be an address, localhost, a name only a
  tailnet or local network resolves, or a name the owner gave this Mac (`~/.config/scenic/names`, a
  name a line, written by `tools/caddy/install.sh`; `pipeline::net::names`, read again each minute,
  so a new name is taken within a minute, no restart): DNS rebinding refused.
  An `Origin` must be the map's own page or this Mac's; CORS only to this Mac's own pages.
- **The address** is `<home>/map-page` (`crates/server/src/remote.rs`): the first name in
  `~/.config/scenic/names` (https), else HTTPS where `tailscale serve --bg --https=8443
  http://127.0.0.1:7311` proxies the server, else the tailnet address. The menu's Copy the Map's
  Address and `scenic status` give it. Only a proxy that says so (`X-Forwarded-For`) keeps its
  requests from passing for this Mac's own.
- **An app** (`web/public/manifest.webmanifest`): Add to Home Screen. Its service worker
  (`web/public/sw.js`, HTTPS only, never on localhost) keeps the page, its scripts and the catalog's
  metadata, plus the fonts, icons and `immutable` data as viewed (12,000 files; 3D buildings' tiles
  in a cache of their own, 2,000). A Mac that doesn't answer within 4 s is taken as away for a
  minute.
- **Touch** (`web/src/trackpad.ts`, `web/src/ui/touch.ts`): one finger pans; two pinch, rotate and
  tilt; double tap zooms; a long press offers Street View, Google Maps and OpenStreetMap. Popups
  stay on screen.
- **A phone's layout:** settings fold into a panel; lists become a bottom sheet (a card on its
  side). On a tablet the panel stays docked. Touch screens get bigger controls.
