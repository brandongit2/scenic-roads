//! Packing (docs/pool.md §7.2, task #239), behind the switch `state/pool/packing` (`SWITCH`; off
//! while missing or `off` in it: one job an ask, as before). The lead's coordinator then lets a
//! member hold several leases, an ask granted as many jobs as it wants that fit its room together
//! (crate::coord's `pack`). It acts only in the pool, with every step to every member.
//!
//! The switch's file may cap the jobs a member holds at once: a line `cap <n>` for every member,
//! `cap <worker> <n>` for one (by the name it asks with). No cap by default.

use crate::coord::Packing;
use std::path::Path;

/// The switch.
pub const SWITCH: &str = "state/pool/packing";

/// Packing as the switch at `root` says (None in it: off); None when it can't be read now.
pub fn read(root: &Path) -> Option<Option<Packing>> {
    match std::fs::read_to_string(root.join(SWITCH)) {
        Ok(s) => Some(parse(&s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
    }
}

/// The switch's text: `off` alone, off; else on, with its `cap` lines (one not understood ignored).
pub fn parse(s: &str) -> Option<Packing> {
    if s.trim() == "off" {
        return None;
    }
    let mut p = Packing::default();
    for l in s.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.as_slice() {
            ["cap", n] => p.cap = n.parse().ok().or(p.cap),
            ["cap", who, n] => {
                if let Ok(n) = n.parse() {
                    p.caps.insert(who.to_string(), n);
                }
            }
            _ => {}
        }
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_says_on_off_and_its_caps() {
        assert_eq!(parse("off\n"), None);
        assert_eq!(parse(""), Some(Packing::default()));
        let p = parse("on\ncap 3\ncap m1 1\n").unwrap();
        assert_eq!((p.cap_of("m1"), p.cap_of("mini")), (Some(1), Some(3)));
        assert_eq!(parse("on\n").unwrap().cap_of("m1"), None);
    }
}
