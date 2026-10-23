# The user's surface

**The map is always there.**
- A launcher (`tools/launcher`, a LaunchAgent) keeps the server running on both Macs; an idle server
  loads nothing. The user opens `http://localhost:7311`.
- Zoomed out, every road is drawn at every zoom. The water zoomed out looks as full detail rendered
  and shrunk would: an island or lake smaller than a pixel is as faint as its share of the pixel,
  never exaggerated nor dropped, with the coastal shading full detail gives it (docs/steps.md §4).
- **Scenic** (`/Applications/Scenic.app`, `tools/status`) is an app opened from Launchpad, Spotlight
  or /Applications, and at login; it has no Dock icon (LSUIElement), only its menu bar item and its
  window, and its icon is built from `tools/status/icon`. Opening it starts this Mac's build service (the agent: the app's
  own launchd job, registered with SMAppService and listed under Scenic in Login Items; launchd
  restarts it after a crash, and it restarts itself into an update). Quitting stops it: at once with
  no job running; with one, the app asks "Finish This Job First" (nothing new taken, the app quits
  when it ends) or "Stop Now" (the job given back, its work kept on disk); a logout, restart or
  shutdown takes Stop Now without asking. A leading Mac quitting hands the lead to a member that can
  take it (waiting up to three minutes for the handover), else the build waits for it
  (`control::Quit`). The map server stays a service of its own: Scenic's window and menu show it
  and start or stop it when asked, never on opening or quitting. On an update Scenic copies the
  installed version's Scenic.app over itself and quits; a helper it starts (the new bundle's
  binary, `--reopen`) waits for it to end, then opens the new app (without its window) after a
  second, checks that it runs, and retries, backing off, for about two minutes (an `open` sent
  while LaunchServices still counts the old process goes to it and is lost), each step in
  `~/Library/Logs/scenic/update.log` (cut to its last 512 KB past 1 MB); if the helper can't
  start, the old app runs on and the new one opens at its next launch. Opened, and
  each poll, it checks that the build service's launcher runs from its own bundle (macOS's
  network-volume permission is granted to /Applications/Scenic.app, not to the old copy an update
  leaves aside, `.Scenic.app.old-<pid>`); a launcher whose executable can't be read counts as
  elsewhere. If not, it restarts the job (`launchctl kickstart -k`, at most once a minute;
  registering it again if that isn't enough): the agent's jobs, in process groups
  of their own, run on through it, and the agent it starts takes them over (docs/building.md §2, Restarts). It
  deletes the old copies
  nothing runs from (the swap itself leaves them). launchd keeps a launch constraint (LWCR) for
  the job from its registration, naming the launcher's cdhash: a launcher signed otherwise (any
  change to `tools/launcher/launcher.c`, its build flags or its signing in `tools/status/bundle.sh`)
  is refused at every spawn (`launchctl print gui/$UID/local.scenic.build`: "job state = spawn
  failed", "last exit code = 78: EX_CONFIG", properties "needs LWCR update"). So the app notes the
  cdhash it registered with (its defaults, `registeredLauncherCDHash`), and opened with another
  launcher (an update that changed it; the swap logs it) it unregisters and registers the job
  again before starting it; and opened, and each poll, a job launchd says it can't start
  (`spawnBroken`: needs LWCR update, or spawn failed with exit 78) is registered again the same
  way. Registering right after an update's swap can keep the old constraint (the job still "needs
  LWCR update"; on the M4, 23 Oct, the next registration 30 s later took), so it's done again while
  the job is refused, 30 s apart, 4 times a launch at most (with the restart's fallback above),
  each step in update.log; the
  window says "launchd can't start it". By hand: `scenic-status --unregister`, quit, open Scenic.
  `scenic-status --test tools/status/test` checks the detection against captured
  `launchctl print` output, the reopen helper and the arguments (check.sh's swift part runs it).
  Strays (#582): after registering again, and each poll, a launcher of this app or of a copy left
  aside that started before the job's own is ended (launchd's state read just before; SIGTERM to
  that pid, logged once). An agent its launcher starts that finds the agent's lock held by an
  orphan agent (ppid 1: a launcher's left behind, but also an agent run by hand and detached, which
  is ended the same way) or by an older launcher at the same path's agent ends that one (the pid
  read again just before the signal, its start and executable unchanged) (SIGTERM to the orphan, or to the older launcher, which passes it
  on) and takes the lock once it lets go, waiting up to 150 s; the stopping agent leaves its jobs
  running for it, as for an update (crates/pipeline/src/agent/takeover.rs). An agent run by hand,
  or under a newer launcher, ends nothing. Before each open the reopen helper ends a running copy
  of the app with its bundle id at another path (`open` would only activate it), and
  `scenic-status` refuses an unknown argument (exit 2) and prints its usage for `--help`/`-h`,
  never starting the app. When the agent's log shows
  "Operation not permitted" on the NAS since it started, the menu and window say the build service
  can't reach the NAS (allow Scenic in Files and Folders).
- **Switching a Mac over** from the login items Scenic replaces (`local.scenic.agent`,
  `local.scenic.status`): until then the menu bar item runs from `app/current` as before, and a
  copy of Scenic run from inside an installed version, or while `local.scenic.agent` is loaded or
  its plist is in `~/Library/LaunchAgents`, manages nothing (it says "Switch over with
  tools/app/install.sh"); it asks again at each poll and as its menu opens, and once the old
  agent's job and plist are gone it registers and starts its build service as a fresh launch
  would. On each Mac once the version is installed, the lead's Mac last (or hand
  the lead away first, `scenic lead give <the other Mac>`): run `tools/app/install.sh` (it removes
  the two login items, the old agent's job given back, copies Scenic.app to /Applications and
  opens it), allow Scenic in Login Items if macOS asks, and check its window says "Build service:
  running". The way back, until both Macs have switched: `tools/app/install.sh --legacy` (Scenic
  unregistered and removed, the two login items as before), then `tools/app/publish.sh --rollback`
  if the version itself is to go.
- `scenic status` and the menu bar item (Scenic's) show the build's state. A click
  opens the build page in a popover; the menu pauses and resumes the build, hands over the lead,
  sets the disk room and clears the Mac's build caches (`scenic clean`). It notifies on every change.
- Every program checks its command line first (docs/building.md, "Command lines"): `--help`/`-h`
  prints usage; an unknown flag, a missing value or an extra argument is refused with usage (exit 2).
- The server mounts the NAS itself when it's missing, at home.

**The Regions panel** (Settings → Regions):
- The regions, read only, grouped by their folders, each with its input-check state (accepted,
  checking, held), and on the map the published coverage and each region's accepted outline.
  Regions change only as files in `inputs/regions/` (docs/inputs.md §6.1); the app writes none.
- **Downloads on this Mac** (docs/macs.md, Mirror): nothing is copied to a Mac unless downloaded there.
  - **World, zoomed out**: its own row, Download or Remove, downloaded on its own (a region doesn't
    bring it; it can go while regions stay). Without it the map needs the NAS; while a region is
    downloaded and the World isn't, the row is marked and warns that away from the NAS, zoomed out,
    the map shows nothing.
  - **Each region**: its size, Download or Remove, and its state (downloaded, copying N %, next,
    waiting for room, away). A region still building says "x of y tiles, on the map as they come"
    (every z6 tile built but not yet recorded built: "finishing: x of x tiles, the last map tiles and
    layers being made")
    (its catalog's `tiles`; the menu bar and `scenic status` say the same, from the agent's
    status) and can be downloaded: its download grows with each catalog, what's here never fetched
    again, copied at the build's slow rate.
  - **Fine terrain** (a region's second line, only where it has any): the share of the region the
    terrain finer than z12 covers, its size, Download or Remove, its state. An add-on: its Download
    is disabled ("needs the region") until the region is downloaded, and the server refuses it
    before; removing the region removes it.
  - Remove confirms with the size. A download that wouldn't fit above the reserve is refused.
  - The Mac's line: what's downloaded, free space, the reserve, the copy under way, and that copies
    keep to 20 MB/s while the build runs.
- The status bar offers to reload when new data or a new app is in, and says when the NAS or the map server can't be reached.
- New areas appear as their catalog goes out, a z6 tile's tiles within minutes of its build (docs/building.md §2, Publishing).

**Translations and descriptions** (docs/names.md) are drop boxes of the input check: `inputs/translations/` and
`inputs/descriptions/` (docs/inputs.md §6.3, §6.4). A drop shows on the map a few minutes after it's
accepted, with nothing rebuilt. After each build the agent lists what still lacks English or a
description in each folder's `todo/`, with the briefs.

**Everything else is automatic:** building whenever a Mac of the pool is awake and reaches the NAS
(at any charge; away from home through Tailscale, slowly), copying downloads and keeping them
current, emptying build caches once the build is done, installing a newly published app when the
map is idle, backups. Publishing the app is a command (`tools/app/publish.sh`, docs/building.md §2, App publishing).
