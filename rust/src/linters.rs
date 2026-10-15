//! `arc-rs linters`: list and inspect lint rules.
//!
//! Port of `src/workflow/ArcanistLintersWorkflow.php`, including exact
//! and search selection (`:66-141`), status detection with version
//! probing (`getLintersInfo()`, `:147-201`), and the plain/verbose
//! renderers (`renderLinters()`, `:206-306`).
//!
//! Divergences from PHP:
//! - Markup (colors, bold) is stripped: output matches piped (non-TTY)
//!   `arc`.
//! - `phutil_console_wrap()` is approximated as word-wrap at 80 columns
//!   (see [`arc_rs::lint::word_wrap`]).
//! - Version-probe failure text cannot match phutil's `CommandException`
//!   rendering; failures surface as `CommandException` with a plain
//!   message describing the failed command.
//! - The XHP rules table lists the ported rules only (1 of ~100), and
//!   its borders are reconstructed (phutil is not vendored).
//! - Linter order is class-name order; PHP uses class-map order.
//! - PHP `getVersion()` quirks are preserved: `lessc` never reports a
//!   version (its method falls off the end), and `jsonlint` tolerates
//!   a nonzero exit.

extern crate self as arc_rs;

use std::path::Path;
use std::process::{Command, ExitCode};

pub const NAME: &str = "linters";

fn usage() -> String {
    "usage: arc-rs linters [--verbose] [--search TERM] [NAME...]\n\
     \n\
     Lists the available linters.\n\
     \n\
     With no arguments, shows every known linter. Positional NAME\n\
     arguments select linters by exact configuration name and imply\n\
     --verbose. --search selects linters whose name or description\n\
     contains TERM (case-insensitive) and may repeat.\n\
     \n\
     Options:\n\
     \x20 --verbose           Show more detailed information about linters.\n\
     \x20 --search TERM       Search case-insensitively for a linter by\n\
     \x20                     name. May be repeated.\n\
     \x20 -h, --help          Show this help.\n"
        .to_string()
}

/// One registered linter: display name plus a one-line summary. Kept
/// for API compatibility; the workflow itself uses [`LINTERS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinterInfo {
    pub name: String,
    pub description: String,
}

/// Every ported linter: display name plus a one-line summary.
#[must_use]
pub fn available_linters() -> Vec<LinterInfo> {
    arc_rs::lint::all_linters()
        .iter()
        .map(|linter| LinterInfo {
            name: linter.name().to_string(),
            description: describe(linter.name()).to_string(),
        })
        .collect()
}

/// One-line summary for a display name.
#[must_use]
pub fn describe(name: &str) -> &'static str {
    if let Some(text) = describe_text_linters(name) {
        return text;
    }
    if let Some(text) = describe_script_linters(name) {
        return text;
    }
    if let Some(text) = describe_compiled_linters(name) {
        return text;
    }
    "Lint rule."
}

/// Summaries for the text, file, and generic linters.
fn describe_text_linters(name: &str) -> Option<&'static str> {
    match name {
        "TXT" => Some("Basic text checks: tabs, trailing whitespace, newlines."),
        "NAME" => Some("Requires boring file names."),
        "JSON" => Some("Detects syntax errors in JSON files."),
        "XML" => Some("Detects malformed XML files."),
        "MERGECONFLICT" => Some("Detects unresolved merge-conflict markers."),
        "GEN" => Some("Skips generated files."),
        "NOLINT" => Some("Lets files opt out of lint."),
        "CHMOD" => Some("Flags dubious executable bits."),
        "SPELL" => Some("Detects common spelling mistakes."),
        "S&RX" => Some("Runs a script and parses output with a regex."),
        "COMPOSER" => Some("Flags stale composer.lock files."),
        "PHL" => Some("Enforces one class per file in PHP."),
        _ => None,
    }
}

/// Summaries for the web and script-language linters.
fn describe_script_linters(name: &str) -> Option<&'static str> {
    match name {
        "JSHint" => Some("Checks JavaScript with JSHint."),
        "JSCS" => Some("Checks JavaScript code style."),
        "CSSLint" => Some("Checks CSS with CSSLint."),
        "LESSC" => Some("Checks LESS files with lessc."),
        "GJSLINT" => Some("Checks JavaScript with Closure Linter."),
        "COFFEE" => Some("Checks CoffeeScript with CoffeeLint."),
        "flake8" => Some("Checks Python with flake8."),
        "PYFLAKES" => Some("Checks Python with PyFlakes."),
        "PyLint" => Some("Checks Python with PyLint."),
        "PEP8" => Some("Checks Python style with pep8."),
        "RuboCop" => Some("Checks Ruby with RuboCop."),
        _ => None,
    }
}

/// Summaries for the compiled-language and remaining linters.
fn describe_compiled_linters(name: &str) -> Option<&'static str> {
    match name {
        "RUBY" => Some("Checks Ruby syntax."),
        "cppcheck" => Some("Analyzes C/C++ with cppcheck."),
        "C++ Google's Styleguide" => Some("Checks C++ style with cpplint."),
        "GOLINT" => Some("Checks Go with Golint."),
        "HLINT" => Some("Checks Haskell with HLint."),
        "C#" => Some("Checks C# with a Mono runner."),
        "PHPCS" => Some("Checks PHP with PHP CodeSniffer."),
        "PHP" => Some("Checks PHP syntax with php -l."),
        "PUPPETLINT" => Some("Checks Puppet with puppet-lint."),
        "XHP" => Some("Lints legacy XHP PHP files."),
        _ => None,
    }
}

/// Find a registered linter by display name (case-insensitive).
#[must_use]
pub fn find_linter(name: &str) -> Option<LinterInfo> {
    available_linters()
        .into_iter()
        .find(|info| info.name.eq_ignore_ascii_case(name))
}

/* -(  Registry  )------------------------------------------------------------ */

/// One `getLinterConfigurationOptions()` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinterOption {
    pub name: &'static str,
    pub option_type: &'static str,
    pub help: &'static str,
}

