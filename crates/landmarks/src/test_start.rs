//! `TEST_START` (lib.rs) checked: these unit tests' process is offline from its start.

/// The probe's environment: set, the probe checks this fresh process; unset, it passes.
const PROBE: &str = "SCENIC_TEST_START_PROBE";

/// A fresh process of these unit tests is offline before any test said so (`TEST_START`): this
/// binary again, running only the probe, with SCENIC_FETCH_OFFLINE unset.
#[test]
fn a_unit_test_process_is_offline_from_its_start() {
    let exe = std::env::current_exe().unwrap();
    let out = store::spawn::output(
        store::spawn::command(exe)
            .args(["--exact", "test_start::probe", "--nocapture", "--test-threads=1"])
            .env(PROBE, "1")
            .env_remove("SCENIC_FETCH_OFFLINE"),
    )
    .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && said.contains("1 passed"), "{said}{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn probe() {
    if std::env::var_os(PROBE).is_some() {
        let e = buildstore::fetch::online("https://example.org/x").unwrap_err();
        assert!(format!("{e:#}").contains("no network here"), "{e:#}");
    }
}
