# Where a build's time goes (task #492, 20 Oct 2026)

Measured from the records the pipeline keeps, not new benchmarks: the coordinator's costs (the
latest measured run of each of 4,238 targets, term 280's `state.json`) and the lead's
`coord/timings.jsonl` through `scenic timings --last 200 --json` (each kind's last 200 runs, the
M4 and the M1, the NAS over Wi-Fi). CPU/wall is the run's CPU time over its wall time. The NAS,
net and compute columns are the shares of the run's wall time in phases of that class (a mixed
phase by its sub-phases); "untimed" is in no phase. Updated by whoever changes a step's shape.

## 1. A full build, by step (the per-target steps, summed)

| step | targets | median s | p90 s | sum h | peak MB p90 |
|---|---|---|---|---|---|
| depth | 558 | 45 | 922 | 42.6 | 2736 |
| unit | 264 | 140 | 1112 | 26.6 | 6250 |
| bldtiles | 380 | 2 | 207 | 7.8 | 3393 |
| depth-lo | 26 | 66 | 1332 | 7.1 | 2073 |
| terrain | 415 | 24 | 91 | 5.0 | 6797 |
| slope | 416 | 10 | 59 | 4.9 | 887 |
| bldprep | 372 | 3 | 56 | 2.1 | 1749 |
| trees | 442 | 7 | 33 | 1.6 | 2569 |
| pack | 472 | 0 | 20 | 1.3 | 392 |
| peaks | 388 | 5 | 24 | 1.3 | 1440 |
| pois, lo, terrain-lo, the rest | | | | ≈ 1.6 | |

≈ 102 h of target time. Depth's 558 are 412 at the way of task #433 (22.7 h, median 31 s) and 146
still at the old way (19.9 h, median 190 s: the slow coastal ones rebuild last). Worldwide steps
run once a pass: heritage 83 min, reach 33 min, heritage-sites 20 min, names-todo 10 min a run.

## 2. A run of each kind, and what it waits on (the last 200 runs of each)

| kind | runs | a run s | CPU/wall | NAS | net | compute | untimed |
|---|---|---|---|---|---|---|---|
| depth | 200 | 1085 | 0.61 | 26 % | 0 % | 67 % | 7 % |
| depth-lo | 49 | 2473 | 0.18 | 6 % | 0 % | 68 % | 26 % |
| unit | 200 | 456 | 0.20 | 38 % | 51 % | 10 % | 1 % |
| names-todo | 51 | 613 | 0.07 | 100 % | 0 % | 0 % | 0 % |
| bldtiles | 38 | 722 | 5.04 | 14 % | 1 % | 84 % | 1 % |
| terrain | 69 | 231 | 1.63 | 10 % | 0 % | 82 % | 0 % |
| heritage | 3 | 4993 | 0.96 | 90 % | 1 % | 7 % | 0 % |
| slope | 70 | 122 | 0.37 | 16 % | 0 % | 17 % | 67 % |
| gc | 7 | 1002 | 0.01 | 100 % | 0 % | 0 % | 0 % |
| pack | 200 | 32 | 0.05 | 46 % | 0 % | 29 % | 25 % |
| peaks | 42 | 138 | 1.22 | 82 % | 0 % | 18 % | 0 % |
| lo | 14 | 325 | 0.55 | 44 % | 0 % | 56 % | 0 % |

The class a phase declares is what it mostly waits on; `composing` (depth) is declared compute
but holds the sources' reads as sub-phases, so depth's NAS share is really higher: see below.

## 3. Inside the big ones

- **depth** (18 min a run, the last 20: the M1's): the basemap's water read 32 % (CPU/wall 0.6:
  a block and its margin asked a hundred tiles of the basemap one round trip each, then rasterised
  them), composing 60 %: the sources' windows read 17 % (CPU/wall 0.7), gaps filled 15 % (CPU 1.0:
  one thread), stored levels scaled up 8 %, the source files opened 1.5 %. Untimed 1 %.