/// How to detect a linter version (`getVersion()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPattern {
    /// `^\d+\.\d+\.\d+$` on stdout.
    ExactTriple,
    /// `^Cppcheck \d+\.\d+(\.\d+)?$` on stdout.
    Cppcheck,
    /// `^v\d+\.\d+\.\d+$` on stdout.
    CssV,
    /// `^(\d+\.\d+(\.\d+)?)\b` prefix on stdout.
    PrefixMinor,
    /// `HLint v(.*),` scan on stdout.
    Hlint,
    /// `^jshint v\d+\.\d+\.\d+$` on stderr.
    JshintStderr,
    /// `^(\d+\.\d+\.\d+)$` on stdout, nonzero exit tolerated.
    LenientTriple,
    /// `^puppet-lint \d+\.\d+\.\d+$` on stdout.
    PuppetLint,
    /// `^(\d+\.\d+\.\d+)( Python.*)?$` on stdout.
    Pyflakes,
    /// `^pylint \d+\.\d+\.\d+` prefix on stdout.
    Pylint,
    /// `^PHP_CodeSniffer version \d+\.\d+\.\d+\b` prefix on stdout.
    Phpcs,
    /// `^ruby \d+\.\d+\.\d+` prefix on stdout (the PHP `(...)+`
    /// repetition quirk is not reproduced: no real version repeats).
    Ruby,
    /// Raw `php --run 'echo phpversion();'` output, trimmed.
    PhpVersion,
    /// Run `<binary> --version`, report nothing (the PHP `lessc`
    /// method runs the command but never returns the version).
    RunOnly,
}

/// An external tool's binary plus version detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalMeta {
    pub binary: &'static str,
    pub version: Option<VersionPattern>,
}

/// One linter class: every `getInfo*()`/options/version fact the
/// `linters` workflow renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinterMeta {
    pub class: &'static str,
    pub config_name: &'static str,
    pub display: &'static str,
    pub info_name: &'static str,
    pub uri: Option<&'static str>,
    pub description: Option<&'static str>,
    /// Custom options, shown before tool/base options (`$options +
    /// parent::...`, except `lessc` which appends; see
    /// [`LinterMeta::append_custom_options`]).
    pub custom_options: &'static [LinterOption],
    /// Append custom options after the tool/base options (only
    /// `lessc`: `parent::... + $options`).
    pub append_custom_options: bool,
    /// `canCustomizeLintSeverities()`: whether the severity/standard
    /// base options apply.
    pub customizes_severities: bool,
    pub external: Option<ExternalMeta>,
}

/// Base configuration options (`ArcanistLinter.php:392-421`).
const BASE_OPTIONS: &[LinterOption] = &[
    LinterOption {
        name: "severity",
        option_type: "optional map<string, string>",
        help: "Map a severity ('advice', 'autofix', 'warning', 'error', or 'disabled') to a lint code, e.g. \"variable.unused\" or \"322\".",
    },
    LinterOption {
        name: "severity.rules",
        option_type: "optional list<wild>",
        help: "Add regular expression rules to override severity based on message text.",
    },
    LinterOption {
        name: "standard",
        option_type: "optional string",
        help: "Select a builtin standard to use.",
    },
];

/// Tool options for an external linter
/// (`ArcanistExternalLinter.php:464-504`). `{binary}` and `{display}`
/// tokens are substituted at render time.
const TOOL_OPTIONS: &[LinterOption] = &[
    LinterOption {
        name: "bin",
        option_type: "optional string",
        help: "The binary to run (default: '{binary}').",
    },
    LinterOption {
        name: "flags",
        option_type: "optional list<string>",
        help: "Additional flags to pass to '{binary}' when running {display}.",
    },
    LinterOption {
        name: "version",
        option_type: "optional string",
        help: "Minimum required version of '{binary}'.",
    },
];

impl LinterMeta {
    /// Full option list in display order.
    #[must_use]
    pub fn options(&self) -> Vec<LinterOption> {
        let mut base = Vec::new();
        if self.external.is_some() {
            base.extend(TOOL_OPTIONS.iter().copied());
        }
        if self.customizes_severities {
            base.extend(BASE_OPTIONS.iter().copied());
        }
        if self.append_custom_options {
            base.extend(self.custom_options.iter().copied());
            base
        } else {
            let mut out = self.custom_options.to_vec();
            out.extend(base);
            out
        }
    }

    /// Render an option's help text, substituting tool tokens.
    #[must_use]
    pub fn option_help(&self, option: &LinterOption) -> String {
        let binary = self.external.map(|external| external.binary).unwrap_or("");
        option
            .help
            .replace("{binary}", binary)
            .replace("{display}", self.display)
    }
}

