//! `arc look`: a small text adventure around the working copy.
//!
//! PHP reference: `src/workflow/ArcanistLookWorkflow.php` ("Arcventure").
//!
//! With no arguments it describes a clearing with time-of-day flavor;
//! `look remotes` lists git remotes and `look published` lists published
//! commits (permanent refs only).
//!
//! Limits: the live clearing uses the system-timezone hour and falls back
//! to UTC where no zone data exists, while the plain-text builders stay
//! UTC so an instant always renders identically; `regexp(...)`
//! permanent-ref rules never match; without a usable Conduit client the
//! command degrades to local-only data instead of failing.

use std::path::Path;
use std::process::ExitCode;

use crate::{
    conduit::{self, EnvelopeKind},
    config, diff_submit, git, terminal,
};

/// Subcommand name, matching `arc look`.
pub const NAME: &str = "look";

const USAGE: &str = "usage: arc look [-- THING...]\n";

/// `pht('Arcventure')`, the `%!` banner title.
pub const TITLE: &str = "Arcventure";

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
///
/// The deterministic flavor used by the plain-text `render` builders; the
/// live `run` path uses `local_hour` (system timezone) instead.
#[must_use]
pub fn hour_of(now_unix: u64) -> u32 {
    ((now_unix / 3600) % 24) as u32
}

/// Hour of day (0-23) for `now_unix` shifted by `offset_secs`, the seconds
/// east of UTC in effect at that instant (negative west of Greenwich).
/// Pure: DST handling belongs to the caller, which supplies the offset
/// already in effect (see `local_utc_offset`).
#[must_use]
pub fn hour_with_offset(now_unix: u64, offset_secs: i64) -> u32 {
    ((now_unix as i64)
        .saturating_add(offset_secs)
        .rem_euclid(86_400)
        / 3_600) as u32
}

/// Days from 1970-01-01 to the civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let adjusted = if month <= 2 { year - 1 } else { year };
    let era = adjusted.div_euclid(400);
    let year_of_era = adjusted.rem_euclid(400);
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * i64::from(month_prime) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Civil date for days since 1970-01-01, the inverse of `days_from_civil`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = (if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    }) as u32;
    let full_year = if month <= 2 { year + 1 } else { year };
    (full_year, month, day)
}

/// Weekday with Sunday = 0 (1970-01-01 was a Thursday).
fn weekday_sunday_zero(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

/// Gregorian leap year.
fn is_leap_year(year: i64) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

/// Days in `month` (1-12) of `year`.
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
    }
}

/// Month/day for a 1-based day-of-year (`ordinal` counts Feb 29 in leap
/// years and must not exceed the days in `year`).
fn month_day_of_ordinal(year: i64, mut ordinal: u32) -> (u32, u32) {
    let mut month = 1;
    while month < 12 && ordinal > days_in_month(year, month) {
        ordinal -= days_in_month(year, month);
        month += 1;
    }
    (month, ordinal)
}

/// Day-of-month for `Mm.w.d`: weekday `day` (Sunday = 0) of week `week`
/// (5 means the last one) of `month`.
fn month_weekday_day(year: i64, month: u32, week: u32, day: u32) -> u32 {
    let first = weekday_sunday_zero(days_from_civil(year, month, 1));
    let mut date = 1 + (day + 7 - first) % 7 + (week - 1) * 7;
    if date > days_in_month(year, month) {
        date -= 7;
    }
    date
}

/// A POSIX `TZ` daylight-saving transition rule (`Jn`, `n`, `Mm.w.d`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DstRule {
    /// Julian day 1-365; leap days are not counted.
    JulianSkip(u16),
    /// Zero-based day 0-365; leap days are counted.
    JulianCount(u16),
    /// Month/week/weekday (week 5 means last, Sunday is 0).
    MonthWeekday { month: u32, week: u32, day: u32 },
}

/// Daylight-saving half of a POSIX `TZ` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DstSpec {
    /// Seconds east of UTC while daylight saving is in effect.
    offset: i64,
    /// Start transition as (rule, wall-clock seconds in standard time).
    start: (DstRule, i64),
    /// End transition as (rule, wall-clock seconds in daylight time).
    end: (DstRule, i64),
}

/// A parsed POSIX `TZ` string (`std offset[dst[offset][,start,end]]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PosixTz {
    /// Seconds east of UTC in standard time.
    std_offset: i64,
    dst: Option<DstSpec>,
}

/// Split a run of ASCII digits off `text`.
fn take_tz_number(text: &str) -> Option<(i64, &str)> {
    let len = text.bytes().take_while(|b| b.is_ascii_digit()).count();
    if len == 0 {
        return None;
    }
    Some((text[..len].parse().ok()?, &text[len..]))
}

/// Split a zone abbreviation (`EST` or `<-03>`) off `text`.
fn take_tz_name(text: &str) -> Option<(&str, &str)> {
    if let Some(rest) = text.strip_prefix('<') {
        let end = rest.find('>')?;
        if end == 0 {
            return None;
        }
        return Some((&rest[..end], &rest[end + 1..]));
    }
    let len = text.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
    if len == 0 {
        return None;
    }
    Some((&text[..len], &text[len..]))
}

/// Split a `[+|-]hh[:mm[:ss]]` offset off `text`, returning raw seconds
/// (POSIX offsets count west as positive) plus the remainder.
fn take_tz_offset(text: &str) -> Option<(i64, &str)> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (hours, mut rest) = take_tz_number(rest)?;
    let mut seconds = hours.saturating_mul(3_600);
    for factor in [60, 1] {
        if let Some(tail) = rest.strip_prefix(':') {
            let (part, next) = take_tz_number(tail)?;
            seconds = seconds.saturating_add(part.saturating_mul(factor));
            rest = next;
        }
    }
    if negative {
        seconds = seconds.saturating_neg();
    }
    Some((seconds, rest))
}

