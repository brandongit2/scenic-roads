//! The languages spoken where, from a pass's outlines and a languages table (the languages input's
//! `territories.tsv`; `-`: the table compiled in): builds the raster (`names::spoken`), times it,
//! prints what it gives at known places, and the hash its content name would have (as
//! `global/spoken.<hash>.bin`).
//!
//! usage: cargo run --release -p pipeline --example spoken -- <outlines .sect> <territories.tsv | -> [<raster out>]

use std::time::Instant;

const PLACES: &[(&str, f64, f64)] = &[
    ("Montréal", -73.57, 45.50),
    ("Moncton", -64.78, 46.09),
    ("Toronto", -79.38, 43.65),
    ("Iqaluit", -68.52, 63.75),
    ("Boston", -71.06, 42.36),
    ("Barcelona", 2.17, 41.39),
    ("Palma", 2.65, 39.57),
    ("Valencia", -0.38, 39.47),
    ("Santiago de Compostela", -8.54, 42.88),
    ("Bilbao", -2.93, 43.26),
    ("Pamplona", -1.64, 42.81),
    ("Madrid", -3.70, 40.42),
    ("Lisbon", -9.14, 38.72),
    ("Funchal", -16.91, 32.65),
    ("Ponta Delgada", -25.67, 37.74),
    ("Andorra la Vella", 1.52, 42.51),
    ("Gibraltar", -5.35, 36.14),
    ("Quimper", -4.10, 48.00),
    ("Paris", 2.35, 48.86),
    ("Ajaccio", 8.74, 41.92),
    ("Strasbourg", 7.75, 48.58),
    ("Geneva", 6.14, 46.20),
    ("Brussels", 4.35, 50.85),
    ("Cardiff", -3.18, 51.48),
    ("Inverness", -4.22, 57.48),
    ("Belfast", -5.93, 54.60),
    ("Galway", -9.05, 53.27),
    ("Douglas (Man)", -4.48, 54.15),
    ("St Helier (Jersey)", -2.11, 49.19),
    ("Tokyo", 139.69, 35.69),
    ("Naha", 127.68, 26.21),
    ("Taipei", 121.52, 25.05),
    ("Kinmen", 118.32, 24.44),
    ("Hong Kong", 114.17, 22.30),
    ("Macau", 113.54, 22.19),
    ("Shenzhen", 114.06, 22.54),
    ("Singapore", 103.85, 1.29),
    ("Helsinki", 24.94, 60.17),
    ("Mariehamn (Åland)", 19.94, 60.10),
    ("Reykjavík", -21.94, 64.15),
    ("Mid-Atlantic", -40.0, 40.0),
];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: spoken <outlines> <territories.tsv | -> [<raster out>]";
    let path = args.get(1).expect(usage);
    let table = match args.get(2).expect(usage).as_str() {
        "-" => names::Table::embedded().clone(),
        p => names::Table::parse(&std::fs::read_to_string(p)?),
    };
    let t0 = Instant::now();
    let o = pipeline::outlines::Outlines::open(std::path::Path::new(path))?;
    eprintln!("{} outlines' records and names read in {:.1?}", o.recs.len(), t0.elapsed());
    let areas = pipeline::outlines::spoken_areas(&table, &o.recs, |i| o.string(i).to_owned(), |r| o.simple_polygons(r))?;
    let points: usize = areas.iter().map(|a| a.polygons.iter().flatten().map(Vec::len).sum::<usize>()).sum();
    let read = t0.elapsed();
    let t1 = Instant::now();
    let s = names::Spoken::build(&table, areas);
    let bytes = s.to_bytes();
    eprintln!("content hash {}", store::naming::hash16(&bytes));
    eprintln!("{} regions, {points} points read in {read:.1?}; raster in {:.1?}, {:.1} MB", s.regions().count(), t1.elapsed(), bytes.len() as f64 / 1e6);
    println!("| Place | Region | Languages |\n|---|---|---|");
    for (name, lon, lat) in PLACES {
        let langs: Vec<&str> = s.langs_at(*lon, *lat).iter().map(|l| l.as_str()).collect();
        println!("| {name} | {} | {} |", s.code_at(*lon, *lat), langs.join(", "));
    }
    if let Some(out) = args.get(3) {
        std::fs::write(out, &bytes)?;
    }
    Ok(())
}
