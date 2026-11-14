//! The uploads not yet verified on the NAS, by content name (docs/storage.md, Unverified uploads):
//! what an `Out` keeps, a hand-off carries and the records hold. Only the names are read: GC's
//! duties check each on the NAS by its size (agentcore::agent::gc::verify_pending), GC's queue holds
//! them from its chunks, and the merge's checks name them.
//!
//! Read in either shape: an object, content name → anything (apps before task #1002 wrote each
//! upload's SHA-256 there, which nothing read: an old member's hand-off, a lead's records saved
//! before, `state/build/pending.json` before the pool), or a list of names. Written as an object of
//! empty strings, the shape every app that can run today requires (its reader a map of strings, the
//! records' field required: an app before task #1002, back to the rollback floor). Planned: written
//! as a list, once no app that requires the object can run (the rollback floor at task #1002's
//! commit or later).

use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

/// Uploads not yet verified on the NAS, by content name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pending(BTreeSet<String>);

impl std::ops::Deref for Pending {
    type Target = BTreeSet<String>;
    fn deref(&self) -> &BTreeSet<String> {
        &self.0
    }
}

impl std::ops::DerefMut for Pending {
    fn deref_mut(&mut self) -> &mut BTreeSet<String> {
        &mut self.0
    }
}

impl FromIterator<String> for Pending {
    fn from_iter<I: IntoIterator<Item = String>>(i: I) -> Self {
        Pending(i.into_iter().collect())
    }
}

impl IntoIterator for Pending {
    type Item = String;
    type IntoIter = std::collections::btree_set::IntoIter<String>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Pending {
    type Item = &'a String;
    type IntoIter = std::collections::btree_set::Iter<'a, String>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<const N: usize> From<[String; N]> for Pending {
    fn from(a: [String; N]) -> Self {
        Pending(a.into())
    }
}

impl Serialize for Pending {
    /// An object of empty strings: what an app before task #1002 reads (module docs).
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|c| (c, "")))
    }
}

impl<'de> Deserialize<'de> for Pending {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Names;
        impl<'de> Visitor<'de> for Names {
            type Value = Pending;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("content names: an object keyed by them, or a list")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Pending, A::Error> {
                let mut p = BTreeSet::new();
                while let Some((c, IgnoredAny)) = m.next_entry::<String, IgnoredAny>()? {
                    p.insert(c);
                }
                Ok(Pending(p))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Pending, A::Error> {
                let mut p = BTreeSet::new();
                while let Some(c) = s.next_element::<String>()? {
                    p.insert(c);
                }
                Ok(Pending(p))
            }
        }
        d.deserialize_any(Names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Task #1002: what apps before it wrote (each upload's SHA-256) reads as its names; a list
    /// (the planned shape) too; what this app writes is an object of empty strings, which their
    /// reader (a map of strings) takes, names alike.
    #[test]
    fn old_and_new_shapes_read_and_the_old_reader_takes_what_is_written() {
        let old = r#"{"a.0123456789abcdef.pack":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08","b.0123456789abcdef.json":"aa"}"#;
        let p: Pending = serde_json::from_str(old).unwrap();
        let names: Vec<&str> = p.iter().map(String::as_str).collect();
        assert_eq!(names, ["a.0123456789abcdef.pack", "b.0123456789abcdef.json"]);
        let listed: Pending = serde_json::from_str(r#"["b.0123456789abcdef.json","a.0123456789abcdef.pack"]"#).unwrap();
        assert_eq!(listed, p);
        assert_eq!(serde_json::from_str::<Pending>("{}").unwrap(), Pending::default());
        assert!(serde_json::from_str::<Pending>(r#""a""#).is_err(), "neither shape");

        let written = serde_json::to_string(&p).unwrap();
        assert_eq!(written, r#"{"a.0123456789abcdef.pack":"","b.0123456789abcdef.json":""}"#);
        // (The reader of every app back to the rollback floor: a map of strings.)
        let theirs: BTreeMap<String, String> = serde_json::from_str(&written).unwrap();
        assert_eq!(theirs.keys().collect::<Vec<_>>(), p.iter().collect::<Vec<_>>());
        assert_eq!(serde_json::from_str::<Pending>(&written).unwrap(), p, "read back as written");
    }
}