/// All 33 linter classes, in class-name order.
pub const LINTERS: &[LinterMeta] = &[
    LinterMeta {
        class: "ArcanistChmodLinter",
        config_name: "chmod",
        display: "CHMOD",
        info_name: "Chmod",
        uri: None,
        description: Some("Make sure files have appropriate permissions."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistClosureLinter",
        config_name: "gjslint",
        display: "GJSLINT",
        info_name: "Closure Linter",
        uri: Some("https://developers.google.com/closure/utilities/"),
        description: Some("Uses Google's Closure Linter to check JavaScript code."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "gjslint",
            version: None,
        }),
    },
    LinterMeta {
        class: "ArcanistCoffeeLintLinter",
        config_name: "coffeelint",
        display: "COFFEE",
        info_name: "CoffeeLint",
        uri: Some("https://github.com/clutchski/coffeelint"),
        description: Some("Use `coffeelint` to check for issues with CoffeeScript source files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "coffeelint",
            version: Some(VersionPattern::ExactTriple),
        }),
    },
    LinterMeta {
        class: "ArcanistComposerLinter",
        config_name: "composer",
        display: "COMPOSER",
        info_name: "Composer Dependency Manager",
        uri: None,
        description: Some("A linter for Composer related files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistCppcheckLinter",
        config_name: "cppcheck",
        display: "cppcheck",
        info_name: "C++ linter",
        uri: Some("http://cppcheck.sourceforge.net"),
        description: Some("Use `cppcheck` to perform static analysis on C/C++ code."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "cppcheck",
            version: Some(VersionPattern::Cppcheck),
        }),
    },
    LinterMeta {
        class: "ArcanistCpplintLinter",
        config_name: "cpplint",
        display: "C++ Google's Styleguide",
        info_name: "C++ Google's Styleguide",
        uri: None,
        description: None,
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "cpplint",
            version: None,
        }),
    },
    LinterMeta {
        class: "ArcanistCSharpLinter",
        config_name: "csharp",
        display: "C#",
        info_name: "C#",
        uri: None,
        description: None,
        custom_options: &[LinterOption {
            name: "embedded",
            option_type: "optional bool",
            help: "true if linter is run in embedded mode (vs. as a build step)",
        }],
        append_custom_options: false,
        // Its options override drops the base table entirely.
        customizes_severities: false,
        external: None,
    },
    LinterMeta {
        class: "ArcanistCSSLintLinter",
        config_name: "csslint",
        display: "CSSLint",
        info_name: "CSSLint",
        uri: Some("http://csslint.net"),
        description: Some("Use `csslint` to detect issues with CSS source files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "csslint",
            version: Some(VersionPattern::CssV),
        }),
    },
    LinterMeta {
        class: "ArcanistFlake8Linter",
        config_name: "flake8",
        display: "flake8",
        info_name: "Python Flake8 multi-linter",
        uri: Some("https://pypi.python.org/pypi/flake8"),
        description: Some(
            "Uses `flake8` to run several linters (PyFlakes, pep8, and a McCabe complexity checker) on Python source files.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "flake8",
            version: Some(VersionPattern::PrefixMinor),
        }),
    },
    LinterMeta {
        class: "ArcanistGeneratedLinter",
        config_name: "generated",
        display: "GEN",
        info_name: "Generated Code",
        uri: None,
        description: Some(
            "Disables lint for files that are marked as \"@generated\", indicating that they contain generated code.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: false,
        external: None,
    },
    LinterMeta {
        class: "ArcanistGoLintLinter",
        config_name: "golint",
        display: "GOLINT",
        info_name: "Golint",
        uri: Some("https://github.com/golang/lint"),
        description: Some("Golint is a linter for Go source code."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "golint",
            version: None,
        }),
    },
    LinterMeta {
        class: "ArcanistHLintLinter",
        config_name: "hlint",
        display: "HLINT",
        info_name: "Haskell Linter",
        uri: Some("https://github.com/ndmitchell/hlint"),
        description: Some("HLint is a linter for Haskell code."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "hlint",
            version: Some(VersionPattern::Hlint),
        }),
    },
    LinterMeta {
        class: "ArcanistJscsLinter",
        config_name: "jscs",
        display: "JSCS",
        info_name: "JavaScript Code Style Checker",
        uri: Some("https://github.com/mdevils/node-jscs/"),
        description: Some("Use `jscs` to detect style issues with JavaScript source files."),
        custom_options: &[
            LinterOption {
                name: "jscs.preset",
                option_type: "optional string",
                help: "If set, the preset passed to jscs via `--preset`. See the jscs documentation for details.",
            },
            LinterOption {
                name: "jscs.config_path",
                option_type: "optional string",
                help: "If set, the config file path passed to jscs. See the jscs documentation for details.",
            },
        ],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "jscs",
            version: Some(VersionPattern::ExactTriple),
        }),
    },
    LinterMeta {
        class: "ArcanistJSHintLinter",
        config_name: "jshint",
        display: "JSHint",
        info_name: "JSHint",
        uri: Some("http://www.jshint.com/"),
        description: Some("Use `jshint` to detect issues with JavaScript source files."),
        custom_options: &[LinterOption {
            name: "jshint.config",
            option_type: "optional string",
            help: "The location of the jshint configuration file (.jshintrc).",
        }],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "jshint",
            version: Some(VersionPattern::JshintStderr),
        }),
    },
    LinterMeta {
        class: "ArcanistJSONLinter",
        config_name: "json",
        display: "JSON",
        info_name: "JSON Lint",
        uri: None,
        description: Some("Detect syntax errors in JSON files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: false,
        external: None,
    },
    LinterMeta {
        class: "ArcanistJSONLintLinter",
        config_name: "jsonlint",
        display: "JSON",
        info_name: "JSON Lint",
        uri: Some("https://github.com/zaach/jsonlint"),
        description: Some("Use `jsonlint` to detect syntax errors in JSON files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "jsonlint",
            version: Some(VersionPattern::LenientTriple),
        }),
    },
    LinterMeta {
        class: "ArcanistLesscLinter",
        config_name: "lessc",
        display: "LESSC",
        info_name: "CSS pre-processor",
        uri: Some("http://lesscss.org/"),
        description: Some(
            "Use the `--lint` mode provided by `lessc` to detect errors in Less source files.",
        ),
        custom_options: &[
            LinterOption {
                name: "lessc.strict-math",
                option_type: "optional bool",
                help: "If set, lessc will be run with `--strict-math=on`.",
            },
            LinterOption {
                name: "lessc.strict-units",
                option_type: "optional bool",
                help: "If set, lessc will be run with `--strict-units=on`.",
            },
        ],
        append_custom_options: true,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "lessc",
            version: Some(VersionPattern::RunOnly),
        }),
    },
    LinterMeta {
        class: "ArcanistMergeConflictLinter",
        config_name: "merge-conflict",
        display: "MERGECONFLICT",
        info_name: "Merge Conflicts",
        uri: None,
        description: Some(
            "Raises errors on unresolved merge conflicts in source files, to catch mistakes where a conflicted file is accidentally marked as resolved.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistNoLintLinter",
        config_name: "nolint",
        display: "NOLINT",
        info_name: "Lint Disabler",
        uri: None,
        description: Some(
            "Allows you to disable all lint messages for a file by putting \"@nolint\" in the file body.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: false,
        external: None,
    },
    LinterMeta {
        class: "ArcanistPEP8Linter",
        config_name: "pep8",
        display: "PEP8",
        info_name: "Python PEP 8",
        uri: Some("https://pypi.python.org/pypi/pep8"),
        description: Some(
            "pep8 is a tool to check your Python code against some of the style conventions in PEP 8.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "pep8",
            version: Some(VersionPattern::PrefixMinor),
        }),
    },
    LinterMeta {
        class: "ArcanistPhpcsLinter",
        config_name: "phpcs",
        display: "PHPCS",
        info_name: "PHP Code Sniffer",
        uri: Some("https://github.com/squizlabs/PHP_CodeSniffer"),
        description: Some("Use `phpcs` to detect issues with PHP source files."),
        custom_options: &[LinterOption {
            name: "phpcs.standard",
            option_type: "optional string",
            help: "The standard to use.",
        }],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "phpcs",
            version: Some(VersionPattern::Phpcs),
        }),
    },
    LinterMeta {
        class: "ArcanistPhpLinter",
        config_name: "php",
        display: "PHP",
        info_name: "php -l",
        uri: Some("http://php.net/"),
        description: Some("Checks for syntax errors in PHP files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: false,
        external: Some(ExternalMeta {
            binary: "php",
            version: Some(VersionPattern::PhpVersion),
        }),
    },
    LinterMeta {
        class: "ArcanistPhutilLibraryLinter",
        config_name: "phutil-library",
        display: "PHL",
        info_name: "Phutil Library Linter",
        uri: None,
        description: Some(
            "Make sure all the symbols used in a libphutil library are defined and known. This linter is specific to PHP source in libphutil libraries.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistPuppetLintLinter",
        config_name: "puppet-lint",
        display: "PUPPETLINT",
        info_name: "puppet-lint",
        uri: Some("https://github.com/rodjek/puppet-lint"),
        description: Some("Use `puppet-lint` to detect issues with Puppet manifests."),
        custom_options: &[LinterOption {
            name: "puppet-lint.config",
            option_type: "optional string",
            help: "The location of the puppet-lint configuration file (Rakefile).",
        }],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "puppet-lint",
            version: Some(VersionPattern::PuppetLint),
        }),
    },
    LinterMeta {
        class: "ArcanistPyFlakesLinter",
        config_name: "pyflakes",
        display: "PYFLAKES",
        info_name: "Python PyFlakes",
        uri: Some("https://pypi.python.org/pypi/pyflakes"),
        description: Some(
            "PyFlakes is a simple program which checks Python source files for errors.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: false,
        external: Some(ExternalMeta {
            binary: "pyflakes",
            version: Some(VersionPattern::Pyflakes),
        }),
    },
    LinterMeta {
        class: "ArcanistPyLintLinter",
        config_name: "pylint",
        display: "PyLint",
        info_name: "PyLint",
        uri: Some("http://www.pylint.org/"),
        description: Some(
            "PyLint is a Python source code analyzer which looks for programming errors, helps enforcing a coding standard and sniffs for some code smells.",
        ),
        custom_options: &[LinterOption {
            name: "pylint.config",
            option_type: "optional string",
            help: "Pass in a custom configuration file path.",
        }],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "pylint",
            version: Some(VersionPattern::Pylint),
        }),
    },
    LinterMeta {
        class: "ArcanistRuboCopLinter",
        config_name: "rubocop",
        display: "RuboCop",
        info_name: "Ruby static code analyzer",
        uri: Some("http://batsov.com/rubocop"),
        description: Some(
            "RuboCop is a Ruby static code analyzer, based on the community Ruby style guide.",
        ),
        custom_options: &[LinterOption {
            name: "rubocop.config",
            option_type: "optional string",
            help: "A custom configuration file.",
        }],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "rubocop",
            version: Some(VersionPattern::ExactTriple),
        }),
    },
    LinterMeta {
        class: "ArcanistRubyLinter",
        config_name: "ruby",
        display: "RUBY",
        info_name: "Ruby",
        uri: Some("https://www.ruby-lang.org/"),
        description: Some("Use `ruby` to check for syntax errors in Ruby source files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: Some(ExternalMeta {
            binary: "ruby",
            version: Some(VersionPattern::Ruby),
        }),
    },
    LinterMeta {
        class: "ArcanistScriptAndRegexLinter",
        config_name: "script-and-regex",
        display: "S&RX",
        info_name: "Script and Regex",
        uri: None,
        description: Some(
            "Run an external script, then parse its output with a regular expression. This is a generic binding that can be used to run custom lint scripts.",
        ),
        custom_options: &[
            LinterOption {
                name: "script-and-regex.script",
                option_type: "string",
                help: "Script to execute.",
            },
            LinterOption {
                name: "script-and-regex.regex",
                option_type: "regex",
                help: "The regex to process output with.",
            },
        ],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistSpellingLinter",
        config_name: "spelling",
        display: "SPELL",
        info_name: "Spellchecker",
        uri: None,
        description: Some("Detects common misspellings of English words."),
        custom_options: &[LinterOption {
            name: "spelling.dictionaries",
            option_type: "optional list<string>",
            help: "Pass in custom dictionaries.",
        }],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistTextLinter",
        config_name: "text",
        display: "TXT",
        info_name: "Basic Text Linter",
        uri: None,
        description: Some("Basic linting for text files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistXHPASTLinter",
        config_name: "xhpast",
        display: "XHP",
        info_name: "XHPAST Lint",
        uri: None,
        description: Some(
            "Use XHPAST to check for errors, some of which can be automatically fixed.",
        ),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
    LinterMeta {
        class: "ArcanistXMLLinter",
        config_name: "xml",
        display: "XML",
        info_name: "SimpleXML Linter",
        uri: None,
        description: Some("Uses SimpleXML to detect formatting errors in XML files."),
        custom_options: &[],
        append_custom_options: false,
        customizes_severities: true,
        external: None,
    },
];

/* -(  Versions  )------------------------------------------------------------ */

/// Parse ASCII digits, returning (value, rest).
fn take_digits(text: &str) -> Option<(&str, &str)> {
    let end = text.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    Some((&text[..end], &text[end..]))
}

/// Parse `major.minor`, returning (version, rest).
fn take_minor(text: &str) -> Option<(String, &str)> {
    let (_, rest) = take_digits(text)?;
    let rest = rest.strip_prefix('.')?;
    let (_, rest) = take_digits(rest)?;
    let len = text.len() - rest.len();
    Some((text[..len].to_string(), rest))
}

/// Parse `major.minor[.patch]` greedily, returning (version, rest).
fn take_triple(text: &str) -> Option<(String, &str)> {
    let (mut version, mut rest) = take_minor(text)?;
    if let Some(dot_rest) = rest.strip_prefix('.')
        && let Some((_, after)) = take_digits(dot_rest)
    {
        version.push_str(&rest[..rest.len() - after.len()]);
        rest = after;
    }
    Some((version, rest))
}

/// True for regex word characters (for `\b` handling).
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// True when `rest` starts a word boundary (end of string or a
/// non-word byte).
fn at_boundary(rest: &str) -> bool {
    rest.bytes().next().is_none_or(|b| !is_word_byte(b))
}

/// Parse a version command's output per `pattern`, mirroring each
/// linter's `getVersion()` regex. `execx()` output keeps its trailing
/// newline, which `$` tolerates; inputs are trimmed first.
pub fn parse_version(pattern: VersionPattern, stdout: &str, stderr: &str) -> Option<String> {
    match pattern {
        VersionPattern::ExactTriple | VersionPattern::LenientTriple => {
            parse_exact_triple_version(stdout)
        }
        VersionPattern::Cppcheck => parse_cppcheck_version(stdout),
        VersionPattern::CssV => parse_css_version(stdout),
        VersionPattern::PrefixMinor => parse_prefix_minor_version(stdout),
        VersionPattern::Hlint => parse_hlint_version(stdout),
        VersionPattern::JshintStderr => parse_jshint_version(stderr),
        VersionPattern::PuppetLint => parse_puppet_lint_version(stdout),
        VersionPattern::Pyflakes => parse_pyflakes_version(stdout),
        VersionPattern::Pylint => parse_pylint_version(stdout),
        VersionPattern::Phpcs => parse_phpcs_version(stdout),
        VersionPattern::Ruby => parse_ruby_version(stdout),
        VersionPattern::PhpVersion => parse_php_version(stdout),
        VersionPattern::RunOnly => None,
    }
}

/// Parse a full `major.minor.patch` triple consuming all of `text`.
fn take_full_triple(text: &str) -> Option<String> {
    let (version, rest) = take_triple(text)?;
    if !rest.is_empty() || version.bytes().filter(|b| *b == b'.').count() != 2 {
        return None;
    }
    Some(version)
}

/// `^\d+\.\d+\.\d+$` on stdout.
fn parse_exact_triple_version(stdout: &str) -> Option<String> {
    take_full_triple(stdout.trim())
}

/// `^Cppcheck \d+\.\d+(\.\d+)?$` on stdout.
fn parse_cppcheck_version(stdout: &str) -> Option<String> {
    let text = stdout.trim().strip_prefix("Cppcheck ")?;
    let (version, rest) = take_triple(text)?;
    if !rest.is_empty() {
        return None;
    }
    Some(version)
}

/// `^v\d+\.\d+\.\d+$` on stdout.
fn parse_css_version(stdout: &str) -> Option<String> {
    let text = stdout.trim().strip_prefix('v')?;
    take_full_triple(text)
}

/// `^(\d+\.\d+(\.\d+)?)\b` prefix on stdout.
fn parse_prefix_minor_version(stdout: &str) -> Option<String> {
    let text = stdout.trim_start();
    let (version, rest) = take_triple(text)?;
    if !at_boundary(rest) {
        return None;
    }
    Some(version)
}

/// `HLint v(.*),` scan on stdout.
fn parse_hlint_version(stdout: &str) -> Option<String> {
    let (_, after) = stdout.split_once("HLint v")?;
    let line = after.split('\n').next().unwrap_or(after);
    let (version, _) = line.rsplit_once(',')?;
    let version = version.trim();
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

/// `^jshint v\d+\.\d+\.\d+$` on stderr.
fn parse_jshint_version(stderr: &str) -> Option<String> {
    let text = stderr.trim().strip_prefix("jshint v")?;
    take_full_triple(text)
}

/// `^puppet-lint \d+\.\d+\.\d+$` on stdout.
fn parse_puppet_lint_version(stdout: &str) -> Option<String> {
    let text = stdout.trim().strip_prefix("puppet-lint ")?;
    take_full_triple(text)
}

/// `^(\d+\.\d+\.\d+)( Python.*)?$` on stdout.
fn parse_pyflakes_version(stdout: &str) -> Option<String> {
    let text = stdout.trim();
    let (version, rest) = take_triple(text)?;
    if version.bytes().filter(|b| *b == b'.').count() != 2 {
        return None;
    }
    if rest.is_empty() || rest.starts_with(" Python") {
        Some(version)
    } else {
        None
    }
}

/// `^pylint \d+\.\d+\.\d+` prefix on stdout.
fn parse_pylint_version(stdout: &str) -> Option<String> {
    let text = stdout.trim_start().strip_prefix("pylint ")?;
    let (version, rest) = take_triple(text)?;
    if version.bytes().filter(|b| *b == b'.').count() != 2 {
        return None;
    }
    let _ = rest;
    Some(version)
}

/// `^PHP_CodeSniffer version \d+\.\d+\.\d+\b` prefix on stdout.
fn parse_phpcs_version(stdout: &str) -> Option<String> {
    let text = stdout
        .trim_start()
        .strip_prefix("PHP_CodeSniffer version ")?;
    let (version, rest) = take_triple(text)?;
    if version.bytes().filter(|b| *b == b'.').count() != 2 || !at_boundary(rest) {
        return None;
    }
    Some(version)
}

/// `^ruby \d+\.\d+\.\d+` prefix on stdout.
fn parse_ruby_version(stdout: &str) -> Option<String> {
    let mut text = stdout.trim_start().strip_prefix("ruby ")?;
    let mut version = None;
    while let Some((triple, rest)) = take_triple(text) {
        if triple.bytes().filter(|b| *b == b'.').count() != 2 {
            break;
        }
        version = Some(triple);
        text = rest;
    }
    version
}

/// Raw `php --run 'echo phpversion();'` output, trimmed.
fn parse_php_version(stdout: &str) -> Option<String> {
    let version = stdout.trim();
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

/// A version-probe failure, shown as an exception in verbose mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinterError {
    pub class: &'static str,
    pub message: String,
}

/// PHP class name for a canonical `.arclint` configuration name, for
/// cache-version computation (`get_class($linter)`). The `closure`
/// alias resolves to the `gjslint` entry; `filename` has no registry
/// entry and maps directly.
#[must_use]
pub fn class_for_config(config_name: &str) -> Option<&'static str> {
    if config_name == "filename" {
        return Some("ArcanistFilenameLinter");
    }
    let lookup = if config_name == "closure" {
        "gjslint"
    } else {
        config_name
    };
    LINTERS
        .iter()
        .find(|meta| meta.config_name == lookup)
        .map(|meta| meta.class)
}

/// Version detection pattern for a `.arclint` configuration name, if
/// the linter implements `getVersion()`. Used for N4 `version`
/// requirement checks against configured binaries.
#[must_use]
pub fn version_pattern_for(config_name: &str) -> Option<VersionPattern> {
    LINTERS
        .iter()
        .find(|meta| meta.config_name == config_name)
        .and_then(|meta| meta.external)
        .and_then(|external| external.version)
}

/// Probe a configured linter's version (`getVersion()`): `Ok(None)`
/// keeps the configured status without a version; `Err` flips the
/// status to error.
pub fn probe_version(meta: &LinterMeta) -> Result<Option<String>, LinterError> {
    if meta.class == "ArcanistXHPASTLinter" {
        // `count($this->rules)`: the ported rules only.
        return Ok(Some(ported_xhpast_rules().len().to_string()));
    }
    let Some(external) = &meta.external else {
        return Ok(None);
    };
    let Some(pattern) = external.version else {
        return Ok(None);
    };
    let binary = external.binary;
    let mut command = Command::new(binary);
    if pattern == VersionPattern::PhpVersion {
        command.arg("--run").arg("echo phpversion();");
    } else {
        command.arg("--version");
    }
    let label = if pattern == VersionPattern::PhpVersion {
        format!("{binary} --run 'echo phpversion();'")
    } else {
        format!("{binary} --version")
    };
    let output = arc_rs::lint::spawn_with_retry(&mut command).map_err(|error| LinterError {
        class: "CommandException",
        message: format!("Failed to run '{label}': {error}."),
    })?;
    if !output.status.success() && pattern != VersionPattern::LenientTriple {
        let code = output
            .status
            .code()
            .map_or("signal".to_string(), |c| c.to_string());
        return Err(LinterError {
            class: "CommandException",
            message: format!("'{label}' exited with status {code}."),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(parse_version(pattern, &stdout, &stderr))
}

/// Probe an explicit `binary` for `pattern` (`<binary> --version`, or
/// the PHP version command), returning the parsed version. Probe
/// failures and unparseable output yield `None` (N4 `version` checks
/// treat the linter as inactive then, like a missing binary).
#[must_use]
pub fn probe_binary_version(pattern: VersionPattern, binary: &Path) -> Option<String> {
    let mut command = Command::new(binary);
    if pattern == VersionPattern::PhpVersion {
        command.arg("--run").arg("echo phpversion();");
    } else {
        command.arg("--version");
    }
    let output = arc_rs::lint::spawn_with_retry(&mut command).ok()?;
    if !output.status.success() && pattern != VersionPattern::LenientTriple {
        return None;
    }
    parse_version(
        pattern,
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Ported XHP rules as (id, class, name), for the additional
/// information table and the version count.
pub fn ported_xhpast_rules() -> Vec<(u32, &'static str, &'static str)> {
    vec![(
        2,
        "ArcanistUnableToParseXHPASTLinterRule",
        "Unable to Parse",
    )]
}

/// The XHP `getAdditionalInformation()` rules table, reconstructed
/// (phutil is not vendored).
pub fn xhpast_rules_table() -> String {
    let rules = ported_xhpast_rules();
    let mut id_width = "ID".len();
    let mut class_width = "Class".len();
    let mut name_width = "Name".len();
    for (id, class, name) in &rules {
        id_width = id_width.max(id.to_string().len());
        class_width = class_width.max(class.len());
        name_width = name_width.max(name.len());
    }
    let border = format!(
        "+-{}-+-{}-+-{}-+\n",
        "-".repeat(id_width),
        "-".repeat(class_width),
        "-".repeat(name_width)
    );
    let row = |id: &str, class: &str, name: &str| {
        format!("| {id:<id_width$} | {class:<class_width$} | {name:<name_width$} |\n")
    };
    let mut out = border.clone();
    out.push_str(&row("ID", "Class", "Name"));
    out.push_str(&border);
    for (id, class, name) in &rules {
        out.push_str(&row(&id.to_string(), class, name));
    }
    out.push_str(&border);
    out
}

/* -(  Status and selection  )-------------------------------------------------- */

/// Linter status (`STATUS_CONFIGURED`, `STATUS_AVAILABLE`,
/// `STATUS_ERROR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinterStatus {
    Configured,
    Available,
    Error,
}

impl LinterStatus {
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            LinterStatus::Configured => "CONFIGURED",
            LinterStatus::Available => "AVAILABLE",
            LinterStatus::Error => "ERROR",
        }
    }
}

/// One linter with resolved status, version, and probe error.
#[derive(Debug, Clone)]
pub struct LinterEntry {
    pub meta: &'static LinterMeta,
    pub status: LinterStatus,
    pub version: Option<String>,
    pub error: Option<LinterError>,
}

/// Resolve the configured linter classes (`newLintEngine()` plus
/// `buildLinters()`, class-grouped): `--engine` is not a `linters`
/// flag, so only `.arcconfig` `lint.engine` and `.arclint` apply. A
/// missing engine (or no working copy) yields an empty set; an
/// invalid config or unknown engine class is fatal.
pub fn configured_classes(cwd: &Path) -> Result<Vec<&'static str>, String> {
    let work = match arc_rs::lint::discover_working_copy(cwd) {
        Ok(work) => work,
        Err(_) => return Ok(Vec::new()),
    };
    let Some(class) = configured_engine_class(&work) else {
        return Ok(Vec::new());
    };
    classes_for_engine(&work, &class)
}

/// Resolve the effective engine class: `.arcconfig` `lint.engine`, else
/// the configuration-driven engine when `.arclint` exists, else none.
fn configured_engine_class(work: &arc_rs::lint::WorkCopy) -> Option<String> {
    let class = arc_rs::lint::arcconfig_value(&work.project_root, "lint.engine")
        .ok()
        .flatten()
        .and_then(|value| value.as_str().map(str::to_string))
        .filter(|name| !name.is_empty());
    if class.is_some() {
        return class;
    }
    if work
        .project_root
        .join(arc_rs::arclint::CONFIG_FILE_NAME)
        .is_file()
    {
        return Some(arc_rs::lint::ENGINE_CONFIGURATION_DRIVEN.to_string());
    }
    None
}

/// Configured linter classes for an engine class name.
fn classes_for_engine(
    work: &arc_rs::lint::WorkCopy,
    class: &str,
) -> Result<Vec<&'static str>, String> {
    match class {
        arc_rs::lint::ENGINE_CONFIGURATION_DRIVEN => configuration_driven_classes(work),
        arc_rs::lint::ENGINE_COMPREHENSIVE => {
            // With no paths, only the filename linter is built.
            Ok(vec!["ArcanistFilenameLinter"])
        }
        arc_rs::lint::ENGINE_SINGLE => single_engine_class(work),
        _ => Err(format!(
            "Configured lint engine \"{class}\" is not a subclass of \"{}\", but must be.",
            arc_rs::lint::ENGINE_BASE_CLASS
        )),
    }
}

/// Classes named by `.arclint`, class-grouped in config order.
fn configuration_driven_classes(
    work: &arc_rs::lint::WorkCopy,
) -> Result<Vec<&'static str>, String> {
    let config = arc_rs::arclint::load_arclint(&work.project_root)?;
    let mut classes = Vec::new();
    for spec in &config.linters {
        if let Some(meta) = LINTERS.iter().find(|meta| meta.display == spec.display)
            && !classes.contains(&meta.class)
        {
            classes.push(meta.class);
        }
    }
    Ok(classes)
}

