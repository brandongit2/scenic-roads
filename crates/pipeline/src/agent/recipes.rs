//! A region as the build reads it: its `id`, `name` and `outline`, from the regions' accepted
//! version (crate::inputs::regions::read; docs/inputs.md §6.1.3, the region file).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub outline: Vec<String>,
}

/// A recipe's outline entries, hashed: what a catalog's `--ready` names a region built with
/// (`<id>=<digest>`), so one redrawn since isn't recorded as built with its new outline.
pub fn outline_digest(outline: &[String]) -> String {
    store::naming::hash16(outline.join("\n").as_bytes())
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !id.starts_with('-')
}
