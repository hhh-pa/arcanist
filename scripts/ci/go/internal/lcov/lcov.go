// Package lcov parses, merges, and emits LCOV coverage records,
// mirroring scripts/ci/lib: coverage-floor-check.py's DA-only merge,
// coverage-summary.py's LF/LH reader, and rust-quality-gates.py's full
// DA/BRDA/FNDA/FN record model with absurd-count clamping.
package lcov

import (
	"math"
	"sort"
	"strconv"
	"strings"
)

// AbsurdCount bounds real execution counts: llvm occasionally emits
// u64::MAX on async-block boundaries. The full parser clamps counts
// above this (or below zero) to 0, mirroring _ABSURD_COUNT.
const AbsurdCount = int64(1) << 62

// Record is one file's coverage: DA line->count, BRDA
// (line,block,branch)->taken ("-" or decimal), FNDA name->count, and
// the FN set of (name,line).
type Record struct {
	DA   map[string]int64
	BRDA map[BRKey]string
	FNDA map[string]int64
	FN   map[FNKey]bool
}

// BRKey identifies one branch record.
type BRKey struct {
	Line   string
	Block  string
	Branch string
}

// FNKey identifies one function record.
type FNKey struct {
	Name string
	Line string
}

// SFRecord pairs a source path with its record. Duplicate SFs stay
// separate entries (merged later by Canonicalize), mirroring the
// Python list-of-tuples.
type SFRecord struct {
	SF  string
	Rec Record
}

// Sanitized reports one clamped absurd count.
type Sanitized struct {
	SF   string
	What string
	Raw  string
}

// NewRecord returns an empty Record.
func NewRecord() Record {
	return Record{
		DA:   map[string]int64{},
		BRDA: map[BRKey]string{},
		FNDA: map[string]int64{},
		FN:   map[FNKey]bool{},
	}
}

// splitLines mirrors text.splitlines + per-line strip: "\n"-split,
// "\r" kept unless the line strip removes it (strip trims all
// whitespace, including "\r").
func splitLines(text string) []string {
	raw := strings.Split(text, "\n")
	out := raw[:0]
	for _, line := range raw {
		// Drop the trailing empty element a final newline leaves,
		// which splitlines does not yield.
		out = append(out, strings.TrimSpace(line))
	}
	if len(out) > 0 && out[len(out)-1] == "" && strings.HasSuffix(text, "\n") {
		out = out[:len(out)-1]
	}
	return out
}

// parseCount parses a decimal count. Huge magnitudes saturate (Python
// ints are unbounded); non-numeric input fails.
func parseCount(raw string) (int64, bool) {
	count, err := strconv.ParseInt(strings.TrimSpace(raw), 10, 64)
	if err == nil {
		return count, true
	}
	if num, ok := err.(*strconv.NumError); ok && num.Err == strconv.ErrRange {
		if strings.HasPrefix(strings.TrimSpace(raw), "-") {
			return math.MinInt64, true
		}
		return math.MaxInt64, true
	}
	return 0, false
}

// saneCount clamps profiler-garbage counts to 0, recording the event.
// Non-numeric input fails (the record is skipped), mirroring _sane_count.
func saneCount(raw, sf, what string, sanitized *[]Sanitized) (int64, bool) {
	count, ok := parseCount(raw)
	if !ok {
		return 0, false
	}
	if count > AbsurdCount || count < 0 {
		*sanitized = append(*sanitized, Sanitized{SF: sf, What: what, Raw: raw})
		return 0, true
	}
	return count, true
}

func mergeBRDATaken(slot map[BRKey]string, key BRKey, taken string) {
	prev, present := slot[key]
	if !present || prev == "-" {
		slot[key] = taken
		return
	}
	if taken == "-" {
		return
	}
	a, aok := parseCount(prev)
	b, bok := parseCount(taken)
	if aok && bok {
		slot[key] = strconv.FormatInt(a+b, 10)
	}
}