/// The single linter class named by `lint.engine.single.linter`.
fn single_engine_class(work: &arc_rs::lint::WorkCopy) -> Result<Vec<&'static str>, String> {
    const KEY: &str = "lint.engine.single.linter";
    let name = arc_rs::lint::arcconfig_value(&work.project_root, KEY)
        .ok()
        .flatten()
        .and_then(|value| value.as_str().map(str::to_string))
        .filter(|name| !name.is_empty());
    let Some(name) = name else {
        return Err(format!(
            "You must configure '{KEY}' with the name of a linter in order to use {}.",
            arc_rs::lint::ENGINE_SINGLE
        ));
    };
    match LINTERS.iter().find(|meta| meta.class == name) {
        Some(meta) => Ok(vec![meta.class]),
        None => Err(format!(
            "Linter '{name}' configured in '{KEY}' does not exist!"
        )),
    }
}

/// Build status entries for every linter class, probing versions of
/// the configured ones (`getLintersInfo()`).
pub fn collect_entries(configured: &[&str]) -> Vec<LinterEntry> {
    LINTERS
        .iter()
        .map(|meta| {
            let is_configured = configured.contains(&meta.class);
            let (status, version, error) = if !is_configured {
                (LinterStatus::Available, None, None)
            } else {
                match probe_version(meta) {
                    Ok(version) => (LinterStatus::Configured, version, None),
                    Err(error) => (LinterStatus::Error, None, Some(error)),
                }
            };
            LinterEntry {
                meta,
                status,
                version,
                error,
            }
        })
        .collect()
}

