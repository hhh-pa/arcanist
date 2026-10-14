//! `arc-rs` — Rust port of Arcanist.
//!
//! Tracks T1128. Only the CLI skeleton exists so far; each subcommand is
//! ported behind the same name `arc` uses.

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const PLANNED: &[&str] = &["diff", "land", "lint", "unit", "call-conduit"];

fn usage() -> String {
    let commands = PLANNED
        .iter()
        .map(|c| format!("  {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "usage: arc-rs [--version] [--help] <command>\n\nplanned commands (not yet ported):\n{commands}\n"
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    Version,
    Planned(String),
    Unknown(String),
    Missing,
}

fn classify(argv: &[String]) -> Action {
    let Some(cmd) = argv.first() else {
        return Action::Missing;
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => Action::Help,
        "-V" | "--version" => Action::Version,
        c if PLANNED.contains(&c) => Action::Planned(c.to_string()),
        c => Action::Unknown(c.to_string()),
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match classify(&argv) {
        Action::Help => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("arc-rs {VERSION}");
            ExitCode::SUCCESS
        }
        Action::Planned(c) => {
            eprintln!("arc-rs {c}: not yet ported (see T1128)");
            ExitCode::FAILURE
        }
        Action::Missing => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
        Action::Unknown(c) => {
            eprintln!("arc-rs: unknown command '{c}'\n\n{}", usage());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn classifies_help_and_version() {
        assert_eq!(classify(&args(&["--help"])), Action::Help);
        assert_eq!(classify(&args(&["-h"])), Action::Help);
        assert_eq!(classify(&args(&["help"])), Action::Help);
        assert_eq!(classify(&args(&["--version"])), Action::Version);
        assert_eq!(classify(&args(&["-V"])), Action::Version);
    }

    #[test]
    fn classifies_planned_commands() {
        for c in PLANNED {
            assert_eq!(classify(&args(&[*c])), Action::Planned((*c).to_string()));
        }
    }

    #[test]
    fn classifies_missing_and_unknown() {
        assert_eq!(classify(&args(&[])), Action::Missing);
        assert_eq!(
            classify(&args(&["bogus"])),
            Action::Unknown("bogus".to_string())
        );
    }

    #[test]
    fn usage_lists_all_planned_commands() {
        let u = usage();
        for c in PLANNED {
            assert!(u.contains(c), "usage missing {c}");
        }
    }
}
