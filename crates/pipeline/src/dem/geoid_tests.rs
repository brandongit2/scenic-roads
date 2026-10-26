use super::*;
use crate::fetch::MapFetch;
use crate::inputs::sources::{parse, Source};

/// A geoid source from its `[data]` lines.
fn src(id: &str, data: &str) -> Source {
    let text = format!(
        "name = \"N\"\ncredit = \"C\"\nlicence = \"CC0-1.0\"\nterritory = \"world\"\n[data]\nrole = \"geoid\"\nkind = \"data\"\n{data}"
    );
    let d = parse(text.as_bytes()).unwrap_or_else(|w| panic!("{id}: {w:?}"));
    Source { id: id.into(), data: d.data, keyed: format!("k-{id}"), index: Vec::new() }
}

fn grid(id: &str, url: &str, from: &str, to: &str) -> Source {
    src(id, &format!("format = \"geoid-grid\"\nurl = \"{url}\"\n[data.params]\nfrom = \"{from}\"\nto = \"{to}\""))
}

/// A 4 × 4 float grid of `px`° pixels from `x0`, `y0` (its north-west corner), each `f(col, row)`.
fn tiff(x0: f64, y0: f64, px: f64, f: impl Fn(u32, u32) -> f32) -> Vec<u8> {
    let v: Vec<f32> = (0..16).map(|i| f(i % 4, i / 4)).collect();
    crate::geotiff::write_f32(4, 4, &v, 16, [x0, px, 0.0, y0, 0.0, -px], 4326, None).unwrap()
}

#[test]
fn the_declarations_check() {
    let ok = grid("g", "https://e.org/g.tif", "NAVD88", "ellipsoid-NAD83");
    assert_eq!(ok.data.params["from"], "NAVD88");
    let bad = |data: &str| {
        crate::inputs::sources::parse(format!("name = \"N\"\ncredit = \"C\"\nlicence = \"CC0-1.0\"\nterritory = \"world\"\n[data]\nrole = \"geoid\"\nkind = \"data\"\n{data}").as_bytes()).unwrap_err().join("\n")
    };
    assert!(bad("format = \"geoid-grid\"\nurl = \"https://e.org/g.tif\"\n[data.params]\nfrom = \"A\"\nto = \"A\"")
        .contains("the same"));
    assert!(
        bad("format = \"geoid-grid\"\nurl = \"https://e.org/g.tif\"\n[data.params]\nfrom = \"A\"").contains("required")
    );
    assert!(bad(
        "format = \"geoid-grid\"\nurl = \"https://e.org/g.tif\"\n[data.params]\nfrom = \"A\"\nto = \"B\"\nsign = 1"
    )
    .contains("`params.sign`"));
    assert!(bad(
        "format = \"helmert\"\nurl = \"https://e.org/p\"\n[data.params]\nfrom = \"A\"\nto = \"B\"\nconvention = \"x\""
    )
    .contains("translation_m"));
    assert!(bad("format = \"helmert\"\nurl = \"https://e.org/p\"\n[data.params]\nfrom = \"A\"\nto = \"B\"\nconvention = \"x\"\ntranslation_m = [0, 0, 0]\nrotation_mas = [0, 0, 0]\nscale_ppb = 0").contains("convention"));
}

