# Roles: the mini serves, the laptops work

**Planned (task #882; nothing here is built yet).** Until each step below lands, docs/pool.md,
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
| Checklist (`checklist`: records brief, outlines, every 5 min and job end) | yes | the 13–932 s loop phase | **drop** (#871 steps 6–7, merging) |
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

Today a member's broker is the whole `Coordinator` with its plan, leases, keep, phase shares and
page state emptied (`become_broker`). It becomes `coord::Broker`: the tasks of this Mac's own jobs
and the `/work/*` calls those jobs make, plus `/work/live` for the lead's page. No phase-sharer,
no keep, no page data. The lead's coordinator stays as it is.

### 3.3 The build page

The page is the lead's alone. A member's port answers `/work/` and the page's data calls
(`/work/swarm`, `/work/history`, `/work/catalogs`, `/work/built`, `/work/regions`, ...) with a
307 to the lead's same path (the lead's contact, from the pool's view of the declared lead,
tailnet address first). Its own `/work/live` and the jobs' calls stay. The menu bar's popover
loads the lead's page directly (its URL from `scenic menu`'s answer), with `?view&mac=<host>` so
the page opens on this Mac. Its keep-warm ask goes: the lead keeps its own readers warm.

With the lead unreachable, the member's port answers the page with a small local page: this
Mac's own status (from `helper.json`) and "the lead (Mac mini) isn't reachable since <time>".

### 3.4 The menu bar

`scenic menu` keeps reading this Mac's own `helper.json` (local, fresh). The build-wide line (the
lead's progress, the pause) comes from the beat's answer, which the agent keeps in
`agent/lead-view.json` (#878 step 3 gives the beat an answer; this adds a few fields: the pause,
the build's one-line state, `all_new`, the lead's contact). No NAS read in the menu path.

### 3.5 Leftovers

At a member's start, the agent removes what only a lead writes, if older than a day: its
`status.json`, `backups/`, `coord/`, `timings.jsonl`, `gc-depth-keep.json`, `publishing.json`,
`pageview-waits.json`. One function (`room::member_leftovers`) lists them; a test names each path
the lead writes in the home and requires it in the list or in a kept list.

## 4. What the member still needs from the lead

| Need | Today | By HTTP (lead's coordinator) |
| --- | --- | --- |
| Work | `/work/ask`, `/work/beat`, NAS mail and journal | unchanged; #878 steps 2, 3, 6 |
| Its jobs to come (caches) | `/work/tocome` | unchanged |
| `all_new`, the pause, the build's state line, the lead's contact | NAS `state/status.json`, `pause.json` | the `POST /pool/beat` answer (#878 step 3, with the fields above) |
| Holds and settings | NAS files | `/pool/hold`, `/pool/settings` (#878 step 5) |
| The build page | its own NAS readers | a 307 to the lead (§3.3) |
| The records brief (jobs' `Out`) | NAS snapshot | `GET /pool/brief` (#878's later step, decision 2) |

## 5. Migration order

Each step ships alone and keeps the build running; each ends with its tests.

1. **The role** (§1): `pool::term::role`, used by `Agent::new` and `retire::retires`; parts built
   by role; `o.helper` derived from it, then removed. A changed declaration restarts the agent.
   Tests: `role_reads_the_declared_lead`, `an_unknown_role_starts_the_members_parts`,
   `a_changed_declaration_restarts_into_the_new_role`; the server's retire tests on the shared
   function.
2. **The regions' states and the checklist off members** (after #871 6–7): no `region-states`
   thread, no checklist phase, no `TerrainTiles` on a member; `pack-idx/` removed there.
   Tests: `a_member_never_counts_the_regions` (the two-agents harness: the member's round makes no
   `start_as_built` call and opens no pack index), and a loop-time test with #871's slow-NAS fake:
   a member's round never waits on the records brief.
3. **The member's broker** (§3.2): `coord::Broker`; `become_broker` and the member's phase sharer
   deleted. Tests: the brokers' task tests moved to `Broker`; `a_member_runs_no_phase_sharer`.
4. **The page on the lead** (§3.3): the 307s, the menu bar loading the lead's page, the local
   unreachable page; pageview, cataloguesview, builtview and region steps built on the lead alone.
   Tests: `a_members_page_redirects_to_the_lead`, `a_members_page_with_the_lead_down_shows_its_own`,
   the Swift self-test for the popover's URL; the touch test on the lead's page with `mac=`.
5. **The menu without the NAS** (§3.4), after #878 step 3: the beat's answer saved as
   `lead-view.json`; `scenic menu` reads it. Tests: `menu_reads_the_lead_from_the_beat`,
   `menu_works_with_the_nas_hung` (slow-NAS fake).
6. **Leftovers** (§3.5). Tests: `member_leftovers_names_every_lead_file`, removal by age.
7. **Docs**: macs.md (Devices: roles), agent-parts.md (the member's parts), pool.md §10 (the page
   is the lead's), this doc folded in.

Steps 1–3 need nothing from #878 and cut the measured cost (the busy core, the loop stalls, the
pack indexes). Step 4 needs only the lead's contact, which members already have. Step 5 waits for
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

## Decisions (PM, 31 Oct night)

1. The offline map on the laptops is kept until the owner decides; nothing deletes it.
2. A member's port answers the build page with a 307 redirect to the lead.
3. With the lead not yet known at start, a member starts its member parts (a subset), and changes role once the declaration is read.
