//! A thing's own English and the languages OSM gives its name, from its tags (docs/plan.md §7,
//! step 1): what the vector tiles and the build's records carry about one thing.
//!
//! Its own English, in order:
//! - the English its record or tile names (`name:en`, `name_en`, our `en`: OSM's, a heritage
//!   register's or UNESCO's, its Wikipedia article's title, as the build gave it);
//! - its romanised name (`name:ja-Latn`, `name:ja_rm`, `name:zh-Latn-pinyin`, …);
//! - its kana reading romanised by rule (`name:ja-Hira`, `name:ja_kana`, `name:ja-Kana`; our
//!   `kana`), Hepburn without macrons ([`crate::romaji`]).
//!
//! The languages OSM gives its name: each `name:<language>` whose value is the name itself (a
//! `name:br` equal to `name` makes it Breton), and our tiles' `l` (a comma-separated list the build
//! wrote from the same tags). A transliteration's key (`name:ja-Latn`) names no language of the
//! name.

use crate::romaji::hepburn;
use crate::spoken::Lang;
use std::borrow::Cow;

/// Keys of kana readings.
pub const KANA_KEYS: [&str; 4] = ["kana", "name:ja-Hira", "name:ja_kana", "name:ja-Kana"];

/// Script or region subtags after a language that still name the name's language (`name:zh-Hant`
/// equal to `name` is Chinese); any other (`-Latn`, `_rm`, `_kana`) is a transliteration.
const SAME_LANGUAGE: [&str; 7] = ["Hant", "Hans", "TW", "HK", "CN", "MO", "SG"];

/// Whether a `name:…` key is a romanisation of the name (`name:ja-Latn`, `name:ja_rm`,
/// `name:zh-Latn-pinyin`), and how much it's preferred (lower first).
fn romanised(key: &str) -> Option<u8> {
    let rest = key.strip_prefix("name:")?;
    if rest.ends_with("_rm") {
        return Some(1);
    }
    let mut parts = rest.split('-');
    let _lang = parts.next()?;
    match (parts.next(), parts.next()) {
        (Some("Latn"), None) => Some(0),
        (Some("Latn"), Some(_)) => Some(2),
        _ => None,
    }
}

/// The thing's own English: from `en_keys` (the first present, in that order), else a romanised
/// name, else a kana reading by rule. `tags` are the thing's (key, value) pairs.
pub fn own_english<'a>(tags: &[(&'a str, &'a str)], en_keys: &[&str]) -> Option<Cow<'a, str>> {
    let get = |k: &str| tags.iter().find(|(key, v)| *key == k && !v.trim().is_empty()).map(|(_, v)| *v);
    if let Some(v) = en_keys.iter().find_map(|k| get(k)) {
        return Some(Cow::Borrowed(v));
    }
    if let Some((_, v)) = tags.iter().filter(|(_, v)| !v.trim().is_empty()).filter_map(|(k, v)| romanised(k).map(|r| (r, *v))).min_by_key(|(r, _)| *r) {
        return Some(Cow::Borrowed(v));
    }
    KANA_KEYS.iter().find_map(|k| get(k)).and_then(hepburn).map(Cow::Owned)
}

/// The languages OSM gives `name`, in the order the tags come: the `name:<language>` keys whose
/// value is the name, and the languages listed in an `l` tag.
pub fn osm_langs(name: &str, tags: &[(&str, &str)]) -> Vec<Lang> {
    let mut out = Vec::new();
    for (k, v) in tags {
        let l = if *k == "l" {
            for l in v.split(',').filter_map(|x| Lang::parse(x.trim())) {
                if !out.contains(&l) {
                    out.push(l);
                }
            }
            continue;
        } else if let Some(rest) = k.strip_prefix("name:") {
            if *v != name {
                continue;
            }
            let mut parts = rest.split(['-', '_']);
            let base = parts.next().unwrap_or_default();
            if !parts.all(|p| SAME_LANGUAGE.contains(&p)) {
                continue;
            }
            Lang::parse(base)
        } else {
            None
        };
        if let Some(l) = l.filter(|l| !out.contains(l)) {
            out.push(l);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[Lang]) -> Vec<&str> {
        v.iter().map(Lang::as_str).collect()
    }

    #[test]
    fn own() {
        let en = ["name:en", "name_en"];
        assert_eq!(own_english(&[("name", "東京"), ("name:en", "Tokyo"), ("name:ja-Latn", "Tōkyō")], &en).as_deref(), Some("Tokyo"));
        assert_eq!(own_english(&[("name", "東京"), ("name:ja_rm", "Tōkyō rm"), ("name:ja-Latn", "Tōkyō")], &en).as_deref(), Some("Tōkyō"));
        assert_eq!(own_english(&[("name", "中山"), ("name:zh-Latn-pinyin", "Zhōngshān")], &en).as_deref(), Some("Zhōngshān"));
        assert_eq!(own_english(&[("name", "松島"), ("name:ja-Hira", "まつしま")], &en).as_deref(), Some("Matsushima"));
        assert_eq!(own_english(&[("name", "松島"), ("kana", "マツシマ")], &en).as_deref(), Some("Matsushima"));
        assert_eq!(own_english(&[("name", "松島"), ("name:ja-Hira", "松しま")], &en), None);
        assert_eq!(own_english(&[("name", "Lac"), ("name:en", " "), ("name_en", "Lake")], &en).as_deref(), Some("Lake"));
        assert_eq!(own_english(&[("name", "Lac"), ("name:fr", "Lac")], &en), None);
        assert_eq!(own_english(&[("n", "Lac"), ("en", "Lake")], &["en"]).as_deref(), Some("Lake"));
    }

    #[test]
    fn languages() {
        assert_eq!(strs(&osm_langs("Kêr", &[("name", "Kêr"), ("name:br", "Kêr"), ("name:fr", "Ker")])), ["br"]);
        assert_eq!(strs(&osm_langs("中山", &[("name:zh-Hant", "中山"), ("name:ja", "中山"), ("name:ja-Latn", "中山"), ("name:zh", "中山")])), ["zh", "ja"]);
        assert_eq!(strs(&osm_langs("Tokyo", &[("name:ja_rm", "Tokyo"), ("name:ja-Latn", "Tokyo")])), Vec::<&str>::new());
        assert_eq!(strs(&osm_langs("A", &[("l", "fr, br"), ("name:fr", "A")])), ["fr", "br"]);
        assert_eq!(strs(&osm_langs("A", &[("name:", "A"), ("name:x", "A"), ("name:latin", "A")])), Vec::<&str>::new());
    }
}
