//! `shell-complete` workflow: install shell completion, generate
//! completion rules, and complete command lines for the shell hook.
//!
//! PHP reference: `src/toolset/workflow/ArcanistShellCompleteWorkflow.php`
//! at ceb. Only `bash` is supported there, and so here. Rules cover the
//! `arc` and `phage` toolsets (`ArcanistToolset::newToolsetMap()`).
//!
//! Paths adapt PHP's install-relative layout to a static binary: the
//! shell support root is `<exe-dir>/support/shell` (rules at
//! `rules/bash-rules.sh`, hook at `hooks/bash-completion.sh`).

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::argparse::{self, FlagSpec};
use crate::conduit;
use crate::terminal;

pub const NAME: &str = "shell-complete";

/// `support/shell/templates/bash-template.sh` at ceb, with a `{{{BIN}}}`
/// placeholder per toolset key.
const BASH_TEMPLATE: &str = "_arcanist_complete_{{{BIN}}} ()\n\
{\n\
  COMPREPLY=()\n\
\n\
  RESULT=$(echo | {{{BIN}}} shell-complete \\\n\
    --current ${COMP_CWORD} \\\n\
    -- \\\n\
    \"${COMP_WORDS[@]}\" \\\n\
    2>/dev/null)\n\
\n\
  if [ $? -ne 0 ]; then\n\
    return $?\n\
  fi\n\
\n\
  if [ \"$RESULT\" == \"<compgen:file>\" ]; then\n\
    RESULT=$( compgen -A file -- ${COMP_WORDS[COMP_CWORD]} )\n\
  fi\n\
\n\
  local IFS=$'\\n'\n\
  COMPREPLY=( $RESULT )\n\
}\n\
complete -F _arcanist_complete_{{{BIN}}} -o filenames {{{BIN}}}\n";

/// One completable `--flag`: `(name, parameter name or None, whether the
/// parameter is a path)`.
type FlagInfo = (&'static str, Option<&'static str>, bool);

/// Workflow argument shapes for completion
/// (`getWorkflowArguments()` at ceb): flags plus whether the wildcard
/// (if any) takes paths.
fn workflow_args(workflow: &str) -> (&'static [FlagInfo], bool) {
    match workflow {
        "alias" => (&[("json", None, false)], false),
        "amend" => (
            &[("show", None, false), ("revision", Some("id"), false)],
            false,
        ),
        "download" => (&[("as", Some("path"), false)], true),
        "inspect" => (&[("explore", None, false)], false),
        "land" => (
            &[
                ("hold", None, false),
                ("keep-branches", None, false),
                ("onto-remote", Some("remote-name"), false),
                ("onto", Some("branch-name"), false),
                ("strategy", Some("strategy-name"), false),
                ("revision", Some("revision-identifier"), false),
                ("preview", None, false),
                ("into", Some("commit-ref"), false),
                ("into-remote", Some("remote-name"), false),
                ("into-local", None, false),
                ("into-empty", None, false),
                ("incremental", None, false),
                ("pick", None, false),
            ],
            false,
        ),
        "liberate" => (&[("clean", None, false)], true),
        "paste" => (
            &[
                ("title", Some("title"), false),
                ("lang", Some("language"), false),
                ("input", Some("path"), true),
                ("browse", None, false),
            ],
            false,
        ),
        "upload" => (
            &[
                ("json", None, false),
                ("browse", None, false),
                ("temporary", None, false),
            ],
            true,
        ),
        "weld" => (&[], true),
        "work" => (&[("start", Some("symbol"), false)], false),
        "prompts" => (&[], false),
        "shell-complete" => (
            &[
                ("current", Some("cursor-position"), false),
                ("generate", None, false),
                ("shell", Some("shell-name"), false),
            ],
            false,
        ),
        _ => (&[], false),
    }
}

/// Parse `shell-complete` arguments.
pub fn parse_args(argv: &[String], autocorrect: bool) -> Result<argparse::Parsed, String> {
    argparse::parse_toolset(
        argv,
        &[
            FlagSpec::valued("current", "cursor-position"),
            FlagSpec::boolean("generate"),
            FlagSpec::valued("shell", "shell-name"),
        ],
        true,
        autocorrect,
    )
}

/// Shell support root for an executable directory.
#[must_use]
pub fn shell_root_for(exe_dir: &Path) -> PathBuf {
    exe_dir.join("support").join("shell")
}

