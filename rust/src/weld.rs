//! `arc weld`: robustly fuse two or more files together.
//!
//! PHP reference: `src/workflow/ArcanistWeldWorkflow.php`. Each input file
//! is appended to the joint with a randomly blended overlap of 16-32
//! lines (halved for short inputs); overlapped line pairs fuse glyph by
//! glyph, biased from the earlier file toward the later one. The joint is
//! much stronger than the one created by tools like `cat`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::conduit::{self, EnvelopeKind};

pub const NAME: &str = "weld";

/// T1153 BROWSE-B23: `--help` renders the generated workflow help
/// (`ArcanistWeldWorkflow.php:11-23`), not a short custom usage
/// string. T1153 RB-D5: the renderer is the shared `crate::help`
/// module; this file keeps only the data. The `--seed` extension
/// (Rule 3: `mt_rand` streams are unportable) stays functional but
/// undocumented in `--help`, like PHP.
const HELP_EXAMPLES: &str = "**weld** [options] -- __file__ __file__ ...";
const HELP_SYNOPSIS: &str = "Robustly fuse files together.";
const HELP_TEXT: &str = "Robustly fuse two or more files together. The resulting joint is much stronger\nthan the one created by tools like __cat__.";
/// Non-wildcard argument specs: (name, short, param, help). Empty:
/// the only argument is the `files` wildcard (`:25-28`), which
/// `renderArgumentSpecs()` skips.
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<crate::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    crate::help::usage_text(
        NAME,
        Some(HELP_EXAMPLES),
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        true,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(crate::terminal::ansi_enabled())
}

/// Parsed `weld` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub files: Vec<String>,
    pub seed: Option<u64>,
}

fn parse_seed(text: &str) -> Result<u64, String> {
    text.parse::<u64>()
        .map_err(|_| format!("invalid --seed '{text}' (expected an unsigned integer)"))
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    parse_options_with_tty(argv, crate::prompt::stdin_is_terminal())
}

/// [`parse_options`] with an injected TTY flag (deterministic tests).
/// T1153 BROWSE-B21: piped runs without `--` fail with
/// `MISSING_TERMINATOR` (checked after unknown-flag scanning, like
/// PHP's `parseFull()`, but before the file-count validation in
/// `execute`). `--help` without `--` when piped reports the
/// terminator.
fn parse_options_with_tty(argv: &[String], stdin_tty: bool) -> Result<Option<Options>, String> {
    let missing = crate::help::terminator_missing(argv, stdin_tty);
    let mut files = Vec::new();
    let mut seed = None;
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            files.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--seed" {
            let Some(value) = iter.next() else {
                return Err("--seed requires a value".to_string());
            };
            seed = Some(parse_seed(value)?);
        } else if let Some(value) = arg.strip_prefix("--seed=") {
            seed = Some(parse_seed(value)?);
        } else if arg == "-h" || arg == "--help" {
            if missing {
                return Err(crate::help::MISSING_TERMINATOR.to_string());
            }
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            files.push(arg.clone());
        }
    }
    if missing {
        return Err(crate::help::MISSING_TERMINATOR.to_string());
    }
    Ok(Some(Options { files, seed }))
}

/// Small splitmix64 generator (std-only; no new dependencies).
///
/// PHP welds with `mt_rand` (`ArcanistWeldWorkflow.php:46,114-130` at
/// ceb), whose stream no std-only generator reproduces; exact stream
/// parity is intentionally not pursued — the joint is random either
/// way (T1153 CORE-47, kept: PHP-`mt_rand` streams are unportable).
/// What matters is determinism under `--seed`, which this provides
/// and the tests pin.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn range_inclusive(&mut self, low: u64, high: u64) -> u64 {
        debug_assert!(low <= high);
        low + self.next_u64() % (high - low + 1)
    }

    /// Uniform draw from [0, 1).
    pub fn unit(&mut self) -> f64 {
        const SCALE: f64 = (1u64 << 53) as f64;
        ((self.next_u64() >> 11) as f64) / SCALE
    }
}

/// Blend threshold for one glyph position, mirroring the PHP: a space on
/// the earlier side always loses, a space on the later side always wins,
/// otherwise the positional bias applies.
fn threshold(earlier_is_space: bool, later_is_space: bool, bias: f64) -> f64 {
    let mut value = bias;
    if earlier_is_space {
        value = 1.0;
    }
    if later_is_space {
        value = 0.0;
    }
    value
}

/// True when the draw keeps the earlier glyph (PHP: `mt_rand()/max >
/// threshold`).
#[must_use]
pub fn keep_earlier(earlier_is_space: bool, later_is_space: bool, bias: f64, draw: f64) -> bool {
    draw > threshold(earlier_is_space, later_is_space, bias)
}

/// Fuse one glyph pair: spaces defer to the other side, otherwise the
/// draw is compared against the bias.
#[must_use]
pub fn pick(earlier: char, later: char, bias: f64, draw: f64) -> char {
    if keep_earlier(earlier == ' ', later == ' ', bias, draw) {
        earlier
    } else {
        later
    }
}