/// Split one `Mm.w.d` rule (after the `M`) off `text`.
fn take_month_weekday_rule(tail: &str) -> Option<(DstRule, &str)> {
    let (month, rest) = take_tz_number(tail)?;
    let (week, rest) = take_tz_number(rest.strip_prefix('.')?)?;
    let (day, rest) = take_tz_number(rest.strip_prefix('.')?)?;
    if !(1..=12).contains(&month) || !(1..=5).contains(&week) || !(0..=6).contains(&day) {
        return None;
    }
    Some((
        DstRule::MonthWeekday {
            month: month as u32,
            week: week as u32,
            day: day as u32,
        },
        rest,
    ))
}

/// Split one DST rule head (`Jn`, `n`, `Mm.w.d`) off `text`.
fn take_dst_rule_head(text: &str) -> Option<(DstRule, &str)> {
    if let Some(tail) = text.strip_prefix('J') {
        let (day, rest) = take_tz_number(tail)?;
        if !(1..=365).contains(&day) {
            return None;
        }
        return Some((DstRule::JulianSkip(day as u16), rest));
    }
    if let Some(tail) = text.strip_prefix('M') {
        return take_month_weekday_rule(tail);
    }
    let (day, rest) = take_tz_number(text)?;
    if !(0..=365).contains(&day) {
        return None;
    }
    Some((DstRule::JulianCount(day as u16), rest))
}

/// Split one DST rule (with optional `/time`, default 02:00) off `text`.
fn take_dst_rule(text: &str) -> Option<(DstRule, i64, &str)> {
    let (rule, rest) = take_dst_rule_head(text)?;
    let (time, rest) = match rest.strip_prefix('/') {
        Some(tail) => take_tz_offset(tail)?,
        None => (7_200, rest),
    };
    Some((rule, time, rest))
}

/// Parse a POSIX `TZ` string. Bare `dst` sits one hour east of standard
/// time with the POSIX default rules (`M3.2.0`/`M11.1.0`).
fn parse_posix_tz(text: &str) -> Option<PosixTz> {
    let (_, rest) = take_tz_name(text)?;
    let (std_west, rest) = take_tz_offset(rest)?;
    let std_offset = std_west.saturating_neg();
    if rest.is_empty() {
        return Some(PosixTz {
            std_offset,
            dst: None,
        });
    }
    let (_, rest) = take_tz_name(rest)?;
    let (dst_west, rest) = match take_tz_offset(rest) {
        Some((offset, rest)) => (offset, rest),
        None => (std_west.saturating_sub(3_600), rest),
    };
    let dst = if rest.is_empty() {
        DstSpec {
            offset: dst_west.saturating_neg(),
            start: (
                DstRule::MonthWeekday {
                    month: 3,
                    week: 2,
                    day: 0,
                },
                7_200,
            ),
            end: (
                DstRule::MonthWeekday {
                    month: 11,
                    week: 1,
                    day: 0,
                },
                7_200,
            ),
        }
    } else {
        let rest = rest.strip_prefix(',')?;
        let (start_rule, start_time, rest) = take_dst_rule(rest)?;
        let rest = rest.strip_prefix(',')?;
        let (end_rule, end_time, rest) = take_dst_rule(rest)?;
        if !rest.is_empty() {
            return None;
        }
        DstSpec {
            offset: dst_west.saturating_neg(),
            start: (start_rule, start_time),
            end: (end_rule, end_time),
        }
    };
    Some(PosixTz {
        std_offset,
        dst: Some(dst),
    })
}

/// Month/day of `rule` in `year`.
fn dst_rule_month_day(rule: DstRule, year: i64) -> (u32, u32) {
    match rule {
        DstRule::JulianSkip(day) => {
            let mut ordinal = u32::from(day);
            if is_leap_year(year) && ordinal > 59 {
                ordinal += 1;
            }
            month_day_of_ordinal(year, ordinal)
        }
        DstRule::JulianCount(day) => {
            let days_in_year = if is_leap_year(year) { 366 } else { 365 };
            month_day_of_ordinal(year, (u32::from(day) + 1).min(days_in_year))
        }
        DstRule::MonthWeekday { month, week, day } => {
            (month, month_weekday_day(year, month, week, day))
        }
    }
}

/// Unix instant of `rule` at wall-clock `time` in `year`, interpreted
/// with `base_offset` (seconds east) already in effect.
fn dst_rule_instant(rule: DstRule, time: i64, year: i64, base_offset: i64) -> i64 {
    let (month, day) = dst_rule_month_day(rule, year);
    days_from_civil(year, month, day)
        .saturating_mul(86_400)
        .saturating_add(time)
        .saturating_sub(base_offset)
}

/// Seconds east of UTC at `now` under POSIX `TZ` string `text`.
fn posix_offset(text: &str, now: i64) -> Option<i64> {
    let parsed = parse_posix_tz(text)?;
    let Some(dst) = parsed.dst else {
        return Some(parsed.std_offset);
    };
    let (year, _, _) = civil_from_days(now.div_euclid(86_400));
    let start = dst_rule_instant(dst.start.0, dst.start.1, year, parsed.std_offset);
    let end = dst_rule_instant(dst.end.0, dst.end.1, year, dst.offset);
    if start == end {
        return Some(parsed.std_offset);
    }
    let in_dst = if start < end {
        now >= start && now < end
    } else {
        // Southern hemisphere: daylight saving wraps the year end.
        now >= start || now < end
    };
    Some(if in_dst {
        dst.offset
    } else {
        parsed.std_offset
    })
}

/// One TZif local-time type: fixed offset plus DST flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TzifType {
    utoff: i64,
    is_dst: bool,
}

