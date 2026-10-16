//! Argument parsing shared by the toolset (new-style) workflows.
//!
//! PHP reference: `src/parser/argument/PhutilArgumentParser.php` (the
//! `parseInternal` + `filterWildcardArgv` + `parseFull` tail),
//! `src/parser/argument/PhutilArgumentSpellingCorrector.php`, and
//! `src/utils/PhutilEditDistanceMatrix.php`, all at ceb.
//!
//! [`parse_toolset`] ports the full-parse path (`parseWorkflowsFull` calls
//! the wrapped `parse()`, so errors here render bold `**Usage
//! Exception:**` and exit 1). New-style workflows at ceb use no short
//! flags and no conflicts, so those are not modeled.
//!
//! Parse-error texts transcribe PHP verbatim (T1153 RB-D1):
//! `Argument "--x" requires a parameter.`, `Unrecognized argument "x".`,
//! `Argument "--x" is unrecognized. Use "--" to indicate the end of
//! flags.`

use std::collections::HashMap;

/// One `--flag` specification (a workflow argument which is not the
/// wildcard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagSpec {
    /// Long flag name without dashes.
    pub name: &'static str,
    /// Value name when the flag takes a parameter.
    pub param: Option<&'static str>,
    /// Whether the flag may repeat.
    pub repeat: bool,
}

impl FlagSpec {
    /// A boolean flag.
    #[must_use]
    pub const fn boolean(name: &'static str) -> Self {
        Self {
            name,
            param: None,
            repeat: false,
        }
    }

    /// A single-valued flag.
    #[must_use]
    pub const fn valued(name: &'static str, param: &'static str) -> Self {
        Self {
            name,
            param: Some(param),
            repeat: false,
        }
    }
}

/// Parsed toolset arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// Flag values by name (one entry per occurrence).
    pub values: HashMap<String, Vec<String>>,
    /// Wildcard positionals (or leftovers when no wildcard exists, which
    /// is an error the parser already rejected).
    pub positionals: Vec<String>,
    /// Spelling-correction notices for stderr, in order.
    pub notices: Vec<String>,
    /// Whether a `--` terminator was seen during the flag scan.
    pub saw_terminator: bool,
}

impl Parsed {
    /// True when a boolean flag was passed.
    #[must_use]
    pub fn flag(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    /// The single value of a valued flag, if passed.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name)?.first().map(String::as_str)
    }
}

