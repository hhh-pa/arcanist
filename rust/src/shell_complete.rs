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

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`).
/// The renderer is the shared `crate::help` module; this file keeps
/// only the data (ceb
/// `src/toolset/workflow/ArcanistShellCompleteWorkflow.php:14-64,67-87`).
const HELP_SYNOPSIS: &str = "Install shell completion.";
const HELP_TEXT: &str = "Install shell completion so you can use the \"tab\" key to autocomplete\n\
commands and flags in your shell for Arcanist toolsets and workflows.\n\
\n\
The **bash** shell is supported.\n\
\n\
**Installing Completion**\n\
\n\
To install shell completion, run the command:\n\
\n\
\x20 $ arc shell-complete\n\
\n\
This will install shell completion into your current shell. After installing,\n\
you may need to start a new shell (or open a new terminal window) to pick up\n\
the updated configuration.\n\
\n\
Once installed, completion should work across all Arcanist toolsets.\n\
\n\
**Using Completion**\n\
\n\
After completion is installed, use the \"tab\" key to automatically complete\n\
workflows and flags. For example, if you type:\n\
\n\
\x20 $ arc diff --draf<tab>\n\
\n\
...your shell should automatically expand the flag to:\n\
\n\
\x20 $ arc diff --draft\n\
\n\
**Updating Completion**\n\
\n\
To update shell completion, run the same command:\n\
\n\
\x20 $ arc shell-complete\n\
\n\
You can update shell completion without reinstalling it by running:\n\
\n\
\x20 $ arc shell-complete --generate\n\
\n\
You may need to update shell completion if:\n\
\n\
\x20 - you install new Arcanist toolsets; or\n\
\x20 - you move the Arcanist directory; or\n\
\x20 - you upgrade Arcanist and the new version fixes shell completion bugs.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "current",
        None,
        Some("cursor-position"),
        "Internal. Current term in the argument list being completed.",
    ),
    (
        "generate",
        None,
        None,
        "Regenerate shell completion rules, without installing any configuration.",
    ),
    (
        "shell",
        None,
        Some("shell-name"),
        "Install completion support for a particular shell.",
    ),
];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<crate::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    crate::help::usage_text(
        NAME,
        None,
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        true,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(terminal::ansi_enabled())
}

/// True when `-h`/`--help` appears before any `--` terminator (the
/// global parser consumes help flags first in PHP; direct calls
/// reroute to usage the same way).
fn wants_help(argv: &[String]) -> bool {
    argv.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "-h" || arg == "--help")
}

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

/// Resolve the `--current` cursor into a word index plus an
/// optional partial override. An unparseable `--current` behaves like
/// position 0 with an empty partial (PHP indexes by a missing key,
/// matching all); a missing one completes the last word.
fn completion_cursor(current: Option<&str>, argc: usize) -> (i64, Option<String>) {
    let pos: Option<i64> = match current {
        None | Some("") => None,
        Some(text) => text.parse::<i64>().ok(),
    };
    match (current, pos) {
        (Some(text), None) if !text.is_empty() => (0, Some(String::new())),
        (_, Some(pos)) => (pos, None),
        _ => (argc as i64 - 1, None),
    }
}

/// Complete the workflow word (cursor at 0 or 1) against the toolset
/// workflows plus `aliases`.
fn complete_workflow_name(
    words: &[String],
    pos: i64,
    partial_override: Option<&str>,
    aliases: &[String],
) -> String {
    let mut complete: Vec<String> = crate::alias::toolset_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    complete.extend(aliases.iter().cloned());
    let partial = partial_override
        .or_else(|| words.get(pos.max(0) as usize).map(String::as_str))
        .unwrap_or("");
    let matches = get_matches(&complete, partial);
    if matches.is_empty() {
        return String::new();
    }
    suggest_strings(matches)
}

/// Complete after a valued `--flag`: path parameters complete files,
/// other parameters complete nothing. `None` when the previous word
/// is not a valued flag (fall through to flag-name completion).
fn valued_param_completion(words: &[String], pos: usize, flags: &[FlagInfo]) -> Option<String> {
    let prev = words.get(pos.wrapping_sub(1))?;
    let stripped = prev.strip_prefix("--")?;
    let (_, param, is_path) = flags.iter().find(|(name, _, _)| *name == stripped)?;
    if param.is_none() {
        return None;
    }
    Some(if *is_path {
        "<compgen:file>".to_string()
    } else {
        String::new()
    })
}

/// Complete `words` (the words after `--`, `words[1]` is the workflow)
/// at cursor `current`, with `aliases` as extra workflow candidates.
pub fn autocomplete(
    words: &[String],
    current: Option<&str>,
    aliases: &[String],
) -> Result<String, String> {
    let argc = words.len();
    let (pos, partial_override) = completion_cursor(current, argc);
    if pos >= argc as i64 {
        let count = argc.to_string();
        let at = current.unwrap_or_default().to_string();
        return Err(terminal::pht(
            "Argument position specified with \"--current\" (\"%s\") is greater than the number of arguments provided (\"%s\").",
            &[terminal::PhtArg::Text(&at), terminal::PhtArg::Text(&count)],
        ));
    }
    if pos <= 1 {
        return Ok(complete_workflow_name(
            words,
            pos,
            partial_override.as_deref(),
            aliases,
        ));
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
    if let Some(text) = valued_param_completion(words, pos, flags) {
        return Ok(text);
    }
    let current_word = words.get(pos).map(String::as_str).unwrap_or("");
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

/// Select the install shell: an explicit `--shell` wins, else
/// detect from `$SHELL`. Returns the detection status output plus the
/// shell key.
fn resolve_install_shell(
    shell_arg: Option<&str>,
    shell_env: Option<&str>,
    ansi: bool,
) -> Result<(String, String), conduit::ClassifiedError> {
    use conduit::ClassifiedError;
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
    Ok((out, shell))
}

/// The install confirmation prompt and its echo: rewrite (an old line
/// was replaced), create (no profile yet), or append.
fn install_prompt_text(
    shell: &str,
    removed: Option<&str>,
    line: &str,
    is_new: bool,
) -> (&'static str, String) {
    match (removed, is_new) {
        (Some(removed), _) => (
            "Rewrite this file?",
            terminal::pht(
                "%s\n\n    %s\n\n%s\n\n    %s\n",
                &[
                    terminal::PhtArg::Text(&terminal::pht(
                        "To update shell completion support for \"%s\", your existing \"%s\" file will be modified. This line will be removed:",
                        &[
                            terminal::PhtArg::Text(shell),
                            terminal::PhtArg::Text("~/.profile"),
                        ],
                    )),
                    terminal::PhtArg::Text(removed),
                    terminal::PhtArg::Text("This line will be added:"),
                    terminal::PhtArg::Text(line),
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
                            terminal::PhtArg::Text(shell),
                            terminal::PhtArg::Text("~/.profile"),
                        ],
                    )),
                    terminal::PhtArg::Text(line),
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
                            terminal::PhtArg::Text(shell),
                            terminal::PhtArg::Text("~/.profile"),
                        ],
                    )),
                    terminal::PhtArg::Text(line),
                ],
            ),
        ),
    }
}

/// The profile content to write: the rewritten data, a fresh file, or
/// the old data plus the new line.
fn install_final_data(
    new_data: String,
    data: &str,
    line: &str,
    had_replacement: bool,
    is_new: bool,
) -> String {
    if had_replacement {
        new_data
    } else if is_new {
        format!("{line}\n")
    } else {
        format!("{}\n\n{line}\n", data.trim_end())
    }
}

/// Map an install-prompt failure into its install error: declines
/// abort, everything else is fatal.
fn install_prompt_error(message: String) -> conduit::ClassifiedError {
    if message == crate::prompt::ABORT {
        conduit::ClassifiedError::user_abort(message)
    } else {
        conduit::ClassifiedError::fatal(message)
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
    let (status, shell) = resolve_install_shell(shell_arg, shell_env, ansi)?;
    out.push_str(&status);
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
    let (prompt, echo) = install_prompt_text(&shell, replaced.as_deref(), &line, is_new);
    out.push_str(&echo);
    let final_data = install_final_data(new_data, &data, &line, had_replacement, is_new);
    crate::prompts::execute_prompt_saving(
        input,
        crate::prompts::INSTALL_PROMPT_KEY,
        crate::prompts::INSTALL_PROMPT_DESCRIPTION,
        prompt,
        stdin_tty,
        saved,
        save,
    )
    .map_err(install_prompt_error)?;
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

/// Print the argument spelling-correction notices to stderr.
fn report_parse_notices(notices: &[String]) {
    for notice in notices {
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
}

/// Triggers of the `arc`-toolset aliases (extra workflow candidates
/// for completion).
fn arc_alias_triggers() -> Vec<String> {
    crate::alias::load_aliases()
        .map(|(aliases, _)| {
            aliases
                .into_iter()
                .filter(|alias| alias.toolset == "arc")
                .map(|alias| alias.trigger)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Run the complete-arguments flow: reject the install/generate flags,
/// then complete `words` at `current`.
fn run_complete(
    words: Vec<String>,
    current: Option<String>,
    is_generate: bool,
    is_shell: bool,
) -> ExitCode {
    use conduit::ClassifiedError;
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
    let aliases = arc_alias_triggers();
    match autocomplete(&words, current.as_deref(), &aliases) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => ClassifiedError::fatal(message).report(),
    }
}

/// Run `--generate` (rules only) or the install flow. `Ok` means the
/// run continues to the NOTE footer; `Err` carries the exit code.
fn run_generate_or_install(
    is_generate: bool,
    current: Option<String>,
    shell_arg: String,
    stdin_tty: bool,
) -> Result<(), ExitCode> {
    use conduit::ClassifiedError;
    if is_generate {
        if current.is_some() {
            return Err(ClassifiedError::toolset_usage(
                "You can not use \"--current\" when generating rules.".to_string(),
            )
            .report());
        }
        if !shell_arg.is_empty() {
            return Err(ClassifiedError::toolset_usage(
                "The flags \"--generate\" and \"--shell\" are mutually exclusive. The \"--shell\" flag selects which shell to install support for, but the \"--generate\" suppresses installation.".to_string(),
            )
            .report());
        }
        print!("{}", generate_in(&default_shell_root()));
        return Ok(());
    }
    if current.is_some() {
        return Err(ClassifiedError::toolset_usage(
            "You can not use \"--current\" when installing support.".to_string(),
        )
        .report());
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
    let mut saver =
        crate::prompts::response_saver(crate::prompts::INSTALL_PROMPT_KEY, user_file, local_file);
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
        Err(error) => return Err(error.report()),
    }
    print!("{}", generate_in(&default_shell_root()));
    Ok(())
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    use conduit::ClassifiedError;
    use std::io::IsTerminal;
    // T1153 R10: `--help` reroutes to the generated workflow help
    // (ceb `PhutilArgumentParser.php:461-475`).
    if wants_help(argv) {
        print!("{}", usage());
        return ExitCode::SUCCESS;
    }
    let stdin_tty = std::io::stdin().is_terminal();
    let parsed = match parse_args(argv, stdin_tty) {
        Ok(parsed) => parsed,
        Err(message) => return ClassifiedError::toolset_parse_error(message).report(),
    };
    report_parse_notices(&parsed.notices);
    let is_generate = parsed.flag("generate");
    let shell_arg = parsed.value("shell").unwrap_or("").to_string();
    let is_shell = !shell_arg.is_empty();
    let current = parsed.value("current").map(str::to_string);
    let words = parsed.positionals;

    if !words.is_empty() {
        return run_complete(words, current, is_generate, is_shell);
    }
    if let Err(code) = run_generate_or_install(is_generate, current, shell_arg, stdin_tty) {
        return code;
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