/// One TZif data block: transitions plus their types.
type TzBlock = (Vec<(i64, usize)>, Vec<TzifType>);

/// Parsed TZif data: explicit transitions plus the POSIX footer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TzifData {
    transitions: Vec<(i64, usize)>,
    types: Vec<TzifType>,
    default_type: usize,
    footer: Option<String>,
}

/// Bounds-checked big-endian reader over TZif bytes.
struct TzCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> TzCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        if end > self.data.len() {
            return None;
        }
        let out = &self.data[self.pos..end];
        self.pos = end;
        Some(out)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn be_u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn be_i32(&mut self) -> Option<i32> {
        self.take(4)
            .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn be_i64(&mut self) -> Option<i64> {
        self.take(8)
            .map(|b| i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
}

/// The six TZif header counts, in file order.
struct TzCounts {
    ut_indicators: u32,
    std_indicators: u32,
    leaps: u32,
    transitions: u32,
    types: u32,
    chars: u32,
}

/// Read the six header counts after the magic, version, and reserved bytes.
fn read_tz_counts(cursor: &mut TzCursor) -> Option<TzCounts> {
    Some(TzCounts {
        ut_indicators: cursor.be_u32()?,
        std_indicators: cursor.be_u32()?,
        leaps: cursor.be_u32()?,
        transitions: cursor.be_u32()?,
        types: cursor.be_u32()?,
        chars: cursor.be_u32()?,
    })
}

/// Skip a data block's leap-second and indicator tails (leaps are wider
/// in 64-bit blocks).
fn skip_tz_tail(cursor: &mut TzCursor, counts: &TzCounts, wide: bool) -> Option<()> {
    let leap_len = if wide { 12 } else { 8 };
    let leaps = usize::try_from(counts.leaps).ok()?;
    cursor.take(leaps.checked_mul(leap_len)?)?;
    let indicators = usize::try_from(counts.std_indicators)
        .ok()?
        .checked_add(usize::try_from(counts.ut_indicators).ok()?)?;
    cursor.take(indicators)?;
    Some(())
}

/// Skip one data block without retaining it (the 32-bit block when a
/// 64-bit block follows).
fn skip_tz_block(cursor: &mut TzCursor, counts: &TzCounts) -> Option<()> {
    let times = usize::try_from(counts.transitions).ok()?;
    let types = usize::try_from(counts.types).ok()?;
    cursor.take(times.checked_mul(5)?.checked_add(types.checked_mul(6)?)?)?;
    cursor.take(usize::try_from(counts.chars).ok()?)?;
    skip_tz_tail(cursor, counts, false)
}

/// Read one data block's transition instants (narrow or wide). Never
/// preallocates from the file's counts, so corrupt headers cannot exhaust
/// memory; every push is gated by bytes actually present.
fn read_tz_instants(cursor: &mut TzCursor, count: u32, wide: bool) -> Option<Vec<i64>> {
    let mut instants = Vec::new();
    for _ in 0..count {
        let at = if wide {
            cursor.be_i64()?
        } else {
            i64::from(cursor.be_i32()?)
        };
        instants.push(at);
    }
    Some(instants)
}

/// Pair transition instants with their type indices, rejecting indices
/// past the type table.
fn read_tz_indices(
    cursor: &mut TzCursor,
    instants: Vec<i64>,
    type_count: usize,
) -> Option<Vec<(i64, usize)>> {
    let mut transitions = Vec::new();
    for at in instants {
        let index = usize::from(cursor.byte()?);
        if index >= type_count {
            return None;
        }
        transitions.push((at, index));
    }
    Some(transitions)
}

/// Read one data block's local-time type table.
fn read_tz_types(cursor: &mut TzCursor, count: u32) -> Option<Vec<TzifType>> {
    let mut types = Vec::new();
    for _ in 0..count {
        let utoff = i64::from(cursor.be_i32()?);
        let is_dst = cursor.byte()? != 0;
        cursor.byte()?;
        types.push(TzifType { utoff, is_dst });
    }
    Some(types)
}

/// Read one data block's transitions and types (narrow or wide instants).
fn read_tz_block(cursor: &mut TzCursor, counts: &TzCounts, wide: bool) -> Option<TzBlock> {
    let instants = read_tz_instants(cursor, counts.transitions, wide)?;
    let type_count = usize::try_from(counts.types).ok()?;
    let transitions = read_tz_indices(cursor, instants, type_count)?;
    let types = read_tz_types(cursor, counts.types)?;
    if types.is_empty() {
        return None;
    }
    cursor.take(usize::try_from(counts.chars).ok()?)?;
    skip_tz_tail(cursor, counts, wide)?;
    Some((transitions, types))
}

/// Type index for instants before the first transition: the first
/// standard-time type, falling back to type 0.
fn default_tz_type(types: &[TzifType]) -> usize {
    types.iter().position(|tt| !tt.is_dst).unwrap_or(0)
}

/// The trailing `\n<posix-tz>\n` of a v2+ block, if well-formed.
fn read_tz_footer(cursor: &TzCursor) -> Option<String> {
    let text = str::from_utf8(cursor.data.get(cursor.pos..)?).ok()?;
    let body = text.strip_prefix('\n')?.strip_suffix('\n')?;
    if body.is_empty() || body.contains('\n') {
        return None;
    }
    Some(body.to_string())
}

/// One TZif header: the version byte plus the six counts.
struct TzHeader {
    version: u8,
    counts: TzCounts,
}

/// Read one header's magic, version, reserved bytes, and counts.
fn read_tz_header(cursor: &mut TzCursor) -> Option<TzHeader> {
    if cursor.take(4)? != b"TZif" {
        return None;
    }
    let version = cursor.byte()?;
    cursor.take(15)?;
    let counts = read_tz_counts(cursor)?;
    Some(TzHeader { version, counts })
}

/// Assemble parsed transitions, types, and footer into zone data.
fn finish_tzif(
    transitions: Vec<(i64, usize)>,
    types: Vec<TzifType>,
    footer: Option<String>,
) -> TzifData {
    TzifData {
        default_type: default_tz_type(&types),
        transitions,
        types,
        footer,
    }
}

/// Parse TZif bytes (v1 narrow, or v2+ wide with a POSIX footer).
fn parse_tzif(data: &[u8]) -> Option<TzifData> {
    let mut cursor = TzCursor::new(data);
    let header = read_tz_header(&mut cursor)?;
    if header.version == 0 {
        let (transitions, types) = read_tz_block(&mut cursor, &header.counts, false)?;
        return Some(finish_tzif(transitions, types, None));
    }
    if !matches!(header.version, b'2' | b'3' | b'4') {
        return None;
    }
    skip_tz_block(&mut cursor, &header.counts)?;
    let header = read_tz_header(&mut cursor)?;
    let (transitions, types) = read_tz_block(&mut cursor, &header.counts, true)?;
    let footer = read_tz_footer(&cursor);
    Some(finish_tzif(transitions, types, footer))
}

/// Seconds east of UTC at `now` under TZif `data`: the last explicit
/// transition at or before `now`, the default type before the first one,
/// or the POSIX footer after the last one.
fn tzif_offset(data: &[u8], now: i64) -> Option<i64> {
    let parsed = parse_tzif(data)?;
    let mut current = parsed.default_type;
    let mut after_last = true;
    for (at, index) in parsed.transitions.iter().copied() {
        if at <= now {
            current = index;
        } else {
            after_last = false;
        }
    }
    if after_last
        && let Some(footer) = parsed.footer.as_deref()
        && let Some(offset) = posix_offset(footer, now)
    {
        return Some(offset);
    }
    Some(parsed.types[current].utoff)
}

/// Zoneinfo files to try for a `TZ` name, in order (a slash also tries
/// the name as a literal path first).
fn tz_file_candidates(name: &str) -> Vec<String> {
    let mut paths = Vec::new();
    if name.contains('/') {
        paths.push(name.to_string());
    }
    for dir in [
        "/usr/share/zoneinfo",
        "/var/db/timezone/zoneinfo",
        "/usr/lib/zoneinfo",
        "/etc/zoneinfo",
    ] {
        paths.push(format!("{dir}/{name}"));
    }
    paths
}

/// Offset from the first candidate file that parses as TZif.
fn files_tz_offset(paths: &[String], now: i64) -> Option<i64> {
    for path in paths {
        if let Ok(bytes) = std::fs::read(path)
            && let Some(offset) = tzif_offset(&bytes, now)
        {
            return Some(offset);
        }
    }
    None
}

/// Offset for an explicit `TZ` value: `:path` reads one file, anything
/// else tries zoneinfo files first (like glibc) then POSIX rules.
fn env_tz_offset(tz: &str, now: i64) -> i64 {
    if let Some(path) = tz.strip_prefix(':') {
        return std::fs::read(path)
            .ok()
            .and_then(|bytes| tzif_offset(&bytes, now))
            .unwrap_or(0);
    }
    if let Some(offset) = files_tz_offset(&tz_file_candidates(tz), now) {
        return offset;
    }
    posix_offset(tz, now).unwrap_or(0)
}

/// Offset with `TZ` unset or empty: the system zone database, else UTC.
fn system_tz_offset(now: i64) -> i64 {
    if let Ok(bytes) = std::fs::read("/etc/localtime")
        && let Some(offset) = tzif_offset(&bytes, now)
    {
        return offset;
    }
    if let Ok(name) = std::fs::read_to_string("/etc/timezone") {
        let name = name.trim();
        if !name.is_empty()
            && let Some(offset) = files_tz_offset(&tz_file_candidates(name), now)
        {
            return offset;
        }
    }
    0
}

/// Seconds east of UTC in effect at `now_unix` under the system timezone:
/// explicit `TZ` first, then `/etc/localtime` (with `/etc/timezone` as a
/// fallback), else UTC. Never fails; unreadable data falls back to UTC.
pub fn local_utc_offset(now_unix: u64) -> i64 {
    let now = now_unix as i64;
    match std::env::var("TZ") {
        Ok(tz) if !tz.is_empty() => env_tz_offset(&tz, now),
        _ => system_tz_offset(now),
    }
}

/// Local hour (0-23) for `now_unix` under the system timezone, mirroring
/// PHP `date('G', $now)` (including DST) for the live clearing.
#[must_use]
pub fn local_hour(now_unix: u64) -> u32 {
    hour_with_offset(now_unix, local_utc_offset(now_unix))
}

/// Approximate lunar phase for flavor text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LunarPhase {
    New,
    Full,
    Waxing,
    Waning,
}

/// `PhutilLunarPhase`: a new moon (Aug 11, 1999).
const LUNAR_NEW_MOON_EPOCH: f64 = 934_354_800.0;
/// `PhutilLunarPhase`: seconds in one lunar phase.
const LUNAR_PHASE_LENGTH: f64 = 2_551_442.8768992;

/// Lunar cycle fraction for `epoch`, mirroring `PhutilLunarPhase`.
#[must_use]
pub fn lunar_fraction(epoch: i64) -> f64 {
    (epoch as f64 - LUNAR_NEW_MOON_EPOCH) % LUNAR_PHASE_LENGTH / LUNAR_PHASE_LENGTH
}

/// Approximate the lunar phase, checking new/full/waxing/waning in the
/// same order as the PHP workflow.
#[must_use]
pub fn lunar_phase(epoch: i64) -> LunarPhase {
    let phase = lunar_fraction(epoch);
    if !(0.02..=0.98).contains(&phase) {
        LunarPhase::New
    } else if (phase - 0.5).abs() < 0.02 {
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

/// `pht('You stand in the middle of a small clearing in the woods.')`.
#[must_use]
pub fn clearing_text() -> &'static str {
    "You stand in the middle of a small clearing in the woods."
}

/// `pht('Several small trails and footpaths cross here, ...')`.
#[must_use]
pub fn trails_text() -> &'static str {
    "Several small trails and footpaths cross here, twisting away from you among the trees."
}

/// `pht("Just ahead to the north, you can see **remotes**.\n")`.
#[must_use]
pub fn north_markup() -> &'static str {
    "Just ahead to the north, you can see **remotes**.\n"
}

