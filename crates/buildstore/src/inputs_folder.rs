//! The inputs folders (docs/inputs.md §7.1): the owner's drop folders, which only the gate and
//! `inputs::inputs::open` read under; a step's `Out::path` refuses a path in one
//! (`in_inputs_folder`).

/// The folders that hold the inputs folders (`inputs/`, and the request–fulfil inputs' folders until
/// they move under it, #137): nothing but the gate and `open` reads under them (docs/inputs.md §7.1).
pub const DROP_ROOTS: [&str; 3] = ["inputs", "translations", "descriptions"];

/// Whether `rel` (a path relative to the NAS project folder) lies in an inputs folder: what the NAS root
/// the steps get refuses (`crate::out::Out::path`).
pub fn in_inputs_folder(rel: &str) -> bool {
    let rel = rel.trim_start_matches("./");
    DROP_ROOTS.iter().any(|d| rel == *d || rel.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_in_an_inputs_folder_only_under_one() {
        assert!(
            in_inputs_folder("inputs/regions/a.toml")
                && in_inputs_folder("translations/x.jsonl")
                && in_inputs_folder("descriptions")
                && !in_inputs_folder("sources/inputs/x/a.0123456789abcdef.toml")
                && !in_inputs_folder("inputsx/a")
        );
    }
}