/// Keep entries whose configuration name is in `exact`
/// (`findExactNames()`).
pub fn filter_exact<'a>(entries: &'a [LinterEntry], exact: &[String]) -> Vec<&'a LinterEntry> {
    entries
        .iter()
        .filter(|entry| exact.iter().any(|name| name == entry.meta.config_name))
        .collect()
}

/// Keep entries whose configuration name, info name, or description
/// contains any of `terms` (case-insensitive, `filterByNames()`).
pub fn filter_search<'a>(entries: &'a [LinterEntry], terms: &[String]) -> Vec<&'a LinterEntry> {
    entries
        .iter()
        .filter(|entry| {
            terms.iter().any(|term| {
                let term = term.to_ascii_lowercase();
                entry.meta.config_name.to_ascii_lowercase().contains(&term)
                    || entry.meta.info_name.to_ascii_lowercase().contains(&term)
                    || entry
                        .meta
                        .description
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&term)
            })
        })
        .collect()
}

/// Sort by info name, case-insensitive (`isort($linter_info,
/// 'short')`), breaking ties by class.
pub fn sort_entries(entries: Vec<&LinterEntry>) -> Vec<&LinterEntry> {
    let mut entries = entries;
    entries.sort_by(|a, b| {
        a.meta
            .info_name
            .to_ascii_lowercase()
            .cmp(&b.meta.info_name.to_ascii_lowercase())
            .then_with(|| a.meta.class.cmp(b.meta.class))
    });
    entries
}

