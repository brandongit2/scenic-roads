//! The step programs' command lines: what a unit's tail runs (extract, elev, tile, scenic-metrics,
//! areaflags, landcover), the tasks' programs (trees, bldtile, terrainsub, also as WebAssembly in
//! a page), the rail step's railfreq, and the tools run by hand (lakefill, coastcheck,
//! terrain). Each binary in src/bin/ of the same name parses with its own.

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Car-accessible public roads (and car ferries) and passenger rail lines from OSM PBF extracts,
/// densified to a spacing (src/bin/extract.rs)
#[derive(Parser, Debug)]
#[command(name = "extract")]
pub struct Extract {
    /// The output folder
    pub out_dir: PathBuf,
    /// Metres between vertices at most, or a spacing file (a unit's spacing.json)
    #[arg(value_name = "SPACING_M|SPACING.JSON")]
    pub spacing: String,
    /// The OSM extracts
    #[arg(value_name = "FILE.OSM.PBF", required = true)]
    pub inputs: Vec<PathBuf>,
    /// Only rail-rels.bin, after pass 1 (a build made before it existed)
    #[arg(long)]
    pub rail_rels_only: bool,
    /// The landmark candidates' points too (the pois job)
    #[arg(long)]
    pub candidates: bool,
    /// The pass's hiking-route ends (pipeline::trailends), instead of working them out
    #[arg(long, value_name = "FILE")]
    pub trailends: Option<PathBuf>,
}

/// Elevations at every road vertex from the declared elevation datasets (pipeline::dem), by the
/// plan the unit job wrote in the build folder (elevation.json). SCENIC_STORE: the stores' folder;
/// SCENIC_FETCH_*: mirror folders, recording, no network (pipeline::fetch)
#[derive(Parser, Debug)]
#[command(name = "elev")]
pub struct Elev {
    /// The unit's build folder
    pub build_dir: PathBuf,
    /// Threads
    #[arg(long, value_name = "N", default_value_t = 48)]
    pub workers: usize,
    /// The DEM cache (default: <build_dir>/../cache)
    #[arg(long, value_name = "DIR")]
    pub cache: Option<PathBuf>,
    /// No cache
    #[arg(long)]
    pub no_cache: bool,
}