/// Split a line into content and its trailing `[\r\n]+` terminator, if any.
#[must_use]
pub fn split_terminator(line: &str) -> (&str, Option<&str>) {
    let end = line.trim_end_matches(['\r', '\n']).len();
    if end == line.len() {
        (line, None)
    } else {
        (&line[..end], Some(&line[end..]))
    }
}

fn split_terminator_bytes(line: &[u8]) -> (&[u8], Option<&[u8]>) {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\r' || line[end - 1] == b'\n') {
        end -= 1;
    }
    if end == line.len() {
        (line, None)
    } else {
        (&line[..end], Some(&line[end..]))
    }
}

/// Split text into lines that retain their terminators, mirroring
/// `phutil_split_lines($data, true)`.
#[must_use]
pub fn split_lines_keep(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            out.push(&text[start..=index]);
            start = index + 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Byte-level line split keeping terminators.
#[must_use]
pub fn split_lines_bytes(data: &[u8]) -> Vec<&[u8]> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (index, byte) in data.iter().enumerate() {
        if *byte == b'\n' {
            out.push(&data[start..=index]);
            start = index + 1;
        }
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

/// True for the combining codepoint ranges in
/// `phutil_utf8_is_combining_character()`
/// (`src/utils/utf8.php:852-870` at ceb).
fn is_combining_mark(codepoint: u32) -> bool {
    (0x0300..=0x036F).contains(&codepoint)
        || (0x1DC0..=0x1DFF).contains(&codepoint)
        || (0x20D0..=0x20FF).contains(&codepoint)
        || (0xFE20..=0xFE2F).contains(&codepoint)
}

/// Split valid UTF-8 text into glyphs, merging combining marks like
/// `phutil_utf8v_combined()` (`src/utils/utf8.php:880-883` at ceb, via
/// `phutil_utf8v_combine_characters()`, `:895-930`): single-byte chars
/// pass through, longer chars merge when combining, and a leading
/// combining mark attaches to a synthetic space (T1153 CORE-48).
#[must_use]
pub fn split_glyphs(text: &str) -> Vec<String> {
    let mut chars = text.chars().peekable();
    let mut glyphs: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut buf_open = false;
    if let Some(&first) = chars.peek()
        && first.len_utf8() > 1
        && is_combining_mark(first as u32)
    {
        buf.push(' ');
        buf_open = true;
    }
    for ch in chars {
        let combining = ch.len_utf8() > 1 && is_combining_mark(ch as u32);
        if combining {
            buf.push(ch);
            buf_open = true;
        } else {
            if buf_open {
                glyphs.push(std::mem::take(&mut buf));
            }
            buf.push(ch);
            buf_open = true;
        }
    }
    if buf_open {
        glyphs.push(buf);
    }
    glyphs
}

/// One side's fuse elements: glyph bytes when the content is valid
/// UTF-8 (`phutil_utf8v_combined`), single bytes otherwise
/// (`str_split`), like `weldLines()` tests each side independently
/// (`ArcanistWeldWorkflow.php:90-103` at ceb, T1153 CORE-48).
fn split_fuse_elements(content: &[u8]) -> Vec<Vec<u8>> {
    match std::str::from_utf8(content) {
        Ok(text) => split_glyphs(text)
            .iter()
            .map(String::as_bytes)
            .map(<[u8]>::to_vec)
            .collect(),
        Err(_) => content.iter().map(|byte| vec![*byte]).collect(),
    }
}

/// Fuse one line pair glyph by glyph. The shorter side is padded with
/// spaces; the terminator is the later line's when present, else the
/// earlier line's.
pub fn weld_lines(earlier: &str, later: &str, bias: f64, rng: &mut Rng) -> String {
    let (ucontent, unewline) = split_terminator(earlier);
    let (vcontent, vnewline) = split_terminator(later);
    let newline = vnewline.or(unewline);
    // T1153 CORE-48: `&str` inputs are always valid UTF-8, so both
    // sides take the glyph path (`ArcanistWeldWorkflow.php:90-103`).
    let mut us = split_glyphs(ucontent);
    let mut vs = split_glyphs(vcontent);
    let len = us.len().max(vs.len());
    us.resize(len, " ".to_string());
    vs.resize(len, " ".to_string());
    let mut out = String::new();
    for (uc, vc) in us.iter().zip(vs.iter()) {
        let keep = keep_earlier(uc == " ", vc == " ", bias, rng.unit());
        out.push_str(if keep { uc } else { vc });
    }
    if let Some(newline) = newline {
        out.push_str(newline);
    }
    out
}

/// Fuse one raw line pair, testing each side for UTF-8 independently
/// like `weldLines()` (`ArcanistWeldWorkflow.php:90-103` at ceb,
/// T1153 CORE-48): a valid side fuses glyph by glyph, an invalid
/// side byte by byte, so binary input round-trips without corruption.
pub fn weld_line_bytes(earlier: &[u8], later: &[u8], bias: f64, rng: &mut Rng) -> Vec<u8> {
    let (ucontent, unewline) = split_terminator_bytes(earlier);
    let (vcontent, vnewline) = split_terminator_bytes(later);
    let mut us = split_fuse_elements(ucontent);
    let mut vs = split_fuse_elements(vcontent);
    let len = us.len().max(vs.len());
    us.resize(len, vec![b' ']);
    vs.resize(len, vec![b' ']);
    let mut out = Vec::with_capacity(len + 2);
    for (uc, vc) in us.iter().zip(vs.iter()) {
        let earlier_space = uc.len() == 1 && uc[0] == b' ';
        let later_space = vc.len() == 1 && vc[0] == b' ';
        let keep = keep_earlier(earlier_space, later_space, bias, rng.unit());
        out.extend_from_slice(if keep { uc } else { vc });
    }
    if let Some(newline) = vnewline.or(unewline) {
        out.extend_from_slice(newline);
    }
    out
}

/// Blend width for one joint: random 16-32 lines, halved for short
/// inputs, and never wider than either side.
pub fn overlap_for(buffered: usize, incoming: usize, rng: &mut Rng) -> usize {
    let mut overlap = rng.range_inclusive(16, 32) as usize;
    if buffered > 6 {
        overlap = overlap.min(buffered.div_ceil(2));
    }
    if incoming > 6 {
        overlap = overlap.min(incoming.div_ceil(2));
    }
    overlap.min(buffered).min(incoming)
}

/// Fuse UTF-8 file contents; empty inputs are skipped.
#[must_use]
pub fn weld_texts(texts: &[&str], rng: &mut Rng) -> String {
    let mut buffer: Vec<String> = Vec::new();
    for text in texts {
        if text.is_empty() {
            continue;
        }
        let lines = split_lines_keep(text);
        let overlap = overlap_for(buffer.len(), lines.len(), rng);
        let start = buffer.len() - overlap;
        for (index, line) in lines.iter().take(overlap).enumerate() {
            let bias = (index as f64 + 0.5) / overlap as f64;
            buffer[start + index] = weld_lines(&buffer[start + index], line, bias, rng);
        }
        for line in lines.iter().skip(overlap) {
            buffer.push((*line).to_string());
        }
    }
    buffer.concat()
}

/// Fuse raw file contents; empty inputs are skipped.
#[must_use]
pub fn weld_buffers(files: &[Vec<u8>], rng: &mut Rng) -> Vec<u8> {
    let mut buffer: Vec<Vec<u8>> = Vec::new();
    for data in files {
        if data.is_empty() {
            continue;
        }
        let lines = split_lines_bytes(data);
        let overlap = overlap_for(buffer.len(), lines.len(), rng);
        let start = buffer.len() - overlap;
        for (index, line) in lines.iter().take(overlap).enumerate() {
            let bias = (index as f64 + 0.5) / overlap as f64;
            buffer[start + index] = weld_line_bytes(&buffer[start + index], line, bias, rng);
        }
        for line in lines.iter().skip(overlap) {
            buffer.push((*line).to_vec());
        }
    }
    buffer.concat()
}

fn seed_from_clock() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (nanos as u64).wrapping_add((std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// Run the workflow rooted at `dir`, returning stdout bytes or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<Vec<u8>, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(usage().into_bytes()),
        // T1153 CORE-49 (R4): PHP renders usage errors with no dump.
        Err(message) => return Err(message),
    };
    if options.files.len() < 2 {
        // T1153 CORE-49 (R4): `PhutilArgumentUsageException`
        // (`ArcanistWeldWorkflow.php:33-36`), no usage dump.
        return Err("Specify two or more files to weld together.".to_string());
    }
    let mut inputs = Vec::with_capacity(options.files.len());
    for file in &options.files {
        let path = if Path::new(file).is_absolute() {
            PathBuf::from(file)
        } else {
            dir.join(file)
        };
        inputs.push(std::fs::read(&path).map_err(|e| format!("cannot read '{file}': {e}"))?);
    }
    let seed = options.seed.unwrap_or_else(seed_from_clock);
    Ok(weld_buffers(&inputs, &mut Rng::new(seed)))
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(bytes) => {
            use std::io::Write;
            if std::io::stdout().write_all(&bytes).is_err() {
                eprintln!("arc {NAME}: failed to write output"); // LCOV_EXCL_START // Reason: stdout write failure; closing stdout is process-global
                return ExitCode::FAILURE; // LCOV_EXCL_STOP
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            // T1153 BROWSE-B21: the terminator failure prints the three
            // cyan hints first (like `ArcanistRuntime.php:172-188`).
            if message == crate::help::MISSING_TERMINATOR {
                crate::help::print_terminator_hints(crate::terminal::ansi_enabled());
            }
            // T1153 CORE-49 (R4): `weld` is a toolset workflow.
            // Argument errors render the red `USAGE EXCEPTION` label
            // (exit 1, matching the old exit); read failures escape
            // into a fatal (255).
            let kind = if parse_options(argv).is_err()
                || message == "Specify two or more files to weld together."
            {
                EnvelopeKind::ToolsetUsage
            } else {
                EnvelopeKind::Fatal
            };
            conduit::report(kind, &message)
        }
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "weld_tests.rs"]
mod weld_tests;
