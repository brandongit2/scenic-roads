//! The command lines of the programs with commands (`scenic`, `scenic-build`, `scenic-metrics`,
//! `lakefill`, `coastcheck`): each program's table of its commands, the flags and arguments each
//! takes, checked before anything runs. An unknown flag, a flag's missing value or an argument too
//! many is refused with the command's usage (exit 2), and `--help`/`-h` anywhere prints the usage
//! and exits 0, so neither does anything. The commands read their flags as before; this only gates.

pub mod tables;

/// No limit on a command's arguments.
pub const ANY: usize = usize::MAX;

/// A command: its words (`"pool on"`: a command of `pool`), what it takes and does.
#[derive(Debug)]
pub struct Cmd {
    /// Its words after the program's name.
    pub name: &'static str,
    /// What follows them, for the usage line (`"<member> [--force]"`).
    pub args: &'static str,
    /// What it does, a line or two.
    pub about: &'static str,
    /// Its flags that take no value.
    pub flags: &'static [&'static str],
    /// Its flags followed by a value.
    pub opts: &'static [&'static str],
    /// How many arguments it takes at most (not flags or their values; `ANY`).
    pub pos: usize,
}

/// A program's commands, as its dispatcher matches them.
pub struct Table {
    pub prog: &'static str,
    /// What the program does.
    pub about: &'static str,
    /// The command run with none named.
    pub default: Option<&'static str>,
    /// Flags with a value every command takes (`--root`).
    pub common: &'static [&'static str],
    pub cmds: &'static [Cmd],
}

/// What a command line asks.
#[derive(Debug)]
pub enum Checked<'a> {
    /// The command, its flags known and its arguments within its count.
    Run(&'a Cmd),
    /// The usage asked for (`--help`, `-h`, `help`): printed, nothing run.
    Help(String),
}

fn is_help(a: &str) -> bool {
    a == "--help" || a == "-h"
}