/// Strip the common prefix and suffix (the matrix works on the middles).
fn strip_affixes(x: &[char], y: &[char]) -> (Vec<char>, Vec<char>) {
    let len = x.len().min(y.len());
    let mut prefix = 0;
    while prefix < len && x[prefix] == y[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < len - prefix && x[x.len() - 1 - suffix] == y[y.len() - 1 - suffix] {
        suffix += 1;
    }
    (
        x[prefix..x.len() - suffix].to_vec(),
        y[prefix..y.len() - suffix].to_vec(),
    )
}

/// Pick the cheapest plain edit step for one command-matrix cell:
/// replacement wins ties over insertion and deletion, insertion wins
/// ties over deletion. Returns the cost and whether replacement won
/// (for the Damerau `last_dy` bookkeeping).
fn command_step_cost(rep_cost: i64, del_cost: i64, ins_cost: i64) -> (i64, bool) {
    if rep_cost <= del_cost && rep_cost <= ins_cost {
        (rep_cost, true)
    } else if ins_cost <= del_cost {
        (ins_cost, false)
    } else {
        (del_cost, false)
    }
}

/// One Damerau cell update for [`command_distance`]: the cheapest edit
/// step, then a strictly-cheaper transpose. Returns the cell cost and
/// whether a matching pair was consumed (replacement won on equal
/// characters).
fn command_cell_cost(
    m: &[Vec<i64>],
    xx: usize,
    yy: usize,
    matched: bool,
    dx: i64,
    dy: i64,
) -> (i64, bool) {
    const INSERT: i64 = 4;
    const DELETE: i64 = 4;
    const REPLACE: i64 = 3;
    const TRANSPOSE: i64 = 2;

    let rep_cost = if matched {
        m[xx][yy]
    } else {
        m[xx][yy] + REPLACE
    };
    let del_cost = m[xx][yy + 1] + DELETE;
    let ins_cost = m[xx + 1][yy] + INSERT;
    let (mut cost, took_rep) = command_step_cost(rep_cost, del_cost, ins_cost);
    let trn_count = (xx as i64 - dx - 2) + (yy as i64 - dy - 2) + 1;
    let trn_cost = m[(dx + 1) as usize][(dy + 1) as usize] + trn_count * TRANSPOSE;
    if trn_cost < cost {
        cost = trn_cost;
    }
    (cost, took_rep && matched)
}

/// The `[-1]`-offset matrix and last-seen map for [`command_distance`],
/// with infinity borders and the insert/delete edges filled.
fn command_init_matrix(x: &[char], y: &[char]) -> (Vec<Vec<i64>>, HashMap<char, i64>) {
    const INSERT: i64 = 4;
    const DELETE: i64 = 4;

    let (xl, yl) = (x.len(), y.len());
    // `m[xx][yy]` with the Damerau `[-1]` row/column at index 0 (offset
    // by one); the `-1` cells hold infinity.
    let inf: i64 = i64::MAX / 4;
    let mut m = vec![vec![inf; yl + 2]; xl + 2];
    let mut d: HashMap<char, i64> = HashMap::new();
    for c in x.iter().chain(y.iter()) {
        d.insert(*c, -1);
    }
    for row in m.iter_mut().take(xl + 2).skip(1) {
        row[0] = inf;
    }
    for cell in m[0].iter_mut().take(yl + 2).skip(1) {
        *cell = inf;
    }
    for xx in 0..=xl {
        m[xx + 1][1] = xx as i64 * DELETE;
    }
    for yy in 0..=yl {
        m[1][yy + 1] = yy as i64 * INSERT;
    }
    (m, d)
}

/// Fill the command matrix cell by cell, tracking each row's last
/// consumed column for the Damerau transpositions.
fn command_fill_matrix(x: &[char], y: &[char], m: &mut [Vec<i64>], d: &mut HashMap<char, i64>) {
    let (xl, yl) = (x.len(), y.len());
    for xx in 1..=xl {
        let mut last_dy: i64 = -1;
        for yy in 1..=yl {
            let matched = x[xx - 1] == y[yy - 1];
            let (cost, consumed) = command_cell_cost(m, xx, yy, matched, d[&y[yy - 1]], last_dy);
            if consumed {
                last_dy = yy as i64 - 1;
            }
            m[xx + 1][yy + 1] = cost;
        }
        d.insert(x[xx - 1], xx as i64 - 1);
    }
}

/// Damerau-Levenshtein distance with the command-corrector costs
/// (insert 4, delete 4, replace 3, transpose 2), following
/// `PhutilEditDistanceMatrix::computeMatrix` exactly: replacement wins
/// ties over insertion and deletion, insertion wins ties over deletion,
/// and a transpose wins only when strictly cheaper.
fn command_distance(a: &str, b: &str) -> i64 {
    const INSERT: i64 = 4;
    const DELETE: i64 = 4;

    let (x, y) = strip_affixes(
        &a.chars().collect::<Vec<_>>(),
        &b.chars().collect::<Vec<_>>(),
    );
    if x.is_empty() {
        return INSERT * y.len() as i64;
    }
    if y.is_empty() {
        return DELETE * x.len() as i64;
    }
    if x == y {
        return 0;
    }

    let (mut m, mut d) = command_init_matrix(&x, &y);
    let (xl, yl) = (x.len(), y.len());
    command_fill_matrix(&x, &y, &mut m, &mut d);
    m[xl + 1][yl + 1]
}

/// Levenshtein distance with the flag-corrector costs (insert 1, delete
/// 1, replace 10, no transpositions).
fn flag_distance(a: &str, b: &str) -> i64 {
    const INSERT: i64 = 1;
    const DELETE: i64 = 1;
    const REPLACE: i64 = 10;

    let (x, y) = strip_affixes(
        &a.chars().collect::<Vec<_>>(),
        &b.chars().collect::<Vec<_>>(),
    );
    if x.is_empty() {
        return INSERT * y.len() as i64;
    }
    if y.is_empty() {
        return DELETE * x.len() as i64;
    }
    if x == y {
        return 0;
    }
    let xl = x.len();
    let yl = y.len();
    let mut m = vec![vec![0i64; yl + 1]; xl + 1];
    for (xx, row) in m.iter_mut().enumerate() {
        row[0] = xx as i64 * DELETE;
    }
    for (yy, cell) in m[0].iter_mut().enumerate() {
        *cell = yy as i64 * INSERT;
    }
    for xx in 1..=xl {
        for yy in 1..=yl {
            let rep_cost = if x[xx - 1] == y[yy - 1] {
                m[xx - 1][yy - 1]
            } else {
                m[xx - 1][yy - 1] + REPLACE
            };
            let del_cost = m[xx - 1][yy] + DELETE;
            let ins_cost = m[xx][yy - 1] + INSERT;
            m[xx][yy] = if rep_cost <= del_cost && rep_cost <= ins_cost {
                rep_cost
            } else if ins_cost <= del_cost {
                ins_cost
            } else {
                del_cost
            };
        }
    }
    m[xl][yl]
}

/// Command spelling correction
/// (`newCommandCorrector()->correctSpelling()`): never correct
/// dash-led inputs; accept a unique prefix; otherwise keep every
/// option at the best distance (capped at 6), dropping a lone
/// over-long match. Returns the matched options in distance order.
#[must_use]
pub fn correct_commands(input: &str, options: &[&str]) -> Vec<String> {
    if input.starts_with('-') {
        return Vec::new();
    }
    let folded = input.to_lowercase();
    let folded_options: Vec<String> = options.iter().map(|o| o.to_lowercase()).collect();
    let prefixes: Vec<String> = folded_options
        .iter()
        .filter(|o| o.starts_with(folded.as_str()))
        .cloned()
        .collect();
    if prefixes.len() == 1 {
        return prefixes;
    }
    let mut distances: Vec<(String, i64)> = folded_options
        .iter()
        .map(|o| (o.clone(), command_distance(o, &folded)))
        .collect();
    distances.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let Some((_, head)) = distances.first() else {
        return Vec::new();
    };
    let best = (*head).min(6);
    distances.retain(|(_, d)| *d <= best);
    if distances.len() > 1 {
        return distances.into_iter().map(|(o, _)| o).collect();
    }
    distances.retain(|(o, d)| o.chars().count() as i64 >= *d);
    distances.into_iter().map(|(o, _)| o).collect()
}

/// Flag spelling correction (`newFlagCorrector()->correctSpelling()`):
/// single insertions/deletions only (max distance 1), no prefixes.
#[must_use]
pub fn correct_flags(input: &str, options: &[&str]) -> Vec<String> {
    let folded = input.to_lowercase();
    let folded_options: Vec<String> = options.iter().map(|o| o.to_lowercase()).collect();
    let mut distances: Vec<(String, i64)> = folded_options
        .iter()
        .map(|o| (o.clone(), flag_distance(o, &folded)))
        .collect();
    distances.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let Some((_, head)) = distances.first() else {
        return Vec::new();
    };
    let best = (*head).min(1);
    distances.retain(|(_, d)| *d <= best);
    if distances.len() > 1 {
        return distances.into_iter().map(|(o, _)| o).collect();
    }
    distances.retain(|(o, d)| o.chars().count() as i64 >= *d);
    distances.into_iter().map(|(o, _)| o).collect()
}

/// `filterWildcardArgv()`: drop the first `--`, then reject any
/// dash-led token left before it.
fn filter_wildcard(argv: Vec<String>) -> Result<Vec<String>, String> {
    let mut out = argv;
    for (index, value) in out.iter().enumerate() {
        if value == "--" {
            out.remove(index);
            break;
        }
        if value.len() > 1 && value.starts_with('-') {
            return Err(format!(
                "Argument \"{value}\" is unrecognized. Use \"--\" to indicate the end of flags."
            ));
        }
    }
    Ok(out)
}

/// One argv token's role in the toolset flag scan.
enum ToolsetToken {
    /// `--`: copy the tail into the leftovers.
    Terminator,
    /// Positional, lone `-`, short flag, or `--name` (known or not):
    /// copied verbatim unless a known flag consumes it.
    Leftover,
    /// A `--name[=value]` token to look up in the spec.
    Flag,
}

/// Classify one argv token for the toolset flag scan.
fn toolset_token_kind(arg: &str) -> ToolsetToken {
    if arg == "--" {
        ToolsetToken::Terminator
    } else if arg == "-" || !arg.starts_with('-') || arg.len() < 2 {
        // A lone `-` is a normal argument (e.g. stdin); anything else
        // here is positional.
        ToolsetToken::Leftover
    } else if !arg.starts_with("--") {
        // No short flags exist at ceb: `-x` falls through to the
        // unrecognized-argument error below.
        ToolsetToken::Leftover
    } else {
        ToolsetToken::Flag
    }
}

/// Split one `--name[=value]` token and apply flag spelling correction,
/// recording the notice. Returns the (possibly corrected) name and the
/// inline value.
fn toolset_flag_name(
    arg: &str,
    specs: &[FlagSpec],
    by_name: &HashMap<&str, &FlagSpec>,
    autocorrect: bool,
    notices: &mut Vec<String>,
) -> (String, Option<String>) {
    let (mut name, inline): (String, Option<String>) = match arg[2..].split_once('=') {
        Some((head, tail)) => (head.to_owned(), Some(tail.to_owned())),
        None => (arg[2..].to_owned(), None),
    };
    if !by_name.contains_key(name.as_str()) && autocorrect {
        let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
        let fixes = correct_flags(&name, &names);
        if fixes.len() == 1 {
            // `(Assuming "--x" is the British spelling of "--y".)`.
            let head = format!("(Assuming \"--{name}\" is the British spelling of ");
            notices.push(format!("{head}\"--{}\".)", fixes[0]));
            name = fixes[0].clone();
        }
    }
    (name, inline)
}

/// Record one flag occurrence, rejecting repeats unless repeatable.
fn toolset_record_value(
    parsed: &mut Parsed,
    spec: &FlagSpec,
    name: &str,
    value: String,
) -> Result<(), String> {
    if !spec.repeat && parsed.values.contains_key(name) {
        return Err(format!("Argument \"--{name}\" was provided twice."));
    }
    parsed
        .values
        .entry(name.to_owned())
        .or_default()
        .push(value);
    Ok(())
}

/// Consume one known flag at `argv[index]` (`next` is the following
/// token for separate parameters), recording it into `parsed`.
/// Returns the argv slots consumed (2 when the parameter comes from
/// the next token).
fn toolset_consume_flag(
    parsed: &mut Parsed,
    spec: &FlagSpec,
    name: &str,
    inline: Option<String>,
    next: Option<&String>,
) -> Result<usize, String> {
    if spec.param.is_none() {
        if inline.is_some() {
            return Err(format!("Argument \"--{name}\" does not take a parameter."));
        }
        toolset_record_value(parsed, spec, name, String::new())?;
        return Ok(1);
    }
    let (value, consumed) = match inline {
        Some(value) => (value, 1),
        None => {
            let Some(next) = next else {
                return Err(format!("Argument \"--{name}\" requires a parameter."));
            };
            (next.clone(), 2)
        }
    };
    toolset_record_value(parsed, spec, name, value)?;
    Ok(consumed)
}

/// Finish the flag scan: with a wildcard the leftovers are the
/// positionals, otherwise any leftover is unrecognized.
fn toolset_finish(
    parsed: &mut Parsed,
    remaining: Vec<String>,
    wildcard: bool,
) -> Result<(), String> {
    if wildcard {
        parsed.positionals = filter_wildcard(remaining)?;
        return Ok(());
    }
    let leftovers = filter_wildcard(remaining)?;
    if let Some(first) = leftovers.first() {
        return Err(format!("Unrecognized argument \"{first}\"."));
    }
    Ok(())
}

/// Parse toolset (full-parse) arguments against `specs`.
///
/// `wildcard` mirrors a wildcard argument: leftovers are collected as
/// positionals instead of erroring. `autocorrect` is
/// `shouldAutocorrect()` (false when stdin is piped): with a single
/// flag correction it is applied with a notice, otherwise unknown
/// flags fall through to the unrecognized-argument error.
pub fn parse_toolset(
    argv: &[String],
    specs: &[FlagSpec],
    wildcard: bool,
    autocorrect: bool,
) -> Result<Parsed, String> {
    let by_name: HashMap<&str, &FlagSpec> = specs.iter().map(|spec| (spec.name, spec)).collect();
    let mut parsed = Parsed::default();
    let mut remaining: Vec<String> = Vec::new();
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        match toolset_token_kind(arg) {
            ToolsetToken::Terminator => {
                parsed.saw_terminator = true;
                remaining.extend(argv[index..].iter().cloned());
                break;
            }
            ToolsetToken::Leftover => {
                remaining.push(arg.clone());
                index += 1;
            }
            ToolsetToken::Flag => {
                let (name, inline) =
                    toolset_flag_name(arg, specs, &by_name, autocorrect, &mut parsed.notices);
                let Some(spec) = by_name.get(name.as_str()) else {
                    remaining.push(arg.clone());
                    index += 1;
                    continue;
                };
                let next = argv.get(index + 1);
                index += toolset_consume_flag(&mut parsed, spec, &name, inline, next)?;
            }
        }
    }
    toolset_finish(&mut parsed, remaining, wildcard)?;
    Ok(parsed)
}

#[cfg(test)]
#[path = "argparse_tests.rs"]
mod argparse_tests;
