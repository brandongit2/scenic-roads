# Where a build's time goes (task #803, 27 Oct 2026; task #492's snapshot before it)

**A snapshot, as measured on 27 Oct 2026; not kept up to date.** Each step's live split of time and
bytes by phase is measured by the lead and shown on the build page (docs/pool.md §10, Phase shares,
Time by phase). The phases are named here as they are now, as doings (task #844 renamed them:
"the sources' windows read" became "reading the sources' windows"; crate::phase_names).

Measured from the records the pipeline keeps, not new benchmarks: the lead's `coord/timings.jsonl`
(every run's phases; here the 1,454 runs since 25 Oct 00:00 UTC, the current rebuild under the
terrain of task #639: terrain-hi, terrain-deep, terrain-lo) and the coordinator's costs (the latest
measured run of each target, `cost/<key>` in the coordinator's store, `coord/store/`). CPU/wall is the run's CPU time over its wall time
(a job has 1–3 threads). The NAS, net and compute columns are the shares of the run's wall time in
top-level phases of that class (classes declared by hand then; since task #870 phases have no
class, their waits on the NAS, network and disk are measured: docs/pool.md §10, Time by phase, so
the "mixed" shares below would now split into those and computing); "untimed" is in no phase. A phase marked `beside` in a record (a
sampling thread's) is summed over the threads that ran it, so a run's `beside` phases add up to
more than its wall.

**A phase's CPU in this file is the process's while it was open** (the records before task #1116,
`v` 1): where phases ran at once on several threads (the samplers' `beside` phases, the main
thread's phase they ran within), each counted the others' work too. Those figures are
overlap-inflated and not a phase's own: §3's CPU/wall of `sampling hrdem-1m` (2.0) and of
`bringing the heights to EGM2008` (1.9); §4b's "of CPU summed over threads" for EGM2008 (4 min 48 s
→ 10.8 s) and `sampling hrdem-1m` (8 min 44 s → 2 min 52 s), and §5.3's 2 min 52 s; §4d's
`blending the edges` at 75 % of terrain-deep's CPU (a profile: 15 %); terrain-lo's `sampling glo30`
at CPU/wall 0.34 in §3 and 3/4/2's at about 0.6 in §5.5 (summed over threads, each thread's phase
counting the others'). §3's CPU/wall 0.5 of terrain-hi's `sampling the terrain pool into raw tiles`
is the job's while it was open (every thread's, its `job_cpu_s` now), not the phase's own. The
runs' CPU/wall (§2, §4c's CPU s) are the process's and hold. Since task #1116 a phase's CPU is its own work's (jobrt::timings:
each thread's CPU charged to the span innermost on it), a run's phases add up to its CPU, and what
may be another phase's is said (`uncertain_cpu_s`).

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

## 4c. What task #888 did (30 Oct)

Measured on the M4 with `terrain-same` and `terrain-deep-same` (release; the NAS and the live app
read-only, under `sandbox-exec`; own scratch, home and cache; the live agent's jobs beside: noisy,
its load 10–90), main's binary (02f165ce) against this branch's, alternated. CPU is user + sys.
Every run's packs and mids had the manifest's content names, before and after, at every thread
count (hi 37fd592ec4c34e7d / mid b5ead4f29c18907f, hi c6e206311767f103 / mid 1dfe15dbfe28ba3c,
deep 8c1e32762eb0b1ab, lo e1ede1a0acc01de7).

