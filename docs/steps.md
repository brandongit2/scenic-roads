# The build's steps

What each step does, in detail. The steps as one table (names, chains, the checklist, the build page) are `docs/pipeline.md`.

Every step's geometry is `roadcore::geo`'s, one of each: the Earth (`EARTH_R`, the mean radius:
every ground distance, `M_PER_DEG` a degree on it), the Web Mercator (`merc`, `merc_clamped`,
`merc_lat`, `tile_box`, `epsg3857`; its sphere WGS 84's semi-major axis, `MERC_C` its equator in
its own metres), the box-grower (`grow`, `tile_box_grown`: a box grown by its km at its farthest
latitude, 89° at most), "near the coverage" (`NEAR_COVERAGE_KM`, 20 km) and the even–odd point-in-polygon
(`inside`, exact on E7 points; the coverage's gridded test, `Coverage::inside`, aside; the lake fill
reads OSM's overlapping water by the nonzero rule, `winding`). Every
transcendental function is `det`'s (`clippy.toml` refuses `f64::sin` and the rest), so a step
makes the same bytes on every Mac.

## 1. The OSM pass

1. **The planet is declared and fetched** (`crate::planet`, docs/inputs.md §6.9): the accepted
   `osm-planet` declaration (`inputs/sources/osm-planet.toml`) pins it, its `version` the planet's
   date; a new planet is the owner's edit of it. When the declared planet is newer than the last
   complete pass and the NAS lacks it, the lead's `planet-fetch` job (a fetch, counted by the pool's
   downloads cap) brings it once from its URLs in turn (the mirrors, then the main site) by the
   build's fetcher (its User-Agent; 429 and 5xx answers waited out), resumably into
   `sources/osm/<date>/planet.osm.pbf.part`, checks it against its published MD5 (read back from
   the NAS; a mismatch starts the part over from the next URL) and names it
   `sources/osm/<date>/planet.osm.pbf`. The pass reads it there; its date and path are the pass's
   target and key, as before the declaration.
2. **Copy to the SSD** when there's room (docs/macs.md).
3. **Filter:** one `osmium tags-filter` into (a), the filtered planet, kept until the next pass so a
   new tag needs no download.