/// A unit folder's sampled elevations post-processed (pipeline::elev): final.u16 and grade.u8
#[derive(Parser, Debug)]
#[command(name = "tile")]
pub struct Tile {
    /// The unit's folder
    pub unit_dir: PathBuf,
    /// What's made (elev, the one so far)
    pub what: TileWhat,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum TileWhat {
    Elev,
}

/// Scenic analysis of a unit's roads: the unit's steps (pipeline::unit)
#[derive(Parser, Debug)]
#[command(name = "scenic-metrics")]
pub struct Metrics {
    /// The unit's folder
    pub unit_dir: PathBuf,
    /// The step
    pub step: MetricsStep,
    /// The footprints' folder (buildings)
    #[arg(required_if_eq("step", "buildings"))]
    pub dir: Option<PathBuf>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricsStep {
    /// Road sample points with observer eye heights; per-vertex drape heights
    Prep,
    /// Canopy heights: grid layers, near-field horizons, roadside trees
    Canopy,
    /// Far-field viewshed and landscape metrics
    View,
    /// Roadside buildings per sample from the footprints (the folder given)
    Buildings,
    /// The flag channels again on the view's metrics
    Flags,
}

impl MetricsStep {
    pub fn name(self) -> &'static str {
        match self {
            MetricsStep::Prep => "prep",
            MetricsStep::Canopy => "canopy",
            MetricsStep::View => "view",
            MetricsStep::Buildings => "buildings",
            MetricsStep::Flags => "flags",
        }
    }
}

/// A unit's area flags (grid.areas.u8) on its own z11 grid (grid.idx), from the flagged polygons
/// near it (pipeline::areaflags)
#[derive(Parser, Debug)]
#[command(name = "areaflags")]
pub struct Areaflags {
    /// The unit's folder
    pub unit_dir: PathBuf,
    /// The flagged polygons
    #[arg(value_name = "AREA-SHAPES.GEOJSONSEQ")]
    pub shapes: PathBuf,
}

/// Land cover on the z11 analysis grid from the declared land-cover source (pipeline::landcover),
/// the unit job's (its folder's sources.json). SCENIC_FETCH_*: mirror folders, recording, no
/// network (pipeline::fetch)
#[derive(Parser, Debug)]
#[command(name = "landcover")]
pub struct Landcover {
    /// The unit's build folder
    pub build_dir: PathBuf,
    /// Threads
    #[arg(long, value_name = "N", default_value_t = 32)]
    pub workers: usize,
    /// Only these grid slots (a file of u32)
    #[arg(long, value_name = "SLOTS.U32")]
    pub only: Option<PathBuf>,
}

/// Trains a day on the rail ways, from the stop pairs (the rail step's)
#[derive(Parser, Debug)]
#[command(name = "railfreq")]
pub struct Railfreq {
    /// The rail ways' folder
    pub build_dir: PathBuf,
    /// The stop pairs
    #[arg(value_name = "PAIRS.BIN", required = true)]
    pub pairs: Vec<PathBuf>,
}

/// The tree cover layers' tiles (pipeline::trees): a z3 or z6 tile's run, a z3 tile's assembly, or
/// the same work a block at a time, for workers. SCENIC_FETCH_*: mirror folders, recording, no
/// network (pipeline::fetch)
#[derive(Parser, Debug)]
#[command(name = "trees")]
#[command(group = clap::ArgGroup::new("mode").required(true).args(["z3", "z6", "block", "blocks", "assemble", "assemble_lo"]))]
pub struct Trees {
    /// A z3 tile's run: its zoom 4–12 tiles in out/trees-{cover,height,leaf}.tiles
    #[arg(long, value_name = "X,Y")]
    pub z3: Option<String>,
    /// A z6 tile's run: its zoom 9–12 tiles, and out/trees-mid.sect its mid
    #[arg(long, value_name = "X,Y")]
    pub z6: Option<String>,
    /// One zoom-8 block: its zoom 8–12 tiles and out/trees-tops.bin its zoom-8 values
    #[arg(long, value_name = "8/X/Y")]
    pub block: Option<String>,
    /// Blocks of one row made together, each strip of the canopy read once (a row's task): out/8-x-y/
    /// for each
    #[arg(long, value_name = "8/X/Y,…")]
    pub blocks: Option<String>,
    /// The blocks' archives and zoom 7–4 from their values (the inputs: the blocks' out folders)
    #[arg(long)]
    pub assemble: bool,
    /// A z3 tile's zoom 8–4 from its z6 tiles' mids (the inputs)
    #[arg(long)]
    pub assemble_lo: bool,
    /// The coverage (the trees program's JSON)
    #[arg(long, value_name = "COV.JSON")]
    pub coverage: Option<PathBuf>,
    /// Where the declared canopy and leaf-type sources are (pipeline::trees::RunSources; a run's)
    #[arg(long, value_name = "SOURCES.JSON")]
    pub sources: Option<PathBuf>,
    /// The canopy squares: a folder, or URLs (a prefix: through the fetch layer) for blocks
    #[arg(long, value_name = "DIR|URL")]
    pub chm: Option<String>,
    /// The NAS's canopy store (a run's squares made ready from it)
    #[arg(long, value_name = "DIR")]
    pub chm_store: Option<PathBuf>,
    /// The leaf-type squares: a folder, or URLs for blocks
    #[arg(long, value_name = "DIR|URL")]
    pub leaf: Option<String>,
    /// The output folder
    #[arg(long, value_name = "DIR")]
    pub out: PathBuf,
    /// Threads
    #[arg(long, value_name = "N")]
    pub workers: Option<usize>,
    /// The bytes read kept in this mirror folder (blocks)
    #[arg(long, value_name = "DIR")]
    pub record: Option<PathBuf>,
    /// The canopy squares the job found (blocks: one not found fails the run; south of the
    /// equator a square's top is negative)
    #[arg(long, value_name = "TOP,LEFT;…", allow_hyphen_values = true)]
    pub squares: Option<String>,
    /// What's assembled (--assemble, --assemble-lo)
    #[arg(value_name = "DIR")]
    pub inputs: Vec<PathBuf>,
}

/// A z8 tile's 3D buildings tiles from its task folder (pipeline::bld::task; docs/buildings3d.md
/// §3.6): a bldtiles job's task, natively or as WebAssembly in a page. Writes area.tiles and
/// area.json in the folder
#[derive(Parser, Debug)]
#[command(name = "bldtile")]
pub struct Bldtile {
    /// The z8 tile's task folder (its work files' blocks, coverage.sect)
    pub dir: PathBuf,
    /// The z8 tile
    #[arg(value_name = "8/X/Y")]
    pub tile: String,
    /// Threads (default: rayon's)
    #[arg(long, value_name = "N")]
    pub workers: Option<usize>,
}

/// A terrain piece's z8 subtrees made from a task's file (pipeline::terrain_task): a terrainsub
/// task, natively or as WebAssembly in a page. Writes the group's tiles (dir/hi.sect) and its
/// mid's part (dir/mid.sect), from what the file holds alone
#[derive(Parser, Debug)]
#[command(name = "terrainsub")]
pub struct Terrainsub {
    /// The task's file
    #[arg(long = "in", value_name = "IN.SECT")]
    pub input: PathBuf,
    /// The output folder
    #[arg(long, value_name = "DIR")]
    pub out: PathBuf,
    /// Threads (default: rayon's)
    #[arg(long, value_name = "N")]
    pub workers: Option<usize>,
}

/// The terrain's repair over the coverage's tiles, every level, from AWS's raw tiles, beside the
/// first repair and repaired again: a check, reading the NAS, writing only under --out
#[derive(Parser, Debug)]
#[command(name = "terrain")]
pub struct Terrain {
    /// The project folder whose records name the terrain's sources
    #[arg(long, value_name = "DIR", default_value = "/Volumes/personal/projects/scenic-roads")]
    pub root: PathBuf,
    #[command(flatten)]
    pub scan: TerrainScan,
}

/// `terrain --scan`'s options.
#[derive(Args, Debug)]
pub struct TerrainScan {
    /// The scan (all it does: the flag kept for its callers)
    #[arg(long, required = true)]
    pub scan: bool,
    /// Where the raw archives are copied, a z3 pack at a time
    #[arg(long, value_name = "DIR", default_value = "data/scan-cache")]
    pub cache: PathBuf,
    /// The scan's output folder
    #[arg(long, value_name = "DIR", default_value = "data/scan")]
    pub out: PathBuf,
    /// Tiles whose values are written to views/ (z/x/y a line)
    #[arg(long, value_name = "FILE")]
    pub views: Option<PathBuf>,
    /// Only these z3 packs (3/x/y,…)
    #[arg(long, value_name = "3/X/Y,…")]
    pub only: Option<String>,
    /// Only these z6 tiles made (6/x/y a line)
    #[arg(long, value_name = "FILE")]
    pub z6: Option<PathBuf>,
    /// AWS's tiles alone (no northern dataset, water or walled patches)
    #[arg(long)]
    pub bare: bool,
    /// The terrain made written there too, as the terrain job's packs (a project folder of its own)
    #[arg(long, value_name = "DIR")]
    pub fixed_out: Option<PathBuf>,
    /// Tiles alone, each from its raw tile only, with their views (z/x/y a line)
    #[arg(long, value_name = "FILE")]
    pub tiles: Option<PathBuf>,
    /// The worldwide z8, each tile alone
    #[arg(long)]
    pub world_z8: bool,
}

/// Canada's lakes missing from OSM, sheet by sheet: the lakefill step's parts (pipeline::lakefill::run)
/// one at a time, and checks of a fill. Scratch folders only
#[derive(Parser, Debug)]
#[command(name = "lakefill")]
pub struct Lakefill {
    #[command(subcommand)]
    pub cmd: LakefillCmd,
}

/// The lake-fill source's declaration (one as dropped into inputs/sources/,
/// producers/sources/canvec-lakes.toml).
#[derive(Args, Debug)]
pub struct Declaration {
    /// The lake-fill source's declaration
    #[arg(long, value_name = "FILE.TOML")]
    pub declaration: PathBuf,
}

#[derive(Subcommand, Debug)]
pub enum LakefillCmd {
    /// Every NTS 1:50,000 sheet across the Canadian regions, with the basemap's z12 water in it
    Survey {
        #[arg(value_name = "BASEMAP.PMTILES")]
        basemap: PathBuf,
        #[arg(value_name = "OUTLINES.SECT")]
        outlines: PathBuf,
        #[arg(value_name = "OUT.CSV")]
        out: PathBuf,
    },
    /// The survey's gap sheets
    Gaps {
        #[arg(value_name = "SHEETS.CSV")]
        sheets: PathBuf,
        #[arg(value_name = "OUT.CSV")]
        out: PathBuf,
    },
    /// The fill of those sheets from the lake-fill source's shapefiles, written to --out content-named
    /// with report.json
    Fill {
        #[arg(long, value_name = "PMTILES")]
        basemap: PathBuf,
        /// The gap sheets
        #[arg(long, value_name = "GAP-SHEETS.CSV")]
        sheets: PathBuf,
        /// The shapefiles (one or more)
        #[arg(long, value_name = "DIR|ZIP", required = true)]
        source: Vec<PathBuf>,
        #[command(flatten)]
        declaration: Declaration,
        /// Only the gap sheets whose middle is in these longitudes
        #[arg(long, value_name = "W,E", allow_hyphen_values = true)]
        band: Option<String>,
        /// A rule off: halves completed
        #[arg(long)]
        no_complete_halves: bool,
        /// The rules' tolerance (m)
        #[arg(long, value_name = "M")]
        tol: Option<f64>,
        /// The rules' confirming share
        #[arg(long, value_name = "SHARE")]
        confirm: Option<f64>,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Bands' fills as one
    Merge {
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// The bands' run folders
        #[arg(value_name = "RUN DIR")]
        runs: Vec<PathBuf>,
    },
    /// All of it, as the step runs it
    All {
        #[arg(long, value_name = "PMTILES")]
        basemap: PathBuf,
        #[arg(long, value_name = "SECT")]
        outlines: PathBuf,
        #[arg(long, value_name = "DIR")]
        source: PathBuf,
        #[command(flatten)]
        declaration: Declaration,
        #[arg(long, value_name = "DIR")]
        work: PathBuf,
    },
    /// A box of the basemap's water with the sheets' edges, as a PNG, for checking by eye
    View {
        #[arg(value_name = "BASEMAP.PMTILES")]
        basemap: PathBuf,
        /// The box
        #[arg(value_name = "W,S,E,N", allow_hyphen_values = true)]
        bbox: String,
        /// Its zoom
        z: u8,
        #[arg(value_name = "OUT.PNG")]
        out: PathBuf,
    },
    /// A test root's newest catalog again, one newer, with the fill (copied into the root's work/).
    /// Never the NAS's
    CatalogAdd {
        /// The test root
        #[arg(long, value_name = "DIR")]
        root: PathBuf,
        #[arg(long, value_name = "LAKEFILL.<HASH>.SECT")]
        fill: PathBuf,
    },
    /// The fill's lakes and their area (km²) by region, as JSON
    Stats {
        #[arg(long, value_name = "F")]
        fill: PathBuf,
        #[arg(long, value_name = "GAP-SHEETS.CSV")]
        sheets: PathBuf,
    },
    /// Where the fill's lakes were cut at a sheet's edge and OSM's water lies across it, as JSON
    Seamcheck {
        #[arg(long, value_name = "F")]
        fill: PathBuf,
        #[arg(long, value_name = "PMTILES")]
        basemap: PathBuf,
        /// Cut edges looked at, at most
        #[arg(long, value_name = "N", default_value_t = 2000)]
        max: usize,
    },
}

/// The shoreline check's reference (tools/coastcheck/README.md): the water at full detail, and its
/// exact coverage served as raster tiles
#[derive(Parser, Debug)]
#[command(name = "coastcheck")]
pub struct Coastcheck {
    #[command(subcommand)]
    pub cmd: CoastcheckCmd,
}

#[derive(Subcommand, Debug)]
pub enum CoastcheckCmd {
    /// The water at full detail into a store: the water areas of the pass's filtered planet and the sea's water
    /// polygons
    Build {
        /// The pass's filtered planet (sources/osm/<date>/filtered)
        #[arg(long, value_name = "FILTERED .OSM.PBF")]
        osm: PathBuf,
        #[arg(long, value_name = "ZIP")]
        water_polygons: PathBuf,
        /// The store folder
        #[arg(long, value_name = "STORE FOLDER")]
        out: PathBuf,
        /// osmium's index (sparse_mem_array, or sparse_file_array,<file>)
        #[arg(long, value_name = "TYPE", default_value = "sparse_mem_array")]
        index: String,
    },
    /// The store's coverage served as raster tiles (/cov/{z}/{x}/{y}?s=<size>&k=<all|sea|inland>)
    Serve {
        #[arg(long, value_name = "STORE FOLDER")]
        store: PathBuf,
        #[arg(long, default_value_t = 18095)]
        port: u16,
    },
    /// One coverage tile as a PNG on stdout
    Tile {
        #[arg(long, value_name = "STORE FOLDER")]
        store: PathBuf,
        z: u8,
        x: u32,
        y: u32,
        #[arg(long, default_value_t = 512)]
        size: usize,
    },
    /// How big a coverage layer would be: the tiles at a zoom some shore passes through, a sample
    /// of them drawn and compressed
    Stats {
        #[arg(long, value_name = "STORE FOLDER")]
        store: PathBuf,
        #[arg(long)]
        z: u8,
        #[arg(long, default_value_t = 200)]
        sample: usize,
        #[arg(long, default_value_t = 512)]
        size: usize,
    },
    /// water-build's tiles as the water layer's packs in a test root, and a catalog one newer than
    /// --catalog with them. Never the NAS's
    WaterPacks {
        #[arg(long, value_name = "DIR")]
        tiles: PathBuf,
        #[arg(long, value_name = "N.JSON.ZST")]
        catalog: PathBuf,
        #[arg(long, value_name = "DIR")]
        root: PathBuf,
    },
    /// pipeline::water's build as the step runs it, its tiles written as files (<dir>/z/x/y.png)
    WaterBuild {
        #[arg(long, value_name = "PMTILES")]
        basemap: PathBuf,
        /// Only these z5 tiles
        #[arg(long, value_name = "X/Y,…")]
        only: Option<String>,
        /// The lakes filled from Canada's own data drawn too
        #[arg(long, value_name = "F")]
        lakefill: Option<PathBuf>,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// The sea's rings marked in a store made before it recorded them
    MarkSea {
        #[arg(long, value_name = "STORE FOLDER")]
        store: PathBuf,
        #[arg(long, value_name = "ZIP")]
        water_polygons: PathBuf,
    },
}