/// `pht('You do not see "%s" anywhere.', ...)` with the thing joined.
#[must_use]
pub fn unknown_message(thing: &[String]) -> String {
    format!("You do not see \"{}\" anywhere.", thing.join(" "))
}

/// `pht('You follow a wide, straight path to the north, ...')`.
#[must_use]
pub fn grove_text() -> &'static str {
    "You follow a wide, straight path to the north and arrive in a grove of fruit trees after a few minutes of walking. The grass underfoot is thick and small insects flit through the air."
}

/// `pht('At the far edge of the grove, you see remotes:')`.
#[must_use]
pub fn grove_remotes_text() -> &'static str {
    "At the far edge of the grove, you see remotes:"
}

/// `pht("Across the grove, a stream flows north toward **published** commits.\n")`.
#[must_use]
pub fn stream_markup() -> &'static str {
    "Across the grove, a stream flows north toward **published** commits.\n"
}

/// `pht('You walk along the narrow bank of the stream, ...')`.
#[must_use]
pub fn river_text() -> &'static str {
    "You walk along the narrow bank of the stream as it winds lazily downhill and turns east, gradually widening into a river."
}

/// `pht('Floating on the water, you see published commits:')`.
#[must_use]
pub fn floating_text() -> &'static str {
    "Floating on the water, you see published commits:"
}

