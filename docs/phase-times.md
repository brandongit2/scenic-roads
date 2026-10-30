# Where a build's time goes (task #803, 27 Oct 2026; task #492's snapshot before it)

**A snapshot, as measured on 27 Oct 2026; not kept up to date.** Each step's live split of time and
bytes by phase is measured by the lead and shown on the build page (docs/pool.md §10, Phase shares,
Time by phase). The phases are named here as they are now, as doings (task #844 renamed them:
"the sources' windows read" became "reading the sources' windows"; crate::phase_names).

Measured from the records the pipeline keeps, not new benchmarks: the lead's `coord/timings.jsonl`
(every run's phases; here the 1,454 runs since 25 Oct 00:00 UTC, the current rebuild under the
terrain of task #639: terrain-hi, terrain-deep, terrain-lo) and the coordinator's costs (the latest
measured run of each target, `coord/costs.json`). CPU/wall is the run's CPU time over its wall time
(a job has 1–3 threads). The NAS, net and compute columns are the shares of the run's wall time in
top-level phases of that class; "untimed" is in no phase. A phase marked `beside` in a record (a
sampling thread's) is summed over the threads that ran it, so a run's `beside` phases add up to
more than its wall.

## 1. A full build, by step (the per-target steps, summed over the latest run of each target)

| step | targets | median s | p90 s | sum h | peak MB p90 |
|---|---|---|---|---|---|
| terrain-hi | 454 | 366 | 1053 | 90.9 | 5489 |
| depth | 841 | 26 | 747 | 54.6 | 2630 |
| terrain-deep | 1070 | 5 | 535 | 46.8 | 7039 |
| unit | 621 | 113 | 431 | 35.1 | 5079 |
| bldtiles | 2185 | 0 | 28 | 18.1 | 3867 |
| trees | 2145 | 5 | 48 | 13.9 | 3997 |
| slope | 2211 | 7 | 42 | 12.5 | 3826 |
| bldprep | 1488 | 1 | 18 | 3.7 | 1038 |
| terrain-lo | 58 | 64 | 600 | 3.5 | 5387 |
| depth-lo | 62 | 34 | 352 | 2.3 | 1927 |
| peaks, tail, pois, bldtile, the -lo assemblies, lo, pack | | | | ≈ 6 | |

≈ 290 h of target time (the costs also still hold 79 h under the retired names `terrain` and
`terrain-fine`, task #639's predecessors, not counted). Many depth and terrain-deep targets are at
their first, cheap run (a z10 tile with no data: terrain-deep's median 5 s); the sums are what
the rebuild has measured so far, not a whole build's.

## 2. The current rebuild's runs, by kind (since 25 Oct: 233 h of worker time)

| kind | runs | sum h | a run s (median) | p90 s | CPU/wall | NAS | net | compute | untimed |
|---|---|---|---|---|---|---|---|---|---|
| terrain-hi | 484 | 141.5 | 512 | 1509 | 0.69 | 4 % | 95 % (mixed: the sampling) | 1 % | 1 % |
| terrain-deep | 262 | 56.6 | 177 | 2254 | 1.42 | 8 % | 92 % (mixed) | 0 % | 0 % |
| terrain-z8 | 5 | 5.3 | 4509 | 5941 | 0.60 | 0 % | 97 % | 0 % | 3 % |
| terrain-lo | 24 | 4.4 | 305 | 1771 | 0.37 | 5 % | 90 % (mixed) | 0 % | 5 % |
| unit | 63 | 3.6 | 121 | 395 | 0.39 | 64 % | 3 % | 30 % | 1 % |
| depth | 30 | 3.3 | 246 | 1210 | 0.80 | 24 % | 0 % | 76 % | 1 % |
| catalog | 134 | 3.1 | 72 | 138 | 0.59 | 95 % | 0 % | 5 % | 0 % |
| pack | 215 | 2.8 | 16 | 123 | 0.29 | 80 % | 0 % | 17 % | 2 % |
| depth-lo | 35 | 2.7 | 117 | 718 | 0.32 | 68 % | 0 % | 86 % (nested) | 0 % |
| inputs | 74 | 2.3 | 8 | 88 | 1.55 | 100 % | 0 % | 0 % | 0 % |
| lo | 11 | 1.7 | 589 | 799 | 0.30 | 73 % | 0 % | 27 % | 0 % |
| bldtiles | 27 | 1.2 | 97 | 340 | 1.20 | 14 % | 1 % | 81 % | 4 % |
| water, names-todo, terrain-water, heritage, pois, task tail, the rest | | ≈ 5 | | | | | | | |

The terrain's steps are 89 % of the rebuild's worker time, and inside them the elevation pool's
sampling (`sampling the terrain pool into raw tiles`, `sampling the fine elevation data into high-zoom terrain`) is 78 % of terrain-hi's run and 92 % of
terrain-deep's; everything else of this rebuild (slope, trees, depth, unit, pack: the steps task
#492 measured) is under 5 % of it, each with the profile §3 of the 20 Oct snapshot gives, minus
what tasks #689, #706, #728 and #532 took out: depth's run is now 76 % compute (composing 51 %,
the floor's shading 22 %), its water 20 %; depth-lo is NAS reads (the sources' windows 39 %,
their files opened 14 %, the whole local copies made beside 54 %).

## 3. Inside the terrain, and what each part waits on

- **terrain-hi** (1,053 s a run on average; 512 median): `sampling the terrain pool into raw
  tiles` 820 s a run, CPU/wall 0.5 with 3 threads: the level's tiles sampled from the pool in
  chunks of 16 tiles on up to 3 threads beside the main one. Then `tiles prepared, the block
  before them levelled, shaded and encoded` 159 s (CPU/wall 1.7: the water rasterised, lakes
  levelled, rivers, WebP), the regions read 22 s, the uploads 20 s.
  - **Where the sampling threads' time goes is in their `beside` phases**, 350 s a run summed
    over the threads on average, but the average hides two populations:
    - **Japan (16 runs, 58.6 h: 41 % of terrain-hi's time), CPU/wall under 0.3: the network.**
      Each GSI layer (gsi-1a, 5a, 5b, 5c, 10) is a tile pyramid fetched live from GSI's server at
      its own zoom, never cached between runs: 6/56/23, a small coastal z6 tile, fetched 2.9 GB of
      GSI tiles in its sampling and 1.8 GB more in `blending the edges` (the tiles were out of the
      512 MB memo by then), 4.7 GB at 1.5–2.5 MB/s, 31 min; run again (the job retried) it
      fetched it all again. 6/54/26 + 6/53/31 ran 15.3 h and failed; 6/53/31 8.5 h, 6/56/25
      7.6 h, 6/52/31 5.8 h. Not the NAS, not CPU (9 min of CPU in 15 h).
    - **Everywhere else (468 runs, 83 h), CPU/wall 0.8–1.7: the CPU, on 3 threads.** A run with
      HRDEM in its plan (Canada: 6/16/19, 350 s in the job) spent its sampling threads' time in
      `sampling hrdem-1m` 180 s, `sampling hrdem` 157 s, `sampling glo30` 185 s, `blending the edges`
      136 s; a GLO-30-only run (6/37/17, 658 s) in `sampling glo30` 417 s and `blending the edges`
      458 s. Profiled on the M4 (`sample`, 10 s of 6/16/19's sampling, before this task): 60 % of
      the CPU was `Cover::contains` (a point-in-outline test per point per dataset, called five
      times a point: the counts, each dataset's projection, its selection, the blend's two
      selections) and the projection of every point to each dataset's CRS (HRDEM's two datasets
      each projected all 4.2 M points of a chunk to EPSG:3979, Helmert and Lambert); the NAS's
      `pread`s were a tenth of it; and 44 % of the threads' wall was waiting on one lock: the raw
      tiles' shard lock, held while a Terrarium tile was read from the NAS's archive by range.
  - The remaining fixed costs a run: `reading the regions and their outlines` 22 s (CPU 14 s: the
    coverage's outlines parsed and gridded; every step's run pays it: 2.1 % of terrain-hi, 2.5 %
    of terrain-deep, 16 % of a unit run, half a pois run), `waiting for the uploads beside to land` 20 s.
- **terrain-deep** (777 s a run on average, 177 median; 42 h of its 57 are 135 runs on the Mac
  mini at CPU/wall ≥ 0.8): `sampling the fine elevation data into high-zoom terrain` 718 s: `sampling hrdem-1m` 370 s (summed over
  threads, CPU/wall 2.0: HRDEM's 1 m blocks decoded and bilinearly sampled, 533 MB a run fetched),
  `bringing the heights to EGM2008` 191 s (CPU/wall 1.9: the geoid grids read bilinearly for every
  point of every chunk, 2.2 G points a run), `sampling hrdem` 139 s, `blending the edges` 104 s,
  `its base made` 70 s, 3DEP 67 s; then its deep pack uploaded 21 s, the regions read 19 s, the
  basemap's water opened 11 s (CPU/wall 0.01: the NAS). Bound by the CPU on its three threads;
  the same sampler as terrain-hi's, so this task's cuts apply. Eleven runs (0.4 h) waited at
  CPU/wall under 0.3: 3DEP's files opened and read ahead over the internet (`store = false`
  datasets), and on the mini the NAS (`opening the basemap's water` 52 s, `the fine datasets'
  declarations read` 32 s in 10/299/385).
- **terrain-lo** (664 s a run, 24 runs): `sampling glo30` 434 s summed over threads at CPU/wall
  0.34: the z8–z3 levels read GLO-30 at its overviews from the NAS in place (not the whole local
  copies: a coarse level reads under half of each file), round trips.
- **terrain-z8** (5 runs, 61 min each, CPU/wall 0.6): the world's z8 tiles sampled and made, once
  a build.
- **unit** (208 s a run in rebuilds): the pass, coverage, pieces and reaches read 32 s, the next
  z6 tile's packs copied ahead 27 s (beside), canopy 24 s, records saved 20 s (the uploads put off
  landed), layers staged 17 s; CPU/wall 0.39: the NAS.
- **depth** (399 s a run): composing 202 s (gaps filled 64 s, the sources' windows read 37 s,
  stored levels scaled up 22 s), the floor's shading 89 s, the water read 44 s and opened 39 s.
  CPU/wall 0.8 with 1–3 threads: half compute on one thread, half the NAS.
- **pack** (46 s): getting the z6 tiles' base packs here 20 s, waiting for the uploads beside to land 17 s, drawn 7 s.
- **catalog** (71 s a run, 134 runs on the mini): `drawing the coverage and finding the credits`
  86 % (CPU/wall 0.6).

## 4. What was done (task #803), and what each is bound by

Measured on the M4 with `scenic-build terrain-same` and `terrain-deep-same` (release, 3 threads,
the NAS read-only, output to scratch, other agents and the live pool's own jobs running beside:
noisy), the "before" binary the installed app's (main at 479145d3, this branch's base).
Byte-identical outputs: each run's hi pack and mid, or deep pack, has the content name the
manifest has (the `-same` tools' line), before and after.

| change | bound by | before → after (measured) |
|---|---|---|
| **The sampler's cover tests once a point** (`dem::generic::membership`, `Cover::contains_box`): which datasets hold each point of a batch is worked out once, in runs of 1,024 neighbours answered by their box where it lies inside a cell away from an outline (the terrain's rows), the rest point by point; the counts, each dataset's selection and the blends read it. Was: `Cover::contains` five times a point a dataset, each a point-in-outline test in an outline's cell. | CPU | 6/16/19 (1,200 tiles, HRDEM + GLO-30): the sampling 64 → 39 s, the run 109 → 87 s |
| **One projection a CRS a batch** (`Sampler::projections`): the datasets of one projected CRS (HRDEM's two in EPSG:3979, 3DEP's zones) share one projection of the points any of them holds. Was: each dataset projected every point its cover held. | CPU | 6/16/19: the sampling 39 → 32 s, the run 87 → 76 s (both: 109 → 76 s, −30 %; `sampling hrdem-1m` 36 → 6 s a thread) |
| **A file's fill mask found once a block, not a point** (`GeoSet::sample`, `distance_group`): `mask()` cloned two handles a point. | CPU | within the noise on 6/16/19 (profiled at 3 % of the sampling's CPU) |
| **The raw tiles' shard lock covers the lookup, not the read** (`RawTiles::archived_by`): a Terrarium tile read from the NAS's archive by range is read outside the shard's lock. Was: the threads sampling a level waited on each other's NAS round trips (44 % of their wall in the profile). | the NAS's round trips | no separate measure: in the runs above |

