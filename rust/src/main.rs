//! `arc-rs` — Rust port of Arcanist.
//!
//! Tracks T1128. `call-conduit`, `diff`, `lint`, `unit`, and `land` are
//! implemented; remaining subcommands keep the same names `arc` uses.

use std::process::ExitCode;

use arc_rs::{conduit, config, diff, land, lint, unit};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const PLANNED: &[&str] = &[];

fn usage() -> String {
    let mut text = "usage: arc-rs [--version] [--help] <command>\n\nimplemented commands:\n  call-conduit <method> [params-json] [--conduit-uri URI]\n  diff [--base REV] [--json]\n  land [--onto BRANCH] [--preview] [--push]\n  lint [PATHS...] [--json]\n  unit [-- <cargo-test-args>]\n"
        .to_string();
    if !PLANNED.is_empty() {
        let planned = PLANNED
            .iter()
            .map(|c| format!("  {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        text.push_str(&format!(
            "\nplanned commands (not yet ported):\n{planned}\n"
        ));
    }
    text
}

#[derive(Debug, PartialEq, Eq)]
struct CallArgs {
    method: String,
    params_json: String,
    conduit_uri: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    Version,
    CallConduit(CallArgs),
    Planned(String),
    Unknown(String),
    Missing,
    Error(String),
}

fn parse_call_conduit(rest: &[String]) -> Result<CallArgs, String> {
    let mut iter = rest.iter();
    let Some(method) = iter.next() else {
        return Err("call-conduit requires a method name".to_string());
    };
    let mut params_json = "{}".to_string();
    let mut conduit_uri = None;
    let mut positional = false;
    while let Some(arg) = iter.next() {
        if arg == "--conduit-uri" {
            let Some(uri) = iter.next() else {
                return Err("--conduit-uri requires a value".to_string());
            };
            conduit_uri = Some(uri.clone());
        } else if arg.starts_with("--") {
            return Err(format!("unknown flag '{arg}'"));
        } else if positional {
            return Err("call-conduit takes at most one params-json argument".to_string());
        } else {
            params_json = arg.clone();
            positional = true;
        }
    }
    Ok(CallArgs {
        method: method.clone(),
        params_json,
        conduit_uri,
    })
}

fn classify(argv: &[String]) -> Action {
    let Some(cmd) = argv.first() else {
        return Action::Missing;
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => Action::Help,
        "-V" | "--version" => Action::Version,
        "call-conduit" => match parse_call_conduit(&argv[1..]) {
            Ok(args) => Action::CallConduit(args),
            Err(msg) => Action::Error(msg),
        },
        c if PLANNED.contains(&c) => Action::Planned(c.to_string()),
        c => Action::Unknown(c.to_string()),
    }
}

fn run_call_conduit(args: &CallArgs) -> ExitCode {
    let params: serde_json::Value = match serde_json::from_str(&args.params_json) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("arc-rs: invalid params JSON: {e}");
            return ExitCode::FAILURE;
        }
    };
    let uri = args
        .conduit_uri
        .clone()
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(config::discover_conduit_uri);
    let Some(uri) = uri else {
        eprintln!(
            "arc-rs: no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project with .arcconfig"
        );
        return ExitCode::FAILURE;
    };
    let token = config::arcrc_path().and_then(|path| config::token_for_file(&path, &uri));
    let Some(token) = token else {
        eprintln!("arc-rs: no token for {uri} in ~/.arcrc");
        return ExitCode::FAILURE;
    };
    let client = match conduit::Client::new(&uri, &token) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("arc-rs: {e}");
            return ExitCode::FAILURE;
        }
    };
    match client.call(&args.method, &params) {
        Ok(result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "null".to_string())
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("arc-rs: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = argv.first() {
        let rest = &argv[1..];
        match cmd.as_str() {
            "diff" => return diff::run(rest),
            "land" => return land::run(rest),
            "lint" => return lint::run(rest),
            "unit" => return unit::run(rest),
            _ => {}
        }
    }
    match classify(&argv) {
        Action::Help => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("arc-rs {VERSION}");
            ExitCode::SUCCESS
        }
        Action::CallConduit(args) => run_call_conduit(&args),
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
        Action::Error(msg) => {
            eprintln!("arc-rs: {msg}\n\n{}", usage());
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
    fn classifies_call_conduit() {
        assert_eq!(
            classify(&args(&["call-conduit", "user.whoami"])),
            Action::CallConduit(CallArgs {
                method: "user.whoami".to_string(),
                params_json: "{}".to_string(),
                conduit_uri: None,
            })
        );
        assert_eq!(
            classify(&args(&[
                "call-conduit",
                "user.whoami",
                "{}",
                "--conduit-uri",
                "https://orsay.hhh.pa/"
            ])),
            Action::CallConduit(CallArgs {
                method: "user.whoami".to_string(),
                params_json: "{}".to_string(),
                conduit_uri: Some("https://orsay.hhh.pa/".to_string()),
            })
        );
    }

    #[test]
    fn rejects_bad_call_conduit_invocations() {
        assert!(matches!(
            classify(&args(&["call-conduit"])),
            Action::Error(_)
        ));
        assert!(matches!(
            classify(&args(&["call-conduit", "m", "--bogus"])),
            Action::Error(_)
        ));
        assert!(matches!(
            classify(&args(&["call-conduit", "m", "{}", "--conduit-uri"])),
            Action::Error(_)
        ));
        assert!(matches!(
            classify(&args(&["call-conduit", "m", "{}", "{}"])),
            Action::Error(_)
        ));
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
        assert!(u.contains("call-conduit"));
    }
}