4. **Sets and the basemap's input (b)**, filtered from (a). Sets are versioned (`summits-v2`).
   `pass-sets` makes a finished pass's missing sets (copying (a) here when there's room for it and
   20 GB).

   A filter whose input isn't copied here reads it off the NAS over SSH, never the SMB mount
   (`osmpass::streamed`: `ssh brandontsang@fishandchips.local cat …`, one stream for each time
   osmium opens it, through a named pipe, cipher list `aes128-gcm@openssh.com,chacha20-poly1305@openssh.com,aes128-ctr` (the first the NAS also offers; GCM measured 25 -> 59 MB/s on the Mac mini); the soft mount timed out on the hour-long reads). It
   needs SSH keys to the NAS (docs/macs.md) and fails saying so. A job under the budgets' sandbox
   reaches the NAS through its agent's proxy (`CONNECT`, unmetered). The bytes count as NAS reads;
   each read osmium takes to its end must carry the file's whole size (taken over SSH first). The first cut (`osmium
   extract`, three reads) streams (a) the same way when the local copy went. osmium extract first
   opens its input once and lets go of it after its first bytes, or before any (osmium 1.19): a
   read let go early stops its `cat` and the next open is fed anew. A feed that fails lets osmium
   go (the pipe's link removed, an open waiting on a pipe given its end), so the job fails with the
   stream's error rather than wait; a read moving no bytes for 5 min of the job's running time
   (`osmpass::STREAM_STALL`; a freeze or a pause isn't counted, as for a NAS call's deadline,
   docs/nas-access.md §6) fails `TimedOut` and the job is tried again later. That bound is a stuck
   stream's deadline, fixed as the NAS calls' deadlines and `SSH_START` are, not a limit on what
   the build uses: it's counted from the stream's last byte, so a read that keeps moving, however
   long or slow, is never ended by it. Known retry cause: a
   job frozen by memguard for over 10 minutes freezes its stream too (the feeder is in the job's
   process group), the proxy ends the idle tunnel, and the job fails; its retry redoes that
   stage.
5. **Outlines:** administrative levels 2–8 and ISO 3166 areas, assembled into polygons in a
   sectioned file, with simplified copies. Made by their own step (`outlines`,
   `osmpass::make_outlines`, keyed by `OUTLINES_V`, the pass, the set and the previous outlines);
   nothing reading outlines is planned while it has work (after three failures the rest goes on).
   - osmium silently leaves out a relation whose rings don't close. `buildstore::outlines::assemble`
     finds those: **countries and territories** are assembled by joining ways and closing gaps of up
     to 1 km (`MAX_GAP_M`) with straight lines crossing nothing, flagged `REPAIRED`; one that still
     can't be keeps the previous pass's outline, flagged `STALE`; with none, the step fails.
     **Others** are left out with a warning.
   - A country's code is its own assigned `ISO3166-1` tag, else read from its ISO 3166-2 code.
6. **Basemap:** Planetiler on (b), worldwide (`layers/basemap/world-<date>`), its jar and data pinned
   in `sources/basemap/`. The pass refuses a jar of another version than
   `inputs::water::PLANETILER_VERSION` until its water has been checked against full detail and pinned.
7. **Cut (a) into z6 tile extracts:** a depth-first quad tree, four outputs per osmium run (osmium
   keeps ~4 GB of id sets per output). Ways whole, multipolygons completed, 10 km buffer. Each
   extract (stored in `sources/osm/<date>/pieces/`: the folder's old name, kept) is uploaded with
   its road links as soon as it's cut. (`scenic-build patch-ferries` gives extracts the ferry ways
   a pass filtered without `w/route=ferry` lacks.)
8. **Road values** (below).
9. **Finish:** the pass's summary is written; older passes retire from the manifest (but the
   previous pass's outlines, which the outlines step reads).

## 2. Global-source layers

- **Terrain** (`terrain::terrain_pack`): one pyramid, z0 to each place's finest zoom, built by one
  rule at every zoom (task #929, the owner's), its jobs split where scheduling wants them:
  - **Each place's finest zoom** (`terrain::terrain_zoom`) is derived from the declared elevation
    datasets meeting it (docs/inputs.md §6.8), never typed: a dataset's finest zoom at a latitude is
    the finest whose pixels are no finer than its cell there (`CELL_FACTOR` 1, the owner's), its cell
    its latitude band's (`Measured::bands`, measured per file by the input check), else its
    resolution; a tile pyramid's, its declared zoom. A place's sources are the datasets with data
    under it (their measured presence, by z10 tile: `Measured::presence`; one not measured so, its
    coverage's cells) in their declared order (rank, then resolution) up to the first holding it
    whole; its finest
    zoom is the finest of theirs at the tile's poleward edge, at most z16 (`DEEPEST`: the deepest
    the map stores). The polar caps take the same rule on their grid's pixel (`polar::finest`).
  - **The rule** (`terrain_pack::Fill`), quarter by quarter of every tile at every zoom, by the child
    one zoom finer under it:

    | the child | the quarter | where |
    |---|---|---|
    | built (inside a region: within 20 km of the coverage, and no finer than its place's finest zoom) | the child's 2×2 means, of its heights before rounding | every zoom below a place's finest: z0–2 from the lo packs' z3, z3–8 from the z6 tiles' mids, z9–11 in a z6 tile's run, z12 from its z10 tiles' deep mids, z13–15 in a z10 tile's run |
    | past its place's finest zoom (inside a region) | sampled from the pool at this zoom | the place's finest zoom: z12 or z11 where only a worldwide dataset is, z13–16 where a finer one is (a quarter of a coarser band whose finer data has a gap, too) |
    | outside every region | the relief: the datasets published at every zoom (their declarations' `every_zoom`: `terrain_zoom::relief`) read at this zoom, one tile a tile | z0–8, and z9–12 past a region's 20 km |

    A tile with no quarter to sample reads nothing from the pool; one with no quarter at all isn't
    stored. Where a dataset has a gap (its voids, its edge), the next in the declared order fills it
    at its own finest detail (each dataset read at most at its own zoom: crate::terrain_base), so
    a place's fill is the next-finest dataset there, whichever it is. A child that should exist but
    has no data at all (no dataset there) is sea level.
  - **Rounding:** a job carries its levels' heights unrounded between them and rounds each tile only
    as it's stored (`finish`): to 1 cm where the tile is its place's finest zoom somewhere, else
    10 cm, ties to even (crate::terrain_codec); only reads across jobs (the lo packs' z3 for the
    root) see rounded tiles.
  - **The jobs** (each boundary scheduling's):
    - **terrain-deep**, a run per z10 tile whose places reach past z12 (`terrain_deep::levels`, near
      the coverage), from the start (it reads no built tile): its tiles z13–16 by the rule, its deep
      pack, and its mid (`work/terrain-deep-mid/10-x-y`: its z13 tiles' means before rounding).
      A z10 tile, not its z6 tile: up to 5,440 tiles and GBs of a fine dataset's reads each (its
      progress its tiles made, said every 5 s). A tile
      of the band is kept only where some of its pixels came from a dataset reaching its zoom
      (`terrain_zoom::reaching`, the sampler's sources by pixel); one that would only upsample
      a coarser dataset isn't, and its parent's quarter there is its own sample: so the next
      dataset's zoom holds there, as the depth's rule (§3, #804). The band is the long part of a
      rebuild: of the ~40 Mac-days estimated for a whole terrain rebuild, most are its targets.
      A target none of whose sources reaching past z12 anywhere in it (its zoom at any of its z16
      rows' poleward edges) has data there (`terrain_zoom::hollow`, #842: the input check's finer
      read found none, their `Measured::hollow`, docs/inputs.md §6.8; or the source's regions'
      outlines leave none of the tile, a source there by its 1° cells alone, `Cover::holds_none`)
      and with none of its outputs named (`deep_outputs_named`: a deep or slope deep pack, or a
      mid) is never run, nor its slope-deep: such a run would keep no tile (the rule above) and
      make an empty mid, which its z6 tile reads as none. It stays a target keyed as before (no key
      moves, the pyramids-fill's and terrain-hi's included), settled without a record
      (`TerrainTargets::deep_hollow`): nothing waits for it, the checklist counts it "with no data,
      not run". One with an output there (an empty mid an earlier run made,
      `TerrainTargets::deep_hollow_named`) keeps it while its key holds: its mid is of the versions
      now. Once its key moves (a version, what it reads), its terrain-deep and slope-deep are each
      a drop, not a run (`agent::build::hollow_drops`, #1087: an empty run took 231-327 s): offered
      under its "none" key, its run drops its outputs (scenic-build drops a hollow target as one no
      target any more, `deep_made`), and its z6 tile's terrain-hi and slope-hi wait for it as for a
      run, so neither reads a mid of another terrain or slope version, which they refuse. Its
      record is the "none" key; with nothing named it is then settled as one never run. Its z6
      tile's keys name its mids, so they move once when they go, and its runs then read none
      there, as over a target never run. One whose key can't be told now waits, as any target
      does: a key unread for a moment never drops outputs that may be current. Only an app that
      drops it is given the drop: a worker builds for a lead on its own app alone.
    - **terrain-hi**, a run per z6 tile near the coverage (`build_piece`): its z9–12 by the rule, its
      z12 tiles' quarters over its z10 tiles' deep mids, its hi pack, and its mid
      (`work/terrain-mid/6-x-y`: its z9 tiles' means). It waits for its own z10 tiles' terrain-deep
      targets alone (its declared reads, docs/readiness.md): a z6 tile with none waits for nothing.
      Its z8 subtrees may go to other workers (`terrainsub`, docs/coordinator.md §3), each alone (a tile
      reads nothing past its subtree), not where it takes in deep mids. Each level a block at a time
      (a z8 subtree's tiles at z12: `blocks`), each tile made whole and let go. Its blocks kept as
      made (crate::terrain_resume), a resumed run takes them.
    - **terrain-lo**, an assembly per z3 tile (`build_lo`): z8 → z3 from its z6 tiles' mids and the
      relief: its lo pack.
    - **terrain-root** (z0–2), once every terrain-lo is done: from the lo packs' z3 tiles and the
      relief, at most 21 of its tiles read; seconds.
    terrain-hi's, terrain-deep's and terrain-lo's uploads land beside the job's next target's work
    (docs/storage.md, Uploads beside the work).
    `scenic-build terrain-same 6/x/y` makes a live z6 tile (its deep mids taken in) and sets
    its pack's and mid's content names beside the manifest's, writing nothing to the NAS;
    `terrain-same 3/x/y` does so for a z3 tile's lo pack, `terrain-same 10/x/y` for a z10 tile's
    deep pack and mid (as its terrain-deep run makes them).
  - **Sources: the elevation pool** (`inputs/elevation/`, docs/inputs.md §6.8; crate::terrain_base),
    the roads' datasets, in the roads' order and blended at their edges as the roads' sampler does
    (crate::dem::generic), so the roads and the terrain use one dataset at every point. A sampled
    tile is sampled at 2 × 2 points a pixel (their mean), each dataset read at the tile's detail or
    its own, the coarser: a GeoTIFF at its overview nearest half a pixel (at most its full
    resolution), a pyramid declared published at every zoom (`every_zoom`) at the tile's zoom up to its own, another pyramid at its own
    and only from z9 on (below, the next dataset serves: a z8 tile would read 16,384 of a z15
    pyramid's tiles over the network, task #704), at sea as on land (its tiles there read from the
    NAS's store, a tile the server hasn't kept as none: crate::pyramids); a tile's detail at its own z6 row's middle latitude
    (`terrain_base::row_mid`), so its bytes are its own whichever tiles a job makes with it; heights
    brought to EGM2008 as the roads' are; where no dataset has data, sea level. The Terrarium tiles
    are read through this Mac's cache and the NAS's archives (fetched once, packed onto the NAS:
    docs/macs.md), the stored datasets from their store (`sources/fetched/`): a file the job reads
    half of or more copied whole to the Mac's SSD once and read there (task #689:
    buildstore::localread, where the disk has room past the owner's target; else NAS range reads; the
    same bytes either way). A run's samplers keep the files they most recently read open, and their
    fill masks, each within the run's own budget (`dem::generic::TileMemo`, `TILE_MEMO_BYTES`), the
    least recently read let go first, so a run's memory is its working set's (task #929: kept for
    the run's length, a run over a continent's GLO-30 files grew by 1.3–2.7 GB a z3 tile, measured by
    `examples/pool_memory`). The sampling goes a chunk a thread of the job's grant (the 64 tiles in
    flight shared among them, `terrain_base::chunk_tiles`), each chunk's datasets in their order
    and its blends after them; with the build pausing it stops before the next chunk (docs/pool.md
    §7.2, Resuming). **A level is a pipeline** (task #1097, `terrain_pack::Maker::pipeline`): its
    tiles are sampled a block at a time (the tiles under one tile four levels up, a z8 subtree's at
    z12), and each tile is made (its children's means taken in, rounded, encoded) as soon as its
    chunk is sampled, beside the sampling of the rest, on as many threads as the
    grant; the making takes the turns the sampling's threads lend while they wait on the NAS, the
    network or a read ahead (store::waits::Turns), so it computes on the cores those waits leave
    idle and the job on no more than its grant's (but for a thread back from a wait whose turn was
    taken, until that tile is made; a sampling thread with no chunk left helps the others' passes
    on a turn too). A block is kept to the run's resume log as its last
    tile is made. A level waits for the one finer (its quarters are those tiles' means). Each is
    pinned in the key (`TERRAIN_V`, the datasets meeting the target by their lines and zooms there,
    its tiles near the coverage), so a rebuild with the same key gives identical packs.
  - **The source's heights, kept** (the owner's, 5 Nov): at sea (its bathymetry, its artefacts
    too), over lakes, ponds, reservoirs, docks and rivers, and below 0 m (the Dead Sea, Death
    Valley, polders). The terrain reads no water, so neither its bytes nor its keys change with
    the basemap's. A tile pyramid's sea floor is data to the deepest trench
    (`dem::generic::TILE_BELOW`, `roadcore::grid::MIN_ELEV`); only where no dataset has data is a
    pixel sea level. The water's surface is the depth's and the map's (§3, The floor).
  - **Stored** (crate::terrain_codec; the owner's, 24 Oct): every terrain tile, at every zoom, is
    Terrarium RGB as lossless WebP (libwebp 1.6.0, `cwebp -lossless -z 6 -exact`, no metadata:
    crates/webpenc, the same bytes on every Mac, its golden hashes; libwebp's combined Shannon
    entropy, its hottest function, replaced by webpenc's own of the same integer sums, task #888),
    heights rounded once, as stored, to 1 cm where the tile is its place's finest zoom and 10 cm
    coarser (`Sources::quantum`), ties to even, then to Terrarium's 1/256 m: at most 0.39 cm and
    5.1 cm off. Chrome and Safari decode it natively, every value exact; a z12 tile is ~47 KB, 0.71×
    the PNG it replaced (.claude/notes/SHADING-PYRAMID.md, S1b).
- **Slope: the shading pyramid** (`terrain::slope_pack`; the owner's decisions of 24 Oct,
  .claude/notes/SHADING-PYRAMID.md): one precomputed pyramid serves both the slope tint and the
  hill-shading. Each pixel holds the mean steepness of the ground beneath it and the mean of its
  unit gradient directions (`roadcore::shade`), whose length, the coherence, is 1 on a plain face
  and about 0 where the faces beneath cancel (a ridge, broken ground). The terrain's rule (§2,
  Terrain): where its terrain quarter is sampled (its place's finest zoom) or the relief (outside
  every region), a pixel's are its own gradient's by Horn's method on that level's terrain, with
  the neighbouring tiles' edges; elsewhere the plain mean of the four beneath it (a steepness over
  400 % counts as 400 %), so the low zooms keep the finest level's steepness everywhere and its
  direction where it agrees. Every level is stored, as `shade-webp` (crate::shade_codec: RGB in
  lossless WebP, the terrain's codec): R 255 × √(steepness ÷ 400 %), G and B the direction's east
  and south parts, 128 + 120 u rounded to 5 bits, (128, 128) where the coherence is under 0.25.
  Inside a job a parent is the mean of its children's values at full precision, coded once; only
  where a job hands its tiles on (a z6 tile's z6 tile to its assembly, the z3 tiles to the root)
  is a parent made from its children as stored, so a z6 tile's run and its assembly give the same
  tiles. (Coded again at every level, the direction drifted about a step of its 5 bits by z8 and
  up to 9 % of pixels changed between none and some by z4: `slope_pack`'s
  `measure_quantisation_drift`, task #632's review.) Its steps split as the terrain's (task #639;
  "slope" was slope-hi's name before): **slope-deep**, a run per terrain-deep target's z10 tile: its
  part of its z6 tile's pyramid at the terrain's deep levels (`slope_pack::build_deep`), each of its
  z13 tiles' subtree from its deep terrain alone: its deep pack (z13–16, `layers/slope/deep/10-x-y`,
  its terrain deep pack copied here first) and its deep mid (`work/slope-deep-mid/10-x-y`: its z13
  tiles' quadrants at full precision), once its own and its edge neighbours' terrain-deep are built
  (task #952: nothing else; not the rest of its z3 tile: task #864); a z10 tile no target any more,
  or with no data past z12 (§2, Terrain: a drop offered once its key moves), has its deep pack and
  deep mid dropped. A missing deep tile (a Horn neighbour, an own tile) is its
  nearest ancestor within the deep levels (z13–15, `slope_pack::fallback_floor`), never a hi tile;
  where the fine data ends and there is none, the tile's own edge pixels are taken again, a strip
  of one pixel along the edge of the fine coverage (the owner's choice, task #952).
  A slope-deep job's z10 tiles, and a slope-hi job's z6 tiles, read the terrain through one reader
  (`slope_pack::Batch`, tasks #928 and #1097): the next target's terrain deep pack (slope-hi's: its
  terrain hi pack) is copied here while one is made and kept for it
  alone, every other copy and open pack let go once a target is made (an edge neighbour's few edge
  tiles are read where they are, on a thread beside the pyramid's compute, ahead of it: task #1096,
  `slope_pack::terrain_wanted`, `ManifestTiles::reading_ahead`; slope-hi's the same, with its z10
  tiles' deep mids), so the job's copies are never more than two packs (the one being
  made's and the next's); and each target's uploads land beside the next one's work
  (docs/storage.md, Uploads beside the work). At a pause, a failure or the job's end its copies are
  deleted.
  **slope-hi**, a run per z6 tile once what it reads is built (its own and its edge neighbours'
  terrain-hi, their z3 tiles' terrain-lo, its z10 tiles' terrain-deep and slope-deep): its hi pack
  (z9–12: over its z10 tiles' deep terrain, z10–12 from their z13 tiles' quadrants, taken in from
  their deep mids as made, and its hi terrain where they don't cover; the same bytes as one run of
  the whole pyramid) and its mid (`work/slope-mid/6-x-y`: its z6–8 tiles), reading the terrain packs
  (its own z6 tile's hi pack copied whole, ahead while the job's z6 tile before is made: `Batch`)
  and its neighbours' edge strips; a deep mid is read
  as the pyramid first reaches its z10 tile and let go once taken in, so a thread holds one at a
  time; **slope-lo**, an
  assembly per z3 tile from its z6 tiles' mids (each read whole, eight at once, under a deadline and
  tried again while it fails transiently); **slope-root**, z0–2 from the z3 tiles. (Each of these waits, slope-lo's on its z6 tiles' slope-hi
  and its own and edge neighbours' terrain-lo, terrain-lo's and trees-lo's on their z6 tiles' mids (`Read::Mids`: a z6 tile made again for its mid alone holds only them), is
  its row's declared reads in the steps table, `agent::steps::Read` (docs/readiness.md): the planner,
  the forecast's waits and the page's notes derive each target's from it (`steps::readiness`); `declared_reads_cover_the_keys` checks
  each against its key's inputs. The terrain root is built in the terrain stage once every terrain-lo
  assembly is done; slope-lo waits for it, and a slope-hi target only when its pyramid reads a
  root tile (an edge neighbour past the lo packs), docs/readiness.md §6 3c.) Memory as measured (docs/pool.md §7.7). The
  pyramid (`slope_pack::Pyramid`, the depth's shading's too) makes the children of the tiles within
  three levels of its top in parallel and each subtree below them on one thread, on at most 16 of
  the job's threads; its tiles don't depend on the threads (each
  pixel the mean of its four children's in a fixed order, nothing summed across tiles).
- **The polar caps** (`terrain::polar`; docs/polar.md): the terrain and shading past the Web
  Mercator tiles' 85.05°, on each pole's polar stereographic grid (EPSG:3031 south, 3413 north;
  `roadcore::polar`), z0 to the cap's finest zoom (`polar::finest`: the terrain's rule on the grid's
  pixel, z8 for the declared datasets now), of the poles built (`polar::POLES`: the south; the north is sea level
  and waits for the depth). Each from the elevation pool as the Mercator terrain is, read at the
  level's detail, the tile pyramids left out (a Web Mercator pyramid has nothing past 85.05°); where
  no dataset has data, a fill mask's own filled value (`dem::generic::Sampler::fill_voids`); a gap
  up to 3 pixels wide (GLO-30's file edges) its neighbours' (`polar::close_gaps`); else sea level; below 0 m kept, as the Mercator terrain's. **polar-terrain**, a run per block (a z4 tile of
  the grid, "s/4/x/y", where the coverage meets its latitudes: `polar::built`): its cap tiles at the finest
  zoom sampled, each coarser level its children's means (a quarter with none, past the cap, sea
  level: no dataset is published at every zoom on this grid, so there is no relief to read), coded
  as the Mercator terrain is: its pack `layers/polar-terrain-<pole>/polar/4-x-y`, the stored files it reads half of or more copied to the Mac's SSD as the terrain's are (buildstore::localread, the area the block's latitudes). **polar-slope**, a
  run per block once its and its neighbours' terrain is built: Horn's method at the finest zoom on the stored
  terrain (its neighbours' edge tiles for the border), each coarser level its children's means at
  full precision (a quarter with none flat), as `shade-webp`: its pack
  `layers/polar-slope-<pole>/polar/4-x-y`. **polar-root**, a pole's z0–3 of both from its blocks'
  z4 tiles as stored, by the same means (nothing sampled); it drops the packs of blocks no longer built.
  Progress (polar-terrain, polar-slope): `progress: <k + tiles done / tiles in the block>/<blocks> polar
  blocks` on stderr (`jobs::report_f`), a line per 16 tiles within a block (a tile counts when sampled or shaded at
  the finest zoom, and again when written out at each level), so a busy block moves the build page's "hasn't moved" check.
  Memory: a block's finest tiles held whole (heights and shading) and the pool's cache.
- **Grids (z11):** land cover, canopy and cover, for analysis only. Each `unit` job makes the grid tiles
  its packs lack and uploads them as its z6 tile's `grid-*` hi packs; they aren't in those jobs' keys
  (made from fixed datasets, the same whether read or made).
- **Trees** (cover, height, leaf type), zoom 4–12, clipped to the coverage (`trees::treepacks`,
  running the `trees` program, `trees::trees`):
  - **trees**, a run per z6 tile the coverage meets (`trees --z6`): its zoom 9–12 hi packs and its
    mid (`work/trees-mid/6-x-y`: zoom-8 tiles and values).
  - **trees-lo**, an assembly per z3 tile (`trees --assemble-lo`): zoom 8–4 from its z6 tiles' mids.
  - Together the same bytes as a z3 tile's whole run (`trees --z3`, `scenic-build trees 3/x/y`), from
    Meta's canopy squares (`sources/canopy/`, each downloaded once under a `<file>.lock`) and the
    leaf-type squares (`sources/trees/leaf/`, made by `trees::leaftype`: the EEA's chunks, or
    NALCMS resampled as GDAL's warper did).
  - Reading (task #606): a run reads this Mac's whole copies of the canopy squares (`chm10/`, the
    z6 tiles' canopy step's) where it has them, held for the job; else only the rows its blocks need
    of the NAS's, read first in 8 MB reads, four at once, into this Mac's cache for the runs after
    (`trees::squares::RowsCache`, `blobs/sources/canopy/<file>.<length>.<first>-<end>.rows`, a span of
    the file by bytes: the tiles of a row read the same strips; task #998) where the disk has room past
    the owner's target, else into its scratch (`trees::squares::rows_here`).
    A z6 tile makes its blocks a row at a time (each strip decoded once for the row), each block's
    band under a permit: a trees job is paced like an input check (docs/inputs.md §4.3, Threads),
    its threads as its lead re-grants them (docs/pool.md §7.1, Threads re-granted). The trees program's NAS reads count in the
    target's NAS weight (`bytes`; `coord::cost_version` 3).
  - Lossless WebP by `codecs::webp`.
  - `trees --block`, `--blocks` and `--assemble` run blocks alone; a row of a z6 tile's blocks is a
    task (`treeblock`, docs/coordinator.md §3), the same bytes at any thread count.
- **Area overlays** (the heritage and designated areas' outlines): `docs/phase5.md`. The `overlays` job runs after marks (it needs the World
  Heritage dots' ids).
- **3D buildings:** `docs/buildings3d.md`. Every building in the coverage from the Overture release,
  tiles z12–14 per z6 tile (`bldprep`, `bldtiles`; `bld::bld`), a z6 tile's
  z8 tiles offered as tasks (`bldtile`); below zoom 14 the far buildings' field (buildings3d.md
  §4.8). Planned: national heights (PLATEAU, BD TOPO).

The server builds missing deeper terrain and shading tiles from their ancestors.

## 3. Depth (per z3 and z6 tile, from the depth sources)

Depth under the seas and lakes (`terrain::depth`; its sources docs/inputs.md §6.10; its files
docs/formats.md, Depth), split by tile like the terrain, so downloads and the mirror take it as it is.
- **The rule** (task #804), the terrain's (§2, Terrain: the rule), for the depth and
  the floor's shading alike, with no zoom special: each place's depth is composed from the sources
  only at the zoom of the finest grid covering it (GEBCO's z8, a chart's z13–16, any new source's
  own), every coarser level is the mean of the finer where the finer has data and composed at its
  own zoom where it hasn't (the same rule there), and no level finer than the data is stored (a
  finer view scales up the nearest stored tile). A new fine source dropped in anywhere, the open
  sea too, gets tiles up to its grids' zoom with no change to the code.
- **Targets:** every z6 tile within 200 nautical miles (`REACH_KM`, a region's EEZ's reach) of the
  coverage that a source with grids finer than the lo packs' (z3–8) meets (`depth`, any Mac: its
  hi packs, z9–16), and every z3 tile within it that a source meets (`depth-lo`, any Mac, once its
  z6 tiles are done: its lo packs, z3–8). Each writes the depth, its classes and the floor's
  shading. Where the two meet (z8 | z9) is where their packs do, as the terrain's, not part of the
  rule. They read only the NAS (the sources' grids, the basemap, the depth's hi packs) and write
  their packs there. The 15″ grid covers the rest of a z3 tile's water.
- **The lakes' estimates (`lake-depth`, a z6 tile, any Mac; crate::depth::lakes, task #959):** the
  sources declared `lake-estimate` (docs/inputs.md §6.10: GLOBathy's, the small lakes') are made by
  the pool, not by a producer: every z6 tile near the coverage (the terrain's, `coverage_tiles`) is
  a target while one is declared, so a region added gets its lakes' estimates as the pool builds
  it, and a tile the coverage left has its packs dropped (its "none" key). Each source's grids of
  the tile are one pack (`work/lake-depth/<id>/6-x-y`, docs/formats.md), none where it has no lake.
  - **GLOBathy's** (`method = "globathy"`): every HydroLAKES v1.0 lake meeting the tile with
    GLOBathy's maximum depth (`Dmax_use_m`, by `Hylak_id`), both streamed from the zips in the
    source's folder, a bowl D = Dmax · (l / L)^0.75 on its outline (l the distance to shore, islands
    shore, smoothed by L/8 with the land 0; the depth lab's fit), z11, a grid a z8 tile cropped to
    its data. A lake wider than 1,024 pixels (the Caspian, Ladoga) is shaped at the coarsest zoom
    holding it within them and scaled up bilinearly inside its outline at z11.
  - **The small lakes'** (`method = "land-slope"`): a z10 tile at a time, its basemap z14 water
    and a ring of one tile: lakes and ponds merged by OSM id (those without one where they touch),
    under 10 ha, not intermittent, not within 3 m of the sea, not a fifth or more under the
    `outlines` source's HydroLAKES polygons; a lake reaching past the tiles read is skipped. Each
    one's maximum depth from the mean land slope 30–100 m from its shore (the terrain hi packs'
    z12, other lakes and land at or below 0.3 m left out) and its largest distance to shore
    (GEOS's maximum inscribed circle), by the depth lab's fit (validation.md §3), the area rule
    where the land is flat or missing; the same bowl at z14, a grid a z10 tile.
  - Its key: the declarations it makes (their versions pin the zips), the basemap whole (a lake
    goes on past the tile; a new basemap makes them all again, and a depth target only where a
    pack's bytes changed) and the terrain hi packs of the tile and the 8 around it, which it waits
    for. A depth target waits for the packs it reads (its tile and ring; an assembly's z6 tiles and
    the ring its blocks' margin reaches), its key naming them by content. Until a tile's
    lake-depth target is first made, GC keeps the depth hi packs of it and its ring
    (`build::depth_kept`): its packs may yet give them depth targets, as the indexes of the
    producers it replaced did.
- **Reading the sources:** a source's grids are packed by z6 tile (docs/inputs.md §6.10, Packs:
  a tar of whole GeoTIFFs, each read in place by its byte range), so a job opens a pack once and
  reads its grids from it, not a file a grid; a pack is kept open by the job's reader like a grid
  (the least recently used let go past the open-files cap, task #438), and its open has a loose
  file's deadline and retries (below). Loose grids (an index feature without `pack`) are read as
  before. Each read over SMB waits round trips, so a source's grids meeting a block
  (hundreds of BlueTopo's) are opened 8 at once, then sampled 8 at once (merged in their order),
  and each window's blocks read 8 at once into the shared block cache (`Tiff::prefetch_window`).
  A grid's open reads its header in windows of 32 KB (`geotiff::Windowed`: its directories, their
  values and its tile arrays lie in one or two clusters, the first image's at the file's start
  and the overviews' after its data) and keeps the tile arrays, so an open is one or two reads
  (it was 24–38, one a value) and a block's read is one, its data's (it was three: the tile's
  offset, its length, then its bytes); task #492.
  A grid finer than the window is read from the coarsest overview whose cells are whole pixels of
  the window's zoom (each pixel's mean exact), else, when its origin lines up with no overview
  (most of BlueTopo's), from the coarsest at least four cells to a pixel's side, each cell counted
  to the pixel its centre is in (`grids::level_for`, `ASKEW_FINER`: the cells counted to a pixel
  are the pixel's moved by at most half a cell, an eighth of a pixel; the sources' overviews
  average their cells, producers/depth/grid.py). A grid's low-water file is read at the overview
  its grid is.
  3/1/3's z6 block: 1,359 s read in turn, 456 s so, 1.6 GB (19 Oct, the M1, release). Each window's
  prefetch is held to its share of the block cache (an eighth: the grids read at once). A file's
  open and header read run on a thread of their own, waited 120 s at most: past it the job fails
  (its own retry takes over). A transient failure (NotFound, as SMB says for a moment on a file
  that's there: two depth-lo jobs, 19 Oct; EIO, ESTALE, an interrupted or reset call) is tried
  again after 2, 5, 15 and 30 s; a file that isn't a GeoTIFF is not.
- **Timings and progress:** a job's phases: the sources' declarations read, the basemap's water
  opened and read, `composing` (its sub-phases: the source files opened, the sources' windows read,
  the stored levels scaled up, feathering, gaps filled), tiles encoded, the coarser levels from
  the finer (depth-lo: z8 from the z9 tiles first, and the z9 shading read), the floor's shading
  and the packs written. Its progress moves within a zoom, by its blocks
  done and the grids of the block under way sampled, said every 5 s; then the floor's shading
  says its pyramid's tiles (`tiles shaded`, every 5 s: a z6 tile's takes up to half an hour after
  its zooms).
- **The water's reads:** a level's blocks share the water tiles their margins overlap
  (`depth::build::WaterCache`), and a z14 tile's polygons, the basemap's deepest, are read and
  decoded once for its 4 z15 and 16 z16 descendants, across the two levels, shared among them and
  kept within `POLYS_BYTES` (48 MB; past it the rows farthest up go first). A block's tiles it
  lacks (a hundred, with its margin) are read from the basemap together
  (`WaterSource::depth_polys_many`, `PmTiles::get_many`: their places sorted and those within
  64 KB of each other read in one range of at most 4 MB, 8 ranges at once; a clustered archive keeps neighbours
  together, so a block is a few reads, not a hundred round trips), then decoded and rasterised on
  the job's threads (task #492). The same polygons and water as one by one.
- **Composed at the finest data's zoom** (`depth::compose::composes_at`): a block of zoom z is
  composed only where a grid of zoom z meets it (a grid's zoom taken within z3–16), and a tile kept
  only where a water pixel is a source's own data of the tile's zoom (that source painted there:
  not one feathered in under half, nor a gap filled from one), so no tile is finer than the data
  under it. Its capped pixels (a coarser source's) are that source's level scaled up as the map's
  fallback makes it, from the job's own composed tiles.
- **The coarser levels from the finer** (`depth::build::means`): each tile with a child one level
  finer, each pixel the mean of the four beneath it that have data (its class the first of them
  with one), elsewhere its own level composed. So a zoomed-out view shows the mean of the depth a
  finer view shows, wherever the finer is.
- **depth (a z6 tile):** its blocks composed z9 to z16 as above, coarse to fine, then z15 down to
  z9 each from the finer. Its capped pixels' coarser levels are its own: before z9 it composes
  z6–8 (as above) over the tile and the ring of tiles around it (across the date line too, where
  the map's fallback takes an ancestor's east–west neighbours), which serve only that and aren't
  stored, so a block's 32-pixel margin across the tile's edge has them too (its neighbours' fine
  tiles are another job's and aren't read). Where a chart's grids are z15, z9–14 are composed
  only where another grid is of that zoom: means elsewhere.
- **depth-lo (a z3 tile, after its z6 tiles' depth):** its blocks composed z3 to z8 as above (on
  today's sources z8 alone, GEBCO's, the coarsest grids: z3–7 have none of their own), then z8
  from the z9 tiles its z6 tiles' hi packs store, z7 down to z3 each from the finer; then its floor's
  shading (below).
- **depth-root (the world, after every z3 tile's depth-lo, lead only):** z2 down to z0 each the
  mean of the finer from the lo packs' z3 tiles (`depth::build::build_root`; no grid is coarser than
  the lo packs', so nothing is composed there), and their floor's shading, z2 the mean of the z3
  shading where it covers; the root packs (`root/0-0-0`) of the three layers, as the terrain's root.
  It waits while a depth z6 tile or assembly is stale or waits to be merged.
- **The floor's shading** (the `depth-shade` layer: `depth::build::shading`): the terrain's
  shading pyramid (§2, Slope; the same codes and codec) over the depth: at
  each place's finest stored level each pixel's steepness and direction by Horn's method on the
  depth, a pixel without data taking its nearest's in the tile (no cliff at the shore, which the map
  draws only in the water) and a neighbour tile the job didn't make read as the tile's own edge;
  each coarser level the mean of the four beneath it where its children cover it, else its own
  depth's. A z6 tile's run (`depth`) makes its z9–16 with its depth; a z3 tile's assembly
  (`depth-lo`, `shading_lo`) its z3–8 over its depth tiles, each z8 pixel under a z6 tile's z9
  shading the mean of those four as the hi pack stores them, so the fine sources' detail reaches z3.
  Each place's shading is worked out at its finest stored level alone and averaged above it, the
  depth's rule.
- **The water** (`depth::build`, `terrain_water`'s `depth_polys`): the basemap's z6–14 (its z14
  scaled at z15–16), its sea and lakes and also its rivers (class `river`). A pixel is
  water when they cover half of it or more, or when it's narrow water (some of it water and none of
  its eight neighbours half so: a channel under a pixel wide). A river's pixel takes only the
  sources declared `rivers` (their depths below the river's own surface: NONNA's mean water level
  from CHS's stations, which slopes down the St. Lawrence with them; BlueTopo's and SHOM's mean sea
  level, in the tidal reaches they cover); a lake's pixel the lake sources and those declared
  `lakes` (NONNA's: its stations' mean water level is the lake's own, as in Lac Saint-Louis; never
  BlueTopo's or SHOM's mean sea level, which isn't a lake's surface), a sea's the sea sources
  (`Source::drawn_in`); a river is gap-filled from a river's data alone, keeps no
  drying heights and never dries. A river's pixel (rivers cover more of it than the sea or lakes,
  which cover under half), a pixel water only with the rivers' share, and narrow water are land
  where no source has a value, so a river no source is drawn in changes nothing. Narrow water a
  source has values in is new depth: the sea's slivers under a pixel wide change many coastal
  z6 tiles (intended).
- **Enclosed seas** (`depth::surface`): on a sea closed off from the world ocean (the water step's
  `global/water-enclosed`, §4: the Caspian) the sources' values are heights on their own datum
  (GEBCO's above mean sea level), so a sea pixel's each is taken less that sea's surface: the
  terrain's height there as the server takes the water's surface (The floor, below), the terrain
  tile of the block's zoom from the terrain's lo packs, else its nearest stored ancestor there
  scaled up (finer zooms from z8's). Nothing is typed in: the Caspian's −28 m is GLO-30's (§2), and
  a shore pixel's the terrain's blend of water and shore, as the surface's. GEBCO above −28 m (much
  of the north Caspian's shelf) is 0 m. Where the lo packs hold no terrain over it, the sources'
  values as they are. A depth target's key names the enclosed seas' file and the terrain's tiles on
  one under its tiles, ring and margin (z3–8 and their ancestors, by their XXH3 in the lo packs'
  indexes: `surface::tiles_read`) only where those meet an enclosed sea, so only those targets
  re-key, and only for a change there; only they wait for the water step and the lo packs holding
  those tiles (`Read::Built`).
- **Composing a block** (8 × 8 tiles and a 32-pixel margin; `depth::compose`), on that water:
  sources sampled (coarser grids
  interpolated, finer ones averaged from the overview that lines up) and painted coarse to fine,
  the surveys first and then the estimates, which fill only what no survey has (a survey beats an
  estimate wherever both cover water);
  past its own zoom a pixel shows the stored level of its zoom scaled up as the map's fallback makes
  it (`depth::fall`, the server's too), detail fading in over three capped pixels; a source ending in
  the water feathered over its `band_m` into what lies beneath, its own whole where nothing does (a
  source off the others' datum fills gaps only); the shore:
  positive values kept as drying heights only from intertidal sources, water without data filled
  linearly to 0 at the shore within 600 m from data reached through the same water body (never
  across a dike or a spit; a lake without data of its own stays empty), the land a 3-pixel band of
  the nearest water's value (class none: a finer shoreline scaled up from it never meets a gap; the
  map and the floor leave it out); dries where above the chart's low water.
- **The floor:** the depth is below the water's surface, so that surface less the depth is the
  floor; the server serves it (`/tiles/terrain/…?floor=1`), and the surface (`?surface=1`), no
  second terrain stored: a surface tile with no pixel at sea is the terrain tile itself, served as
  stored (its ETag, cached for good under the current water version); one with sea is made (the
  terrain decoded, levelled, encoded again) and kept in the server's made tiles, with an ETag of
  its own (the terrain tile's and the water's version: cached for good as a stored tile is); one
  made from the terrain's ancestor or from the water's stand-in (the basemap unreadable) as made
  tiles are (the stand-in's not kept: made again when asked for again). The sea's surface is sea level, 0 m, where the water tiles say sea (half a
  pixel or more, as the depth's water: server `water::sea_shares`) and the sea is the world
  ocean's (§4, The world ocean), whatever the
  terrain reads there (the source's bathymetry: §2, Terrain). An enclosed sea's surface is the
  terrain's source there, as a lake's: the Caspian's GLO-30 heights, −28 m. The depth there is
  below that surface (Enclosed seas, above), so its floor is the surface less the depth, never above
  it. A lake's is the terrain's source there
  (not levelled), so a lake's floor follows its source's surface, artefacts too; so does a river's
  (3DEP's hydro-flattening, where its source has it). A pixel of narrow water is the terrain's
  blend of water and shore, so its floor is rough. The map's toggle (Layers → Depth, "3D sea and
  lake floor"; web/src/depth.ts): the 3D terrain from the floor, the water's surface its surface
  (our MapLibre's `Map#setTerrainSurface`). Without the floor the 3D terrain is that surface (the
  sea flat at sea level, the ferries on it; lakes and land below sea level at their source's
  heights), display only: the terrain's data keeps its source's heights. The elevation tint and
  its histogram, the contours, and whatever reads the ground's height on the map (the camera's
  clamp, buildings, labels) read the same surface; the hill-shading is the slope layer's, made from
  the terrain, so it shades the bathymetry at sea (Shaded by the terrain beneath, below); a road's
  profile is its own heights (§2);
  the water draped on the floor under the depth's colour, a see-through copy of it drawn at the
  surface; the ferries' lines lifted to the surface and the water's and ferries' names standing on
  it (the elevation reference and anchor `surface`); roads and bridges keep their own heights.
  Past the depth's stored zooms the floor takes the nearest stored depth tile scaled up, as the
  depth's own tiles do.