/// Default shell support root, next to the running executable.
fn default_shell_root() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    shell_root_for(&exe_dir)
}

/// Completion rules for every toolset, joined like PHP's
/// `implode("\n", $rules)`.
#[must_use]
pub fn completion_rules() -> String {
    ["arc", "phage"]
        .iter()
        .map(|bin| BASH_TEMPLATE.replace("{{{BIN}}}", bin))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Bash-completion candidates for `candidates` under `prefix`: exact
/// prefix matches, else case-insensitive ones, else everything when
/// the prefix is empty (`getMatches()`).
#[must_use]
pub fn get_matches(candidates: &[String], prefix: &str) -> Vec<String> {
    if prefix.is_empty() {
        return candidates.to_vec();
    }
    let exact: Vec<String> = candidates
        .iter()
        .filter(|c| c.starts_with(prefix))
        .cloned()
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let folded = prefix.to_lowercase();
    candidates
        .iter()
        .filter(|c| c.to_lowercase().starts_with(&folded))
        .cloned()
        .collect()
}

/// Render completion strings: sorted, newline-joined, no trailing
/// newline (`suggestStrings()`).
#[must_use]
pub fn suggest_strings(mut strings: Vec<String>) -> String {
    strings.sort();
    strings.join("\n")
}

/// Complete `words` (the words after `--`, `words[1]` is the workflow)
/// at cursor `current`, with `aliases` as extra workflow candidates.
pub fn autocomplete(
    words: &[String],
    current: Option<&str>,
    aliases: &[String],
) -> Result<String, String> {
    let argc = words.len();
    let pos: Option<i64> = match current {
        None | Some("") => None,
        Some(text) => text.parse::<i64>().ok(),
    };
    // An unparseable `--current` behaves like position 0 with an
    // empty partial (PHP indexes by a missing key, matching all).
    let (pos, partial_override) = match (current, pos) {
        (Some(text), None) if !text.is_empty() => (0, Some(String::new())),
        (_, Some(pos)) => (pos, None),
        _ => (argc as i64 - 1, None),
    };
    if pos >= argc as i64 {
        let count = argc.to_string();
        let at = current.unwrap_or_default().to_string();
        return Err(terminal::pht(
            "Argument position specified with \"--current\" (\"%s\") is greater than the number of arguments provided (\"%s\").",
            &[terminal::PhtArg::Text(&at), terminal::PhtArg::Text(&count)],
        ));
    }
    if pos <= 1 {
        let mut complete: Vec<String> = crate::alias::toolset_names()
            .iter()
            .map(|name| name.to_string())
            .collect();
        complete.extend(aliases.iter().cloned());
        let partial = partial_override
            .as_deref()
            .or_else(|| words.get(pos.max(0) as usize).map(String::as_str))
            .unwrap_or("");
        let matches = get_matches(&complete, partial);
        if matches.is_empty() {
            return Ok(String::new());
        }
        return Ok(suggest_strings(matches));
    }
    let pos = pos as usize;
    let Some(workflow) = words.get(1).and_then(|name| {
        crate::alias::toolset_names()
            .contains(&name.as_str())
            .then_some(name.as_str())
    }) else {
        return Ok(String::new());
    };
    let (flags, wildcard_path) = workflow_args(workflow);
    let current_word = words.get(pos).map(String::as_str).unwrap_or("");
    if let Some(prev) = words.get(pos.wrapping_sub(1))
        && let Some(stripped) = prev.strip_prefix("--")
        && let Some((_, param, is_path)) = flags.iter().find(|(name, _, _)| *name == stripped)
        && param.is_some()
    {
        if *is_path {
            return Ok("<compgen:file>".to_string());
        }
        return Ok(String::new());
    }
    let flag_words: Vec<String> = flags
        .iter()
        .map(|(name, _, _)| format!("--{name}"))
        .collect();
    let matches = get_matches(&flag_words, current_word);
    if (current_word.is_empty() || matches.is_empty()) && wildcard_path {
        return Ok("<compgen:file>".to_string());
    }
    Ok(suggest_strings(matches))
}

/// Detect the shell from `$SHELL` (`detectShell()`): the status lines
/// (always printed, even on failure) plus the detected key or a usage
/// error.
pub fn detect_shell(shell_env: Option<&str>, ansi: bool) -> (String, Result<String, String>) {
    let mut out = terminal::format_log("blue", "DETECT", "Detecting current shell...", ansi);
    let mut detected: Vec<&str> = Vec::new();
    match shell_env {
        None | Some("") => {
            out.push_str(&terminal::format_log(
                "yellow",
                "SHELL",
                "The \"SHELL\" environment variable is not defined, so it can not be used to detect the shell to install rules for.",
                ansi,
            ));
        }
        Some(shell) => {
            if shell == "/bin/bash" {
                detected.push("bash");
                out.push_str(&terminal::format_log(
                    "green",
                    "SHELL",
                    &terminal::pht(
                        "The \"SHELL\" environment variable has value \"%s\", so the target shell was detected as \"%s\".",
                        &[terminal::PhtArg::Text(shell), terminal::PhtArg::Text("bash")],
                    ),
                    ansi,
                ));
            } else {
                out.push_str(&terminal::format_log(
                    "blue",
                    "SHELL",
                    "The \"SHELL\" environment variable does not match any recognized shell.",
                    ansi,
                ));
            }
        }
    }
    if detected.is_empty() {
        return (out, Err("Unable to detect any supported shell, so autocompletion rules can not be installed. Use \"--shell\" to select a shell.".to_string()));
    }
    if detected.len() > 1 {
        return (out, Err("Multiple supported shells were detected. Unable to determine which shell to install autocompletion rules for. Use \"--shell\" to select a shell.".to_string()));
    }
    (out, Ok(detected[0].to_string()))
}

/// Validate an explicit `--shell` value (`selectShell()`).
pub fn select_shell(shell_arg: &str) -> Result<String, String> {
    if shell_arg == "bash" {
        return Ok("bash".to_string());
    }
    Err(terminal::pht(
        "The shell \"%s\" is not supported. Supported shells are: %s.",
        &[
            terminal::PhtArg::Text(shell_arg),
            terminal::PhtArg::Text("bash"),
        ],
    ))
}

/// Replace the `# arcanist-shell-complete` line in `$HOME/.profile`
/// data, returning the new data and the removed line (trimmed) when a
/// line was replaced.
#[must_use]
pub fn replace_profile_line(data: &str, line: &str) -> (String, Option<String>) {
    let pattern = regex::Regex::new(r"(\s*\n)?[^\n]+# arcanist-shell-complete\s*(\n\s*)?").unwrap();
    let Some(hit) = pattern.find(data) else {
        return (data.to_string(), None);
    };
    let replacement = if hit.start() == 0 {
        format!("{line}\n")
    } else {
        format!("\n\n{line}\n")
    };
    let mut new_data = data.to_string();
    new_data.replace_range(hit.range(), &replacement);
    (new_data, Some(data[hit.range()].trim().to_string()))
}

/// The `source ... # arcanist-shell-complete` line (`%R` minimal
/// quoting: quote only when the path needs it).
#[must_use]
pub fn source_line(hook_path: &str) -> String {
    if hook_path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~'))
    {
        format!("source {hook_path} # arcanist-shell-complete")
    } else {
        format!(
            "source '{}' # arcanist-shell-complete",
            hook_path.replace('\'', "'\\''")
        )
    }
}

/// Install completion into `$HOME/.profile`, prompting as needed.
/// `home`/`root` override `$HOME` and the shell root (tests); `input`
/// answers the prompt; `save` persists `*`/`!` prompt suffixes.
#[allow(clippy::too_many_arguments)]
pub fn install_in(
    shell_arg: Option<&str>,
    shell_env: Option<&str>,
    home: Option<&str>,
    root: &Path,
    input: &mut dyn BufRead,
    stdin_tty: bool,
    saved: Option<&str>,
    save: &mut dyn FnMut(crate::prompts::PromptScope, &str) -> Result<(), String>,
) -> Result<String, conduit::ClassifiedError> {
    use conduit::ClassifiedError;
    let ansi = terminal::ansi_enabled();
    let mut out = String::new();
    let shell = match shell_arg {
        Some(arg) if !arg.is_empty() => {
            select_shell(arg).map_err(ClassifiedError::toolset_usage)?
        }
        _ => {
            let (status, detected) = detect_shell(shell_env, ansi);
            out.push_str(&status);
            detected.map_err(ClassifiedError::toolset_usage)?
        }
    };
    let Some(home) = home.filter(|h| !h.is_empty()) else {
        return Err(ClassifiedError::toolset_usage(
            "The \"HOME\" environment variable is not defined, so this workflow can not identify where to install shell completion.".to_string(),
        ));
    };
    let file_path = Path::new(home).join(".profile");
    let (data, is_new) = match std::fs::read_to_string(&file_path) {
        Ok(data) => (data, false),
        Err(_) => (String::new(), true),
    };
    let hook = root.join("hooks").join("bash-completion.sh");
    let line = source_line(&hook.to_string_lossy());
    let (new_data, replaced) = replace_profile_line(&data, &line);
    out.push_str(&terminal::format_log(
        "green",
        "INSTALL",
        &terminal::pht(
            "Installing shell completion support for \"%s\" into \"%s\".",
            &[
                terminal::PhtArg::Text(&shell),
                terminal::PhtArg::Text("~/.profile"),
            ],
        ),
        ansi,
    ));
    if replaced.is_some() && new_data == data {
        out.push_str(&terminal::format_log(
            "blue",
            "SKIP",
            &terminal::pht(
                "Shell completion for \"%s\" is already installed.",
                &[terminal::PhtArg::Text(&shell)],
            ),
            ansi,
        ));
        return Ok(out);
    }
    let had_replacement = replaced.is_some();
    let (prompt, echo) = match (replaced, is_new) {
        (Some(removed), _) => (
            "Rewrite this file?",
            terminal::pht(
                "%s\n\n    %s\n\n%s\n\n    %s\n",
                &[
                    terminal::PhtArg::Text(&terminal::pht(
                        "To update shell completion support for \"%s\", your existing \"%s\" file will be modified. This line will be removed:",
                        &[
                            terminal::PhtArg::Text(&shell),
                            terminal::PhtArg::Text("~/.profile"),
                        ],
                    )),
                    terminal::PhtArg::Text(&removed),
                    terminal::PhtArg::Text("This line will be added:"),
                    terminal::PhtArg::Text(&line),
                ],
            ),
        ),
        (None, true) => (
            "Create this file?",
            terminal::pht(
                "%s\n\n    %s\n",
                &[
                    terminal::PhtArg::Text(&terminal::pht(
                        "To install shell completion support for \"%s\", a new \"%s\" file will be created with this content:",
                        &[
                            terminal::PhtArg::Text(&shell),
                            terminal::PhtArg::Text("~/.profile"),
                        ],
                    )),
                    terminal::PhtArg::Text(&line),
                ],
            ),
        ),
        (None, false) => (
            "Append to this file?",
            terminal::pht(
                "%s\n\n     %s\n",
                &[
                    terminal::PhtArg::Text(&terminal::pht(
                        "To install shell completion support for \"%s\", this line will be added to your existing \"%s\" file:",
                        &[
                            terminal::PhtArg::Text(&shell),
                            terminal::PhtArg::Text("~/.profile"),
                        ],
                    )),
                    terminal::PhtArg::Text(&line),
                ],
            ),
        ),
    };
    out.push_str(&echo);
    let final_data = if had_replacement {
        new_data
    } else if is_new {
        format!("{line}\n")
    } else {
        format!("{}\n\n{line}\n", data.trim_end())
    };
    crate::prompts::execute_prompt_saving(
        input,
        crate::prompts::INSTALL_PROMPT_KEY,
        crate::prompts::INSTALL_PROMPT_DESCRIPTION,
        prompt,
        stdin_tty,
        saved,
        save,
    )
    .map_err(|message| {
        if message == crate::prompt::ABORT {
            ClassifiedError::user_abort(message)
        } else {
            ClassifiedError::fatal(message)
        }
    })?;
    std::fs::write(&file_path, final_data).map_err(|e| {
        ClassifiedError::fatal(format!("cannot write '{}': {e}", file_path.display()))
    })?;
    out.push_str(&terminal::format_log(
        "green",
        "INSTALLED",
        &terminal::pht(
            "Installed shell completion support for \"%s\" to \"%s\".",
            &[
                terminal::PhtArg::Text(&shell),
                terminal::PhtArg::Text("~/.profile"),
            ],
        ),
        ansi,
    ));
    Ok(out)
}

/// Regenerate `rules/bash-rules.sh` under `root` (`runGenerate()`).
pub fn generate_in(root: &Path) -> String {
    let ansi = terminal::ansi_enabled();
    let mut out = terminal::format_log(
        "blue",
        "GENERATE",
        "Generating shell completion rules...",
        ansi,
    );
    let rules = completion_rules();
    let rules_path = root.join("rules").join("bash-rules.sh");
    let current = std::fs::read_to_string(&rules_path).unwrap_or_default();
    if current == rules {
        out.push_str(&terminal::format_log(
            "blue",
            "SKIP",
            &format!(
                "Rules are already up to date for \"bash\" in: {}",
                rules_path.display()
            ),
            ansi,
        ));
        return out;
    }
    let status = if std::fs::create_dir_all(root.join("rules"))
        .and_then(|()| std::fs::write(&rules_path, &rules))
        .is_ok()
    {
        terminal::format_log(
            "blue",
            "RULES",
            &format!(
                "Wrote updated completion rules for \"bash\" to: {}.",
                rules_path.display()
            ),
            ansi,
        )
    } else {
        terminal::format_log(
            "blue",
            "SKIP",
            &format!(
                "Rules are already up to date for \"bash\" in: {}",
                rules_path.display()
            ),
            ansi,
        )
    };
    out.push_str(&status);
    out
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    use conduit::ClassifiedError;
    use std::io::IsTerminal;
    let stdin_tty = std::io::stdin().is_terminal();
    let parsed = match parse_args(argv, stdin_tty) {
        Ok(parsed) => parsed,
        Err(message) => return ClassifiedError::toolset_parse_error(message).report(),
    };
    for notice in &parsed.notices {
        eprint!(
            "{}",
            terminal::tsprintf_ansi(
                "%s\n",
                &[terminal::TVal::Text(notice)],
                terminal::ansi_enabled()
            )
            .map(|t| t.to_string())
            .unwrap_or_default()
        );
    }
    let is_generate = parsed.flag("generate");
    let shell_arg = parsed.value("shell").unwrap_or("").to_string();
    let is_shell = !shell_arg.is_empty();
    let current = parsed.value("current").map(str::to_string);
    let words = parsed.positionals;

    if !words.is_empty() {
        if is_generate {
            return ClassifiedError::toolset_usage(
                "You can not use \"--generate\" when completing arguments.".to_string(),
            )
            .report();
        }
        if is_shell {
            return ClassifiedError::toolset_usage(
                "You can not use \"--shell\" when completing arguments.".to_string(),
            )
            .report();
        }
        let aliases = crate::alias::load_aliases()
            .map(|(aliases, _)| {
                aliases
                    .into_iter()
                    .filter(|alias| alias.toolset == "arc")
                    .map(|alias| alias.trigger)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        return match autocomplete(&words, current.as_deref(), &aliases) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(message) => ClassifiedError::fatal(message).report(),
        };
    }

    if is_generate {
        if current.is_some() {
            return ClassifiedError::toolset_usage(
                "You can not use \"--current\" when generating rules.".to_string(),
            )
            .report();
        }
        if is_shell {
            return ClassifiedError::toolset_usage(
                "The flags \"--generate\" and \"--shell\" are mutually exclusive. The \"--shell\" flag selects which shell to install support for, but the \"--generate\" suppresses installation.".to_string(),
            )
            .report();
        }
        print!("{}", generate_in(&default_shell_root()));
    } else {
        if current.is_some() {
            return ClassifiedError::toolset_usage(
                "You can not use \"--current\" when installing support.".to_string(),
            )
            .report();
        }
        let home = std::env::var("HOME").ok();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let user_file = crate::config::arcrc_path();
        let local_file = crate::prompts::working_copy_config_file(&cwd);
        let saved = crate::prompts::load_saved_response(
            &cwd,
            user_file.as_deref(),
            crate::prompts::INSTALL_PROMPT_KEY,
        );
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        let mut saver = crate::prompts::response_saver(
            crate::prompts::INSTALL_PROMPT_KEY,
            user_file,
            local_file,
        );
        match install_in(
            (!shell_arg.is_empty()).then_some(shell_arg.as_str()),
            std::env::var("SHELL").ok().as_deref(),
            home.as_deref(),
            &default_shell_root(),
            &mut locked,
            stdin_tty,
            saved.as_deref(),
            &mut saver,
        ) {
            Ok(text) => print!("{text}"),
            Err(error) => return error.report(),
        }
        print!("{}", generate_in(&default_shell_root()));
    }

    eprint!(
        "{}",
        terminal::format_log(
            "cyan",
            "NOTE",
            "You may need to open a new terminal window or launch a new shell before the changes take effect.",
            terminal::ansi_enabled()
        )
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "shell_complete_tests.rs"]
mod shell_complete_tests;
