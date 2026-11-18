//! A tile of any zoom (`TileId`): its key (roadcore::archive::tile_key) and its two notations,
//! "6/32/21" (targets, catalogue keys, the command lines) and "6-32-21" (file and logical names).

use roadcore::archive::{tile_key, tile_zxy, valid_tile};
use roadcore::geo::merc;

/// A tile of any zoom: z/x/y (the grid's z3, z6 and z8 tiles, or a map tile).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// The tile of key `key` (roadcore::archive::tile_key).
    pub fn of_key(key: u64) -> TileId {
        let (z, x, y) = tile_zxy(key);
        TileId { z, x, y }
    }

    pub fn key(&self) -> u64 {
        tile_key(self.z, self.x, self.y)
    }

    /// "6/32/21".
    pub fn slash(&self) -> String {
        format!("{}/{}/{}", self.z, self.x, self.y)
    }

    /// "6-32-21", for file names.
    pub fn dash(&self) -> String {
        format!("{}-{}-{}", self.z, self.x, self.y)
    }

    pub fn of_point(z: u8, p: [i32; 2]) -> TileId {
        let (x, y) = merc(p[0] as f64 * roadcore::E7, p[1] as f64 * roadcore::E7);
        let n = (1u64 << z) as f64;
        let c = |v: f64| ((v * n).floor().max(0.0) as u32).min((1u32 << z) - 1);
        TileId { z, x: c(x), y: c(y) }
    }

    /// The tile `slash` or `dash` wrote, and nothing else: three numbers (no sign, no leading
    /// zero) with one separator throughout, nothing after them, and a tile a key can hold
    /// (roadcore::archive::valid_tile: z at most 29, x and y within its grid).
    pub fn parse(s: &str) -> Option<TileId> {
        let sep = if s.contains('/') { '/' } else { '-' };
        let mut it = s.split(sep).map(|t| {
            let digits = !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && (t == "0" || !t.starts_with('0'));
            digits.then(|| t.parse::<u32>().ok()).flatten()
        });
        let (z, x, y) = (it.next()??, it.next()??, it.next()??);
        let z = u8::try_from(z).ok()?;
        (it.next().is_none() && valid_tile(z, x, y)).then_some(TileId { z, x, y })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notations_round_trip() {
        for t in [TileId { z: 0, x: 0, y: 0 }, TileId { z: 6, x: 32, y: 21 }, TileId { z: 14, x: 16383, y: 10 }] {
            assert_eq!(TileId::parse(&t.slash()), Some(t));
            assert_eq!(TileId::parse(&t.dash()), Some(t));
            assert_eq!(TileId::of_key(t.key()), t);
        }
        assert_eq!(TileId { z: 6, x: 32, y: 21 }.slash(), "6/32/21");
        assert_eq!(TileId { z: 6, x: 32, y: 21 }.dash(), "6-32-21");
    }

    #[test]
    fn parse_is_strict() {
        for s in [
            "",
            "6",
            "6/32",
            "6/32/21/",
            "6/32/21/0",
            "6/32-21",
            "6-32/21",
            "6/32/21 ",
            " 6/32/21",
            "6/+32/21",
            "06/32/21",
            "6/032/21",
            "6/64/0",
            "6/0/64",
            "30/0/0",
            "262/0/0",
            "6/a/21",
            "6//21",
            "6/32/21.pack",
        ] {
            assert_eq!(TileId::parse(s), None, "{s:?}");
        }
        assert_eq!(TileId::parse("6/0/63"), Some(TileId { z: 6, x: 0, y: 63 }));
        assert_eq!(TileId::parse("29/536870911/0"), Some(TileId { z: 29, x: (1 << 29) - 1, y: 0 }));
    }
}
