# Roles: the mini serves, the laptops work

**Status: planned (task #882); built: steps 1–5 of §5; steps 6–9 planned.** §8 is the audit
again after step 5 (6 Nov): every service on each Mac, measured, where it runs and how the
members reach it. Until each step below lands, docs/pool.md,
docs/agent-parts.md and the code are what is true. It builds on #881 (the Mac mini is the fixed
lead), #874 (the map server runs on the mini alone), #871 (the agent in parts: planning on the lead
only; steps 6–7 take the checklist and regions out of a member's status), #883 (the map log on the
lead), #876 (page helpers removed) and docs/coordination.md (#878: coordination by HTTP).

The owner (30 Oct, architecture item 6): the mini serves (lead, map server, catalogues and
publishing, GC, forecast, build-page data); the laptops are workers only, running jobs plus a thin
status for their own menu bar and page.

## 1. One role, decided once

Today each service decides for itself whether it runs here: `o.helper`, `self.coord.is_some()`,
`keeps_publishing()`, `!lead` in `tend_pageview`, `retire::retires` in the server, the pool's
`Role` flipped by `change_part`. Several look at different things, and some look at none (the
regions' states, the build page's readers, the broker's phase shares run on every Mac).

The fix is one role, read once from the declared lead:

- `pool::term::role(nas, me) -> Role::{Lead, Member, Unknown}`: `state/build/lead-member.json`
  (`term::declared`) against this Mac's member id. `Unknown` (unread NAS, no member file yet)
  starts the member's set, which is a subset of the lead's, and is read again each minute until
  known. The server's `retire::retires` calls the same function.
- `Agent::new` builds the parts for the role (`Parts::lead()` / `Parts::member()`), and nothing
  below asks again. A change of `lead-member.json` (the owner moving the lead, rare under #881)
  restarts the agent into the new role through the existing restart path (`pool_restart`), the
  way a new app does: no in-process demotion or promotion.
- `o.helper`, `change_part`'s promotion and demotion, `become_broker` and the coordinator kept on
  as a broker go; the member's broker is its own small type (§3.2).

## 2. Audit (31 Oct, app 20261031-0527-09ca51fd)

Measured read-only on both laptops: `ps`, `top`, `footprint`, `sample` (5 s) of the agent,
`du` of `~/Library/Application Support/scenic`, and `~/Library/Logs/scenic/agent.log` since the
agent last started.

| | M1 (16 GB, no job running) | M4 (four terrain-deep jobs running) |
| --- | --- | --- |
| Agent CPU | 108 % at the sample, 25 % of a core averaged over 31 min (7:46 CPU) | 45 % of a core averaged over 47 min (20:51 CPU), outside its jobs |
| Agent memory | 2.8–3.0 GB footprint (39 MB resident) | 4.0 GB footprint (1.4 GB resident) |
| Agent threads | 44 | 145–265 (178 of them `budgets-relay`/`budgets-conn`) |
| Loop stalls | "the checklist and counts" took 932, 126, 107, 48, 43 s; one stall sample written | the same phase 13–30 s |
| Menu bar (`scenic-status`) | 25 MB, 0.3 % | 24 MB, ~0 % |
| Disk in the home | `agent/` 13 GB (cache 4.8, scratch 6.2, backups 1.1, pack-idx 0.55), `mirror/` 15 GB, `accepted/` 0.9, `idx/` 0.28, `catalog/` 0.12 | `agent/` 69 GB (cache 55, scratch 12, backups 1.1, pack-idx 0.55) |

On the M1 the sample put the whole busy core on the `region-states` thread
(`build::region_states` → `unit_keys` → `coverage::Shape::meets_rect`, and serialising `Reach`
for the terrain units' keys), taking 0.7–131 s a run, started after every job end and every 5 min.
The member read the indexes of all 3,934 terrain packs for it (358 s over SMB on the M1). The pool
thread spent ~10 % serialising mail (`Side::save_mail`).

| Service | Runs on members today | Cost measured | Decision |
| --- | --- | --- | --- |
| Jobs (`jobs` part, slots, ramp, memory and disk guards, `memory-guard` sampler) | yes | the work itself | **keep** |
| Grants, beats, hand-offs, outbox (`helper_grants`, `send_outbox`, `tell_owed`) | yes, NAS + HTTP | small | **keep**, moving to HTTP under #878 |
| The member's broker (tasks of its own jobs, `/work/*` for its jobs) | yes, a whole `Coordinator` made `become_broker` | 5 `coordinator` threads | **keep, slimmed**: a broker type of its own (§3.2) |
| The broker's phase shares (`phase-shares` thread: the timings log read every 60 s) | yes | a thread; 4.5 MB log (the M1's is a stale lead copy) | **drop** on members |
| The budgets' proxy and relays | yes | 1 + up to ~180 threads, one per job connection (before #908) | **keep** (jobs need it); since #908 its connections share a few threads (§6) |
| Live store (`live`, `live-nas`, `live-sampler`: this Mac's measures) | yes | 21 MB file | **keep**; the lead's page reads it over HTTP |
| NAS mount, heal, NAS read/write pools (`nas`, `nas-read-*`, `nas-write-*`, `nas-switch-*`, `nas-probe`) | yes | 13 threads | **keep** (jobs read and write the NAS); the switch pool goes with #878 step 5 |
| Pool step (`pool` thread: mail, beat, hold on the NAS) | yes | ~10 % of a core at the sample | **keep** until #878 steps 3–6 replace it with HTTP |
| Control, beat, app updater, loop watchdog | yes | small | **keep** |
| Caches: room target, freeing, clears, `ask_upcoming` (`/work/tocome` from the lead) | yes | cache 4.8 / 55 GB (the work's) | **keep**; already by HTTP |
| Checklist (`checklist`: records brief, outlines; with each plan, its regions' part the plan's) | yes | the 13–932 s loop phase | **drop** (#871 steps 6–7, merging) |
| Regions' states (`region-states` thread, `build::region_states`) | yes | the busy core on the M1; 0.7–131 s a run | **drop** on members (#871 steps 6–7 take its output out of the status; this stops computing it) |
| Terrain pack indexes (`tiles::TerrainTiles`, `agent/pack-idx/`) | yes, for the regions' states | 555 MB each; 358 s of SMB reads on the M1 | **drop** with the regions' states, if no job path reads it (verified in step 2) |
| Regions loaded for the status (`load_regions` in the member's branch) | yes, each `plan_due` | 24 KB in `helper.json` | **drop** (#871 6–7) |
| `all_new` (the lead's `state/status.json` read each minute) | yes | one NAS read a minute | **move**: in the beat's answer (#878 step 3) |
| Build page on the member's port: `pageview` (lead's files every 12 s, history, the forecast run locally), `cataloguesview`, `builtview`, region steps | yes, kept warm by the menu bar's `/work/swarm` ask every 50 s | 1.6 MB `pageview-waits.json` written each read; forecast 5–6 s a run | **move**: the lead serves the page; the member's port redirects (§3.3) |
| `scenic menu` (every 5 s: the lead's `state/status.json` from the NAS when the member's own isn't fresh) | yes | one NAS read each 5 s | **move**: the lead's summary over HTTP (§3.4) |
| Inputs watch, gate's listing, redo, answers seeding, GC, backup, catalogues, publishing, forecast, coordinator, map log | no (lead only: `!o.helper`, `LEAD_ONLY`, `keeps_publishing`) | — | already lead only; the per-service checks become the role (§1) |
| Map server | no (retired by #874) | — | done |
| Leftovers of a Mac that once led (`agent/status.json` 328 KB, `agent/backups/` 1.1 GB, `agent/coord/`, `timings.jsonl`, `gc-depth-keep.json`, `publishing.json`) | on disk, not refreshed (the M1 last led 28 Oct) | ~1.2 GB each laptop | **drop**: removed at a member's start (§3.5) |
| The offline map left by #874 (`mirror/` 15 GB, `catalog/`, `idx/`, `accepted/`, `names/` on the M1) | on disk; nothing serves it | ~16 GB on the M1 | **open decision** (§7) |

## 3. Design

### 3.1 The member's parts

A member runs: **control**, **beat**, **nas** (mount, heal, its read/write pools), **jobs**, the
**pool** client (#871's, becoming #878's HTTP client), the caches, the live store, the memory and
disk guards, the budgets' proxy, the updater, and its **broker** (§3.2). Nothing else is built:
no checklist, regions' states or steps, pageview, catalogues or built views, forecast, inputs
watch, plan, coordinator.

Its status (`helper.json`, and the beat to the lead) carries only this Mac: conditions, slots and
jobs, waiting lines, caches, memory, ramp, budgets, hold, app.

### 3.2 The member's broker

A member's broker is `coord::Broker` (built, step 3): the tasks of this Mac's own jobs and the
`/work/*` calls those jobs make, plus `/work/live` for the lead's page. It runs on the
coordinator's server and folder (one at a time on a Mac; the jobs' token kept there, so a job
outlives a change of role), started as a broker: no phase sharer, nothing kept in the pool's store,
no job lease (those of a term the Mac once led are left to the lead as it starts). Its answers say
term 0 and its member, so a member still asking that Mac as the lead after a change of lead reads
the contact again (an older member too: it compares the term). Its API is its own, not the
coordinator's: its addresses and jobs' token, its member, its jobs' tasks closed and leases lapsed,
its pause, placement and the members' brokers (`coord::Brokering` is what the run's steps tell,
the lead's coordinator or a member's broker). Of the page it serves only what the lead's page asks
of this Mac (`/work/live`, `/work/settings`; step 4, §3.3). The lead's coordinator stays as it is.

### 3.3 The build page

The page is the lead's alone (built, step 4: `coord::memberpage`). A member's port (its broker,
or with tasks brokered by every member off a viewer alone) answers `/work/`, the page's files and
its data and asks (`/work/swarm`, `/work/history`, `/work/catalogs`, `/work/built`,
`/work/regions`, `/work/pause`, `/work/act`, `/work/settings`, ...) from a page of its own origin
or none (an address typed, the menu bar's web view) with a 307 to the lead's same path and query,
at the lead's address of the network the request came by (`memberpage::location`), so any device
that reached the member reaches the lead: its HTTPS page when the request came through
`tailscale serve`; its tailnet address for a request to a tailnet address or `.ts.net` name; its
LAN name (`<host>.local`) for one to a LAN address or `.local` name (a device without Tailscale);
else (this Mac itself, the menu bar's web view) its first. Answered there still: `/work/live` and
`/work/settings` asked by the lead's page from any origin it is served at (CORS, the lead's
heartbeat's addresses: docs/pool.md §8, Cross-member calls), the jobs' calls (their key), and
`sw.js` (an installed page updates its worker from its own origin; the worker follows a redirect
at once, never answering from what it kept). The build's own asks (pause, inputs, cancel and
retry) are refused there from any other origin: the lead's page sends them to the lead.

It redirects only to a lead that serves the page: the lead's beat answer says `page`
(`beats::PAGE`), heard within 35 s (`beats::PAGE_AWAY_S`, three beats). An older lead, none
known, or one not answering: the member's port answers `/work/` with a small page of its own,
this Mac's status (from `helper.json`: its jobs, what it waits on) and "The lead (<name>) isn't
reachable since <time>", refreshed every 10 s, and the page's data calls 503.

The menu bar's popover loads this Mac's own port with `?view&mac=<host>` (its own status's
`host`); the 307 takes it to the lead's page, which opens on this Mac (named in its title, its
card first in the Machines tab), and with the lead down this Mac's own page shows. (Planned was
the lead's URL from `scenic menu`'s answer; the port's redirect gives the same page and the
lead-down page from one rule.) Its keep-warm ask is gone: the app's version check reads this
Mac's own status (`app`). Per-Mac controls act on the chosen Mac as before: its hold through the
lead (by member id), its settings at its own port (CORS).

### 3.4 The menu bar

Built (step 5). `scenic menu` (`pipeline::menu`) reads this Mac's disk alone: its own agent's
status (the fresher of `status.json` and `helper.json`: a file left from a part this Mac had
before is the staler), and on a member the lead's line, which the beat's answer carries
(`held::Answer::lead`, `LeadLine`: the lead's Mac and app, the build in a line, the build's pause,
`all_new`) and the member's beat poster keeps in `agent/lead-view.json` at each answer, with when
the lead answered (`beats::LeadView`; an older lead's 404 counts as an answer with no line;
written outside the registry's lock and not synced: a slow disk never holds up the beats, and a
line lost to a crash is said again at the next answer). The build in a line is the lead's agent's
(`Status::build_said`, handed its coordinator each loop: `Coordinator::set_said`, and kept in its
own status as `build`, said again at each beat): its own verdict, or while it builds nothing and the build isn't paused,
"Building on <member>; the lead: …" for a member in touch with a job running; the pause and
`all_new` are the coordinator's at each answer. The lead's contact isn't in it: the member reaches
the lead by the contact it has, and the menu needs only the lead's name. A member's agent takes
`all_new` from the same line (it read the lead's `state/status.json` on the NAS each minute). No
NAS read in the menu path.

The menu bar (tools/status) shows on a member its own state while it builds, else the lead's
line, or "Lead (<name>) out of touch since <time>" when the lead hasn't answered for the
out-of-touch threshold, and its own agent out of touch as this Mac's; the build's pause from the
lead's line. The lead's menu shows its status's `build`, the same line (an older agent's: its
verdict): the rule is Rust's alone. A member's menu no longer shows the lead's checklist, forecast, regions or the other
Macs' jobs (the popover's page, the lead's, has them); the pool's lines from this Mac's agent's
view stay.

### 3.5 Leftovers

At a member's start, the agent removes, if older than a day, what only a lead writes and what no
code writes any more (on both laptops, 6 Nov):
- the lead's: `status.json`, `backups/`, `gc-depth-keep.json`, `publishing.json`,
  `pageview-waits.json`, `inputs-listing/` (the plan's, agent/plan.rs), `maplog/` (the map log's
  writer, agent/publish.rs `MAPLOG`), `plan-stall-sample-*.txt`, and in `coord/` the coordinator's
  files (`costs.json`, `disks.json`, `floors.json`, `cache-fills.json`, `job-disks*.json`,
  `leases.json`, `pause.json`, `timings.jsonl`, `store/`), each read by a broker at its start as
  a first fill only;
- written by nothing since the code that wrote them went: `loop-stall-sample-*.txt` (the loop
  watchdog, cut by #871 step 6), `round.json` (the map update's old name, #209), `costs-<n>.jsonl`,
  `battery-floor.moved`, and `helper.json.<host>.<pid>.<n>.tmp` (a whole write cut off; five on
  each laptop).

Kept: `coord/` itself, the member's broker's folder (§3.2), with `jobs-token`, `page`,
`history.jsonl` (written on the M1 on 6 Nov), `workers-token` (the pool's, copied in by
`pool::seed` at each broker's start: agent/mod.rs `make_broker`) and `journal/` (made by every
broker's start, `Coordinator::start_with`); and the agent's own `timings.jsonl`: every Mac appends
its own jobs' timings there (`take_timings`, crates/pipeline/src/agent/mod.rs), which `scenic`
reads as "this Mac's jobs". One function (`room::member_leftovers`) lists them; a test names each
path the lead writes in the home and requires it in the list or in a kept list. The map server's
files on a laptop are §8.3's.

## 4. What the member still needs from the lead

| Need | Today | By HTTP (lead's coordinator) |
| --- | --- | --- |
| Work | `/work/ask`, `/work/beat`, NAS mail and journal | unchanged; #878 steps 2, 3, 6 |
| Its jobs to come (caches) | `/work/tocome` | unchanged |
| `all_new`, the pause, the build's state line | the `POST /pool/beat` answer (`held::LeadLine`, built: step 5) | the same |
| Holds and settings | NAS files | `/pool/hold`, `/pool/settings` (#878 step 5) |
| The build page | its own NAS readers | a 307 to the lead (§3.3) |
| The records brief (jobs' `Out`) | NAS snapshot | `GET /pool/brief` (#878's later step, decision 2) |

## 5. Migration order

Each step ships alone and keeps the build running; each ends with its tests.

1. *Built.* **The role** (§1): `pool::term::role`, used by `Agent::new` and `retire::retires`; parts built
   by role; `o.helper` derived from it, then removed. A changed declaration restarts the agent.
   Tests: `role_reads_the_declared_lead`, `an_unknown_role_starts_the_members_parts`,
   `a_changed_declaration_restarts_into_the_new_role`; the server's retire tests on the shared
   function. As built: `Options::helper` and the CLI's `--helper` are gone (no Mac's run file passes it: the status app
   writes `run/agent` without it);
   a role change, or the lead losing its term, restarts at once (`role_restart`), its jobs handed
   over; the coordinator is built from the role, and the in-process promotion, demotion and the
   coordinator kept on as a broker are deleted. An unknown role learnt a member's is taken in place.
2. *Built.* **The regions' states and the checklist off members** (after #871 6–7): no `region-states`
   thread, no checklist phase, no `TerrainTiles` on a member; `pack-idx/` removed there.
   Tests: `a_member_never_counts_the_regions` (the two-agents harness: the member's round makes no
   `start_region_steps` call and opens no pack index), and a loop-time test with #871's slow-NAS fake:
   a member's round never waits on the records brief (`a_members_round_never_waits_on_the_records_brief`). As built: the member had none of these since #871
   step 6 (no plan); the M1's were a lead's parts started out of the pool (the old `o.helper` false),
   which step 1 ends. Also: a job's `--expect-same` reads the brief under the NAS's deadline. The idle footprint was not measured: the
   harness has no regions' data to fill one.
3. *Built.* **The member's broker** (§3.2): `coord::Broker`; `become_broker` and the member's phase sharer
   deleted. Tests: the brokers' task tests moved to `Broker` (`a_task_goes_out_and_comes_back`,
   `a_task_given_back_is_not_a_failure`, `a_members_broker_passes_the_owners_asks_from_any_device_to_its_agent`);
   `a_member_runs_no_phase_sharer`; `a_member_asking_the_old_lead_after_a_change_of_lead_asks_the_new_one`
   with the old lead's Mac's broker. As built: a broker says term 0, which an older member's
   client also takes as not the lead's; an older member's broker (a coordinator made a broker)
   and a newer one answer jobs' and pages' task calls alike.
4. *Built.* **The page on the lead** (§3.3): the 307s, the menu bar loading the lead's page, the local
   unreachable page; pageview, cataloguesview, builtview and region steps built on the lead alone.
   Tests: `a_members_page_redirects_to_the_lead`, `a_members_page_with_the_lead_down_shows_its_own`,
   `a_viewer_shows_the_leads_page_does_nothing_else_and_keeps_nothing`,
   `a_members_page_goes_only_to_a_lead_that_serves_it` (beats), `memberpage`'s own, the Swift
   self-test for the popover's URL; the touch test's `--mac`. As built: the agent builds the
   page's views (pageview, Catalogues, built tiles) only beside its coordinator; a member's port is
   told where the page is each loop (`Agent::member_page`). Cut: `coord::Pages`, a member's page
   asks passed on by its agent (`set_forward`, `retry_blocked`), the Catalogues map's member
   source (`cataloguesview::Lead`), the popover's keep-warm ask; in review, the pageview's ways a
   member's page had: the lead's view by HTTP (`pageview::ByHttp`) and the older apps' path (the
   lead's files read on the NAS: `nas_lead_files`, `refresh_pause`, `by_lead`).
5. *Built.* **The menu without the NAS** (§3.4), after #878 step 3: the beat's answer saved as
   `lead-view.json`; `scenic menu` reads it. Tests: `menu_reads_the_lead_from_the_beat`,
   `menu_works_with_the_nas_hung` (slow-NAS fake: the member's NAS copy hangs while its menu shows
   its jobs and the lead's newest line), `the_menu_bars_lead_line_is_scenics` (the line pinned
   both sides by tools/status/test/lead-view-said.json, the Swift self-test's member checks),
   `the_build_said_counts_a_member_building`, `the_leads_menu_shows_its_own_status`. From #878
   step 5 it had: the menu's pause asks go to the agent and on to the lead (`/pool/hold`), and the
   pool's settings as the agent last had them (`pool-settings.json`). As built: the lead's contact
   left out (§3.4). Cut: `menu::lead_heartbeat` and `scenic menu`'s NAS read (every 5 s, on every
   Mac), its `--root`, the lead's status shown in a member's menu with the member put in its
   `helpers` (`here`), a member's own status standing in for a lead unheard with the lead's last
   checklist copied in, the reply's "via the NAS" (`local`, still written for a menu bar before
   this step), the agent's `lead_all_new` read of the NAS; `LEAD_STATUS` moved to
   `crate::agent` (the server reads it). Also fixed: `scenic menu`'s log path took the old
   `job` field, which an all-new pool no longer writes (no Open the Build Log); now the first of
   `jobs`. tools/dev/health.sh reads the lead's heartbeat on the NAS itself.
6. **Leftovers** (§3.5). Tests: `member_leftovers_names_every_lead_file` (the agent's own
   `timings.jsonl` and the broker's `coord/` files, `workers-token` and `journal/` among them, in
   the kept list), removal by age. Saves ~1.1 GB
   of disk on each laptop (`backups/`; §8.1). Touches agent/room.rs only: land it before #916 6f
   moves the agent's runtime (§8.4).
7. **The laptops' offline map on demand** (7a, §8.3; the PM's decision 4 in §7, which the owner
   may overrule for 7b: the files deleted). Either way `catalog/` and
   `~/Library/Caches/scenic/catalogue` go with #883 step 9, not here.
8. **The member's NAS fallbacks**, with #878's gates (§8.4): nothing to build in #882; each gate's
   deletion takes a member's share of it (the `nas-switch` pool, `beat-nas`'s status copy, the pool
   thread's mail and journal), and `pool::term::role` moves out of `pool::term` before #878 step 8
   deletes that module.
9. **Docs**: macs.md (Devices: roles; the App Macs' offline map as 7a builds it), agent-parts.md
   (the member's parts), pool.md §5 (a member no longer "passes its build page's asks to the lead": its
   port redirects), pool.md §10 (the page is the lead's), this doc folded in.

Steps 1–3 need nothing from #878 and cut the measured cost (the busy core, the loop stalls, the
pack indexes). Step 4 needs only the lead's contact, which members already have. Step 5 needed
#878 step 3.

## 6. Out of scope, noted

- The budgets' relay used a thread per job connection (178 threads on the M4 with four jobs). Task
  #908 serves them on an event loop (`crate::budget::proxy`): 256 tunnels open at once took 512
  threads and 20 MB resident before, 7–18 threads and 15 MB after (in-process test).
- The agent's 2.8 GB footprint on an idle M1 was not broken down here; step 2's tests should show
  how much of it the regions' states and checklist held.

## 7. Open decisions

1. **The laptops' offline map** (`mirror/` ~15 GB, `catalog/`, `idx/`, `accepted/` on the M1):
   #874 kept it, but no server on a member reads it now. Delete it on members (proposed, as
   "removed means deleted"), or keep it for an offline map the laptops don't serve today.
2. **The page on a member**: a 307 to the lead (proposed: one page, one source) or a proxy (the
   member's URL stays in the address bar, but every answer then crosses the member).
3. **`Unknown` role** (NAS unread at start): start the member's parts (proposed: a subset, safe),
   or wait for the declaration.
4. **The laptops' offline map, again** (6 Nov). Decision 1 below and docs/maplog.md's of 1 Nov
   (the owner's) keep it, but nothing on a laptop serves it since #874 (§8.3: a regression).
   **Decided by the PM, 6 Nov: on demand (7a)**, since an offline map is what Scenic Roads is for:
   the menu bar's "Open the Map on This Mac" starts the app's server on 127.0.0.1 alone, which
   quits after 30 min unused; nothing resident. The owner may overrule it. The alternative, 7b:
   drop the laptops' offline map and delete its files (~1 GB each).

## 8. After step 5: every service, where it runs (6 Nov)

### 8.1 Measured

Read-only, on 6 Nov 09:58–10:04 local, app 20261106-1441-d2c80ec6 (every agent restarted into it
10–16 min before): `ps` every 30 s for 5 min (CPU time, resident, threads), `footprint` at the
start and end, `sample` (3 s) of each agent for its threads, `du` of the homes, `lsof` for
listeners, `launchctl list`. CPU is the CPU time used over the 5 min, in a core's share.

| | M1 (16 GB; two jobs: terrain-hi, bldtiles) | M4 (48 GB; three jobs: terrain-deep ×2, terrain-hi) | Mac mini (32 GB; the lead) |
| --- | --- | --- | --- |
| Agent (`scenic agent`), outside its jobs | 0.5 % of a core; 67–68 MB footprint; 32–33 threads | 1.5 %; 62–71 MB; 42–48 threads | 234 %; 3.3 GB; 72–89 threads |
| Menu bar (`scenic-status`, its `scenic menu` each 5 s) | 0.75 %; 22–23 MB | 0.8 %; 22–29 MB | 0.7 %; 23–26 MB |
| Launcher | 0; 1.2 MB | 0; 1.3 MB | 0; 1.2 MB |
| Map server | not installed | not installed | 3.4 %; 1.7–1.8 GB; 30 threads |
| Caddy, dev tools app (`node`, :8077) | — | — | 0.3 %, 34 MB; 0.07 %, 105 MB |
| `scenic menu`, one call | <10 ms, 5 MB | | |
| Home, `agent/` | 69 GB: cache 50, scratch 17, backups 1.1 | 40 GB: cache 17, scratch 21, backups 1.1 | 556 GB: cache 507 |
| Home, the map server's files | `accepted/` 0.9 GB, `catalog/` 119 MB, `names/` 3 MB, `regions.json`, `region-areas.json`, `profiles.json`, `map-page`; no `mirror/`, `idx/` or `downloads.json` | the same (`catalog/` 63 MB) | `idx/` 338 MB, `accepted/` 0.9 GB, `mirror/` 64 KB, no download |
| `~/Library/Caches/scenic` | `map/` 20 MB (saved 5 Nov), `catalogue/` 36 MB | `map/` 19 MB (saved 4 Nov), `catalogue/` 36 MB | `map/` 32 MB, `catalogue/` 34 MB |

Against §2's audit (31 Oct, before steps 1–5): a member's agent fell from 2.8–4.0 GB to 62–71 MB,
from 25–45 % of a core to 0.5–1.5 %, and from 44–265 threads to 32–48; the menu bar now costs it
about what the agent does. (Measured minutes after a restart, where §2's ran 31–47 min after
one.) A member's threads (`sample`): `control`,
`control-watch`, `beat`, `beat-lead` (its poster), `beat-nas`, `nas`, `nas-probe`, `nas-read-0..3`,
`nas-write-0..3`, `nas-switch-0..1`, `jobs`, `pool`, five `coordinator` (its broker's server),
`budgets-proxy`, `budgets-io` (2 on the M1, 17 on the M4: by the jobs' connections), `live-sampler`,
`live-nas`, `memory-guard`, `app-update`. The lead's adds `plan`, `region-states`, `forecast`,
`forecast-archive`, `pageview`, `pageview-lead`, `phase-shares`, `inputs-watch`, `maplog`,
`pool-store`, `pool-store-backup`, three more `coordinator`. The members' NAS status copies
(`state/helpers/<host>.json`) were written at their agents' start alone (before the lead's first
answer, #878 step 3's gate), none after.

Besides the app: `local.scenic.sweep` (the dev tools' target sweep, every 20 min) on all three;
supervised long runs (`local.scenic.run.*`, tools/dev/longrun.sh) and the agents' test runs on the
M4, the test Mac (tools/dev/testing.md). Dev tools, not services of the map or the build.

### 8.2 Where each runs, and how a member reaches it

| Service | Today | Proposed | A member reaches it by |
| --- | --- | --- | --- |
| Jobs: slots, the memory and disk guards, ramp | every Mac | every Mac | — |
| The member's broker (its jobs' tasks and `/work/*`, `/work/live`, `/work/settings` for the lead's page) | every Mac (a lead's coordinator on the mini) | every Mac | its jobs at its own port |
| Control (this Mac's hold, quit, settings), the live store, the memory sampler, caches and room, the updater | every Mac | every Mac | local; the hold and settings told the lead by the beat |
| Beat poster | members | members | `POST /pool/beat` to the mini |
| Budgets' proxy | every Mac | every Mac (jobs ask it) | it asks the mini (`/pool/budget/take`) |
| NAS mount, heal, read and write pools | every Mac | every Mac (jobs read and write the NAS); the switch pool goes with #878 | — |
| Pool thread (journal, mailboxes, terms) | every Mac, its NAS side gated | an HTTP client alone after #878's gates | the mini's `/pool/*` |
| Menu bar | every Mac: this Mac's status and `lead-view.json` (step 5) | the same: the thin local status | the lead's line in the beat's answer |
| Build page and its data: pageview, forecast and its archive, Catalogues map, built tiles, region steps, phase shares, history | the mini (step 4) | the mini | a member's port 307s to it (§3.3) |
| Plan, checklist, regions' states, gate, inputs watch, redo | the mini (steps 1–2) | the mini | grants by `/work/ask` |
| Coordinator, pool store and backup, hand-off merge, the map's log and feed, publishing, GC, backup, catalogue/prune and the other `LEAD_ONLY` steps | the mini | the mini | `/pool/*`, `/work/*`, `/map/*` |
| Map server (tiles, place search, region areas, profiles, accepted versions, names) and its mirror | the mini (#874) | the mini; a laptop's offline map on demand only (§8.3) | devices: map.brandontsang.net |
| Caddy, dev tools app and its backup | the mini | the mini | — |

So the owner's split already holds in what runs: the laptops run jobs, their broker, the beat and
the menu bar's local status, with what those need of their own Mac (control, the NAS pools, the
budgets' proxy, the live store, the memory guard, the updater) and the pool thread until #878's
gates. What #882 has left is disk (§8.1: lead and server leftovers) and the laptops' offline map
(§8.3).

### 8.3 When something is away, and the offline map

- **The mini unreachable** (#881: no takeover): a member finishes its held jobs and starts none
  (no grant); its hand-offs stay in its saved state and are posted when the lead answers again
  (#878 step 6); its budgets' proxy gives nothing, so a Wikimedia job waits. Its menu says "Lead
  (<name>) out of touch since <time>" from `lead-view.json`; its port shows its own small page
  (§3.3). The map and the build page are the mini's, so devices have neither until it's back;
  a laptop's offline map would serve it there, but none runs today (below).
- **The M4 off the network:** over Tailscale it stays a member (docs/coordination.md §7.2): beats
  to the mini, the NAS at ~12 MB/s, jobs slower. Fully offline its jobs fail on the NAS and the
  lead keeps its leases until it's heard or 12 h pass (`held_by_away`). Off the network the M4 is
  also no test Mac: check.sh's tests part and publish's suite fail to reach it
  (`SCENIC_TEST_HOST`); that is the dev tools', not a role of #882's.
- **A laptop's offline map: broken since #874, a regression.** Until #874 every Mac's own server
  served its map and its downloads on localhost with nothing else reachable: the M1's synced its
  mirror until it retired on 31 Oct 00:37 (its `server.log`). Since then no laptop has a server
  (`retire.rs` ends one at its start), and on 6 Nov neither has `mirror/`, `idx/` or
  `downloads.json` (the M1 had ~15 GB of mirror on 31 Oct, §2); each has a saved map of 4–5 Nov
  that nothing serves. The owner's decision of 1 Nov (docs/maplog.md) kept this map, so its loss
  is a regression, not a choice. What a laptop has without the mini is what a phone has: the
  map's page at map.brandontsang.net and its service worker's kept files (`web/public/sw.js`,
  registered on any HTTPS host but localhost: the page, and data as viewed, at most 12,000
  files), no downloaded region, no search, nothing it hasn't looked at. For the offline map to
  work away from home again (decision 4 in §7: 7a), the laptop keeps:
  - the app's `server`, `web/` and fonts (in every app copy already);
  - its followed map: `~/Library/Caches/scenic/map/current.json.zst` (~20 MB) and `lead.json`;
  - its downloads: `mirror/`, `downloads.json`, `reserve.json`, `idx/` (the World ~10 GB, regions
    as chosen; docs/macs.md, Mirror);
  - what the server keeps for offline use: `accepted/` (0.9 GB, the translations' and descriptions'
    accepted versions), `names/` (3 MB), `profiles.json`, `region-areas.json`, `map-page`.
  Not needed by anything: `catalog/`, `~/Library/Caches/scenic/catalogue` (no catalogue read since
  #883 step 6; #883 step 9 deletes them) and the home's `regions.json` (no code reads it; written
  22 Oct). **7a (the PM's decision; planned):** the server starts only when asked (the menu bar;
  `retire.rs`'s start check then lets an on-demand server on a member run, installing no
  LaunchAgent), answers 127.0.0.1 alone (no
  second map on the tailnet: the mini's is the devices'), follows the log while it runs (its
  `server/<host>` cursor; the NAS's snapshot or its saved map without the lead), copies and
  Updates downloads only while it runs (`mirror/<host>`), and quits 30 min after its last use
  (the server's own "in use"). Resident, it would cost what the mini's does (1.7–1.8 GB, 3 % of a
  core with the phone's use); on demand, nothing while closed. Its cursors hold the files of the
  map it was at for at most `CURSOR_TTL_S` (7 days) after it closes. **7b (the alternative, if
  the owner overrules 7a):** the offline map is dropped: the agent deletes those files on a
  member (~1 GB each laptop with `accepted/`), and `retire.rs`'s note that the offline map stays
  goes.

### 8.4 With the other architecture tasks

- **#916 6f (agentcore and `Plan`)** waits on "#882's later steps (coord)". None left touches
  coord: steps 6 and 8 are agent/room.rs and the member's side of #878's gates; 7 is
  crates/server (`retire.rs`) and tools/status. Step 6 should land before 6f moves the agent's
  runtime, or go in after it at the new path; 6f need not wait for 7 or 9. The broker
  (`coord::Broker`, `Brokering`) and the page's redirect (`coord::memberpage`) are built and stay
  in coord for 6f to move.
- **#880 (incremental planning, in flight):** the plan is the mini's alone since steps 1–2, so its
  kept state costs the laptops nothing; #882 has nothing left in agent/plan.rs or build.rs.
- **#878 (coordination off the NAS):** its step 3 carried step 5 here. Left on the
  members: the gated NAS fallbacks (step 8 above), and records by HTTP (`GET /pool/brief`, its open
  decision 2), which would take a job's `--expect-same` brief read off SMB. Its step 8 ("terms
  out") deletes `pool::term`, where `role` lives (§1): `role` moves (to `pool`, reading
  `lead-member.json`, which stays) in that step.
- **#883 (the map's log):** the log, its feed, GC and its queue are the mini's already; #882 moves
  none. 7b (built) gave the mirror its cursor; on 6 Nov only the mini runs a mirror, so no
  laptop's cursor holds GC. 8a (not before 15 Nov) and 8b are the lead's. With 7a, a laptop's two
  cursors exist only while its map runs and `CURSOR_TTL_S` drops them, so it never holds GC more
  than 7 days. Step 9 owns `catalog/` on every Mac.

### 8.5 What the steps left save

| Step | Laptop saving | Mini |
| --- | --- | --- |
| 1–5 (built) | measured in §8.1: per laptop ~2.7–3.9 GB of agent footprint and 25–45 % of a core outside jobs; `pack-idx/` (555 MB) gone | the same services, as before |
| 6, leftovers | ~1.1 GB disk each (`backups/` 1.1 GB, `status.json`, `coord/`'s lead files ~6 MB, stall samples 2 MB, the rest of §3.5's list KBs) | — |
| 7a, map on demand | nothing resident; ~1.7 GB and ~3 % of a core avoided against a resident server; disk as downloaded | — |
| 7b, no offline map | ~1 GB disk each (`accepted/` 0.9 GB and the rest of §8.3's list) | — |
| 8, #878's gates | three threads (`nas-switch` ×2, `beat-nas`), the NAS status and mailbox writes at each start | the journal listings and old-member paths |
| #883 step 9 | 100–150 MB each (`catalog/`, `Caches/scenic/catalogue`) | 80 MB |

Nothing moves to the mini that it doesn't run already, so its load stays as measured (the agent
2.3 cores and 3.3 GB, the server 1.7–1.8 GB, 71 GB of disk free of 926). Each step ships alone:
6 and 8 on the agent's start or a gate, 7 as the PM decided (7a, unless the owner overrules it);
none stops the build or the map.

## Decisions (PM, 31 Oct night)

1. The offline map on the laptops is kept until the owner decides; nothing deletes it.
2. A member's port answers the build page with a 307 redirect to the lead.
3. With the lead not yet known at start, a member starts its member parts (a subset), and changes role once the declaration is read.
