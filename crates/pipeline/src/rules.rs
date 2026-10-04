//! The unit steps' rules that depend on where a road is (docs/plan.md §5, By location), each with
//! the areas it applies to and a version. A unit's key names the versions of the rules whose areas
//! meet its reach, so a changed rule reruns only the units it applies to: change the rule, bump its
//! version here (the code of each rule points back to this table).
//!
//! Areas are the boxes the rules' code tests (w, s, e, n, degrees). A rule that applies everywhere
//! its more particular neighbours don't (FABDEM, the European road numbers) is listed with the
//! world: changing it reruns every unit, as it may change any of them.

/// One rule: its name, version and areas (none: the whole world).
pub struct Rule {
    pub name: &'static str,
    pub version: u32,
    pub areas: &'static [[f64; 4]],
}

const WORLD: &[[f64; 4]] = &[];
const NORTH_AMERICA: &[[f64; 4]] = &[[-180.0, -90.0, -40.0, 90.0]];
const JAPAN: &[[f64; 4]] = &[[122.5, 20.0, 154.0, 46.5]];
const TAIWAN: &[[f64; 4]] = &[[118.0, 21.5, 122.5, 26.6]];

pub const RULES: &[Rule] = &[
    // dem/sample.py: the DEM sources and their order.
    Rule { name: "dem-north-america", version: 1, areas: NORTH_AMERICA },
    Rule { name: "dem-japan", version: 1, areas: JAPAN },
    Rule { name: "dem-taiwan", version: 1, areas: TAIWAN },
    Rule { name: "dem-fabdem", version: 1, areas: WORLD },
    // extract: densification (8 m in North America and Japan, 15 m elsewhere).
    Rule { name: "spacing", version: 1, areas: WORLD },
    // extract::network_code: road numbers by area.
    Rule { name: "networks-north-america", version: 1, areas: NORTH_AMERICA },
    Rule { name: "networks-hong-kong", version: 1, areas: &[[113.8, 22.1, 114.5, 22.6]] },
    Rule { name: "networks-singapore", version: 1, areas: &[[103.5, 1.1, 104.2, 1.5]] },
    Rule { name: "networks-japan", version: 1, areas: JAPAN },
    Rule { name: "networks-taiwan", version: 1, areas: TAIWAN },
    Rule { name: "networks-andorra", version: 1, areas: &[[1.40, 42.42, 1.79, 42.66]] },
    // France's and Monaco's, Spain's, Portugal's, Ireland's and Britain's numbers, and E-roads: tested
    // by their form and loose boxes, after the areas above.
    Rule { name: "networks-europe", version: 1, areas: WORLD },
];

/// The current version of the rule `name` (0 when there's no such rule).
pub fn version(name: &str) -> u32 {
    RULES.iter().find(|r| r.name == name).map_or(0, |r| r.version)
}

/// The DEM rules, in the order the units' kept samples record their versions (`unit`'s DEM cache).
pub const DEM_RULES: [&str; 4] = ["dem-north-america", "dem-japan", "dem-taiwan", "dem-fabdem"];

/// The DEM rules a sample depends on, by its source (dem/sample.py's `SRC_*`) and place (E7): the
/// rule that chose its source, and for FABDEM (the fallback) also the rule of the area whose own
/// DEMs it stood in for. Unknown sources depend on every rule. Indexes into `DEM_RULES`.
pub fn dem_rules_of(src: u8, lon: i32, lat: i32) -> Vec<usize> {
    let meets = |a: &[[f64; 4]]| {
        let (x, y) = (lon as f64 * 1e-7, lat as f64 * 1e-7);
        a.iter().any(|b| b[0] <= x && x <= b[2] && b[1] <= y && y <= b[3])
    };
    match src {
        1..=3 => vec![0],
        5..=7 => vec![1],
        8 => vec![2],
        4 => {
            let mut v = vec![3];
            if meets(NORTH_AMERICA) {
                v.push(0);
            }
            if meets(JAPAN) {
                v.push(1);
            }
            if meets(TAIWAN) {
                v.push(2);
            }
            v
        }
        _ => vec![0, 1, 2, 3],
    }
}

/// Whether the box w, s, e, n (E7) meets Taiwan, where the MOI DTM (`inputs/moi-dtm/`, put there by
/// hand) is the DEM when it's there: its files' digest enters those units' keys.
pub fn meets_taiwan(b: [i32; 4]) -> bool {
    let a = TAIWAN[0];
    let (w, s, e, n) = (b[0] as f64 * 1e-7, b[1] as f64 * 1e-7, b[2] as f64 * 1e-7, b[3] as f64 * 1e-7);
    a[0] <= e && a[2] >= w && a[1] <= n && a[3] >= s
}

/// The versions of the rules whose areas meet the box w, s, e, n (E7), as one string for a key.
pub fn versions_meeting(b: [i32; 4]) -> String {
    let (w, s, e, n) = (b[0] as f64 * 1e-7, b[1] as f64 * 1e-7, b[2] as f64 * 1e-7, b[3] as f64 * 1e-7);
    RULES
        .iter()
        .filter(|r| r.areas.is_empty() || r.areas.iter().any(|a| a[0] <= e && a[2] >= w && a[1] <= n && a[3] >= s))
        .map(|r| format!("{}={}", r.name, r.version))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e7(w: f64, s: f64, e: f64, n: f64) -> [i32; 4] {
        [(w * 1e7) as i32, (s * 1e7) as i32, (e * 1e7) as i32, (n * 1e7) as i32]
    }

    #[test]
    fn rules_by_area() {
        let tokyo = versions_meeting(e7(139.0, 35.0, 140.0, 36.0));
        assert!(tokyo.contains("dem-japan=1") && tokyo.contains("networks-japan=1") && !tokyo.contains("north-america"));
        let quebec = versions_meeting(e7(-72.0, 46.0, -71.0, 47.0));
        assert!(quebec.contains("dem-north-america=1") && !quebec.contains("japan"));
        // The world's rules everywhere.
        assert!(tokyo.contains("dem-fabdem=1") && quebec.contains("spacing=1"));
    }
}
