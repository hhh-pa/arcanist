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

/// Damerau-Levenshtein distance with the command-corrector costs
/// (insert 4, delete 4, replace 3, transpose 2), following
/// `PhutilEditDistanceMatrix::computeMatrix` exactly: replacement wins
/// ties over insertion and deletion, insertion wins ties over deletion,
/// and a transpose wins only when strictly cheaper.
fn command_distance(a: &str, b: &str) -> i64 {
    const INSERT: i64 = 4;
    const DELETE: i64 = 4;
    const REPLACE: i64 = 3;
    const TRANSPOSE: i64 = 2;

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

    for xx in 1..=xl {
        let mut last_dy: i64 = -1;
        for yy in 1..=yl {
            let dx = d[&y[yy - 1]];
            let dy = last_dy;
            let rep_cost = if x[xx - 1] == y[yy - 1] {
                m[xx][yy]
            } else {
                m[xx][yy] + REPLACE
            };
            let del_cost = m[xx][yy + 1] + DELETE;
            let ins_cost = m[xx + 1][yy] + INSERT;
            let mut cost;
            if rep_cost <= del_cost && rep_cost <= ins_cost {
                cost = rep_cost;
                if x[xx - 1] == y[yy - 1] {
                    last_dy = yy as i64 - 1;
                }
            } else if ins_cost <= del_cost {
                cost = ins_cost;
            } else {
                cost = del_cost;
            }
            let trn_count = (xx as i64 - dx - 2) + (yy as i64 - dy - 2) + 1;
            let trn_cost = m[(dx + 1) as usize][(dy + 1) as usize] + trn_count * TRANSPOSE;
            if trn_cost < cost {
                cost = trn_cost;
            }
            m[xx + 1][yy + 1] = cost;
        }
        d.insert(x[xx - 1], xx as i64 - 1);
    }
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
        if arg == "--" {
            parsed.saw_terminator = true;
            remaining.extend(argv[index..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') || arg.len() < 2 {
            // A lone `-` is a normal argument (e.g. stdin); anything
            // else here is positional.
            remaining.push(arg.clone());
            index += 1;
            continue;
        }
        if !arg.starts_with("--") {
            // No short flags exist at ceb: `-x` falls through to the
            // unrecognized-argument error below.
            remaining.push(arg.clone());
            index += 1;
            continue;
        }
        let (mut name, inline): (String, Option<String>) = match arg[2..].split_once('=') {
            Some((head, tail)) => (head.to_string(), Some(tail.to_string())),
            None => (arg[2..].to_string(), None),
        };
        if !by_name.contains_key(name.as_str()) && autocorrect {
            let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
            let fixes = correct_flags(&name, &names);
            if fixes.len() == 1 {
                // `(Assuming "--x" is the British spelling of "--y".)`.
                let head = format!("(Assuming \"--{name}\" is the British spelling of ");
                parsed.notices.push(format!("{head}\"--{}\".)", fixes[0]));
                name = fixes[0].clone();
            }
        }
        let Some(spec) = by_name.get(name.as_str()) else {
            remaining.push(arg.clone());
            index += 1;
            continue;
        };
        if spec.param.is_none() {
            if inline.is_some() {
                return Err(format!("Argument \"--{name}\" does not take a parameter."));
            }
            if !spec.repeat && parsed.values.contains_key(name.as_str()) {
                return Err(format!("Argument \"--{name}\" was provided twice."));
            }
            parsed
                .values
                .entry(name.clone())
                .or_default()
                .push(String::new());
            index += 1;
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => {
                let Some(next) = argv.get(index + 1) else {
                    return Err(format!("Argument \"--{name}\" requires a parameter."));
                };
                index += 1;
                next.clone()
            }
        };
        if !spec.repeat && parsed.values.contains_key(name.as_str()) {
            return Err(format!("Argument \"--{name}\" was provided twice."));
        }
        parsed.values.entry(name).or_default().push(value);
        index += 1;
    }
    if wildcard {
        parsed.positionals = filter_wildcard(remaining)?;
    } else {
        let leftovers = filter_wildcard(remaining)?;
        if let Some(first) = leftovers.first() {
            return Err(format!("Unrecognized argument \"{first}\"."));
        }
    }
    Ok(parsed)
}

#[cfg(test)]
#[path = "argparse_tests.rs"]
mod argparse_tests;
