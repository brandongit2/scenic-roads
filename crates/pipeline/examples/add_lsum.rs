//! Copies of hidata files with the zoomed-out summaries added (`roadcore::lsum`, as pack(T) writes
//! them since PACK_V 2), content-named like the originals, for testing the summaries' query path
//! on a development root before the re-pack:
//!
//!     cargo run --release -p pipeline --example add_lsum -- <hidata dir> <out dir>
//!
//! Prints `tile logical-name content-name` per file.

use anyhow::{Context, Result};
use roadcore::packs::{Here, PSample, Part, RailInfo};
use roadcore::scenic::ch;
use std::path::Path;
use store::range::MmapFile;
use store::sect::{SectReader, SectWriter};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (src, dst) = (Path::new(args.get(1).context("hidata dir")?), Path::new(args.get(2).context("out dir")?));
    std::fs::create_dir_all(dst)?;
    let mut files: Vec<_> = std::fs::read_dir(src)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "sect")).collect();
    files.sort();
    for p in files {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let tile = name.split('.').next().unwrap().to_string();
        let r = SectReader::open(MmapFile::open(&std::fs::canonicalize(&p)?)?)?;
        let parts: &[Part] = r.cast("parts")?;
        let ps: &[PSample] = r.cast("psamples")?;
        let pch: &[[u8; ch::N]] = r.cast("pch")?;
        let here: &[Here] = r.cast("here")?;
        let ri: &[RailInfo] = if r.section("railinfo").is_some() { r.cast("railinfo")? } else { &[] };
        let ls = roadcore::lsum::build(parts, ps, pch, here, ri);
        let mut meta = r.meta().clone();
        meta["lsum"] = serde_json::json!(roadcore::lsum::LSUM_V);
        let tmp = dst.join(format!("{tile}.tmp"));
        let mut w = SectWriter::create(&tmp, meta)?;
        for s in r.sections() {
            w.add(&s.name, r.slice(&s.name).context("section")?)?;
        }
        w.add_pod("lparts", &ls.lparts)?;
        w.add_pod("lbins", &ls.lbins)?;
        w.add_pod("lrparts", &ls.lrparts)?;
        w.add_pod("lrbins", &ls.lrbins)?;
        w.finish()?;
        let logical = format!("hidata/{tile}");
        let content = store::naming::content_name(&logical, &store::naming::hash16_file(&tmp)?, "sect");
        let out = dst.join(Path::new(&content).file_name().unwrap());
        std::fs::rename(&tmp, &out)?;
        println!("{} {logical} {content}", tile.replacen('-', "/", 2));
    }
    Ok(())
}