- **depth-lo** (41 min a run): the sources' windows read 74 % (CPU/wall 0.2), the source files
  opened 21 % (10,000 a run, 51 ms each with 8 at once: each open was 24–38 round trips), the
  water 1.5 %. These are round trips, not bytes: the overviews read are a few MB a grid.
- **unit** (a full build's: 46 h over 319 runs): elevations 58 % (3DEP sampled 19 %, the edges
  blended 28 %: the internet, 0.5–3 MB/s a connection from S3, task #216), layers staged from the
  packs 8 %, the next area's packs copied ahead 5 % (beside), base pack written 5 %, DEM cache
  slice 5 %. In the small rebuilds of the last 20 runs, "missing grids written" is 38 %
  (1 min 36 s a run at CPU/wall 0.1; cause not found: §5).
- **bldtiles** (12 min): tiles made 84 % (CPU/wall 5.9), uploaded 13 % (1.2 GB a run at 13 MB/s).
- **terrain** (4 min): tiles prepared 63 %, shaded and encoded 18 %, uploaded 6 %.
- **heritage** (83 min): the filtered planet clipped to the cover 84 % (osmium extract, strategy
  smart: the 60.6 GB `sources/osm/<date>/filtered` read three times from the NAS, 182 GB at
  43 MB/s).
- **slope** (2 min, the last 20: 24 s): its terrain pack copied here 30 % (52 MB/s), uploaded
  42 % (13 MB/s), tiles worked out 17 %.
- **pack** (32 s): the areas' base packs copied here 46 % (26 MB/s); the uploads beside landed
  (untimed before #445) 25 %.
- **names-todo** (10 min, all NAS reads: the roads' names 58 %, the labels' 24 %); runs only when
  its inputs change since #445.
- **gc** (17 min a run, folders swept: NAS creates and removes).

## 4. What was done (task #492), and what each is bound by

| change | bound by | before → after (measured) | ethernet |
|---|---|---|---|
| A GeoTIFF's header read in windows and its tile arrays kept (`geotiff::Windowed`, `STRILES_KEPT`): an open 1–2 reads (was 24–38), a block's read 1 (was 3); commit 64a7c64a | round trips | 3.7–5.9× fewer range reads on three tiles; wall 36.9 → 26.2 s warm on 3/3/2, cold-to-cold not measured (§6) | helps little: a round trip on the NAS under load is its disks' seeks, not the link |
| A block's water tiles read from the basemap in runs (`PmTiles::get_many`, `WaterSource::depth_polys_many`), decoded and rasterised on the job's threads; commit ac2387a1 | round trips, then CPU | the water phase 11.3 → 1.8 s and 6.4 → 1.8 s warm, 14.3 → 1.3 s cold-then-warm (§6) | the same |

Byte-identical outputs: the same bytes are read, in fewer reads; the measure's digest of the
tiles made is the same before and after (§6). No version bump.

## 5. Not done, with what the data says

1. **unit, "missing grids written"** (1 min 36 s a run in rebuilds, CPU/wall 0.1): three packs of
   zstd-9 tiles made locally and put off for upload; nothing in it reads the NAS. Not found in
   this pass; the next step is a timing inside it (the tiles encoded, the pack written, the
   upload queued).
2. **depth's compute** (gaps filled 15 %, scaled up 8 %, encoding): one thread; a depth job runs
   with 2 threads. The EDTs (`depth::edt`) could run by rows on the job's threads, the same
   result; a block's water could be read ahead while the one before composes. Worth about a
   quarter of a depth run; after ethernet it is the largest share left.
3. **heritage's clip**: NAS bandwidth (three passes over 60.6 GB). Ethernet alone (110 MB/s)
   brings 70 min to about 28. A local copy doesn't fit the M4's disk (43 GB free). `-s
   complete_ways` reads twice but changes what is clipped: not byte-identical.
4. **The uploads at 13 MB/s** (bldtiles 13 %, slope 42 %, lo 20 %, pack 13 %): Wi-Fi; ethernet.
5. **The unit's elevations**: the internet (S3), tasks #216 and #445; not the NAS.
6. **slope's untimed 67 %** over the window: the landing of put-off uploads, timed since #445
   ("the uploads beside landed"); the last 20 runs are 0.4 % untimed.

## 6. The measure (`SCENIC_DEPTH_MEASURE`, release, the NAS read-only, a z3 tile's assembly to z8)

The M4, 20 Oct 19:35–20:27, other agents' test suites running beside (noisy). "Cold": the tile's
grids not read on this Mac today; "warm": read minutes before (the page cache holds some of it;
what it holds varies, so only a cold run is a job's case, and a cold pair needs two tiles). The
digest of the tiles made is the same before and after on every tile: 3/3/2 23c0bb46697c9d15,
3/0/1 397ae0955752fa00, 3/1/3 554253309d46a6cf.

| tile | run | wall | range reads | water read | files opened | windows read |
|---|---|---|---|---|---|---|
| 3/3/2 (1,716 grids) | before, cold, the publish suite beside | 122.3 s | 64,802 | 17.0 s | 36.6 s | 52.8 s |
| | the GeoTIFF change, warm | 31.7 s | 17,525 | 6.3 s | 12.7 s | 3.7 s |
| | both changes, warm, under load | 83.1 s | 17,525 | 2.1 s | 24.0 s | 47.1 s |
| | before, warm | 36.9 s | 64,802 | 11.3 s | 5.8 s | 6.4 s |
| | both changes, warm | 26.2 s | 17,525 | 1.8 s | 5.0 s | 6.4 s |
| 3/0/1 (1,900 grids) | final code, cold | 83.6 s | 14,219 | 2.7 s | 40.4 s | 30.1 s |
| | before, warm | 24.6 s | 55,971 | 6.4 s | 5.6 s | 2.8 s |
| | final code, warm | 23.8 s | 14,219 | 1.8 s | 3.9 s | 3.8 s |
| 3/1/3 (29,909 opens: more grids than a reader keeps) | before, cold | 464.0 s | 520,752 | 14.3 s | 84.1 s | 347.5 s |
| | final code, warm | 361.4 s | 87,648 | 1.3 s | 76.0 s | 273.8 s |

What is measured: **3.7–5.9× fewer range reads** on every tile (64,802 → 17,525; 55,971 →
14,219; 520,752 → 87,648), and **the water phase 4–11× faster** in every pair (11.3 → 1.8 s
warm, 6.4 → 1.8 s warm, 14.3 → 1.3 s). What is not: a cold-to-cold wall time on one tile (3/3/2's
cold before, 122 s, was under the publish suite; 3/0/1's cold after, 84 s, under other agents'
tests; the warm pairs, 36.9 → 26.2 s and 24.6 → 23.8 s, hide the round trips in the page cache).
On 3/1/3 the second run still read the NAS (361 s against 464 s cold: the reads left are the
grids' block data, one each, and its 29,909 opens are SMB opens, not reads). So the opens' and
windows' gain in a job is the round trips removed (depth-lo's "files opened" 21 % and "windows
read" 74 % at CPU/wall 0.0–0.2, depth's windows 17 %), bounded by the data reads that remain;
the water read's (depth's 32 %) is measured. Estimated: a depth run of 18 min to roughly 10–12, a
depth-lo run of 41 min to roughly 20–30; the next runs' `scenic timings` will say.

## 7. What ethernet changes

- **Bandwidth-bound (ethernet helps, up to 4×)**: heritage's clip (84 % of 83 min), every upload
  (bldtiles, slope, lo, pack, terrain: 13 MB/s now), the packs and pieces copied to a job (pack 46 %,
  slope 30 %, lo 23 %), names-todo, gc, peaks' z12 tiles (82 %).
- **Round-trip-bound (ethernet helps little: the NAS's HDD seeks and queueing, 50–120 ms a read
  under load, 5–10 ms idle)**: depth's and depth-lo's opens, windows and water reads, the unit's
  staging and small writes. These are what this task cut.
- **Compute-bound (nothing changes)**: bldtiles' tiles (84 %), terrain's (82 %), depth's gaps
  filled and scaling, lo's drawing, peaks' measures.
- **Internet-bound**: the unit's elevations (58 % of a full build's unit time).
