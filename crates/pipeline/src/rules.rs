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
