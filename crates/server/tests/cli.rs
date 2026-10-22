//! The map server's command line, run as the built program (as crates/pipeline/tests/cli.rs runs
//! every other binary): `--help` prints its usage and exits 0, a flag it doesn't take is refused
//! with its usage (exit 2), and neither starts it: its home (HOME, the working folder) is left
//! empty, no port taken, no NAS reached.

fn run(args: &[&str]) -> (i32, String, String) {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = store::spawn::command(env!("CARGO_BIN_EXE_server"));
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
}