/// A flag: `--x`, or `-x` (not a number such as `-12.5,40`).
fn is_flag(a: &str) -> bool {
    a.starts_with("--") || (a.len() > 1 && a.starts_with('-') && !a[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.'))
}

impl Table {
    pub fn get(&self, name: &str) -> Option<&Cmd> {
        self.cmds.iter().find(|c| c.name == name)
    }

    /// Its usage: every command's.
    pub fn listing(&self) -> String {
        let mut s = format!("{}: {}\n\nusage: {} <command> [flags]   (`{} <command> --help`: a command's own)\n\n", self.prog, self.about, self.prog, self.prog);
        for c in self.cmds {
            s += &format!("  {}\n", line(self.prog, c));
        }
        s
    }

    /// A command's usage, with its own commands'.
    pub fn usage(&self, c: &Cmd) -> String {
        let mut s = format!("usage: {}\n\n{}\n", line(self.prog, c), c.about);
        let sub = format!("{} ", c.name);
        let subs: Vec<&Cmd> = self.cmds.iter().filter(|d| d.name.starts_with(&sub)).collect();
        if !subs.is_empty() {
            s += "\nits commands:\n";
            for d in subs {
                s += &format!("  {}\n", line(self.prog, d));
            }
        }
        s += "\n--help, -h: this, nothing run\n";
        s
    }

    /// `args`: the command line after the program's name. Refused (with the usage to print) for an
    /// unknown command or flag, a flag without its value or an argument too many.
    pub fn check(&self, args: &[String]) -> Result<Checked<'_>, String> {
        // The command: the longest whose words lead the line.
        let words: Vec<&str> = args.iter().take_while(|a| !is_flag(a)).map(String::as_str).collect();
        let found = self.cmds.iter().filter(|c| {
            let n: Vec<&str> = c.name.split(' ').collect();
            n.len() <= words.len() && n[..] == words[..n.len()]
        });
        let cmd = found.max_by_key(|c| c.name.split(' ').count());
        let cmd = match (cmd, words.first()) {
            (Some(c), _) => c,
            (None, Some(&"help")) => return Ok(Checked::Help(self.listing())),
            (None, Some(w)) => return Err(format!("{}: unknown command {w:?}\n\n{}", self.prog, self.listing())),
            (None, None) if args.iter().any(|a| is_help(a)) => return Ok(Checked::Help(self.listing())),
            (None, None) => match self.default.and_then(|d| self.get(d)) {
                Some(c) => c,
                None if args.is_empty() => return Err(format!("{}: which command?\n\n{}", self.prog, self.listing())),
                None => return Err(format!("{}: {:?} before a command\n\n{}", self.prog, args[0], self.listing())),
            },
        };
        let named = if words.first().is_some_and(|w| cmd.name.split(' ').next() == Some(*w)) { cmd.name.split(' ').count() } else { 0 };
        let mut pos = 0;
        let mut refused = None;
        let mut it = args.iter().skip(named);
        while let Some(a) = it.next() {
            if is_help(a) {
                return Ok(Checked::Help(self.usage(cmd)));
            }
            if refused.is_some() {
                continue;
            }
            if cmd.opts.contains(&a.as_str()) || self.common.contains(&a.as_str()) {
                match it.next() {
                    None => refused = Some(format!("{a} needs a value")),
                    Some(v) if is_help(v) => return Ok(Checked::Help(self.usage(cmd))),
                    Some(_) => {}
                }
            } else if cmd.flags.contains(&a.as_str()) {
            } else if is_flag(a) {
                refused = Some(format!("unknown flag {a}"));
            } else {
                pos += 1;
                if pos > cmd.pos {
                    refused = Some(format!("unexpected argument {a:?}"));
                }
            }
        }
        match refused {
            Some(why) => Err(format!("{} {}: {why}\n\n{}", self.prog, cmd.name, self.usage(cmd))),
            None => Ok(Checked::Run(cmd)),
        }
    }

    /// `check` on this process's command line (`std::env::args`): the command to run; else the
    /// usage printed and the process ended (0 for help, 2 refused), before anything is done.
    pub fn gate(&self, args: &[String]) -> &Cmd {
        match self.check(args.get(1..).unwrap_or_default()) {
            Ok(Checked::Run(c)) => c,
            Ok(Checked::Help(s)) => {
                print!("{s}");
                std::process::exit(0)
            }
            Err(s) => {
                eprint!("{s}");
                std::process::exit(2)
            }
        }
    }
}

fn line(prog: &str, c: &Cmd) -> String {
    let head = format!("{prog} {}{}{}", c.name, if c.args.is_empty() { "" } else { " " }, c.args);
    let about = c.about.lines().next().unwrap_or("");
    if head.len() < 44 {
        format!("{head:<44}{about}")
    } else {
        format!("{head}\n{:<46}{about}", "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Table = Table {
        prog: "p",
        about: "a test",
        default: Some("status"),
        common: &[],
        cmds: &[
            Cmd { name: "status", args: "", about: "how it stands", flags: &[], opts: &["--root"], pos: 0 },
            Cmd { name: "pool", args: "", about: "the pool", flags: &[], opts: &[], pos: 0 },
            Cmd { name: "pool on", args: "[--force]", about: "on", flags: &["--force"], opts: &[], pos: 0 },
            Cmd { name: "give", args: "<m>", about: "give", flags: &[], opts: &[], pos: 1 },
        ],
    };

    fn check(line: &str) -> Result<&'static str, String> {
        let a: Vec<String> = line.split_whitespace().map(String::from).collect();
        match T.check(&a)? {
            Checked::Run(c) => Ok(c.name),
            Checked::Help(_) => Ok("HELP"),
        }
    }

    #[test]
    fn commands_flags_and_arguments() {
        assert_eq!(check(""), Ok("status"));
        assert_eq!(check("--root x"), Ok("status"));
        assert_eq!(check("pool"), Ok("pool"));
        assert_eq!(check("pool on --force"), Ok("pool on"));
        assert_eq!(check("give m"), Ok("give"));
        assert_eq!(check("give -1.5,2"), Ok("give"));
        assert!(check("pool on --nwo").unwrap_err().contains("unknown flag --nwo"));
        assert!(check("pool off").unwrap_err().contains("unexpected argument \"off\""));
        assert!(check("give m n").is_err());
        assert!(check("status --root").unwrap_err().contains("needs a value"));
        assert!(check("nope").unwrap_err().contains("unknown command"));
        assert!(check("-x").is_err());
    }

    #[test]
    fn help_anywhere_runs_nothing() {
        for l in ["--help", "-h", "help", "pool --help", "pool on -h", "give m n --help", "pool on --nwo --help", "status --root --help"] {
            assert_eq!(check(l), Ok("HELP"), "{l}");
        }
        let Ok(Checked::Help(s)) = T.check(&["pool".into(), "--help".into()]) else { panic!() };
        assert!(s.contains("p pool on [--force]"), "{s}");
    }
}
