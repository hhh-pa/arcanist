//! `arc-rs look`: a small text adventure around the working copy.
//!
//! Port of `src/workflow/ArcanistLookWorkflow.php` ("Arcventure"): with no
//! arguments it describes a clearing (with time-of-day flavor), `look
//! remotes` lists git remotes, and `look published` lists published commits.
//!
//! Known limits (see T1128): the hour is UTC (the PHP uses local time);
//! lunar phases are approximated from the synodic month; published commits
//! are the tips of remote-tracking branches; push/fetch repository names
//! need conduit repository mapping and always render as `-`; paragraph
//! wrapping follows plain lines instead of the PHP console width.

use std::path::Path;
use std::process::ExitCode;

use crate::git;

/// Subcommand name, matching `arc look`.
pub const NAME: &str = "look";

const USAGE: &str = "usage: arc-rs look [-- THING...]\n";

/// Parse `argv` (excluding the subcommand name) into the thing words.
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_args(argv: &[String]) -> Result<Option<Vec<String>>, String> {
    let mut thing = Vec::new();
    let mut end_of_flags = false;
    for arg in argv {
        if end_of_flags {
            thing.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            thing.push(arg.clone());
        }
    }
    Ok(Some(thing))
}

/// Hour of day (0-23) for a unix timestamp, in UTC.
#[must_use]
pub fn hour_of(now_unix: u64) -> u32 {
    ((now_unix / 3600) % 24) as u32
}

/// Approximate lunar phase for flavor text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LunarPhase {
    New,
    Full,
    Waxing,
    Waning,
}

const SYNODIC_SECS: f64 = 29.530_588_853 * 86_400.0;
/// Reference new moon: 2000-01-06T18:14:00Z.
const REF_NEW_MOON: f64 = 947_182_440.0;

/// Approximate the lunar phase from the synodic month.
#[must_use]
pub fn lunar_phase(now_unix: u64) -> LunarPhase {
    let age = (now_unix as f64 - REF_NEW_MOON).rem_euclid(SYNODIC_SECS);
    let phase = age / SYNODIC_SECS;
    if phase < 0.05 || phase >= 0.95 {
        LunarPhase::New
    } else if (phase - 0.5).abs() < 0.05 {
        LunarPhase::Full
    } else if phase < 0.5 {
        LunarPhase::Waxing
    } else {
        LunarPhase::Waning
    }
}

/// Time-of-day description for `hour`, mirroring the PHP ranges.
#[must_use]
pub fn hour_text(hour: u32, phase: LunarPhase) -> &'static str {
    if (5..=7).contains(&hour) {
        "It is early morning. Glimses of sunlight peek through the trees and you hear the faint sound of birds overhead."
    } else if (8..=10).contains(&hour) {
        "It is morning. The sun is high in the sky to the east and you hear birds all around you. A gentle breeze rustles the leaves overhead."
    } else if (11..=13).contains(&hour) {
        "It is midday. The sun is high overhead and the air is still. It is very warm. You hear the cry of a hawk high overhead and far in the distance."
    } else if (14..=16).contains(&hour) {
        "It is afternoon. The air has changed and it feels as though it may rain. You hear a squirrel chittering high overhead."
    } else if (17..=19).contains(&hour) {
        "It is nearly dusk. The wind has picked up and the trees around you sway and rustle."
    } else if (21..=23).contains(&hour) {
        "It is late in the evening. The air is cool and still, and filled with the sound of crickets."
    } else {
        match phase {
            LunarPhase::New => {
                "Night has fallen, and the thin sliver of moon overhead offers no comfort. It is almost pitch black. The night is bitter cold. It will be difficult to look around in these conditions."
            }
            LunarPhase::Full => {
                "Night has fallen, but your surroundings are illuminated by the silvery glow of a full moon overhead. The night is cool and the air is crisp. The trees are calm."
            }
            LunarPhase::Waxing => {
                "Night has fallen. The moon overhead is waxing, and provides just enough light that you can make out your surroundings. It is quite cold."
            }
            LunarPhase::Waning => {
                "Night has fallen. The moon overhead is waning. You can barely make out your surroundings. It is very cold."
            }
        }
    }
}

/// The clearing description for `now_unix`.
#[must_use]
pub fn render_clearing(now_unix: u64) -> String {
    let mut out = "You stand in the middle of a small clearing in the woods.\n\n".to_string();
    out += hour_text(hour_of(now_unix), lunar_phase(now_unix));
    out += "\n\n";
    out += "Several small trails and footpaths cross here, twisting away from you among the trees.\n\n";
    out += "Just ahead to the north, you can see remotes.\n";
    out
}