/* -(  Rendering  )------------------------------------------------------------- */

/// Render entries (`renderLinters()`), markup stripped.
pub fn render_entries(entries: &[&LinterEntry], verbose: bool) -> String {
    let mut out = String::new();
    for entry in entries {
        if verbose {
            render_verbose_entry(entry, &mut out);
        } else {
            render_brief_entry(entry, &mut out);
        }
    }
    if !verbose {
        out.push_str("(Run `arc linters --verbose` for more details.)\n");
    }
    out
}

/// One non-verbose entry line.
fn render_brief_entry(entry: &LinterEntry, out: &mut String) {
    let meta = entry.meta;
    out.push_str(&format!(
        " {:<10}  {} ({})\n",
        entry.status.text(),
        meta.config_name,
        meta.info_name
    ));
}

/// One verbose entry: header plus error, version, description, options,
/// and XHP rules sections.
fn render_verbose_entry(entry: &LinterEntry, out: &mut String) {
    let meta = entry.meta;
    out.push('\n');
    out.push_str(&format!(
        " {:<10}  {} ({})\n",
        entry.status.text(),
        meta.info_name,
        meta.class
    ));
    let mut tail = false;
    tail |= render_entry_error(entry, out);
    tail |= render_entry_version(entry, out);
    tail |= render_entry_description(entry, out);
    tail |= render_entry_options(entry, out);
    if meta.class == "ArcanistXHPASTLinter" {
        out.push_str("\n    Linter Rules\n\n");
        out.push_str(&xhpast_rules_table());
    }
    if tail {
        out.push('\n');
    }
}

