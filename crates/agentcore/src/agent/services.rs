//! The services a Mac runs beside the app (docs/macs.md, Services: the dev tools app and Caddy on
//! the Mac mini), kept at the version its agent runs. They're set up once by hand
//! (`services.zsh setup`, the app's `services/`), which leaves a `setup` file for each in the
//! app's home's `services/<service>/`; after that, once this Mac's port answers (its coordinator,
//! broker or viewer listening: Caddy's /build routes are theirs), the agent runs its version's
//! `services/services.zsh update` on a thread of its own, unless `services/version` (done) or
//! `services/failed` (tried) names that version already. Nothing on a Mac with none set up, nor
//! from a program not run from an installed version (development, tests).

use std::path::{Path, PathBuf};

/// The services' folder under the app's home.
pub const DIR: &str = "services";

/// The script `update` runs, in version `v`'s folder under `home` (the app's home), when its
/// services are set up and neither done nor tried at `v`.
pub fn due(home: &Path, v: &str) -> Option<PathBuf> {
    let d = home.join(DIR);
    let set_up = std::fs::read_dir(&d).ok()?.flatten().any(|e| e.path().join("setup").is_file());
    let at = |f: &str| std::fs::read_to_string(d.join(f)).is_ok_and(|s| s.trim() == v);
    (set_up && !at("version") && !at("failed")).then(|| home.join("app").join(v).join(DIR).join("services.zsh"))
}

/// This Mac's port answers: the services brought to this agent's version, once per process (its
/// output to the agent's log).
pub fn serving(agent_home: &Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if buildstore::out::testing() {
        return;
    }
    let Some(home) = agent_home.parent().map(Path::to_path_buf) else { return };
    ONCE.call_once(|| {
        let Some(v) = jobrt::appupdate::running_version(&home) else { return };
        let Some(script) = due(&home, &v) else { return };
        std::thread::Builder::new()
            .name("services".into())
            .spawn(move || {
                eprintln!("agent: the services to {v} ({})", script.display());
                match store::spawn::output(store::spawn::command("/bin/zsh").arg(&script).arg("update")) {
                    Ok(o) => {
                        for l in
                            String::from_utf8_lossy(&o.stdout).lines().chain(String::from_utf8_lossy(&o.stderr).lines())
                        {
                            eprintln!("agent: {l}");
                        }
                        if !o.status.success() {
                            eprintln!("agent: the services' update failed ({})", o.status);
                        }
                    }
                    Err(e) => eprintln!("agent: the services' update didn't run: {e}"),
                }
            })
            .ok();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_only_when_set_up_and_not_at_the_version() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        let v = "20261112-2000-abc1234";
        assert_eq!(due(h, v), None, "no services folder");
        std::fs::create_dir_all(h.join("services/devtools")).unwrap();
        assert_eq!(due(h, v), None, "nothing set up");
        std::fs::write(h.join("services/devtools/setup"), "label=x\n").unwrap();
        assert_eq!(due(h, v), Some(h.join("app").join(v).join("services/services.zsh")));
        std::fs::write(h.join("services/version"), "20261101-0000-0000000\n").unwrap();
        assert!(due(h, v).is_some(), "at another version");
        std::fs::write(h.join("services/version"), format!("{v}\n")).unwrap();
        assert_eq!(due(h, v), None, "done");
        std::fs::write(h.join("services/version"), "20261101-0000-0000000\n").unwrap();
        std::fs::write(h.join("services/failed"), format!("{v}\n")).unwrap();
        assert_eq!(due(h, v), None, "tried and failed");
    }
}