/// `pht('The river bubbles quietly, ...')`.
#[must_use]
pub fn bubbles_quiet_text() -> &'static str {
    "The river bubbles quietly, but you do not see any published commits anywhere."
}

/// `pht('They river bubbles peacefully.')`.
#[must_use]
pub fn bubbles_text() -> &'static str {
    "They river bubbles peacefully."
}

/// One git remote with its fetch/push URIs and resolved repository names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub push_uri: Option<String>,
    pub fetch_uri: Option<String>,
    pub push_repository: Option<String>,
    pub fetch_repository: Option<String>,
}

impl RemoteInfo {
    fn new(name: String, push_uri: Option<String>, fetch_uri: Option<String>) -> Self {
        Self {
            name,
            push_uri,
            fetch_uri,
            push_repository: None,
            fetch_repository: None,
        }
    }
}

/// List git remotes under `cwd` (repository names unresolved).
pub fn query_remotes(cwd: &Path) -> Result<Vec<RemoteInfo>, String> {
    if !git::is_inside_work_tree(cwd) {
        return Err("not inside a git repository".to_string());
    }
    let names = git::stdout_text(cwd, &["remote"])?;
    let mut remotes = Vec::new();
    for name in names.lines().filter(|l| !l.trim().is_empty()) {
        let push_uri = git::stdout_text(cwd, &["remote", "get-url", "--push", name]).ok();
        let fetch_uri = git::stdout_text(cwd, &["remote", "get-url", name]).ok();
        remotes.push(RemoteInfo::new(name.to_string(), push_uri, fetch_uri));
    }
    Ok(remotes)
}

/// The four ref-view lines for one remote (`pht('Push URI: %s')`, ...).
#[must_use]
pub fn remote_view_lines(remote: &RemoteInfo) -> [String; 4] {
    let push = remote.push_uri.as_deref().unwrap_or("-");
    let push_repo = remote.push_repository.as_deref().unwrap_or("-");
    let fetch = remote.fetch_uri.as_deref().unwrap_or("-");
    let fetch_repo = remote.fetch_repository.as_deref().unwrap_or("-");
    [
        format!("Push URI: {push}"),
        format!("Push Repository: {push_repo}"),
        format!("Fetch URI: {fetch}"),
        format!("Fetch Repository: {fetch_repo}"),
    ]
}

/// Build the `ArcanistRefView` for one remote (object name + lines).
#[must_use]
pub fn remote_ref_view(remote: &RemoteInfo) -> terminal::RefView {
    terminal::RefView {
        object_name: Some(remote.name.clone()),
        title: None,
        uri: None,
        lines: remote_view_lines(remote).to_vec(),
        children: Vec::new(),
    }
}

/// A repository from `diffusion.repository.search`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRef {
    pub name: String,
    pub uris: Vec<String>,
    /// `fields.refRules.permanentRefRules` (`None` when absent).
    pub permanent_rules: Option<Vec<String>>,
}

/// Search params for repository URIs (house `constraints` shape).
#[must_use]
pub fn repository_search_params(uris: &[String]) -> serde_json::Value {
    serde_json::json!({"constraints": {"uris": uris}, "attachments": {"uris": true}})
}