Profiled with `sample` (terrain-same 6/37/17, GLO-30 alone, 3 threads, main): of the CPU, the WebP
encode 52 % (libwebp's `CombinedShannonEntropy_C` alone 29 %, `GetCombinedEntropyUnrefined_C` 8 %),
the sampler 15 %, the blends' distances 10 %, finding each point's file 6 %; and 45 s of sys
time in a 157 s run that a single thread doesn't have (8.6 s): the threads' locks.

| change | before → after |
|---|---|
| **libwebp's combined entropy replaced** (`webpenc::entropy`, installed once before the first encode): the same integer sums without the branch on each bin's emptiness, its slog2 below 65,536 inline. libwebp 1.6 has it in SSE2 and AVX2, not NEON, so the Macs ran its C, which branches on each bin being empty. Wrapping integer sums of the same terms (an empty bin's are 0), so the same result to the bit on every input: the argument in its module's doc; tests against libwebp's own on 5,000 histograms and slog2 at every value it answers; 178 stored terrain and slope tiles decoded and encoded again came out their stored bytes, with main's encoder and with this one | its share of a hi run's CPU 31 → 16 % |
| **A point's file found by the square kept from the point before** (`FileGrid::near`, `GeoSet::holding`): one hash lookup a grid square, not a point | in the runs below |
| **The sampler's passes over a batch on the pool** (`Sampler::sample`: each dataset's points, the blends' points and radii, the second datasets', the geoid's, the blend itself; order kept), and **a distance measure's files read once a file, its last window's cleanness kept** (`GeoSet::distance`: was the open files' lock, the level, the mask and the blocks' map behind their lock, a point) | sys time 45 → 8 s a hi run |

The runs, 3 threads (a job's share before part 1's climb), the same hour:

| target | main: s, CPU s | after: s, CPU s | targets an hour on 3 threads |
|---|---|---|---|
| terrain-same 6/37/17 (GLO-30; 3,665 tiles) | 137.6, 348 (2.5 cores) | 101.7, 275 (2.7 cores) | 26 → 35 |
| terrain-same 6/16/19 (HRDEM, GLO-30; 1,200 tiles) | 41.9, 108 | 36.6, 99 | 86 → 98 |
| terrain-deep-same 10/258/345 (HRDEM 1 m; 5,011 tiles, 2.7 GB fetched) | 222, 439 | 209, 390 | 16 → 17 |
| terrain-same 3/4/2 (lo, GLO-30 read in place on the NAS; 1,347 tiles) | 240, 212 | 379, 205 (the NAS: 715 s of the threads' waits, against 419 s) | NAS-bound |

**Scaling** (the whole process's seconds at 1, 2, 4, 8, 14 threads; the speedup over one thread):

| target | 1 | 2 | 4 | 8 | 14 |
|---|---|---|---|---|---|
| hi 6/37/17, main | 316 | 217 (1.5×) | 140 (2.3×) | 103 (3.1×) | 105 (3.0×) |
| hi 6/37/17, after | 313 | 178 (1.8×) | 113 (2.8×) | 94 (3.3×) | 96 (3.3×) |
| deep 10/258/345, after | 444 | 241 (1.8×) | 172 (2.6×) | 136 (3.3×) | 116 (3.8×) |
| lo 3/4/2, after | 427 | 191 | 185 | 189 | 100 |

A hi run stops scaling at about 8 threads: 12–25 s of each is before the terrain's first level
(the coverage, the water, the plan), and the sampling's chunks were at most four at once
(`AT_ONCE`, before task #896; since, as many as the job's threads, the 64 tiles in flight shared
among them: `terrain_base::at_once`, `chunk_tiles`; the same bytes at any threads, a test samples
at 1, 3 and 7; not timed since). A deep run keeps gaining to 14 (a fifth of its threads' samples wait in `recvfrom`, HRDEM's files
fetched, which more threads overlap). A lo run is the NAS's: its times follow the NAS's load, not the threads (its sys
time grows to 144 s at 14 threads, the SMB reads').

**pyramids-fill** (GSI's tiles fetched and packed onto the NAS) has no CPU to speak of (CPU/wall
0.03 in its live runs, 35–58 min a z6 tile): its fetches are its own pool, as many as the fetcher lets a host
have in flight (then `fetch::HOST_REQUESTS`, 16; since task #1096 the host's limit, store::aimd; task #896: was its own figure, `pyramids::FETCHES`),
whatever threads the job is given, so its rate is GSI's.

**The live runs since part 1's publish (16:00 UTC):** no terrain-hi or terrain-lo ran by 18:40
UTC; the M4's terrain-deep runs, at 4–5 threads, were Japan's (GSI's live layers): CPU/wall 1.02,
1,524 s a run, half of the sampling's wall waiting on the network. So the climb's threads idle
there; the hours before it (3 threads, mostly HRDEM) ran at CPU/wall 1.80.

## 4d. What task #1095 did (10 Nov): the blends' distances

The lead's timings put `blending the edges` at a median 75 % of terrain-deep's CPU. That share
wasn't the blend's own: a phase's CPU was then the whole process's while it was open (records
before task #1116), and terrain-deep samples 4 chunks at once, so each chunk's blend phase also
counted the other chunks' sampling (the sampler's phases' CPUs summed to about twice the run's).
Task #1116 made a phase's CPU its own work's. Profiled with `sample`
(`terrain-same 10/321/338`, HRDEM 1 m and 2 m, GLO-30, Terrarium; 4 threads; 120 s of the
sampling), the sampler's own CPU was the points' projection to EPSG:3979 (24 %), the GeoTIFF
sampling (20 %), the reading order's sort (17 %), the blends' distances (15 %, before the run
grouping below) and the read-ahead (5 %).

| change | measured |
|---|---|
| **The distances by group in parallel** (`GeoSet::distance`, `distance_group`, `Invalid`, `Past`): no serial pass over the points (it tested each point's window for clean blocks, reading missing blocks one at a time); a group whose windows lie in clean blocks at the radius; else a summed-area count of the invalid pixels (a point whose band holds none at the radius at once) and the rows looked at outward only while nearer than the best found; the pixels past a file's edge read once a group from the files holding them, not a lookup a pixel (`value_at`); the groups found as runs of neighbours, not by sorting every point. | 10/321/338: 727, 792, 807 CPU-s before (three runs), 733 after the first part (before the run grouping); the same deep and mid content names as the manifest's (02e54b896b199b6f, e2a73323ce2adb71), before and after |
| **A GeoTIFF dataset's reading order by file and band of about a block's rows** (`Reader::in_reading_order`, the band's height from the dataset's resolution, `GeoSet::row`: no file opened to order the points, where a batch over more files than the memo keeps would open them all, then each again as sampled): was each point's own y, so in a projected CRS every point was its own run and millions were sorted, for each dataset's sampling and the blends' second heights. | not yet timed: the run was held up by the NAS (the owner's work on it); the heights don't depend on the order |
| **A file opened once whatever the threads** (`GeoSet::open`: a cell a file): two threads opening one file at once each read its blocks into the cache. | — |

The heights are the scan's to the bit: unit tests compare every point with a scan of its whole
band (one file; two files of different pixel sizes with fill-mask voids past the edge; four files
at their corner in a projected CRS, one with no data and a hole across a group's border), and
`Invalid::nearest` with a scan of its window over random maps (ties, the rows' early stop). The
early stop is safe as libm's `hypot(x, y)` is never below the larger of x and y, and a row's least
distance only grows outward.

## 4e. A phase's own CPU (task #1116, 11 Nov)

`terrain-same 10/321/338` (this branch's release binary, 4 threads, the M4 at load 60-70 with
other agents' work: its wall is noisy, its shares aren't): 421 s, 678.6 s of CPU. Its phases' own
CPU (the top-level ones, the sampling threads' `beside` ones too) and its 0.24 s in no phase add
up to 678.6 s. Of it, `making the tiles, shading and encoding them` 184.9 s (27 %); the sampling
threads' phases 215.2 s (32 %): `blending the edges` 96.9 s (14 %, where the profile of §4d
found 15 %), `sampling hrdem-1m` 65.6 s, `sampling hrdem` 25.5 s, `sampling glo30` 14.5 s,
`bringing the heights to EGM2008` 12.2 s; and `sampling the terrain pool into raw tiles` itself
223.4 s (33 %), 197.8 s of it uncertain: the sampling threads' work outside their named phases
(the points' projections and reading order, which §4d's profile put at 41 % of the sampler) and
the pool's, taken in by the main thread's phase while the threads' own phases were open. Then
`reading the regions and their outlines` 50.9 s (7.5 %). The timing's own cost (`overhead_s`): 74 ms
for 1,721 spans, 0.02 % of the run. The deep pack and mid had the manifest's content names
(02e54b896b199b6f, e2a73323ce2adb71).

## 4f. What task #1096 did (10 Nov): reads on threads of their own

Before, from the lead's `coord/timings.jsonl` (the runs of 7–10 Nov over 20 s, median a run; busy is
CPU over wall; the waits are the top-level phases' by wall clock, store::waits):

| step | Mac | runs | wall s | busy of threads | NAS wait s | network wait s |
|---|---|---|---|---|---|---|
| terrain-deep | mini | 295 | 612 | 3.41 of 5 | 24 | 178 |
| terrain-deep | all | 693 | 688 | 3.45 of 5 | 25 | 219 |
| slope-deep | mini | 311 | 158 | 2.70 of 4 | 50 | 0 |
| terrain-hi | all | 146 | 130 | 0.66 of 2 | 75 | 0 |
| slope-hi | all | 43 | 111 | 0.46 of 2 | 87 | 0 |
| terrain-lo | all | 12 | 107 | 1.33 of 3 | 65 | 0 |

Where the threads waited: terrain-hi's and terrain-lo's `sampling glo30` (45 and 88 s of NAS a run:
a stored dataset's blocks were read on the sampling threads as sampled, its files opened one after
another, a whole copy made on the thread that first opened the file), slope's `working out the slope
tiles` (20 s of slope-hi's 25, 56 s of slope-deep's 184: a neighbour's edge tiles and slope-hi's deep
mids read on the pyramid's threads as it came to them), terrain-deep's fetched datasets (read ahead
already, `GEO_FETCHES` threads a dataset, each decoding what it fetched).

| change | where |
|---|---|
| **One mechanism** (`store::ahead`): `ahead`, a list read in the compute's order on threads of their own, at most a window past its cursor; `each`, every item at once, in order; `Shelf`, a thread beside the compute reading by key in batches, the compute taking each (waited for while its batch is read, else read by the taker, never twice). How many at once: each source's limit, found as TCP finds its window (`store::aimd`, the owner's design of 10 Nov): the NAS's one a process (from 2, decided once a window on the median of its reads' latency over their size class's floor (the least their smoothed latency was over the last 30–60 s, never under 2 ms; this Mac's cache's answers no samples): doubled until the first halving and one more a window after while near it, halved past three times it or when a read times out, 1 to 16), every read of the NAS taking its turn where it goes (store::range, nasfs: so a raw tile from the NAS's archives counts on the NAS, one from AWS on AWS's); a host's one a host in the fetcher (halved on a slow answer, a 429 or a 5xx; S3's ceiling 64, every other host's 16). No allowance handed out: a quiet reader holds nothing. | store::ahead, store::aimd, buildstore::fetch |
| **The sampler reads every GeoTIFF dataset ahead**, stored ones in place on the NAS too (a whole copy on this Mac's disk is read as sampled); a dataset's files opened together first (`blocks_ahead`); the I/O threads read a block's bytes as stored (`Tiff::fetch`, held within the cache's budget, under a quarter of it shared by the chunks sampled at once) and the sampling's threads decode them (were: the fetch threads decoded too, past the job's grant). | inputs::dem::generic, buildstore::geotiff |
| **The slope's terrain tiles and deep mids read beside its pyramid**: the tiles its shading reads from the NAS (`terrain_wanted`: each tile shaded from its own terrain's four edge neighbours, a missing one's ancestor; not its own pack, copied here) and slope-hi's z10 deep mids, in the order its threads take them (`mids_order`: the depth-first order cut into a run a thread, the runs interleaved). | terrain::slope_pack, terrain_pack::ManifestTiles::reading_ahead |
| **The other readers on it**: get_many's spans, an assembly's mids (terrain-lo, slope-lo), the raw tiles' prefetch. | terrain::terrain_pack, slope_pack |

Not done here: the blends' distance reads (`GeoSet::distance`, `Past`: a window around each group,
mostly blocks the sampling cached), the next chunk's, dataset's or target's reads ahead (each
known only once the stage before it has run: task #1097's pipelining), a pyramid's tiles
(`TileSet::rolling` keeps its own fetches in flight, on its own threads already).

After, on the M1 in a quiet period (11 Nov 00:10–01:04: its pool agent paused, nothing else of
ours running there; the NAS shared with the M4's pool job), `terrain-same 3/7/6` (3 threads; a fresh
`SCENIC_CACHE` each run, so the regions and outlines are read from the NAS as a job's first run on a
Mac reads them; 3.7 GB peak), main (eb0bfc65) and the branch in turn, ABBA, a run's wall s (NAS
wait s):

| binary | runs | mean wall | mean NAS wait |
|---|---|---|---|
| main | 123.3 (17.0), 129.4 (23.0), 134.7 (27.4), 124.8 (16.7), 129.4 (20.8), 127.3 (19.9), 133.0 (26.1), 142.4 (36.2), 127.3 (20.2), 123.7 (17.1), 131.0 (24.3), 129.2 (22.8) | 129.6 (sd 5.3) | 22.6 |
| AIMD as first built (67e0de55) | 155.6 (48.1), 150.3 (43.0), 158.0 (50.0) | 154.6 | 47.0 |
| decided once a window, a slowdown past 2× (af087bdd) | 131.6 (23.5), 128.2 (20.5), 133.9 (25.1), 132.4 (23.6), 142.2 (33.3), 131.3 (22.4) | 133.3 | 24.7 |
| the same, past 3× (this branch) | 127.1 (19.5), 127.7 (20.4), 131.6 (24.3), 128.9 (21.6), 125.6 (17.8), 142.0 (34.3) | 130.5 (sd 6.0) | 23.0 |

Busy threads 1.3 of 3 in every binary (CPU 168–170 s a run); the NAS's part is `reading the regions
and their outlines` (16 threads reading the regions' files whole, `read_all`), the rest CPU. A trace of the
NAS's limiter showed why the first build was slow: each read's own smoothed latency decided, and the
smoothing began afresh at each halving, so one slow answer of the share's long tail (25–500 ms over
the M1's Wi-Fi) halved again: 290 halvings in the phase's first 36 s, the limit held at 1–2 where
main's readers kept 16 in flight; and 64 KB reads this Mac's cache answered in 20 µs had become the
floor of the NAS's real ones. Deciding once a window on its median read fixed the most of it; at 2×
the limit still halved every few seconds at 8–16 in flight (the Wi-Fi full, not the NAS loaded), at
3× it is as quick as main: +0.9 s, within the runs' own spread. terrain-hi (`6/21/20`, 2 threads)
was not timed: on the M1 its `terrain-same` peaks 4.3–6.1 GB (6/18/22, 6/40/54 too), past the quarter
of its memory an agent's run may take; terrain-deep, slope and the network's limits not timed.

## 4g. What task #1097 did (11 Nov): stages that overlap

Each step's stages in the order a target ran them (→ waits for the stage before, ‖ beside it):

| step | before | after |
|---|---|---|
| terrain-deep, terrain-hi, terrain-lo: a level (`terrain_pack::Maker`) | the level's tiles all sampled (its chunks on the grant's threads; in a chunk each dataset in its order, then the blends, then EGM2008) → the level's tiles made (their children's means taken in, rounded, encoded) a block at a time → the next level | each tile made as soon as its chunk is sampled ‖ the sampling of the rest, on the turns the sampling's waits lend (store::waits::Turns) → the level's last tiles made → the next level |
| terrain-deep | its levels → its deep pack and mid written → uploaded → the next z10 tile | its levels → written → the next z10 tile ‖ its uploads (`Landing`, as terrain-hi's were) |
| terrain-lo | its levels → its lo pack written and uploaded → the next z3 tile | its levels → written → the next z3 tile ‖ its upload |
| terrain-hi | its levels → written → the next z6 tile ‖ its uploads | its levels as above, the rest the same |
| slope-hi | the terrain packs' indexes → its own terrain hi pack copied here (the NAS, one sequential read) → its pyramid (edge tiles and deep mids read ahead, #1096) → written → the next z6 tile ‖ its uploads | its own pack copied while the z6 tile before it is made (`slope_pack::Batch`, slope-deep's since #928) → the indexes → its pyramid → written → the next ‖ its uploads |
| slope-deep | the next target's pack copied ahead, the uploads beside (#928) | the same (`Batch::deep`) |

What still waits, on what it reads: a chunk's later dataset on the earlier one's pass over that
chunk (which points are left; the chunks themselves go side by side, a chunk a thread of the
grant), the blends on the chunk's datasets, a level on the finer level's tiles (its quarters are
their means, and whether a child is built decides a quarter's sampling), a pack on its last level,
a target's pyramid on its own pack's copy (the first of a job). The making computes only on turns
the sampling lends: where the sampling keeps every thread computing nothing is gained (as it
shouldn't: the grant is the job's cores), and where the Mac's cores are all taken a lent turn's
core isn't idle either. Not read ahead: a chunk's next dataset (its points known only once the
dataset before has run over the chunk; a sampling thread waiting on its first blocks lends its
turn to the making instead).

In a run's phases a level is now the main thread's `sampling the terrain pool into raw tiles`, then
`making the last tiles sampled` (what the making had left once the sampling ended); the making
itself, `making the tiles, shading and encoding them`, a main thread's sub-phase before, is its
threads' (beside, a phase a tile: its wall the threads' making summed, not the level's span), so a
run before and one after differ there by more than their times. terrain-deep's `writing and
uploading its deep pack and mid` reads as `writing its deep pack and mid` (jobrt::phase_names);
terrain-lo's `uploading the lo pack` is `writing the lo pack` from now on, not mapped (slope-lo's
phase keeps the name, its upload still in it).

Measured with `terrain-same` (release; the NAS read only; `SCENIC_CACHE` a cache of the runs' own,
its coverage file cloned in, so the regions are read from this Mac; a target's Terrarium tiles and
whole local copies warmed by a first run not counted). On the M4, main (03e6cd6b) and the branch
side by side, each round both at once, so both see the same load (the M4 at load 20–110, other
agents' work); on the M1 in a quiet period (11 Nov 02:24–02:37, its pool agent paused; one depth job
of 2 threads still running there to its safe point), ABBA. "s" is the target's own (`secs`, the
regions' read and the process's start not in it), busy the run's CPU over its wall, peak the
process's footprint (`time -l`). Every run's packs and mids had the manifest's content names (deep
02e54b896b199b6f / mid e2a73323ce2adb71; hi 9a23248d786d9a1e / mid 5fd1d526190eb3bc; lo
b7effd8690bb7665).

| step, target | Mac, threads | main s | branch s | change | busy of the threads | peak MB |
|---|---|---|---|---|---|---|
| terrain-deep 10/321/338 (HRDEM 1 m, 2 m, GLO-30, Terrarium; 3,733 tiles) | M4, 5, 3 rounds | 199.6, 191.8, 277.7 | 166.8, 159.2, 217.0 | −33, −33, −61 s (−16 to −22 %) | 2.79, 2.97, 2.33 → 3.32, 3.59, 2.93 of 5 | 6226–6735 → 6215–6529 |
| terrain-hi 6/37/17 (GLO-30, Terrarium; 1,344 tiles) | M4, 3, 3 rounds | 97.5, 93.4, 90.2 | 89.7, 87.7, 72.1 | −8, −6, −18 s (−11 % mean) | 1.30, 1.33, 1.34 → 1.43, 1.43, 1.66 of 3 | 4952–5019 → 4861–5002 |
| terrain-lo 3/7/6 (1,365 tiles) | M1, 3, ABBA ×3 | 49.9, 56.8, 57.4 | 43.7, 43.4, 45.4 | −10.5 s mean, of which the levels −2.7 s (33.5 → 30.8 s; the rest the NAS's reads of the mids and declarations, 13 s slower in two of main's runs) | 1.79 → 2.20 of 3 | 3718–3792 → 3730–3762 |

The CPU of a run didn't change (deep 654–675 s both, hi 122–130, lo 99–101): the making ran on
cores the sampling left idle. Where the sampling waits most (deep: HRDEM's blocks over the network)
the most is hidden; terrain-lo's levels are its making's (26 s of CPU-bound encoding on 3 threads
after 4 s of sampling from this Mac's cache), so a few seconds. The peaks are the same: a tile is
made soon after it's sampled, so fewer raw tiles are held than when a level's were all sampled
first, and the making holds a tile a thread. No step's cost version moved: its memory is what it
was, and its busy cores are measured afresh by the uses' window and the jobs' beats.

slope-hi (`measure_a_slope_batch hi 37/17,37/18,38/17,38/18`, 4 threads, the M4, two rounds, each
a batch then its z6 tiles alone, the same content names): waiting on a z6 tile's own pack's copy
9.7 and 6.9 s for the four alone, 2.8 and 3.9 s as a batch (the first's alone; the others' 12.2 s
of copying ahead beside the work); the batches 117.1 and 127.8 s against 141.9 and 127.8 s alone,
within the M4's noise. slope-deep unchanged (its batch since #928), not timed again.

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