/// Message for looking at something which is not there.
#[must_use]
pub fn render_unknown(thing: &[String]) -> String {
    format!("You do not see \"{}\" anywhere.\n", thing.join(" "))
}

/// One git remote with its fetch/push URIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub push_uri: Option<String>,
    pub fetch_uri: Option<String>,
}

/// List git remotes under `cwd`.
pub fn query_remotes(cwd: &Path) -> Result<Vec<RemoteInfo>, String> {
    if !git::is_inside_work_tree(cwd) {
        return Err("not inside a git repository".to_string());
    }
    let names = git::stdout_text(cwd, &["remote"])?;
    let mut remotes = Vec::new();
    for name in names.lines().filter(|l| !l.trim().is_empty()) {
        let push_uri = git::stdout_text(cwd, &["remote", "get-url", "--push", name]).ok();
        let fetch_uri = git::stdout_text(cwd, &["remote", "get-url", name]).ok();
        remotes.push(RemoteInfo {
            name: name.to_string(),
            push_uri,
            fetch_uri,
        });
    }
    Ok(remotes)
}

/// Render one remote block.
#[must_use]
pub fn render_remote(remote: &RemoteInfo) -> String {
    let push = remote.push_uri.as_deref().unwrap_or("-");
    let fetch = remote.fetch_uri.as_deref().unwrap_or("-");
    format!(
        "Remote \"{}\":\n  Push URI: {push}\n  Push Repository: -\n  Fetch URI: {fetch}\n  Fetch Repository: -\n",
        remote.name
    )
}

/// Render the `remotes` grove.
#[must_use]
pub fn render_remotes(remotes: &[RemoteInfo]) -> String {
    let mut out = "You follow a wide, straight path to the north and arrive in a grove of fruit trees after a few minutes of walking. The grass underfoot is thick and small insects flit through the air.\n\n".to_string();
    out += "At the far edge of the grove, you see remotes:\n\n";
    for remote in remotes {
        out += &render_remote(remote);
    }
    out += "\nAcross the grove, a stream flows north toward published commits.\n";
    out
}

/// Tips of remote-tracking branches under `cwd` (the published commits).
pub fn query_published(cwd: &Path) -> Result<Vec<String>, String> {
    if !git::is_inside_work_tree(cwd) {
        return Err("not inside a git repository".to_string());
    }
    let out = git::stdout_text(
        cwd,
        &["for-each-ref", "--format=%(objectname)", "refs/remotes"],
    )?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect())
}

/// Render the `published` river.
#[must_use]
pub fn render_published(hashes: &[String]) -> String {
    let mut out = "You walk along the narrow bank of the stream as it winds lazily downhill and turns east, gradually widening into a river.\n\n".to_string();
    if hashes.is_empty() {
        out += "The river bubbles quietly, but you do not see any published commits anywhere.\n";
    } else {
        out += "Floating on the water, you see published commits:\n\n";
        for hash in hashes {
            out += hash;
            out.push('\n');
        }
        out += "\nThey river bubbles peacefully.\n";
    }
    out
}

/// Render `thing` under `cwd`, returning the full stdout text and exit code.
pub fn render(cwd: &Path, thing: &[String], now_unix: u64) -> Result<(String, ExitCode), String> {
    let mut out = "Arcventure\n\n".to_string();
    if thing.is_empty() {
        out += &render_clearing(now_unix);
        return Ok((out, ExitCode::SUCCESS));
    }
    if thing == ["remotes"] {
        out += &render_remotes(&query_remotes(cwd)?);
        return Ok((out, ExitCode::SUCCESS));
    }
    if thing == ["published"] {
        out += &render_published(&query_published(cwd)?);
        return Ok((out, ExitCode::SUCCESS));
    }
    out += &render_unknown(thing);
    Ok((out, ExitCode::FAILURE))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let thing = match parse_args(argv) {
        Ok(Some(thing)) => thing,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("arc-rs {NAME}: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    match render(&cwd, &thing, now_unix()) {
        Ok((out, code)) => {
            print!("{out}");
            code
        }
        Err(msg) => {
            eprintln!("arc-rs {NAME}: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "look_tests.rs"]
mod look_tests;