- **The map:** two layers, each switched on its own (Layers → Depth): the depth's colour with the
  floor's shading (nautical blues on a log scale to 300 m by default; the panel's ramp picker; linear,
  log or fitted to the view; the shading from the stored `depth-shade` tiles, as the terrain's
  hill-shading), and contours with labels (on demand). The colour and shading are drawn
  as the water itself: our MapLibre draws them over the water tiles the map draws, as those tiles'
  alpha (`scenic:mask`, web/maplibre FORK.md), so they never leave the water nor leave it uncovered
  while tiles load or overzoom; the depth is interpolated bilinearly among pixels with data and
  reaches 8 pixels past the water's own data (the nearest's value; never the land's band, and
  never across land into water again), so water a coarse tile's rule left out takes its neighbour's
  depth, and water with no depth near, or across a dike from it, shows the water's own colour. The
  drying colour is only for water marked drying (depth-class bit 3); any other water at or above
  its surface takes the ramp's shallowest blue (web/src/depthgrid.ts reliefValues). Without
  the water tiles, the colour masked by the basemap's water at the display's resolution, and no
  floor shading (its tiles reach onto the shore). Over the water fill
  and under the coast shading, the whitewater and the terrain's hill-shading (§4), draped on the 3D
  terrain. Hovering water (no road, line or place under the cursor) shows in the footer its depth with the ramp's colour, the water's name and kind where the map labels it, the depth's class, source, resolution and datum; the map names the sources in
  view, "not for navigation", and the notices their licences ask for (CHS's, naming the owner as the
  maker), which the credits carry too. The Depth section's note names the sources whose depths are
  estimates (the map's `estimated`: GLOBathy's, the small lakes'). The map's depth tiles, each
  made from the zoom below, stop one zoom before the layers' z16 and are scaled up past it
  (web/src/depthgrid.ts depthZooms).
- **The rollout:**
  1. Both Macs run the app with the depth (until a depth declaration is accepted the chain has no
     targets).
  2. The producers make the grids on the M4 (`producers/depth/`, `uv run --project producers`) into a
     local folder, packed by z6 tile, each source's packs then copied to `sources/fetched/<id>/`
     with `tar` over SSH (the NAS's small-file limits; docs/storage.md). A `lake-estimate`
     source has no producer: its files (GLOBathy's zips) are kept in its folder, and the pool's
     lake-depth step makes its grids.
  3. The declarations (`producers/depth/declarations/`) and the indexes are added to
     `inputs/sources/`. Once accepted, the lake-depth targets run (each once its terrain hi packs
     are made), the terrain-water job works out the depth targets' water digests (once a
     basemap), then the z6 tiles, then the assemblies.

## 4. Water (per pass, worldwide)

The basemap's water drops small polygons at each zoom and its vector fills' anti-aliasing pushes
shores out, so zoomed out small islands and lakes went missing. The water layer
(`inputs::water`) gives each pixel its exact share of water at every zoom, sea and inland apart,
drawn as a raster.
- **The water** is the basemap's own at z14 (simplified by 1/4096 of a tile, nothing over 1/256 px²
  left out). The sea is OpenMapTiles' class `ocean`, the rest inland; water in tunnels is left out.
- **Coverage is exact** (`codecs::watercov::Raster`: each edge adds its area to the pixels it
  crosses).
- **Tiles:** 512 px, two channels (sea, inland). The bytes also say whether a pixel holds anything
  but sea, and any land at all (`water::bytes`), so a tiny island stays land zoomed out, for the
  coastal shading.
  - **Stored for z0–9:** each z10 tile from its 256 z14 tiles, coarser tiles the exact mean of their
    children; only tiles not one value throughout, as grey-and-alpha PNGs (`layers/water/`;
    docs/formats.md).
  - **Deeper zooms are drawn by the server** (`crates/server/src/water.rs`) from the z14 tiles under
    the tile or the one over it.
- **The world ocean and the enclosed seas:** the sea is OSM's coastline's, and OSM maps the
  Caspian with coastline too, so the Caspian is sea in the tiles, drawn and shaded as sea and the
  depth's sea (§3). With the stored tiles the step finds the sea closed off from the world ocean
  (`water::Enclosed`, `global/water-enclosed`; docs/formats.md): at z6's pixels (512 a tile, 1.2 km
  at the equator), a pixel holding any sea joined to the eight round it (and round the
  antimeridian), the largest body is the world ocean and every other body is enclosed. Nothing is
  named by hand. A strait narrower than a pixel still joins (it holds some sea); two bodies within a
  pixel of each other are one. The water's surface is sea level only on the world ocean (§3, The
  floor). The step's peak memory adds two bits a z6 pixel (256 MB), after the z14 directory is let
  go.