// ParseRecords parses full LCOV text, mirroring parse_lcov_records:
// summary lines are dropped (recomputed on emit), absurd counts are
// clamped and reported.
func ParseRecords(text string) ([]SFRecord, []Sanitized) {
	var records []SFRecord
	var sanitized []Sanitized
	var sf string
	haveSF := false
	rec := NewRecord()
	flush := func() {
		if haveSF {
			records = append(records, SFRecord{SF: sf, Rec: rec})
		}
	}
	for _, line := range splitLines(text) {
		switch {
		case strings.HasPrefix(line, "SF:"):
			flush()
			sf = line[len("SF:"):]
			rec = NewRecord()
			haveSF = true
		case line == "end_of_record":
			flush()
			haveSF = false
		case !haveSF:
			continue
		case strings.HasPrefix(line, "DA:"):
			parts := strings.Split(line[len("DA:"):], ",")
			if len(parts) < 2 {
				continue
			}
			count, ok := saneCount(parts[1], sf, "DA:"+parts[0], &sanitized)
			if !ok {
				continue
			}
			rec.DA[parts[0]] += count
		case strings.HasPrefix(line, "BRDA:"):
			parts := strings.Split(line[len("BRDA:"):], ",")
			if len(parts) < 4 {
				continue
			}
			taken := parts[3]
			if taken != "-" {
				sane, ok := saneCount(
					taken, sf,
					"BRDA:"+parts[0]+","+parts[1]+","+parts[2],
					&sanitized,
				)
				if !ok {
					taken = "-"
				} else {
					taken = strconv.FormatInt(sane, 10)
				}
			}
			mergeBRDATaken(rec.BRDA, BRKey{parts[0], parts[1], parts[2]}, taken)
		case strings.HasPrefix(line, "FNDA:"):
			parts := strings.SplitN(line[len("FNDA:"):], ",", 2)
			if len(parts) != 2 {
				continue
			}
			count, ok := saneCount(parts[0], sf, "FNDA:"+parts[1], &sanitized)
			if !ok {
				continue
			}
			rec.FNDA[parts[1]] += count
		case strings.HasPrefix(line, "FN:"):
			parts := strings.SplitN(line[len("FN:"):], ",", 2)
			if len(parts) == 2 {
				rec.FN[FNKey{parts[1], parts[0]}] = true
			}
		}
	}
	flush()
	return records, sanitized
}

// ParseDA merges DA-only records per file, mirroring
// coverage-floor-check.py's parse_lcov_lines (no count clamping).
func ParseDA(text string) map[string]map[string]int64 {
	files := map[string]map[string]int64{}
	var sf string
	haveSF := false
	for _, line := range splitLines(text) {
		switch {
		case strings.HasPrefix(line, "SF:"):
			sf = line[len("SF:"):]
			haveSF = true
			if _, ok := files[sf]; !ok {
				files[sf] = map[string]int64{}
			}
		case line == "end_of_record":
			haveSF = false
		case haveSF && strings.HasPrefix(line, "DA:"):
			parts := strings.Split(line[len("DA:"):], ",")
			if len(parts) < 2 {
				continue
			}
			count, ok := parseCount(parts[1])
			if !ok {
				continue
			}
			files[sf][parts[0]] += count
		}
	}
	return files
}

// FileSummary is one file's LF/LH totals, mirroring
// coverage-summary.py's parse_lcov rows.
type FileSummary struct {
	Path  string
	Found int64
	Hit   int64
}

// ParseSummaries reads raw LF/LH totals per file (trusted as-is, never
// recomputed), mirroring coverage-summary.py. Lines keep a trailing
// "\r" (Python rstrips "\n" only).
func ParseSummaries(text string) (found, hit int64, files []FileSummary) {
	var current *FileSummary
	for _, raw := range strings.Split(text, "\n") {
		line := strings.TrimSuffix(raw, "\n")
		switch {
		case strings.HasPrefix(line, "SF:"):
			current = &FileSummary{Path: line[len("SF:"):]}
		case current != nil && strings.HasPrefix(line, "LF:"):
			current.Found = mustCount(line[len("LF:"):])
		case current != nil && strings.HasPrefix(line, "LH:"):
			current.Hit = mustCount(line[len("LH:"):])
		case line == "end_of_record" && current != nil:
			files = append(files, *current)
			found += current.Found
			hit += current.Hit
			current = nil
		}
	}
	return found, hit, files
}

// mustCount parses summary counts. Python int() would crash on garbage
// here (no such input exists in real LCOV); fail closed at zero.
func mustCount(raw string) int64 {
	count, ok := parseCount(raw)
	if !ok {
		return 0
	}
	return count
}

// SortedSFs returns SFs in byte order.
func SortedSFs(files map[string]map[string]int64) []string {
	sfs := make([]string, 0, len(files))
	for sf := range files {
		sfs = append(sfs, sf)
	}
	sort.Strings(sfs)
	return sfs
}