The three sampler changes together, main's binary against this branch's, the same hour:

| target | before | after | the content names |
|---|---|---|---|
| terrain-same 6/16/19 (Canada: HRDEM's two datasets, GLO-30; 1,200 tiles) | 109 s (sampling 64 s) | 76 s (32 s) | hi c6e206311767f103, mid 1dfe15dbfe28ba3c, both the manifest's |
| terrain-deep-same 10/258/345 (HRDEM 1 m; 5,011 deep tiles) | 371 s | 306 s (`sampling hrdem` 83 → 8 s, `sampling hrdem-1m` 246 → 153 s, `blending the edges` 59 → 35 s, EGM2008 102 → 94 s, summed over threads) | deep 926a54b38aabbe2c, the manifest's |
| terrain-same 6/37/17 (GLO-30 and Terrarium alone, no outline, no projection; 4,000 tiles) | 227 s | 216 s | hi 37fd592ec4c34e7d, mid b5ead4f29c18907f, the manifest's |

So a run with a regional dataset (an outline, a projected CRS: Canada, the US, Japan's land)
loses a fifth to a third of its time; one over GLO-30 alone changes within the noise (its
sampling is GLO-30's blocks read and decoded from the NAS, and the blend's distances). Japan's
runs (§3) are the network's and change by the same CPU amount only.

## 4b. What task #807 did

Measured as §4 was (the M4, release, 3 threads, the NAS read-only, output to scratch, the live
pool's jobs beside), one binary with the old path switched on by a temporary flag for "before",
so both ran the same code otherwise. The heights compared tile by tile, every pixel decoded from
the two deep packs: the pixels changed, the largest and the mean difference.

| change | before → after | the output |
|---|---|---|
| **The geoid's offsets on a lattice** (`dem::geoid::Grids::offsets`): found exactly at the corners of a 7.5″ lattice over a call's points and bilinear between them; a point whose corners lack a value or were served by different sources (a grid's edge, a link's grids meeting) read alone, and points spread wider than twice their count in corners read as before. 7.5″ divides EGM2008's 2.5′ grid and its half-pixel, so inside its cells the result is the grid's own bilinear to rounding. Was: every point read bilinearly from the grids (2.2 G a run). | terrain-deep-same 10/258/345 (HRDEM 1 m; 5,011 deep tiles): `bringing the heights to EGM2008` 4 min 48 s → 10.8 s of CPU summed over threads; the run 394 → 258 s | 26 of 328 M pixels changed, each by one quantum (1–2 cm: rounding at a quantum's edge), the mean 0.000 m: invisible. Not byte-identical (content name 8c1e32762eb0b1ab, the manifest's 926a54b38aabbe2c) |
| **The coverage kept with its grids** (`coverage::write_kept`, `RDCOV005`): the cache file each Mac keeps by the regions' accepted version (`<SCENIC_CACHE>/coverage/<hash16>.cov`, written by the first job to read a version) holds every shape's grid as well as its rings, so a job reads it rather than gridding the outlines again (`Shape::new`); `pois` and the z6 tiles' steps read through it too (they passed no cache). Not on the NAS: a local read beats a NAS one, and each Mac makes it once a version. | `reading the regions and their outlines`: 18.7 s (CPU 14.4 s) → 1.1 s (CPU 0.7 s) a run once the file is there; the file 1.84 GB for the current version, read and written as a stream (`from_kept`, `write_kept`), so a run holds the shapes alone, no more than before (as first written it was read whole: a run's peak 2.35 → 3.6 GB) | the same shapes (a test: the grids read back equal those made) |
| **A window's pixels copied a row at a time** (`geotiff::Block::row_f32`, `Tiff::read_window`), **a block's cleanness noted once** (`dem::sample::sample_raster_noting`). Profiled on 10/258/345: the largest part of the sampling's CPU was `read_window` copying each 512² window a sample at a time through `Block::f32_at` (its type matched per sample), and `Clean::note` scanning every block again each time a group of points read it; the bilinear itself under 1 %. | terrain-deep-same 10/258/345: `sampling hrdem-1m` 8 min 44 s → 2 min 52 s of CPU summed over threads (now the LZW decode and the download); the target 271 → 206 s | byte-identical (8c1e32762eb0b1ab, the geoid's change's) |
| **An overview read whole from the NAS** (`geotiff::Tiff::read_level_whole`, from `dem::generic::GeoSet::source`): a stored file read in place on the NAS at a coarser level than its own (terrain-lo's GLO-30) has that level's blocks read in one range (where they're at least half the bytes they span, and 32 MB at most) and served from memory. Was: a round trip a block. | terrain-same 3/4/2 (1,347 lo tiles): `sampling glo30` 12 min 49 s → 9 min 35 s summed over threads, the target 333 → 261 s; the run's peak 7.5 → 8.8 GB uncapped (the levels held while their files are open: a process's spans now capped at 512 MB, `geotiff::SPANNED_MAX`, past which a level is read a block at a time) | byte-identical (lo 61ef52dfb4f7ab6c both; the manifest's bc55fdaa05b56820 differs before this change as after; not traced) |

## 5. Not done, with what the data says (the ranking of what's left)

1. **GSI's tiles fetched live, every run (41 % of terrain-hi: 58.6 h in 16 Japan runs; the
   network at 1.5–2.5 MB/s).** The pyramids a live server gives (`[data] encoding = gsi-png`,
   `url`, `zoom`) have no store: a run fetches each tile it reads, the blend fetches most of them
   again (the 512 MB memo holds a tenth of a run's), a terrain-deep target fetches the same tiles
   at its zooms, and a job run again fetches it all again. The Terrarium pyramid has the answer
   already: fetched once into the lead's cache and packed onto the NAS in shards
   (crate::rawpack, `RawTiles`), read from there by every later run. Giving the live pyramids
   the same store (a `sources/fetched/<id>/` of packed tiles, as the stored GeoTIFFs have) makes
   Japan's z6 tiles a fetch once a build (2.9 GB a tile at the server's rate), not 4.7 GB a run
   and again a retry; it is the largest single item left and is a feature (the rawpack's keys,
   the packing job, room-making), not a fix. The same bytes: a tile is the server's bytes
   whichever copy it's read from. Until then, `TILE_MEMO_BYTES` (512 MB) could hold a run's GSI
   tiles for the blend (a small tile's 2.9 GB wouldn't fit the mini's jobs: the memo is per job).
2. *(Done by task #807: §4b.)* Bringing the heights to EGM2008.
3. **HRDEM at 1 m decoded for every deep level** (after task #807, §4b: 2 min 52 s of CPU a run
   summed over threads, nearly all the 1 m blocks' LZW decode and their download; the bilinear
   itself is under 1 % in a profile, so a nearest-pixel read would buy nothing). z13–15 read the
   same 1 m blocks z16 does; reading HRDEM's overviews nearest those levels' pixels (as the base's
   levels read GeoTIFFs: `read_resolution`) would decode about a third as much, at the price of
   averaged heights there (to be measured before it's kept).
4. *(Done by task #807: §4b.)* `reading the regions and their outlines`.
5. **terrain-lo's GLO-30 reads**, after task #807 (§4b): 3/4/2's `sampling glo30` is still 9.6 min
   summed over threads at CPU/wall about 0.6; what's left is most likely each file's opening (its
   header windows, a round trip or two a file, some 1,800 files a z3 tile), not yet profiled.
6. **depth's compute** (composing on one thread: gaps filled 16 %, scaled up 6 %): as task #492
   left it (§5.2 of the 20 Oct snapshot); depth is 1.4 % of this rebuild.
7. **The uploads at Wi-Fi rates** (terrain-hi's 20 s a run, terrain-deep's 21 s, pack's 17 s):
   ethernet, as before.

## 6. The 20 Oct snapshot (task #492), kept for the depth chain and the uploads

The measures of depth, depth-lo, unit, bldtiles, slope, pack and heritage, and what task #492
did (a GeoTIFF's header read in windows and its tile arrays kept; a block's water tiles read from
the basemap in runs), are in the commit history of this file (64a7c64a, ac2387a1, and the file
as of 479145d3); their profiles still hold where §2 doesn't say otherwise.