- **Served** at `/tiles/water/{z}/{x}/{y}`: `?c=<sea>,<lake>[,<land>]` gives a coloured PNG (with
  the land's colour, the alpha that mixes water and land as light mixes: `alpha_for`); `?raw=1` the
  shares; a tile not stored takes its stored ancestor's value; a stored zoom it can't read is drawn
  from the basemap at z9. Drawn tiles are kept (192); at most 4 drawn at once, on threads of their
  own; the open sea's z14 tile is drawn once.
- **Drawn** (`web/src/basemap.ts`) as a raster source of 256-CSS-px tiles to z18, linear, no
  cross-fade, under the Water switch. MapLibre is told the tiles are 256/√2 px (`WATER_TILE_SIZE`)
  so it never stretches a coarser level; shrunk, tiles are sampled trilinearly (the `scenic:mipmap`
  metadata, read by our MapLibre's `draw_raster.ts`). Colours are in the URL (Settings → Map →
  Water), so a new colour needs no rebuild.
  - **Shaded by the terrain beneath:** the hill-shading is drawn over the water as over the land,
    and the depth's colour and the coastal shading go under it. The terrain keeps its source's
    heights at sea and over lakes (§2, Terrain), so the shading shows the source's relief there: at
    sea its bathymetry (AWS's, its artefacts too), a lake as its source draws it; where the source is
    flat, under the shading methods that leave flat ground clear (standard, combined, Igor: the
    default) the water shows its colour; basic and multidirectional light flat ground, and the water
    with it. Water on a slope (falls, cascades, a river's steep reach) shows its relief.
    Where the terrain under a river steps (its source's survey lines, or two datasets meeting, as
    across the upper Niagara River), the step shows as a line across it.
  - **Whitewater** (`whitewater`, only with the water tiles; Settings → Map → Water, in the link's
    `wa`): the water pales toward its colour as the slope layer's steepness rises from its start
    to its full (8 % and 50 %, at 80 %), where the water covers its least share of a pixel or more
    (75 %, fading in to whole: the tile's drawn alpha, so a shore pixel's bank steepness isn't
    counted). A colour relief of the `slope` source with `scenic:mask` of the water and
    `scenic:mask-cover` its least share (our MapLibre's `color_relief.fragment.glsl`); no waterfall
    tags. Zoomed out, a pixel's steepness is the mean of the ground beneath it, so a fall narrower
    than a pixel shows fainter, then not at all.
  - The coastal shading (`web/src/coast.ts`) measures its shore from the same shares at 2 texels a
    CSS px: every pixel holding any land is a shore, placed by its share (`web/src/coastdist.ts`).
    The distance is exact within 192 pixels; past 60° the far field comes from coarser ancestors.
    Stored in metres in MapLibre's 'custom' raster-DEM encoding.
  - Without the layer in the map, the basemap's water polygons are drawn.
  - The water has no hover or click; its names are the labels'.
- **Checked** against full detail (8 Oct, Planetiler 0.10.2) over 137 views, the screen in the
  app's colours against full detail rendered and shrunk in linear light: visible difference about
  0.03 % (0.15 % on the densest small-water views), against 7.7 % for the earlier dots over
  polygons. No tool repeats the check for a new Planetiler (not built).
- **Lakes filled from Canada's own data** (`inputs::lakefill`; the `lakefill` step once a pass,
  before the water, and the `lakefill` tool). OSM's CanVec import skipped some map sheets, whose
  small lakes vanish in rectangles.
  - **The survey** (`lakefill::run::survey`, `gaps`): every NTS 1:50,000 sheet in the Canadian
    regions (also one with no basemap tile over it at z12: an empty tile isn't stored, so a skipped
    sheet of bare land has none), its small lakes (under 1 km²) per 100 km² of land. A sheet is a gap when that's under a
    tenth of its neighbours' (75th percentile within 3 sheets), under 2, or its size curve is cut
    off.
  - **The source:** CanVec 1:50,000 Hydro `waterbody_2`, the provinces' zips, downloaded by hand
    and kept in the declared source's folder,
    `sources/fetched/canvec-lakes/` (`lakefill::run::source_dir`, docs/inputs.md §6.9), read from
    there (`lakefill::zip`). Taken: lakes, reservoirs, ponds and unidentified water bodies; intermittent
    ones drawn as water but not flattened. Licence: OGL–Canada, credited.
  - **The rules** (`lakefill::select`): whole lakes only (pieces sharing an edge or a sheet line are
    one); none touching OSM's water within 10 m; OSM's lakes cut at a gap sheet's edge completed with
    the source's part inside the gap; duplicates once; lakes over 1° across left out; a sheet filled
    only where under half the source's lakes in it touch OSM's water (the source's `params.confirm`, `FillOpts::declared`).
  - `lakefill seamcheck` checks the edges where filled lakes meet OSM's water.
  - **Where it plugs in:** the step makes `work/lakefill/<date>.<h>.sect` (docs/formats.md), keyed
    by the basemap, the outlines and the source's zips (`run::source_pin`); only the fill from the
    latest basemap is drawn (`lakefill::current`). The water, the water digests (`terrain-water`) and
    the map updates wait for it (`hold_for_lakefill`) until it has failed 3 times in a row
    (`water_waits_for_lakefill`), then go without it. The water layer draws its lakes as an extra
    z14 tile (`water::Extra`, the map's `global.lakefill`), read only for tiles in its reach
    (`lakefill::REACH`, Canada's box, which the step refuses a lake outside), so away from the NAS a
    region elsewhere draws its deeper zooms without it; the depth's water reads them as lakes
    (`lakefill::FilledWater`; `terrain_pack::water_source_pin` names the fill); the terrain reads
    none of it (§2, Terrain).

## 5. Worldwide road values (in the OSM pass)

**One chaining** serves whole roads, strokes, drives, hover, profiles and rides.
- **How ways pair:** at each node, way ends of the same kind pair by mutual best continuation: same
  ref (any shared token), else same name, else same class when both are unnamed, else a nameless way
  continuing a named one of its class within 35°. The straightest go first, within 100°. Oneways
  pair only in their direction. Ties: higher level, straightest, lower way id. Rail pairs by line
  identity; ferries and ways with under two vertices never chain.
- **Per extract:** the pairing at a node depends only on the ways through it, so it's exact per z6 tile extract.
- **Roads:** pairings form paths and cycles; a union-find and an ordered walk give each way its road
  id (the road's lowest way id), the road's length, and its offset and direction.
- **Storage:** per z6 tile, `sources/osm/<date>/roads/<u>`, copied by each `unit` job into
  `global/roads/<u>` with `byroad` (ways sorted by road).
- Lengths include parts outside the coverage.
- Elevation smoothing uses a local continuation rule (`Net.cont`) in base(U).

## 6. Roads, per z6 tile: base(U)

The `unit` job ("Roads"; its step name is kept as stored) runs the programs for one z6 tile U on
a scratch folder, wiped at each run:
1. **extract:** U's ways that touch the coverage, on U's extract. Rail tracks without a route relation
   are kept by type.
2. **Elevations:** `elev` (crate::road_terrain), each vertex's height from the finished terrain
   (docs/inputs.md §6.8, The roads' heights), bilinear between pixel centres at the finest zoom stored
   there, its place's finest (crate::terrain_zoom: a deep tile, z13–16, where it is past z12),
   else coarser. It reads `verts.bin` and what step 4 stages alone: `terrain.tiles`
   (the terrain's z0–12 over U's staged area, step 4, and its deep tiles under U's vertices, one archive) and
   `codes.json` (the datasets' codes by coverage, giving `src.u8`); no NAS, no
   server. It writes `elev.f32`, `src.u8` and `dem-stats.json` (vertices, missing, seconds). The
   ways were densified by `extract` at the terrain's pixel there, never closer than 4 m
   (crate::dem::spacing, `FLOOR_M`). U waits for the stale terrain targets under its box (U +
   `stage::MARGIN_KM`) and along its long ways (`steps::Read::Along`, from the pass's reaches), and
   its key names the terrain tiles it reads (hi and deep), along its long ways too.
3. **Heritage:** the heritage-sites job's slices within U + `stage::MARGIN_KM`; `areaflags` rasterises the heritage and designated areas
   onto U's grid.
4. **Terrain and grids:** terrain z0–12 and the z11 grids, staged from the packs the manifest names,
   with the deep tiles step 2 reads, over U's staged area (crate::stage::Area): U grown by
   `MARGIN_KM` (35 km: `reach::LONG_KM`, 20 km, where the ordinary ways U owns lie, plus a sample's
   far field, `scenic::FAR_MAX_M`, 15 km), and along each long way U owns (ferries aside) its nodes
   in runs of about 15 km, each grown by 15 km. Every sample U makes sees staged terrain and canopy
   to its far field's end. Missing grid tiles are made.
5. **`tile elev`:** clean-up and grade, with junction context from the extract. Causeways and
   embankments (the ways `extract` lists in `causeways.i64`: `embankment=yes`, `causeway=yes`,
   `man_made=causeway`) and ways whose every vertex lies on the staged land cover's water class are
   structures (crate::elev::raised): straight between their ends, as bridges are.
6. **scenic:** `scenic-metrics` prep, canopy, view, buildings and flags for every sample of the ways
   U owns, each sample's eye its ground plus a driver's 1.5 m (`scenic::EYE_M`), on a railway a
   passenger's 2.8 m (`RAIL_EYE_M`, the sample's `sflag::RAIL`). Buildings come from the roadside buildings' z8 tiles within 1 km of U's tile + 20 km and
   its long roads. Canopy and view results are kept in `cache/scenic-units/` (`scache::Carry`), so a
   later run redoes only new samples and those near grid tiles whose terrain or land cover changed;
   a change of `scache::SCENIC_V` redoes all.
7. **Output:** the base pack (per-vertex arrays and records, indexed by z9 sub-tile),
   `global/roads/<u>`, `global/roadids/<u>` (its roads' ids, what the road → z6 tiles index reads),
   `global/roaden/<u>` (the roads' own English), and `grid-*` hi packs. A z6 tile
   left with no ways in the coverage drops them.

Elevations are u16 decimetres from −500 m (to 6,053.5 m); readers also take older packs' i16.

Landmark candidates and peaks have their own per-z6-tile jobs (`docs/phase5.md`).

## 7. Roadside buildings: the world, once per release

The buildings job (`bld::buildtiles`) makes Overture's building boxes for the world from one
pinned release, before any `unit` job runs: `buildtiles::scan` reads the release's bbox columns from
Overture's bucket (each file's footer, then its row groups' bbox chunks alone), 8 files at a time, into local parts by z8 tile (each file marked done once written, so a run cut
short resumes); each tile's parts are merged onto the NAS, then the index. A z6 tile's key names the
index, so a new release rebuilds every z6 tile.

## 8. Per z6 pack: pack(T)

- **Reads:** the base packs and road values of T's owners, every z6 tile whose ways come into T (its
  owned extent from the pass's reach; without one, its tile + 20 km), grown by the base pack's own
  extent where that lies further (a pack built by other reaches; each pack's header extent kept
  in `state/build/extents.json`, which the lead keeps as it plans, reading each new pack's header
  once; jobs only read it, and read the headers of packs it lacks without writing). A job opens
  only its tiles' owners (`build::owners_of`, the rule their keys read); one given no tiles, or
  whose reaches don't list every z6 tile, opens every one. Of each it reads from the NAS only the
  byte ranges of the ways that can reach its tiles (task #396, `crate::basepart`): the ways cut
  into chunks of 1,024 in file order, each chunk's box and first sample in the pack's *way index*,
  kept on the Mac (`<cache>/base/<content name>.idx`, a few kB) once worked out from the pack's
  ways, vertices and samples (read whole that once); the chunks whose box meets a tile are read,
  their records, vertices, per-vertex values, samples and road values, neighbouring ranges in
  one read, into sparse files at the top of the job's scratch (the disk its agent counts),
  deleted as the job ends. What it makes is byte-identical to what the whole files make. Measured
  on the M1 over Wi-Fi, 6/18/22's nine owners (5.6 GB of files): copying them whole took 75–119 s;
  read in part, 2.1 GB in 49 s the first time (the indexes worked out), 0.57 GB in 21 s after
  (18 Oct, the M1 busy with other builds).
- **Writes:** road and rail hi tiles (z9–14) and T's hidata: the ways-here index; query parts (each
  road's 100 m samples and channels with offsets, for drives and rides); rail lines; the zoomed-out
  summaries (`docs/phase5.md`).
- **Lo packs** per z3 tile hold roads and rail z4–8, made from the base packs (`lo`, its base packs
  read as pack's are) a tile at a time (task #1182, `hipack::tiles_one_by_one`): a zoom at a time,
  each tile from the ways whose box meets it (and the z3 tile), in draw order then by way id,
  cut 32,768 at a time (a way's copies in two packs in one run), each run's pieces fed to the
  tile's encoder (`tiling::TileEnc`: the lines it keeps and the dots it merges). Each tile gets its
  pieces in the order a cut of all the z3 tile's ways gives them, so the bytes are the same. The
  tiles go into the two packs as they're made (`layers::LocalPackWriter`), in key order, 16 to a
  resume piece (a z4's 4 tiles one piece); so the job holds a run's pieces, the tiles at work and
  16 tiles made, not the z3 tile's ways' lines or pieces. What still grows with the z3 tile: its
  base packs' way boxes (16 bytes a way, read once and held throughout) and its densest z4 tile's
  list of ways (16 bytes a way) and dots (3/4/2's 8/5: 21 M ways, 3.2 M dots). Measured on the M4 at 8 threads (12
  Nov), peak and drawing time: 3/4/2 17.3 GB, 91 s → 2.3 GB, 88 s (2.2 GB at 2 threads); 3/6/3
  10.9 GB, 48 s → 1.4 GB, 43 s; 3/3/3 1.95 GB, 8 s → 0.41 GB, 8 s. Its disk is the base packs'
  copies, as pack's (3/6/3: 20.6 GB read).
- Landmarks, stations, ferries and overlays come from their own jobs, so no road pack depends on
  worldwide rankings.

## 9. Worldwide steps after the z6 tiles

- **Road → z6 tiles index** (`global/roadunits`, its stored name): for whole-road hover. From the z6 tiles' road ids
  (`global/roadids/<u>`; a z6 tile built before them: its road values), keyed on them by content; it
  reads only the lists changed since the index before, which names each by content (`srcs`).
- **Labels** (per pass): places, states, seas, lakes and parks from the labels set (`crates/landmarks/src/labels.rs`,
  with own English, kana reading, OSM languages and OSM object: docs/names.md). Those past the Web
  Mercator tiles' 85.05° are ranked with the rest but left out of the tiles: the polar caps draw them.
- **The polar caps' points** (per pass, `polar-points`, a run per pole, "n" and "s", both whatever the
  coverage; `landmarks::polar`, docs/polar.md §10): the labels, landmarks and roads past 85.05° as
  `global/polar/<pole>` (GeoJSON), from the pass's filtered planet cut to each pole past 75° in one
  `osmium extract` (smart strategy: the input read three times; copied here first where there's
  room, as pass-sets does, else streamed over SSH): over each cut the labels' rows
  (`labels::read_rows`), the extract tool's landmark candidates and roads (`extract --candidates`),
  each point ranked by its isolation on the sphere over the cut.
- **Water** (per pass): above.
- **Spoken languages** (per pass, the `spoken` job): `global/spoken` (docs/names.md).
- **Rail stops:** from the rail set, for the built z6 tiles + 20 km.
- **Ferries:** worldwide, from the ferries set, the ferry feeds (`landmarks::gtfs::ferrycount`, docs/inputs.md
  §6.7.0) and the static timetables (docs/inputs.md §6.7.2).
- **Rail service** (below).
- **Landmarks:** `docs/phase5.md`.
- **The to-do lists:** `names-todo` after a batch of the map's log that changed what it reads
  (`namestodo::INPUTS`: the z6 tiles, regions, languages spoken, labels' hi packs, base packs,
  own-road lists, hi data, landmark points and area overlays, their keys and files; its key the
  log's last batch touching any, docs/formats.md, Marks; docs/names.md; a batch of depth or terrain alone changes
  none), at most daily while z6 tiles build, after any such once none's left
  (docs/names.md, `NAMES_TODO_EVERY_S`); `timetables-todo` after each rail
  and ferries run (docs/inputs.md §6.7.3).

## 10. Rail service

Trains a day on each rail way of the coverage (`global/railfreq`), from operators' GTFS timetables
and static timetables compiled by hand (`inputs::rail`; docs/inputs.md §6.7). Two jobs, in a chain
of their own (docs/building.md §2):
- **`rail-feeds`** (`landmarks::railfeeds`) finds the feeds from the accepted declarations of
  `inputs/timetables/feeds/` (docs/inputs.md §6.7.1):
  - The Mobility Database catalogue's active, keyless GTFS feeds of the countries the coverage is in
    (`inputs::rail::countries`: a country holding 5 % of one of the coverage's outlines, or half of
    whose area the coverage holds; a territory with an ISO 3166-2 code brings its country too) whose
    box meets it (as `buildstore::coverage` answers: an outline's edge meeting the box grown by its
    buffer as a rectangle, else the box inside it under the even–odd rule; never of the outlines
    merged). The declaration's `own_code` and `country` correct the catalogue's codes.
  - Of those, the ones running rail, checked once by reading `routes.txt` with range requests (only
    a whole answer counts).
  - The declared feeds first, in their `order`, where a territory matches and their stops meet the
    coverage; keyed feeds when `inputs/keys.env` holds the key.
  - Left out: a feed another replaces, and what the catalogue declaration's `exclude` names.
  - Each zip is fetched once into `sources/rail/gtfs/` and fetched again only after its
    declaration's `refresh` days (a week by default), or if it was already out of date. A request
    without an answer is tried three times; a feed still without one fails the job, and after 3 days
    is left out until rail-feeds next runs. A definite refusal leaves it out.
  - It also lists the ferry feeds (all of them: ferries are worldwide).
- **`rail`** counts the trains (`landmarks::gtfs::railcount`: each feed's median-busy Tuesday to Thursday from
  30 days before to 90 after its zip's day, a train in two feeds counted once; the static
  timetables likewise, or their count rows as given), marks stops beyond the coverage, and
  `railfreq` matches the pairs onto the pass's rail ways touching the coverage (`extract` at 8 m).
- The chain waits for the declarations' first accepted version, and `rail` for the static
  timetables'.
- **Planned:** Japan's ODPT and Taiwan's TDX feeds once keys exist (`ODPT_KEY`, `TDX_CLIENT_ID`,
  `TDX_CLIENT_SECRET`); refetching the catalogue and timetables every ~6 months.

## 11. Hand-made inputs

What no job makes, kept on the NAS with how it was made. Scripts run in `dem/` (`uv run python
<script>`); `scenic-build` takes `--root <NAS project folder> --scratch <local dir>`.
- **Heritage registers** (`inputs/heritage/`, made by `producers/heritage/`: docs/inputs.md §6.5)
  and the owner's matches (`inputs/heritage-matches/`, docs/inputs.md §6.5.4).
- **Timetable feeds' declarations** (`inputs/timetables/feeds/`, docs/inputs.md §6.7.1), how they were found in its
  `how/`.
- **Static timetables** (`inputs/timetables/gtfs/`, docs/inputs.md §6.7.2): the ferry timetables looked up by hand
  (the brief and batches in its `how/`); the MTR's lines (`mtr.zip`, made by
  `producers/timetables/mtr.py --mtr mtr.json --stations hk-stations.geojsonseq --out mtr.zip`).
- **Translations' and descriptions' methods** (docs/names.md): `inputs/translations/how/`,
  `inputs/descriptions/how/`.
- **Every inputs folder's `how/`** says how its input is made, pointing to `producers/<input>/`
  (docs/inputs.md §8); its README is kept in the repository as `producers/how/<box>/README.md`, and
  `inputs/README.md` as `producers/how/inputs-README.md`.
- **Fonts** (`data/fonts` in a checkout, which publish.sh puts in each version's `fonts/`): three
  Noto Sans styles' glyph ranges, `scripts/fonts.sh <folder>`. publish.sh needs every range in the
  publishing checkout's `data/fonts` (`scripts/fonts.sh --check`, before its build) and stops
  otherwise.

## 12. Job keys

A job's key is its step version plus what it reads, mostly by content name. The ones that cascade:
- **terrain-water (one):** the water source's pin (the latest basemap, and the lake fill) and what
  each depth target reads of it; it fills the digests file (`work/water-idx/<hash16 of the pin>`)
  when a target's digest is missing, keeping only the targets still wanted. A depth target's digest
  (`terrain_water::tiles_digest`) is over all its water, the rivers too (`depth_polys`). Until a
  target's digest exists its key is unknown and it waits. The terrain reads no water.
- **pyramids-fill (a z6 tile a pyramid read tile by tile meets, making hi tiles or with terrain-deep
  targets; the lead's):** `pyramids::V` (bumped
  with any change to which tiles it wants), the tile, and what `pyramids::want` reads for it: the
  coverage as its terrain-hi run reads it (`agent::build::piece_coverage`: the tiles of its levels), the declared pyramids meeting it (their lines as
  the terrain's keys name them) and its terrain-deep targets (every native tile under their
  pixels, at sea as on land; with the ring round them its blends' bands reach); until the declared
  datasets are read its key is unknown and it waits (`agent::build::pyramids_key`). Its z6 tile's
  terrain-hi and terrain-deep targets wait for it (a declared read keyed on nothing: the tiles are
  the same from the store as from the server), and it isn't done while a tile couldn't be fetched,
  so no job asks GSI for a tile the fill asked (task #975); a store that can't be read fails the
  job. Its time is weighed by the tiles it fetched (`needs::PER_WORK`):
  a z6 tile whose tiles were stored already says nothing of the next one's.
- **terrain-hi (a z6 tile):** `TERRAIN_V` and the codec (`terrain_codec::version`: the encoding,
  libwebp's version and level, the rounding), the coverage as it reads it
  (`agent::build::piece_coverage`: which of its z9–12 tiles are built by the rule, digested, not the
  rings there, task #832), the elevation pool's datasets meeting the tile
  (`TerrainTiles::terrain_datasets`: their lines as the roads' keys name them, each dataset's
  regions' outlines read so `declared::cut_note` names the outlines inside the tile, never their
  files: task #832), each place's sources there with the zoom each reaches
  (`terrain_zoom::key_lines`), its z10 tiles' terrain-deep mids by content (`idents::terrain_deep_mids`, a declared read of terrain-deep:
  it waits for those targets), and "no hi tiles" for a z6 tile making none. It waits for its own
  pyramids-fill too. Until the datasets are
  read its key is unknown and it waits. A "none" z6 tile is keyed on the version and the tile
  (`steps::none_key`).
- **terrain-deep (a z10 tile):** `TERRAIN_DEEP_V` and the codec, the tile, its band's code and the
  sources meeting it with the zoom each reaches there (`terrain_deep::target_key_lines`), its
  levels' tiles (`agent::build::deep_coverage`) and the pool's datasets meeting it. It reads no built tile: no target of it waits for another, only its z6 tile's pyramids-fill. A z10 tile no target any more
  is keyed on the version and the tile (`steps::none_key`), and so is one with no data past z12
  and outputs named whose key moved (`hollow_drops`; slope-deep's alike).
- **Drops:** a target under any of these "none" keys (`agent::build::is_none_key`) drops its packs
  and reads nothing; its run notes a `drops` measure (`needs::DROPS`, docs/pool.md §7.7) so it is
  sized as a drop, never as its step's build (each through scenic-build's `run_drop`), and the plan
  runs every step's drops first of the terrain, slope and tree cover work (`seam::Planned::drops`), the
  forecast and build page showing them as drops.
- **terrain-lo (an assembly):** its z6 tiles' mids by content ("-" for a z6 tile without one) and the
  pool's datasets meeting it; `TERRAIN_LO_V` and the codec. "none" likewise (`steps::none_key`).
- **slope-hi (a z6 tile):** `SLOPE_V` and the codec (`shade_codec::version`: the encoding, libwebp's
  version and level, the codes' constants), its z10 tiles' deep mids by content, and every terrain
  tile it reads itself (the z13 tiles taken in from those mids, with their tiles in the terrain deep
  packs' indexes, and their subtrees left out) by content, from the packs' indexes
  (`agent::build::slope_reads`), so a change along a neighbour's edge reruns it, one inside doesn't.
- **slope-deep (a z10 tile):** `SLOPE_V` and the codec, and every terrain tile its part of the
  pyramid reads (its terrain deep pack's, Horn's border past them in its edge neighbours' deep
  packs) by content (`slope_reads`).
- **slope-lo:** its z6 tiles' mids and the z3 tile's terrain lo pack; `SLOPE_LO_V` and the codec.
  **slope-root:** the lo packs and the codec.
- **available (a theme, `available-<theme>`):** `available::KEY_V` and each of the theme's
  `.geojson` files in the accepted extents by its keyed digest (§14); a theme with none has no
  target.
- **polar-terrain (a polar block):** `polar::TERRAIN_V`, the terrain codec's version, the block,
  and the pool's datasets meeting its latitudes (`roadcore::polar::lat_box`) by their lines, as the
  Mercator terrain's keys name them (`agent::build::polar_targets`). **polar-slope:** `polar::SLOPE_V`,
  the shading codec's, the block, and the terrain packs of it and its built neighbours by content.
  **polar-root (a pole):** `polar::ROOT_V`, both codecs', the pole, the datasets meeting the root's
  latitudes, the blocks built, and every block pack of its layers by logical and content name.
- **polar-points (a pole):** `landmarks::polar::POINTS_V`, the labels', candidates' and z6 tiles'
  versions (`LABELS_V`, `POIS_V`, `UNIT_V`: the rules it draws by), the pole, and the pass's filtered
  planet by content.
- **trees (a z6 tile):** the coverage inside the tile; `TREES_V` changes with any change to the bytes
  a z6 tile writes. A z6 tile the coverage has left, with tree hi packs or a mid still named, is
  keyed on the version and the tile (`steps::none_key`): a drop.
- **trees-lo:** its z6 tiles' mids; `TREES_LO_V`. A z3 tile with no z6 tile the coverage meets, and
  tree lo packs still named: "none" likewise (`steps::none_key`), a drop.
- **heritage-sites:** the pass, its areas set (the protected and Indigenous areas), the registers meeting the cover (their `keyed`
  digests), the matches' accepted version, and the cover (`in coverage:cover`,
  `landmarks::heritage::cover_key`: the z12 tiles within 20 km of the coverage's land and the z6
  tiles holding them, what its run clips to), not the coverage's shapes: a region change that
  leaves the cover as it was (water given from one region to another, land moved between
  neighbours) keeps its key. **heritage** likewise: heritage-sites' outputs, the pass's sets, the
  cover.
- **unit** (a z6 tile): its extract, the pass's road values, the coverage as its ways meet it, the versions of the
  location rules where its ways go, its heritage slices, the roadside buildings' index, the elevation
  datasets' plan, and the terrain tiles it reads by content (`agent::build::unit_terrain`: each
  tile's XXH3 from its pack's index): what it stages, every z11 tile, and at its ways' points the
  z12 tile or the finest staged above it. A z6 tile whose terrain indexes can't be read has no key and
  isn't built; the status says so.
- **pack(T):** the base packs and road values of its owners, the z6 tiles whose ways come into T.
- **lo:** the base packs and road values of the z6 tiles whose owned extent meets its z3 tile.
- **rail-feeds:** the declarations' accepted index, the catalogue's copy, the pass's outlines,
  which keys `inputs/keys.env` holds by name (never values), and the coverage as its run asked it
  (`in coverage:asked`, below): the feeds' boxes, and its countries (`inputs::rail::countries`,
  of every outline: asked again of the coverage whole, from the outlines). Not what it writes.
- **rail:** the feeds' list (each zip by content name and its day), the static timetables' accepted
  index, the pass's rail set, the cover (as heritage-sites': it clips the rail set to it), and the
  coverage as its run asked it: its stops and its ways' vertices.
- **timetables-todo:** the rail and ferries jobs' lines, and the coverage as its run asked it: the
  ferry lines' ends.
- **The coverage as a run asked it** (`in coverage:asked`, `buildstore::asked`; rail-feeds, rail,
  timetables-todo, bld-fetch): the run asks through `coverage::Asking` (whether a point is in,
  whether a box meets the land or any shape; no other way in: `asked::Run` hands it out) and puts
  what it asked as `work/asked/<step>`: every question once, its questions of the whole coverage
  with their answers (rail-feeds' countries), the digest of every answer (`Coverage::answers`, each
  asked again: which region holds a point never enters it), the coverage's version it asked and the
  key line it ran under. The key's line is that line while the coverage is that version (nothing
  asked again) or answers it all the same; otherwise a new one naming the record, and none yet, one
  of the coverage's version. The run works its line out from the record before it as the plan does
  (`asked::Run::start`), so the key it's recorded done under is the plan's after it: one run. So a
  region change reruns these only where it changes an answer they read: water given from one region
  to another, or a region added where none of them asked, changes none (task #1127's regions
  change, measured on the real inputs, changed no answer of the four, nor the cover). The plan reads
  a record once by content name (`TerrainTiles`) and asks it again only once a coverage version
  other than the one it asked and the record (on the plan's thread: rail's 4.3 M points 1.2 s,
  rail-feeds' countries 49 s, measured by task #1130 on the mini), opening the outlines a question
  names only then; outlines no longer named (a pass gone) answer `GONE`. A record that can't be
  read leaves its step waiting, its run failing.
- **water:** its version and the pass's basemap.
- **bld-fetch** (network): its version, the release and the coverage as its run asked it (each row
  group's and GHSL tile's box grown by 20 km, `bld::fetch::Near`).
- **bldprep (per z6 tile):** `BLDPREP_V`, the release, and each downloaded file's row groups and GHSL
  tile meeting it (`crate::bld::sources`).
- **depth (a z6 tile):** `DEPTH_V`, `DEPTH_LO_V` and the shading's codec (`shade_codec::version`),
  the tile, the sources its blocks read (meeting the tile and the ring of z6 tiles around it with
  z6's 32-px margin: its coarse levels', the widest; across the date line too), and the digest of its water, rivers included
  ("depth 6/x/y"): at z6–8 its tiles and two rings around them (its coarse levels are composed over
  it and a ring, each block reading a ring past it), and its tiles and the ring around them at each
  zoom from z9 to z14 its blocks compose (a grid of that zoom meets the tile with the blocks' 32-px
  margin; z15–16 are drawn from z14's polygons), so a change the basemap shows only at z13–14
  remakes the z6 tiles whose grids of that zoom reach it, and one at a zoom no grid there is none;
  and the lake-depth packs of its tile and the 8 around it by content.
- **lake-depth (a z6 tile):** `depth::lakes::V`, the pins of the declarations it makes, the water
  source's pin (the latest basemap and the lake fill, whole) and the terrain hi packs of the tile
  and the 8 around it by content.
- **depth-lo (a z3 tile):** the same versions, the tile, the depth sources meeting it with its
  blocks' 32-px margin at z3, the widest (id and digest of declaration and index), the digest of
  its z3–8 water as the depth reads it, rivers included ("depth 3/x/y", worked out by the
  terrain-water job), the content names of its z6 tiles' `depth`, `depth-class` and
  `depth-shade` hi packs (its z8 is made from their z9), and those of the lake-depth packs of its
  z6 tiles and the ring of z6 tiles around them.
- **depth-root:** `DEPTH_ROOT_V`, the depth's versions and the content names of every `depth`,
  `depth-class` and `depth-shade` lo pack (recorded with the other roots, by its name).
- **bldtiles (per z6 tile):** `BUILDINGS_V`, the normalized files of the tile and its 8 neighbours,
  and the coverage's shapes over it with their countries (`Coverage::shapes_key`).

The landmark jobs, stations, ferries and overlays: `docs/phase5.md`. An output identical to before
keeps its content name, so jobs keyed on it stop there.

## 13. Served

| What | How |
|---|---|
| our layers | the pack for the tile; names attached |
| polar caps | `/tiles/polar-terrain/<pole>/z/x/y`, `/tiles/polar-slope/…`: the pole's root (z0–3) or block (z4–9) pack's tile as stored, else 204 (none) |
| polar caps' points | `/api/layer/polar-<pole>`: `global/polar/<pole>` whole, names attached, gzip'd (404 while the map has none) |
| basemap | tiles from the map's PMTiles archives (`/tiles/base`); names attached |
| drives, rides, rail lines | parts from the hidata in view plus half the window, joined by offset; zoomed out (wider than ~1,200 km) from the 500 m summaries (`approx`) |
| whole road (hover) | the road's parts, through the road → z6 tiles index; away from the NAS, cut where the Mac's downloads end |
| details, profiles | by OSM id plus location (ways-here index → base pack) |
| landmarks, overlays, stations, ferries | by view (`docs/phase5.md`) |
| `/api/meta` | the map's meta, summed from the base packs' summaries |
| `/api/catalog` | the map's number, layers, zoom ranges, versions, credits, regions, NAS and build state |
| `/api/regions/outlines` | each region's outline simplified for a zoom (kept per zoom) |

**Browser:** the map is drawn by our own MapLibre, `web/maplibre/` (MapLibre GL JS 6.11.2's source,
changed where needed: `web/maplibre/FORK.md`), compiled with the app. English comes from the server.
The page switches to an updated map in place (it polls `/api/catalog` every minute, every 5 s
while the server is away from the NAS).

## 14. Data available (task #1334)

The layers that colour the map by the best data any declared source has for each place, from the
source extents (docs/inputs.md §6.11): for a theme (`terrain` first; `bathymetry` and `buildings`
later), a **score** layer, the primary one, and a layer per metric. **Built** (task #1347): the
step `available`, its tiles, and the server's score, maxima, tiles and click. **Planned:** the
same layers from only the data built into the map (the set `built`, task #1349, with
`piece.dataset`: extents from the build's own data, below), and the app's layers (task #1348). The
map's regions play no part in the first set; in the built-in set they will enter only as the
build's own cut of where a dataset is used (its `regions`, §6.8), which is what the map has built
in.

**The step** `available` (terrain::available; stage `finish`, a target per theme,
`available-<theme>` for a theme whose accepted extents have pieces, recorded with the worldwide steps' keys; `extents` is taken in the build: the
base packs' header extents, `codecs::extents`, `state/build/extents.json`). It reads the accepted
`extents` unit through the accessor (the theme's `.geojson` files: its pieces, those with an
extent) and nothing else. Its key is its version (`terrain::available::KEY_V`) and each of the
theme's `.geojson` files by its keyed digest (`idents::extents_pieces`, from the index the planner
reads, `theplan::key_inputs`' `extents:<theme>`); never `score.toml` nor a declaration: the tiles
hold which pieces are where, never a score or a metric, so tuning a weight builds nothing again.
- **Pixels hold piece sets, by a palette per tile** (docs/formats.md, Data available): Web
  Mercator tiles of 256 px, z0 to z12 (a pixel 38 m at the equator); a pixel's value is an index
  into its tile's palette, a list of the sets of pieces met in that tile (each set by the pieces'
  ids, `<source>/<piece>`), a byte a pixel while a tile has 256 sets or fewer, two past it (an
  overview's union can have more). A tile all of one set, at z12 under it, isn't stored, nor
  anything under it: its nearest stored ancestor's pixels say it (z0 is stored unless it holds
  nothing), so the tiles follow the outlines' length, not the area.
- **Overviews:** above z12 a pixel's set is the union of the four under it, so a small piece of fine
  data stays in sight zoomed out (a layer's colour there is the best anywhere in the pixel), where a
  pixel's centre alone would drop it; a click reads z12.
- **Packs:** the root pack (`layers/available-<theme>/root/0-0-0`, z0–5) and a pack per z6 tile
  holding a stored tile (`layers/available-<theme>/hi/6-x-y`, z6–12 under it), RDPACK, gzip'd;
  the catalogue's layer `available-<theme>`, encoding `avail-sets`, zooms 0–12.
- **Keys of the tiles:** a z6 tile's tiles depend only on the polygons whose boxes meet it, so its
  key is those polygons' pieces' ids and each polygon's digest (`tile_keys`); finer than a piece's
  digest, a source fetched again makes again only the z6 tiles where one of its polygons changed.
  The step keeps each z6 tile's key, and its z6 tile, in its own record
  (`work/available-<theme>/mids`, an RDPACK of the z6 tiles holding a piece, its meta the keys):
  a run makes again the z6 tiles whose key moved or whose pack is missing, drops those no polygon
  meets now (any z6 pack in the records, so one a record lost or of another version doesn't name
  goes too), and leaves the other packs as they are; a record missing, of another `KEY_V` or not
  readable makes every tile again; the root is made again from the record's z6
  tiles whenever one moved. A run with nothing moved writes the record again as it was (the same
  bytes, the same content name), and nothing else. On the 3,519 pieces with an extent of 13 Nov
  (below), 3,820 z6 tiles are keyed and 1,409 have a pack.
- **The score and the layers' values are the server's**, from `score.toml` and the pieces'
  metrics, which it reads from the accepted unit (below).
- **Why raster tiles and not vector:** the maximum over overlapping pieces is a per-place rule,
  exact on a raster by construction, where a vector answer needs the overlay of thousands of
  outlines as a planar partition (slow, and fragile on publishers' geometry); one set of tiles
  serves every layer, both settings of the switch, any weights, and the click; and the extents are
  good to their tolerance (tens of metres at best, kilometres for a mask read at a coarse overview:
  each piece's `read` says, §6.11), which a pixel of the finest level matches.
- **Rasterising:** a piece's rings are lines in longitude and latitude (RFC 7946), densified so none
  strays from its image by a quarter of a finest pixel (halving an edge while its midpoint's image
  lies further from the chord). Each row of pixel centres is cut by every polygon's rings: a pixel
  is in a polygon when an odd number of crossings lie left of its centre (even-odd, holes out), and
  in a piece when it's in any of its polygons, so overlapping polygons of one piece add up, never
  cancel. A row becomes runs of one set, and a tile all of one run in every row is uniform. The
  antimeridian needs nothing: pieces arrive cut there, and the plane runs from −180° (column 0 of
  z12) to 180° (column 4095's end). The Web Mercator tiles end at 85.05°: a ring closed along 90°
  lies past the plane's edge. A row of z6 tiles is cut at once, its tiles' quadtrees made in
  parallel (rayon), their sets over the row's.
- **The polar caps** (docs/polar.md) take the same sets in each pole's grid: a root pack a pole
  (`layers/available-<theme>-<n|s>/root/0-0-0`, z0–3, a pixel 1,024 m of the grid at z3, the
  polar terrain's root's zooms), from the polygons whose boxes reach the pole's square, keyed
  alike (in the same record). A polygon's latitudes past the equator are put on it for the
  projection (the square is far from it: what the polygon holds there doesn't change). ArcticDEM's
  and REMA's pieces close along 90° where they hold a pole, and REMA's keeps its hole round the
  South Pole.
- **Its requirements** are measured as every step's (agentcore::needs): nothing is kept by hand.
  On the 3,519 pieces with an extent of 13 Nov (producers/extents/terrain/, North America's
  sources among them; `build` on them in a scratch folder, the test profile's build, on the Mac
  mini beside its jobs): 47 s (3 min 34 s of CPU), a peak footprint of 1.03 GB, 1,409 hi packs of
  610,482 tiles, 321 MB with the root (2.3 MB) and the record (2.1 MB); 681 tiles have more than
  256 sets (two bytes a pixel; the most, 9,163, in z4's tile over Texas and northern Mexico, where
  3DEP's and the US inventory's thousand pieces each overlap); run again with nothing moved,
  0.4 s.
- **Extents from the build's own data (the owner's rule A.1; planned, task #1349).** A piece
  block naming the elevation dataset that holds its files (`dataset`, §6.11) will have its extent
  taken from that dataset's mask wherever the dataset is accepted (class `mask`), its listed extent
  standing only outside the dataset's coverage: Copernicus GLO-30, which the store holds whole
  with its fill masks, drawn where its own pixels are (its fill mask below 3), not where its 1°
  tiles are.
- **The built-in set (planned, task #1349):** palettes of the same shape whose pieces are the
  build's own datasets, each where the terrain took its heights from it: the order's first dataset
  with data at a place, within its `regions` (§6.8, `declared::Cover`), so a place's built-in set
  is the one dataset the map used there, not a maximum over every dataset holding data. Its extent
  is the dataset's valid-data mask at the layer's own pixel, read from the store's files at the
  coarsest level no wider than ~38 m (the hollow tiles' second read, 1/1024 of a z10 tile; a GLO-30
  tile's fill mask below 3), over the dataset's presence tiles: the z10 presence and hollow tiles
  alone are 38 km squares, which would draw a dataset over every tile it touches. Read on the lead,
  where the store is. Its key names the elevation unit's index and each dataset's store digest.
- **Tests:** terrain::available's (hand-made pieces: the sets and a tile's palette, a tile all of
  one set not stored, an overview keeping a piece smaller than its pixel, overlapping polygons of
  one piece and a hole, a piece cut at the antimeridian, the polar caps with a pole held and a
  ring's hole round the other, a moved piece making only its z6 tiles again, a removed one dropping
  them, a fresh run making the same files, a run with nothing moved making nothing, and a record
  lost or unreadable making everything again, a pack it doesn't name dropped; and the real pieces round Vancouver, two sources overlapping: every piece
  in some tile, z0 naming every piece a finer tile does, each stored tile holding what its stored
  children do; every real piece the same, `every_real_piece`, a benchmark run by hand);
  codecs::available's (the format, the layers' names and packs); the server's below.

**The score and the layers' values** (crate::available in the server; `Score::of` in
crate::inputs::extents). The server keeps the extents' accepted version on the Mac as it keeps the
translations' (crate::accepted, synced every minute while the map is in use), and reads each
theme's `score.toml` and pieces from it (the `.geojson`s' properties and `read`, the declarations'
descriptions). For each palette entry, its pieces that pass the availability switch (`avail=open`,
the default: those whose availability is `open`; `all`: every one): the score layer's piece is
the one of the best score, a metric layer's the one of the best 0–1 value of that metric (the
finest spacing, bare earth over surface, the newest year, the best stated accuracy; a piece not
stating it takes no part; of equal values, the first id); none passing, transparent
(`Theme::best`). That piece's colour (`Theme::colour`) is as `score.toml` declares the layer
(docs/inputs.md §6.11): a metric with classes, its class's (by its value in the metric's unit: the
terrain's spacing in eight, 0.5 m to coarser); a word's colour where the metric gives them (bare
earth and surface); otherwise the theme's ramp at its value (the year, the accuracy). The score
layer is the primary's classes, since the primary decides the score: the piece's class, shaded by
its tie-break `shade` of the way at most towards the next coarser class's colour, so 0.5 m, 1 m and
2 m lidar read as three colours, not three near-equal scores on one ramp. A `score.toml` declaring
no colours (one older than these fields) is drawn as before them, every layer on the server's own
ramp (viridis's ends and middle) over its 0–1 value. The score and the colours are read at each
sync, so changing either draws within a minute and builds nothing again. `Score::of` is
producers/extents/score.py's formula: tested on score.toml's own examples and more pieces
(`score_pieces.json`) against what score.py makes of them (`score_expected.json`, made by
`score.py <score.toml> --json <pieces.json>`; producers/extents/test_extents.py makes it again and
checks the file), to 10⁻¹² and to the three digits score.py shows. The legend
`/api/available/<theme>` gives each layer (docs/formats.md) is the same colours: the classes with
their labels and edges, the words', or the ramp with stops in the metric's own unit. The server's
tests (crate::available's): each layer's best piece under the switch, each layer's colours and
legend from the real `score.toml`, one declaring no colours drawn on the built-in ramp, the tiles
and the click; and `renders_the_real_pieces`, which with `SCENIC_RENDER_DIR` set builds every real
piece and writes each layer as PNGs over the land's colour (the world at z2, France at z5,
Vancouver at z8; ~1 min), a look at the colours by hand.

**Served.** `/tiles/available/<theme>/<set>/<layer>/z/x/y?avail=open|all`: the tile coloured by the
server as above (PNG; 204 where no piece is, or none passes the switch), so the map
draws a plain raster layer and holds no logic of its own; a tile not stored is its ancestor's part
over it (codecs::available::tile_at). Past 85.05°, `/tiles/polar-available/<pole>/<theme>/<set>/<layer>/z/x/y`
(z0–3 of the pole's grid). `<set>`: `best` (`built` is planned). `<layer>`: `score` or a metric of
the theme's score; anything else, or past the zooms, 404. Its ETag names the layer's packs' version,
the extents' version, the layer, the switch and the tile; `?v=` with the version `/api/meta` gives
the layer (`available-<theme>`, `available-<theme>-<n|s>`: the packs' version and the extents'
version's hash) is cached for good while it's current. `/api/available/<theme>/<set>?lng&lat&avail`:
the pieces at a point (the z12 pixel holding it; past 85.05°, the pole's z3 pixel), best first, each
with its source's name, credit, licence and link, its metrics, their 0–1 values and its score, its
availability, extent class and what its extent was read from (§6.11's `read`: its block's reading,
with the method, its grain, its date). `/api/available/<theme>`: the score's metrics, scales and
weights as declared, each layer's legend, and the sources with their pieces counted by
class (those of class `unknown` named here, as they're drawn nowhere). Away from the NAS a tile
partly downloaded is cut to the downloads as every layer's (its pixels outside hold no piece).

**In the app** (planned, task #1348). A group "Data available" in the layers panel, a section per
theme: the set (best available anywhere / built into the map), the layer (Score, then the theme's
metrics), the availability switch ("only data that downloads freely"), an opacity. The settings are
a profile's, as every layer's are: their values when a profile lacks them come from
`web/default-profile.json` alone (task #1199: no defaults in the code; the switch on, the group
off), and the native app reads the same profile schema. The legend and the formula are read from
`/api/available/<theme>`. A click, while a layer of the group is on, opens a panel listing the
pieces there, best first. In the code the layer is a raster source and layer and two requests;
nothing of it is MapLibre's but the raster layer itself, and in the native port it is a ground layer
painted into the drapes (docs/native-port.md, Land).
