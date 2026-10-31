//! The inputs' identities (docs/readiness.md §4): for each `Read::Input` a step declares
//! (crate::agent::steps), what a target's key names of it, as lines (`in <name> <line>`). Each is
//! the function the run's own reads are worked out by, so the key and the run can't name the input
//! differently. None while it can't be worked out now: the target's key is unknown and it waits.

use super::build::KeyCtx;
use crate::legacy::TileId;

/// The spacing the extract reads (crate::dem::spacing::key_line); None while it isn't known.
pub fn spacing(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![crate::dem::spacing::key_line(x.inputs)?])
}

/// The heritage registers meeting the coverage, each by its keyed digest
/// (crate::heritage::parse_registers, from the records' accepted version in `inputs` "heritage");
/// a plan given no such line names the registers' accepted index whole. None while they have no
/// accepted version or it can't be read ("?").
pub fn registers(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let index = x.m.get(&crate::inputs::logical(crate::inputs::heritage::UNIT))?;
    match x.inputs.get("heritage").map(String::as_str) {
        Some("?") => None,
        Some(line) => {
            let cover = crate::heritage::cover_boxes(x.cov);
            Some(
                crate::heritage::parse_registers(line)
                    .into_iter()
                    .filter(|(_, _, b)| b.is_some_and(|b| crate::heritage::meets(&cover, b)))
                    .map(|(id, keyed, _)| format!("{id} {keyed}"))
                    .collect(),
            )
        }
        None => Some(vec![format!("index {index}")]),
    }
}

/// The World Heritage dots the last marks run wrote (crate::markconv::HERITAGE_DOTS), by content;
/// None before there are any.
pub fn heritage_dots(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![x.m.get(crate::markconv::HERITAGE_DOTS)?.clone()])
}

/// The 3D buildings' declared release (`inputs` "bld-key": crate::bld::sources::declared), "-"
/// without one.
pub fn bld_key(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![x.inputs.get("bld-key").map_or("-", String::as_str).to_string()])
}

/// Whether the release's sources are downloaded (`inputs` "bld-release",
/// crate::bld::sources::digests); None when that can't be read now, or the sources aren't declared.
pub fn bld_sources(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    match x.inputs.get("bld-release").map(String::as_str) {
        None | Some("?") => None,
        Some(_) if !x.inputs.contains_key("bld-key") => None,
        Some(r) => Some(vec![if r.is_empty() { "none" } else { "downloaded" }.to_string()]),
    }
}

/// What a terrain z6 tile's run reads of the coverage (task #832, `build::piece_coverage`: the
/// tiles of its levels it makes); a terrain-deep z10 tile's, that of each z6 tile its base is made
/// of (crate::terrain_deep::base_want).
pub fn coverage_piece(t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    let u = TileId::parse(t)?;
    let pieces: std::collections::BTreeSet<(u32, u32)> = match u.z {
        6 => [(u.x, u.y)].into(),
        z if z == crate::terrain_deep::PACK_Z => {
            crate::terrain_deep::base_want((u.x, u.y)).iter().map(|&(z, x, y)| (x >> (z - 6), y >> (z - 6))).collect()
        }
        _ => return None,
    };
    Some(pieces.into_iter().map(|p| format!("6/{}/{} {}", p.0, p.1, super::build::piece_coverage(x.cov, p))).collect())
}

/// The pass the target is of (its planet's date).
pub fn pass(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![x.date.to_string()])
}

/// The pass's set `kind` (crate::osmpass::set_name) by content; None before pass-sets makes it.
fn set(kind: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![x.m.get(&crate::osmpass::set_name(x.date, kind))?.clone()])
}

/// One `Read::Input` per pass set a step reads, `set_<kind>`.
macro_rules! sets {
    ($($f:ident $kind:literal),* $(,)?) => {$(
        #[doc = concat!("The pass's `", $kind, "` set by content (`set`).")]
        pub fn $f(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
            set($kind, x)
        }
    )*};
}
sets!(set_outlines "outlines");

/// The previous pass's outlines the outlines step may keep a country from
/// (crate::osmpass::previous_outlines): its date and content, "-" with none.
pub fn previous_outlines(_t: &str, x: &KeyCtx) -> Option<Vec<String>> {
    Some(vec![crate::osmpass::previous_outlines(x.m, x.date).map_or("-".to_string(), |(d, c)| format!("{d} {c}"))])
}