#[test]
fn a_chain_is_found_the_same_way_each_time_and_names_what_it_reads() {
    let s = vec![
        grid("egm", "https://e.org/egm.tif", "EGM2008", "ellipsoid-WGS84"),
        grid("g18", "https://e.org/g18.tif", "NAVD88", "ellipsoid-NAD83"),
        grid("g12", "https://e.org/g12.tif", "NAVD88", "ellipsoid-NAD83"),
        src("h", "format = \"helmert\"\nurl = \"https://e.org/p\"\n[data.params]\nfrom = \"ellipsoid-WGS84\"\nto = \"ellipsoid-NAD83\"\nconvention = \"coordinate-frame\"\ntranslation_m = [1, -2, -0.5]\nrotation_mas = [0, 0, 0]\nscale_ppb = 0"),
    ];
    assert_eq!(conversion("EGM2008", &s).unwrap(), None);
    let c = conversion("NAVD88", &s).unwrap().unwrap();
    let ends: Vec<(&str, &str)> = c.hops.iter().map(|h| (h.from.as_str(), h.to.as_str())).collect();
    assert_eq!(
        ends,
        [("NAVD88", "ellipsoid-NAD83"), ("ellipsoid-NAD83", "ellipsoid-WGS84"), ("ellipsoid-WGS84", "EGM2008")]
    );
    // The two grids of one link in id order, each walked forwards; the Helmert and EGM2008's back.
    assert!(
        matches!(&c.hops[0].by[..], [(Link::Grid { id: a, .. }, 1), (Link::Grid { id: b, .. }, 1)] if a == "g12" && b == "g18")
    );
    assert_eq!(c.hops[1].by[0].1, -1);
    assert_eq!(c.hops[2].by[0].1, -1);
    // The same in any order of the declarations; a declaration edited is a new digest.
    let mut r = s.clone();
    r.reverse();
    assert_eq!(conversion("NAVD88", &r).unwrap().unwrap(), c);
    let mut e = s.clone();
    e[1].keyed = "k-edited".into();
    assert_ne!(conversion("NAVD88", &e).unwrap().unwrap().digest, c.digest);
    // A source off the chain isn't in its digest.
    let mut more = s.clone();
    more.push(grid("cgg", "https://e.org/cgg.tif", "CGVD2013", "ellipsoid-NAD83"));
    assert_eq!(conversion("NAVD88", &more).unwrap().unwrap().digest, c.digest);
    // No chain: an error naming the reference.
    assert!(format!("{:#}", conversion("TWVD2001", &s).unwrap_err()).contains("TWVD2001"));
}

#[test]
fn a_helmert_changes_heights_by_its_translation_along_the_vertical() {
    // A pure translation: the change is its component along the ellipsoid's normal.
    let dh = helmert_dh(0.0, 0.0, [1.0, 0.0, 0.0], [0.0; 3], 0.0, true);
    assert!((dh - 1.0).abs() < 1e-6, "{dh}");
    let dh = helmert_dh(0.0, 90.0, [0.0, 0.0, -0.5], [0.0; 3], 0.0, true);
    assert!((dh + 0.5).abs() < 1e-6, "{dh}");
    // Rotations move heights by millimetres (the ellipsoid isn't a sphere); a scale of 1 ppb about 6.4 mm.
    assert!(helmert_dh(-100.0, 40.0, [0.0; 3], [25.0, 9.0, 11.0], 0.0, true).abs() < 1e-2);
    let s = helmert_dh(-100.0, 40.0, [0.0; 3], [0.0; 3], 1.0, false);
    assert!((s - 0.00637).abs() < 2e-5, "{s}");
}

#[test]
fn offsets_add_grids_bilinearly_the_first_holding_a_point_serving() {
    // Two regional grids of one link (a: west of 2°, b: east), and EGM2008's (2° pixels) over both.
    let mut m = std::collections::BTreeMap::new();
    m.insert("https://e.org/a.tif".to_string(), Some(tiff(-2.0, 2.0, 1.0, |c, _| 10.0 + c as f32)));
    m.insert("https://e.org/b.tif".to_string(), Some(tiff(2.0, 2.0, 1.0, |_, _| 20.0)));
    m.insert("https://e.org/egm.tif".to_string(), Some(tiff(-2.0, 2.0, 2.0, |_, r| 5.0 + r as f32)));
    let s = vec![
        grid("a", "https://e.org/a.tif", "R", "ell"),
        grid("b", "https://e.org/b.tif", "R", "ell"),
        grid("egm", "https://e.org/egm.tif", "EGM2008", "ell"),
    ];
    let c = conversion("R", &s).unwrap().unwrap();
    let f = MapFetch(m);
    let o = Opts { fetch: &f, store: None, local: None };
    let g = Grids::new(&o);
    // (0, 0): halfway between a's pixel centres −0.5° and 0.5° (11, 12), and egm's rows 1° and −1° (5, 6).
    let off = g.offsets(&c, &[0.0, 3.0, 2.0, 9.0], &[0.0, 1.0, 0.5, 0.0]).unwrap();
    assert!((off[0] - (11.5 - 5.5)).abs() < 1e-9, "{off:?}");
    // East of 2°: b's 20, egm's 5 on its first row's centre.
    assert!((off[1] - (20.0 - 5.0)).abs() < 1e-9, "{off:?}");
    assert!(off[2].is_finite());
    // Outside every grid of a link: none.
    assert!(off[3].is_nan());
}