/// A probe failure, shown as an exception. Returns true when written.
fn render_entry_error(entry: &LinterEntry, out: &mut String) -> bool {
    let Some(error) = &entry.error else {
        return false;
    };
    out.push('\n');
    out.push_str(&format!("    {}\n", error.class));
    out.push_str(&arc_rs::lint::word_wrap(&error.message, 4));
    out.push('\n');
    true
}

/// The version/URI block. Returns true when written.
fn render_entry_version(entry: &LinterEntry, out: &mut String) -> bool {
    let meta = entry.meta;
    if entry.version.is_none() && meta.uri.is_none() {
        return false;
    }
    out.push('\n');
    if let Some(version) = &entry.version {
        out.push_str(&format!("    Version {version}\n"));
    }
    if let Some(uri) = meta.uri {
        out.push_str(&format!("    {uri}\n"));
    }
    true
}

/// The description block. Returns true when written.
fn render_entry_description(entry: &LinterEntry, out: &mut String) -> bool {
    let Some(description) = entry.meta.description else {
        return false;
    };
    out.push('\n');
    out.push_str(&arc_rs::lint::word_wrap(description, 4));
    out.push('\n');
    true
}

/// The configuration-options block. Returns true when written.
fn render_entry_options(entry: &LinterEntry, out: &mut String) -> bool {
    let meta = entry.meta;
    let options = meta.options();
    if options.is_empty() {
        return false;
    }
    out.push_str("\n    Configuration Options\n\n");
    let rendered: Vec<String> = options
        .iter()
        .map(|option| {
            format!(
                "    {} ({})\n{}\n",
                option.name,
                option.option_type,
                arc_rs::lint::word_wrap(&meta.option_help(option), 6)
            )
        })
        .collect();
    out.push_str(&rendered.join("\n"));
    true
}