/// Parse a `diffusion.repository.search` result into repository refs.
pub fn parse_repository_search(value: &serde_json::Value) -> Vec<RepositoryRef> {
    let Some(rows) = value.get("data").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .map(|row| {
            let name = row
                .get("fields")
                .and_then(|f| f.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let uris = row
                .get("attachments")
                .and_then(|a| a.get("uris"))
                .and_then(|u| u.get("uris"))
                .and_then(serde_json::Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|entry| {
                            entry
                                .get("fields")
                                .and_then(|f| f.get("uri"))
                                .and_then(|u| u.get("effective"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let permanent_rules = row
                .get("fields")
                .and_then(|f| f.get("refRules"))
                .and_then(|r| r.get("permanentRefRules"))
                .and_then(serde_json::Value::as_array)
                .map(|rules| {
                    rules
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                });
            RepositoryRef {
                name,
                uris,
                permanent_rules,
            }
        })
        .collect()
}

/// Collect the distinct fetch/push URIs of `remotes` for the search.
#[must_use]
pub fn collect_remote_uris(remotes: &[RemoteInfo]) -> Vec<String> {
    let mut uris = Vec::new();
    for remote in remotes {
        for uri in remote.fetch_uri.iter().chain(remote.push_uri.iter()) {
            if !uris.contains(uri) {
                uris.push(uri.clone());
            }
        }
    }
    uris
}

/// Resolve repositories for `uris` via `diffusion.repository.search`.
pub fn resolve_repositories(
    call: &diff_submit::ConduitCall,
    uris: &[String],
) -> Result<Vec<RepositoryRef>, String> {
    if uris.is_empty() {
        return Ok(Vec::new());
    }
    let result = call(
        "diffusion.repository.search",
        &repository_search_params(uris),
    )?;
    Ok(parse_repository_search(&result))
}

/// Split a git URI into `(domain, path)`, covering `scheme://` and
/// scp-like (`[user@]host:path`) spellings; plain paths have no domain.
fn split_git_uri(uri: &str) -> (Option<String>, String) {
    if let Some(rest) = uri.split_once("://").map(|(_, rest)| rest) {
        let after_host = rest.find('/').map_or("", |i| &rest[i..]);
        let authority = rest.split('/').next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        let host = host.split(':').next().unwrap_or("");
        if host.is_empty() {
            return (None, after_host.to_string());
        }
        return (Some(host.to_string()), after_host.to_string());
    }
    // Windows drive paths (`C:\...`, `C:/...`) are local, not scp-like.
    if uri.len() >= 2
        && uri.as_bytes()[0].is_ascii_alphabetic()
        && uri.as_bytes()[1] == b':'
        && uri[2..].starts_with(['/', '\\'])
    {
        return (None, uri.to_string());
    }
    if let Some((host_part, path)) = uri.split_once(':')
        && !host_part.contains('/')
        && !host_part.contains('\\')
    {
        let host = host_part.rsplit('@').next().unwrap_or("");
        if !host.is_empty() {
            return (Some(host.to_string()), path.to_string());
        }
    }
    (None, uri.to_string())
}

/// Normalize a git URI for comparison, porting
/// `ArcanistRepositoryURINormalizer` (`TYPE_GIT`): lowercased domain plus
/// the trimmed path without `.git`, with Phabricator `/diffusion/X/`
/// prefixes collapsed to the callsign.
#[must_use]
pub fn normalize_git_uri(uri: &str) -> String {
    let (domain, path) = split_git_uri(uri);
    let domain = domain.map_or("<void>".to_string(), |d| d.to_lowercase());
    let mut path = path.trim_matches('/').to_string();
    if let Some(stripped) = path.strip_suffix(".git") {
        path = stripped.to_string();
    }
    if let Some(rest) = path.strip_prefix("diffusion/") {
        let head = rest.split('/').next().unwrap_or("");
        if !head.is_empty()
            && (head.chars().all(|c| c.is_ascii_uppercase())
                || head.chars().all(|c| c.is_ascii_digit()))
        {
            path = format!("diffusion/{head}");
        }
    }
    format!("{domain}/{path}")
}

/// Find the repository whose URIs contain `uri` after normalization.
#[must_use]
pub fn match_repository<'a>(repos: &'a [RepositoryRef], uri: &str) -> Option<&'a RepositoryRef> {
    let wanted = normalize_git_uri(uri);
    repos
        .iter()
        .find(|repo| repo.uris.iter().any(|u| normalize_git_uri(u) == wanted))
}

/// Fill push/fetch repository display names from resolved repositories.
pub fn fill_remote_repositories(remotes: &mut [RemoteInfo], repos: &[RepositoryRef]) {
    for remote in remotes {
        if let Some(uri) = remote.push_uri.clone() {
            remote.push_repository = match_repository(repos, &uri).map(|r| r.name.clone());
        }
        if let Some(uri) = remote.fetch_uri.clone() {
            remote.fetch_repository = match_repository(repos, &uri).map(|r| r.name.clone());
        }
    }
}

/// A remote-tracking branch marker (`refs/remotes/<remote>/<name>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub remote: String,
    pub name: String,
    pub hash: String,
}

/// Parse `for-each-ref` `refname + objectname` output into markers.
#[must_use]
pub fn parse_marker_refs(stdout: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        let Some((refname, hash)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Some(label) = refname.strip_prefix("refs/remotes/") else {
            continue;
        };
        let Some((remote, name)) = label.split_once('/') else {
            continue;
        };
        if remote.is_empty() || name.is_empty() || hash.trim().is_empty() {
            continue;
        }
        markers.push(Marker {
            remote: remote.to_string(),
            name: name.to_string(),
            hash: hash.trim().to_string(),
        });
    }
    markers
}

/// List remote-tracking branch markers under `cwd`.
pub fn query_markers(cwd: &Path) -> Result<Vec<Marker>, String> {
    if !git::is_inside_work_tree(cwd) {
        return Err("not inside a git repository".to_string());
    }
    let out = git::stdout_text(
        cwd,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/remotes",
        ],
    )?;
    Ok(parse_marker_refs(&out))
}

/// Mirror `ArcanistRepositoryRef::isPermanentRef`: absent rules match
/// nothing, empty rules match everything, otherwise exact names match.
/// `regexp(...)` rules never match (no regex engine).
#[must_use]
pub fn is_permanent_ref(rules: Option<&[String]>, name: &str) -> bool {
    let Some(rules) = rules else {
        return false;
    };
    if rules.is_empty() {
        return true;
    }
    rules.iter().any(|rule| rule == name)
}

/// Published hashes: markers whose remote resolves to a push repository
/// whose permanent-ref rules match the branch name.
#[must_use]
pub fn published_hashes(
    markers: &[Marker],
    remotes: &[RemoteInfo],
    repos: &[RepositoryRef],
) -> Vec<String> {
    let mut hashes = Vec::new();
    for marker in markers {
        let Some(remote) = remotes.iter().find(|r| r.name == marker.remote) else {
            continue;
        };
        let Some(push_uri) = remote.push_uri.as_deref() else {
            continue;
        };
        let Some(repo) = match_repository(repos, push_uri) else {
            continue;
        };
        if is_permanent_ref(repo.permanent_rules.as_deref(), &marker.name) {
            hashes.push(marker.hash.clone());
        }
    }
    hashes
}

/// Remotes under `cwd`, with repositories resolved when `call` is given.
pub fn look_remotes_data(
    cwd: &Path,
    call: Option<&diff_submit::ConduitCall>,
) -> Result<Vec<RemoteInfo>, String> {
    let mut remotes = query_remotes(cwd)?;
    if let Some(call) = call {
        let uris = collect_remote_uris(&remotes);
        let repos = resolve_repositories(call, &uris)?;
        fill_remote_repositories(&mut remotes, &repos);
    }
    Ok(remotes)
}

/// Published hashes under `cwd`: permanent refs when `call` resolves
/// repositories, else every remote-tracking tip (offline fallback).
pub fn look_published_data(
    cwd: &Path,
    call: Option<&diff_submit::ConduitCall>,
) -> Result<Vec<String>, String> {
    let markers = query_markers(cwd)?;
    let Some(call) = call else {
        return Ok(markers.into_iter().map(|m| m.hash).collect());
    };
    let remotes = query_remotes(cwd)?;
    let uris = collect_remote_uris(&remotes);
    let repos = resolve_repositories(call, &uris)?;
    Ok(published_hashes(&markers, &remotes, &repos))
}

/// Tips of remote-tracking branches under `cwd` (unfiltered).
pub fn query_published(cwd: &Path) -> Result<Vec<String>, String> {
    Ok(query_markers(cwd)?
        .into_iter()
        .map(|marker| marker.hash)
        .collect())
}

/// Plain-text clearing description for `now_unix` (no console markup),
/// in UTC so an instant always renders identically.
#[must_use]
pub fn render_clearing(now_unix: u64) -> String {
    let mut out = format!("{}\n\n", clearing_text());
    out += hour_text(hour_of(now_unix), lunar_phase(now_unix as i64));
    out += "\n\n";
    out += &format!("{}\n\n", trails_text());
    out += "Just ahead to the north, you can see remotes.\n";
    out
}

/// Plain-text message for looking at something which is not there.
#[must_use]
pub fn render_unknown(thing: &[String]) -> String {
    format!("{}\n", unknown_message(thing))
}

/// Plain-text rendering of one remote block.
#[must_use]
pub fn render_remote(remote: &RemoteInfo) -> String {
    let push = remote.push_uri.as_deref().unwrap_or("-");
    let push_repo = remote.push_repository.as_deref().unwrap_or("-");
    let fetch = remote.fetch_uri.as_deref().unwrap_or("-");
    let fetch_repo = remote.fetch_repository.as_deref().unwrap_or("-");
    format!(
        "Remote \"{}\":\n  Push URI: {push}\n  Push Repository: {push_repo}\n  Fetch URI: {fetch}\n  Fetch Repository: {fetch_repo}\n",
        remote.name
    )
}

/// Plain-text rendering of the `remotes` grove.
#[must_use]
pub fn render_remotes(remotes: &[RemoteInfo]) -> String {
    let mut out = format!("{}\n\n", grove_text());
    out += &format!("{}\n\n", grove_remotes_text());
    for remote in remotes {
        out += &render_remote(remote);
    }
    out += "\nAcross the grove, a stream flows north toward published commits.\n";
    out
}

/// Plain-text rendering of the `published` river.
#[must_use]
pub fn render_published(hashes: &[String]) -> String {
    let mut out = format!("{}\n\n", river_text());
    if hashes.is_empty() {
        out += &format!("{}\n", bubbles_quiet_text());
    } else {
        out += &format!("{}\n\n", floating_text());
        for hash in hashes {
            out += hash;
            out.push('\n');
        }
        out += &format!("\n{}\n", bubbles_text());
    }
    out
}

/// Render `thing` under `cwd` as plain text (no console markup),
/// returning the full stdout text and exit code. Local-only: repository
/// names stay `-` and published lists every remote-tracking tip.
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

/// The `%!` title banner plus blank line.
pub fn render_title(ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%!\n\n", &[terminal::TVal::Text(TITLE)], ansi)
        .map(|line| line.as_str().to_string())
}

/// One `%W` paragraph plus blank line.
pub fn render_paragraph(text: &str, ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%W\n\n", &[terminal::TVal::Text(text)], ansi)
        .map(|line| line.as_str().to_string())
}

/// A markup pattern with no substitutions (the `**remotes**` /
/// `**published**` lines and bare newlines).
pub fn render_markup(markup: &str, ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi(markup, &[], ansi).map(|line| line.as_str().to_string())
}

/// One `%s` line (unknown things and commit hashes).
pub fn render_line(text: &str, ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%s\n", &[terminal::TVal::Text(text)], ansi)
        .map(|line| line.as_str().to_string())
}

/// The empty-river paragraph (`"%W\n"`, no blank line).
pub fn render_quiet_river(ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%W\n", &[terminal::TVal::Text(bubbles_quiet_text())], ansi)
        .map(|line| line.as_str().to_string())
}

/// The peaceful-river tail (`"\n%W\n"`).
pub fn render_bubbles_tail(ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("\n%W\n", &[terminal::TVal::Text(bubbles_text())], ansi)
        .map(|line| line.as_str().to_string())
}

/// One remote's ref view (`tsprintf('%s', $view)`).
#[must_use]
pub fn render_remote_view(remote: &RemoteInfo, width: usize, ansi: bool) -> String {
    remote_ref_view(remote).render(0, width, ansi)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Best-effort Conduit client for repository mapping (`None` degrades to
/// local-only data).
fn look_conduit_client() -> Option<conduit::Client> {
    // NOTE: `conduit::Client` is not `Debug`; keep the error private.
    config::client_for(None).ok()
}

fn fail(message: String) -> ExitCode {
    // T1153 R4 (RB-D4): `look` runs in the toolset runtime
    // (`ArcanistLookWorkflow extends ArcanistArcWorkflow`), so runtime
    // errors escape into a PHP fatal (255). PHP never throws here,
    // but the envelope's fatal arm is the faithful default.
    conduit::report(EnvelopeKind::Fatal, &message)
}

fn conduit_error_text(error: conduit::Error) -> String {
    error.to_string()
}

fn call_or_unavailable(
    client: Option<&conduit::Client>,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match client {
        Some(client) => client.call(method, params).map_err(conduit_error_text),
        None => Err("conduit unavailable".to_string()),
    }
}

fn optional_call<'a>(
    client: &'a Option<conduit::Client>,
    call: &'a dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String>,
) -> Option<&'a diff_submit::ConduitCall<'a>> {
    if client.is_some() { Some(call) } else { None }
}

fn render_remotes_output(remotes: &[RemoteInfo], ansi: bool) -> Result<String, String> {
    let mut out = String::new();
    for part in [
        render_title(ansi),
        render_paragraph(grove_text(), ansi),
        render_paragraph(grove_remotes_text(), ansi),
    ] {
        out.push_str(&part?);
    }
    let width = terminal::terminal_width();
    for remote in remotes {
        out.push_str(&render_remote_view(remote, width, ansi));
    }
    let blank = render_markup("\n", ansi)?;
    let tail = render_markup(stream_markup(), ansi)?;
    out.push_str(&blank);
    out.push_str(&tail);
    Ok(out)
}

fn render_published_output(hashes: &[String], ansi: bool) -> Result<String, String> {
    let mut out = String::new();
    for part in [render_title(ansi), render_paragraph(river_text(), ansi)] {
        out.push_str(&part?);
    }
    if hashes.is_empty() {
        out.push_str(&render_quiet_river(ansi)?);
        return Ok(out);
    }
    out.push_str(&render_paragraph(floating_text(), ansi)?);
    for hash in hashes {
        out.push_str(&render_line(hash, ansi)?);
    }
    out.push_str(&render_bubbles_tail(ansi)?);
    Ok(out)
}

fn run_clearing(ansi: bool) -> ExitCode {
    let now = now_unix();
    let time = hour_text(local_hour(now), lunar_phase(now as i64));
    let parts = [
        render_title(ansi),
        render_paragraph(clearing_text(), ansi),
        render_paragraph(time, ansi),
        render_paragraph(trails_text(), ansi),
        render_markup(north_markup(), ansi),
    ];
    let mut out = String::new();
    for part in parts {
        match part {
            Ok(text) => out.push_str(&text),
            Err(message) => return fail(message),
        }
    }
    print!("{out}");
    ExitCode::SUCCESS
}

fn run_remotes(cwd: &Path, ansi: bool) -> ExitCode {
    let client = look_conduit_client();
    let call = |method: &str, params: &serde_json::Value| {
        call_or_unavailable(client.as_ref(), method, params)
    };
    let remotes = match look_remotes_data(cwd, optional_call(&client, &call)) {
        Ok(remotes) => remotes,
        Err(message) => return fail(message),
    };
    match render_remotes_output(&remotes, ansi) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(message) => fail(message),
    }
}

fn run_published(cwd: &Path, ansi: bool) -> ExitCode {
    let client = look_conduit_client();
    let call = |method: &str, params: &serde_json::Value| {
        call_or_unavailable(client.as_ref(), method, params)
    };
    let hashes = match look_published_data(cwd, optional_call(&client, &call)) {
        Ok(hashes) => hashes,
        Err(message) => return fail(message),
    };
    match render_published_output(&hashes, ansi) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(message) => fail(message),
    }
}

fn run_unknown(thing: &[String], ansi: bool) -> ExitCode {
    let mut out = String::new();
    for part in [
        render_title(ansi),
        render_line(&unknown_message(thing), ansi),
    ] {
        match part {
            Ok(text) => out.push_str(&text),
            Err(message) => return fail(message),
        }
    }
    print!("{out}");
    ExitCode::FAILURE
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
            // T1153 R4 (RB-D4): toolset parser errors render the red
            // `USAGE EXCEPTION` label with no usage dump.
            return conduit::report(EnvelopeKind::ToolsetUsage, &msg);
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let ansi = terminal::ansi_enabled();
    if thing.is_empty() {
        return run_clearing(ansi);
    }
    if thing == ["remotes"] {
        return run_remotes(&cwd, ansi);
    }
    if thing == ["published"] {
        return run_published(&cwd, ansi);
    }
    run_unknown(&thing, ansi)
}

#[cfg(test)]
#[path = "look_tests.rs"]
mod look_tests;
