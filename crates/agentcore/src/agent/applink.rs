//! A path to the app's home without a space (task #1579): `~/scenic`, a symbolic link to
//! `~/Library/Application Support/scenic`, made by every Mac's agent as it starts (`scenic agent`
//! with its own home). Claude Code refuses a worktree agent's command it can't verify, and a path
//! with `Application\ Support` in it was one ("too complex to verify"); agents name `~/scenic/…`
//! instead (tools/dev/agent-rules.md).

use std::path::Path;

/// The link's name in the home folder.
pub const NAME: &str = "scenic";

/// Makes `<home>/scenic` a link to `app` (the app's home) unless it is one already: what it did, or
/// None. A link to elsewhere is pointed at `app`; a file or folder there is left as it is, an error
/// saying so.
pub fn ensure(home: &Path, app: &Path) -> std::io::Result<Option<String>> {
    let link = home.join(NAME);
    let did = match std::fs::read_link(&link) {
        Ok(to) if to == app => return Ok(None),
        Ok(to) => {
            std::fs::remove_file(&link)?;
            format!("{} pointed at {}, now at {}", link.display(), to.display(), app.display())
        }
        Err(_) if std::fs::symlink_metadata(&link).is_ok() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} is not a link to the app's home: left as it is", link.display()),
            ))
        }
        Err(_) => format!("{} made, a link to {}", link.display(), app.display()),
    };
    std::os::unix::fs::symlink(app, &link)?;
    Ok(Some(did))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_is_made_once_pointed_back_and_never_over_a_folder() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("Library/Application Support/scenic");
        std::fs::create_dir_all(app.join("agent")).unwrap();
        let link = t.path().join(NAME);
        assert!(ensure(t.path(), &app).unwrap().unwrap().contains("made"));
        assert_eq!(std::fs::read_link(&link).unwrap(), app);
        assert!(link.join("agent").is_dir(), "through it, the app's home");
        assert_eq!(ensure(t.path(), &app).unwrap(), None, "already there: nothing done");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(t.path().join("elsewhere"), &link).unwrap();
        assert!(ensure(t.path(), &app).unwrap().unwrap().contains("now at"));
        assert_eq!(std::fs::read_link(&link).unwrap(), app);
        std::fs::remove_file(&link).unwrap();
        std::fs::create_dir(&link).unwrap();
        let e = ensure(t.path(), &app).unwrap_err();
        assert!(e.to_string().contains("left as it is"), "{e}");
        assert!(link.is_dir() && std::fs::read_link(&link).is_err(), "the folder kept");
    }
}