/// The exact-match failure message (`renderLinters()`, `:226-235`).
#[must_use]
pub fn no_match_message(exact: &[String]) -> String {
    format!(
        "No match found. Try `arc linters --search {}` to search for a linter.\n",
        exact.join(",")
    )
}

/* -(  Entry points  )---------------------------------------------------------- */

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Options {
    verbose: bool,
    search: Vec<String>,
    exact: Vec<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut options = Options::default();
    let mut queue: std::collections::VecDeque<String> = argv.iter().cloned().collect();
    let mut positional = Vec::new();
    while let Some(arg) = queue.pop_front() {
        if arg == "--" {
            positional.extend(queue.drain(..));
            break;
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg == "--verbose" {
            options.verbose = true;
        } else if arg == "--search" {
            let Some(term) = queue.pop_front() else {
                return ParseOutcome::Error("Option '--search' requires a parameter.".to_string());
            };
            options.search.push(term);
        } else if let Some(term) = arg.strip_prefix("--search=") {
            options.search.push(term.to_string());
        } else if let Some(stripped) = arg.strip_prefix("--") {
            let key = stripped.split('=').next().unwrap_or("");
            // `--verbose=value` splices back like any boolean flag.
            if key == "verbose" {
                options.verbose = true;
                if let Some((_, value)) = stripped.split_once('=') {
                    queue.push_front(value.to_string());
                }
                continue;
            }
            return ParseOutcome::Error(format!("Unknown argument '{key}'. Try 'arc help'."));
        } else if arg.starts_with('-') && arg.len() > 1 {
            let spelling = &arg[1..];
            return ParseOutcome::Error(format!("Unknown argument '{spelling}'. Try 'arc help'."));
        } else {
            positional.push(arg);
        }
    }
    options.exact = positional;
    ParseOutcome::Args(options)
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    run_in(&cwd, argv)
}

/// Entry point with an explicit working directory.
pub fn run_in(cwd: &Path, argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        ParseOutcome::Args(options) => options,
    };
    let configured = match configured_classes(cwd) {
        Ok(configured) => configured,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    let entries = collect_entries(&configured);
    let verbose = options.verbose || !options.exact.is_empty();
    if !options.exact.is_empty() {
        let selected = sort_entries(filter_exact(&entries, &options.exact));
        if selected.is_empty() {
            print!("{}", no_match_message(&options.exact));
            return ExitCode::SUCCESS;
        }
        print!("{}", render_entries(&selected, verbose));
        return ExitCode::SUCCESS;
    }
    if !options.search.is_empty() {
        let selected = sort_entries(filter_search(&entries, &options.search));
        print!("{}", render_entries(&selected, verbose));
        return ExitCode::SUCCESS;
    }
    let selected = sort_entries(entries.iter().collect());
    print!("{}", render_entries(&selected, verbose));
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "linters_tests.rs"]
mod linters_tests;
