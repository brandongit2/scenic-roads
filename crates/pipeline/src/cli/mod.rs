//! Every program's command line, parsed by clap (docs/building.md, "Command lines"): `--help` or
//! `-h` prints the usage made from these definitions and exits 0, nothing run; an unknown flag,
//! command or argument, a flag without its value or given twice is refused with the usage (exit 2),
//! before anything runs. One definition a program, here rather than in its binary, so the agent's
//! tests check the command lines it makes for its jobs against them (`check_command`), and
//! tests/cli.rs runs every binary with `--help` and a flag it doesn't take.

pub mod build;
pub mod progs;
pub mod scenic;

use clap::CommandFactory;

/// Every program of this crate (its src/bin/), by name.
pub const PROGRAMS: [&str; 16] = [
    "scenic",
    "scenic-build",
    "scenic-metrics",
    "extract",
    "elev",
    "tile",
    "areaflags",
    "landcover",
    "railfreq",
    "trees",
    "bldtile",
    "terrainsub",
    "lakefill",
    "coastcheck",
    "slope",
    "terrain",
];

/// A program's command line, by its name (`PROGRAMS`).
pub fn command(prog: &str) -> Option<clap::Command> {
    use progs::*;
    Some(match prog {
        "scenic" => scenic::Cli::command(),
        "scenic-build" => build::command(),
        "scenic-metrics" => Metrics::command(),
        "extract" => Extract::command(),
        "elev" => Elev::command(),
        "tile" => Tile::command(),
        "areaflags" => Areaflags::command(),
        "landcover" => Landcover::command(),
        "railfreq" => Railfreq::command(),
        "trees" => Trees::command(),
        "bldtile" => Bldtile::command(),
        "terrainsub" => Terrainsub::command(),
        "lakefill" => Lakefill::command(),
        "coastcheck" => Coastcheck::command(),
        "slope" => Slope::command(),
        "terrain" => Terrain::command(),
        _ => return None,
    })
}

/// A command line as a job runs it (env settings and wrappers first, then the program by its path):
/// None when it runs none of these programs, else whether that program's command line takes it.
pub fn check_command(cmd: &[String]) -> Option<Result<(), String>> {
    let name = |a: &String| std::path::Path::new(a).file_name().map(|f| f.to_string_lossy().into_owned());
    let at = cmd.iter().position(|a| name(a).is_some_and(|n| PROGRAMS.contains(&n.as_str())))?;
    let c = command(&name(&cmd[at])?)?;
    Some(c.try_get_matches_from(&cmd[at..]).map(|_| ()).map_err(|e| format!("{cmd:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_definition_is_sound() {
        for p in PROGRAMS {
            command(p).unwrap().debug_assert();
        }
    }

    fn line(l: &str) -> Vec<String> {
        l.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn job_lines_are_checked() {
        let ok = |l: &str| check_command(&line(l)) == Some(Ok(()));
        assert!(ok("SCENIC_X=1 /app/scenic-build unit --root /r --scratch /s 6/1/1 --pass 2026-10-01"));
        assert!(ok("/app/scenic-build --root /r regions-fixture b4test -73.5,45,20"));
        assert!(ok("/app/scenic-build geoid-offsets --root /r --sources s --from f --at -73.5,45"));
        assert!(ok("/app/scenic-build osm-pass --root /r --planet p --date d --clear a --clear b"));
        assert!(ok("/app/scenic-metrics /u buildings /b"));
        assert!(ok("/app/scenic gc --root /r --days 3"));
        assert!(ok("/app/trees --blocks 8/1/2,8/2/2 --coverage c --chm x --leaf y --squares 1,2 --out o"));
        assert!(ok(
            "/app/lakefill fill --basemap b --sheets s --source a --source b --declaration d --band -100,-90 --out o"
        ));
        assert!(!ok("/app/scenic-build unit --root /r --nwo"));
        assert!(!ok("/app/scenic-build unit --pass a --pass b"));
        assert!(!ok("/app/scenic-build verify stray"));
        assert!(!ok("/app/scenic-metrics /u buildings"));
        assert!(!ok("/app/scenic-build pack --help"));
        assert!(!ok("/app/trees --out o"), "no mode");
        assert_eq!(check_command(&line("/bin/echo hi")), None);
    }

    #[test]
    fn a_steps_arguments() {
        let t = |l: &str| build::targets(&line(l));
        assert_eq!(t("scenic-build inputs --full regions --root /r"), ("inputs".into(), vec!["regions".to_string()]));
        assert_eq!(t("scenic-build pack --pass d 6/1/1 6/1/2").1, ["6/1/1", "6/1/2"]);
        assert_eq!(t("scenic-build regions-fixture --root /r b4 -7,46,20").1, ["b4", "-7,46,20"]);
        assert!(t("scenic-build verify").1.is_empty());
    }
}
