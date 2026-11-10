//! The map server's command line, run as the built program (as crates/pipeline/tests/cli.rs runs
//! every other binary): `--help` prints its usage and exits 0, a flag it doesn't take is refused
//! with its usage (exit 2), and neither starts it: its home (HOME, the working folder) is left
//! empty, no port taken, no NAS reached. A server on demand whose port another holds ends so too.

fn run(args: &[&str]) -> (i32, String, String) {
    let home = tempfile::tempdir().unwrap();
    // (The program as cargo and nextest name it when the test runs: never `env!`'s compiled-in path,
    // tools/dev/testing.md.)
    let mut cmd = store::spawn::command(std::env::var("CARGO_BIN_EXE_server").expect("CARGO_BIN_EXE_server"));
    cmd.args(args).current_dir(home.path()).env("HOME", home.path());
    let o = store::spawn::output(&mut cmd).unwrap();
    let left: Vec<_> = std::fs::read_dir(home.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(left.is_empty(), "server {args:?} wrote {left:?}");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn it_helps_and_refuses_without_starting() {
    for help in ["--help", "-h"] {
        let (code, out, err) = run(&[help]);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("Usage:") && out.contains("--listen"), "{out}");
    }
    let (code, out, err) = run(&["--nwo"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("--nwo") && err.contains("Usage:"), "{err}");
    assert!(out.is_empty(), "{out}");
    let (code, _, err) = run(&["--port"]);
    assert_eq!(code, 2, "{err}");
    // (On demand it answers this Mac's loopback alone: no address of its own to listen on.)
    let (code, _, err) = run(&["--on-demand", "--listen", "0.0.0.0"]);
    assert!(code == 2 && err.contains("--on-demand"), "{err}");
}

/// A server on demand (docs/roles.md §8.3, #882 step 7a) takes this Mac's loopback first of all:
/// with another server holding it (a second click, the installed one), it ends at once, before it
/// reads a map or touches a mirror (its home left empty).
#[test]
fn a_server_on_demand_ends_at_once_when_its_port_is_held() {
    let held = store::spawn::listen("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port().to_string();
    let (code, _, err) = run(&["--on-demand", "--port", &port]);
    assert!(code == 1 && err.contains("this Mac's map already runs"), "{code}: {err}");
}

/// Started with malloc's large cache (as launchd starts it), it starts itself again in its place
/// without it (store::spawn::without_large_cache, #1092; the exec itself is store's test): once,
/// never again and again, its arguments kept, then runs as it would have (here: ends at once, its
/// port held), with no warning that it couldn't.
#[test]
fn a_server_started_with_the_large_cache_runs_once_without_it() {
    let held = store::spawn::listen("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port().to_string();
    let home = tempfile::tempdir().unwrap();
    let mut cmd = store::spawn::command(env!("CARGO_BIN_EXE_server"));
    cmd.args(["--on-demand", "--port", &port])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env_remove(store::spawn::NO_LARGE_CACHE.0);
    let o = store::spawn::output(&mut cmd).unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert_eq!(err.matches("this Mac's map already runs").count(), 1, "{err}");
    assert!(!err.contains("WARNING"), "{err}");
}
