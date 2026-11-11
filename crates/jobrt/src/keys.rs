//! The project's keys file, `inputs/keys.env` (`KEY=value` lines, a value quoted or not; not an
//! input, docs/inputs.md §2), as the NAS's own services' readers take their keys from it: the
//! Synology's SNMPv3 reader (crate::budget::snmp: `NAS_SNMP_USER`, `NAS_SNMP_AUTH`) and the agent's
//! DSM web API client (agentcore's `agent::dsm`: `NAS_DSM_USER`, `NAS_DSM_PASS`). A value is never
//! logged or printed by them.

/// The keys' file, under the project folder.
pub const KEYS: &str = "inputs/keys.env";

/// The keys' file's text under the project folder `root`.
pub fn read(root: &store::nasfs::NasRoot) -> std::io::Result<String> {
    root.join(KEYS).read_to_string()
}

/// The value of the key `k` in a keys file's text `s` (its first line `k=…`, the value trimmed and
/// unquoted); None when it's missing or empty.
pub fn value(s: &str, k: &str) -> Option<String> {
    s.lines().find_map(|l| {
        let (a, b) = l.trim().split_once('=')?;
        (a.trim() == k).then(|| b.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_found_trimmed_and_unquoted() {
        let s = "OTHER=1\n K = \"v w\" \nEMPTY=\nQ='x'\n";
        assert_eq!(value(s, "K").as_deref(), Some("v w"));
        assert_eq!(value(s, "Q").as_deref(), Some("x"));
        assert_eq!(value(s, "EMPTY"), None);
        assert_eq!(value(s, "MISSING"), None);
    }
}
