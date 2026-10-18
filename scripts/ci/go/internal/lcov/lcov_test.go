package lcov

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestParseDAMergesDuplicateFiles(t *testing.T) {
	files := ParseDA("SF:a\nDA:1,2\nDA:2,0\nend_of_record\nSF:a\nDA:2,3\nend_of_record\n")
	want := map[string]map[string]int64{"a": {"1": 2, "2": 3}}
	if !reflect.DeepEqual(files, want) {
		t.Fatalf("got %v want %v", files, want)
	}
}

func TestParseDADropsGarbage(t *testing.T) {
	files := ParseDA("SF:a\nDA:1,xx\nDA:2\nDA:3,-4\nDA:4,99999999999999999999999\nend_of_record\n")
	got := files["a"]
	if len(got) != 2 || got["3"] != -4 || got["4"] <= 0 {
		t.Fatalf("got %v", got)
	}
}

func TestParseRecordsSanitizesAbsurdCounts(t *testing.T) {
	text := "SF:a\nDA:1,18446744073709551615\nDA:2,-1\nDA:3,xx\nDA:4,5\nend_of_record\n"
	records, sanitized := ParseRecords(text)
	if len(records) != 1 {
		t.Fatalf("records: %v", records)
	}
	da := records[0].Rec.DA
	if da["1"] != 0 || da["2"] != 0 || da["4"] != 5 {
		t.Fatalf("da: %v", da)
	}
	if _, ok := da["3"]; ok {
		t.Fatalf("non-numeric kept: %v", da)
	}
	if len(sanitized) != 2 {
		t.Fatalf("sanitized: %v", sanitized)
	}
}

func TestParseRecordsMergesBranches(t *testing.T) {
	text := "SF:a\nBRDA:1,0,0,3\nBRDA:1,0,0,4\nBRDA:2,0,1,-\nBRDA:2,0,1,5\nFNDA:2,main\nFN:10,main\nend_of_record\n"
	records, _ := ParseRecords(text)
	rec := records[0].Rec
	if rec.BRDA[BRKey{"1", "0", "0"}] != "7" {
		t.Fatalf("brda: %v", rec.BRDA)
	}
	if rec.BRDA[BRKey{"2", "0", "1"}] != "5" {
		t.Fatalf("brda dash-then-taken: %v", rec.BRDA)
	}
	if rec.FNDA["main"] != 2 || !rec.FN[FNKey{"main", "10"}] {
		t.Fatalf("fn: %v %v", rec.FNDA, rec.FN)
	}
}

func TestParseSummariesTrustsLFAndLH(t *testing.T) {
	found, hit, files := ParseSummaries(
		"SF:b\nLF:10\nLH:7\nend_of_record\nSF:a\nLF:5\nLH:5\nend_of_record\n",
	)
	if found != 15 || hit != 12 || len(files) != 2 {
		t.Fatalf("got %d %d %v", found, hit, files)
	}
	if files[0].Path != "b" || files[1].Path != "a" {
		t.Fatalf("order: %v", files)
	}
}

func TestParseSummariesDropsUnterminatedFile(t *testing.T) {
	_, _, files := ParseSummaries("SF:a\nLF:5\nLH:5\n")
	if len(files) != 0 {
		t.Fatalf("got %v", files)
	}
}

func TestParseSummariesSkipsOrphanCounts(t *testing.T) {
	// Python int() would crash on LF-before-SF; fail closed at zero.
	found, hit, files := ParseSummaries("LF:5\nLH:5\nend_of_record\n")
	if found != 0 || hit != 0 || len(files) != 0 {
		t.Fatalf("got %d %d %v", found, hit, files)
	}
}

func TestNormPathMatchesPosix(t *testing.T) {
	for in, want := range map[string]string{
		"/a/./b/../c": "/a/c",
		"//a":         "//a",
		"//":          "//",
		"///a":        "/a",
		"":            ".",
		"a//b":        "a/b",
	} {
		if got := NormPath(in); got != want {
			t.Fatalf("NormPath(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestCanonicalizeMergesTwinsAndFilters(t *testing.T) {
	text := "SF:/ws/c/app/tests/../src/t.rs\nDA:1,2\nend_of_record\n" +
		"SF:/ws/c/app/src/t.rs\nDA:1,3\nDA:2,0\nend_of_record\n" +
		"SF:/ws/c/app/tests/u.rs\nDA:1,9\nend_of_record\n" +
		"SF:/ws/target/x.rs\nDA:1,9\nend_of_record\n" +
		"SF:/other/y.rs\nDA:1,9\nend_of_record\n" +
		"SF:rel/z.rs\nDA:1,4\nend_of_record\n"
	records, _ := ParseRecords(text)
	canon := Canonicalize(records, "/ws")
	if len(canon) != 2 {
		t.Fatalf("files: %v", canon)
	}
	if canon[0].SF != "/ws/c/app/src/t.rs" || canon[1].SF != "/ws/rel/z.rs" {
		t.Fatalf("sfs: %v", canon)
	}
	if canon[0].Rec.DA["1"] != 5 || canon[0].Rec.DA["2"] != 0 {
		t.Fatalf("merged da: %v", canon[0].Rec.DA)
	}
	if canon[1].Rec.DA["1"] != 4 {
		t.Fatalf("relative da: %v", canon[1].Rec.DA)
	}
}

func TestEmitRecomputesSummaries(t *testing.T) {
	records, _ := ParseRecords(
		"SF:/ws/a.rs\nFN:10,f\nFN:20,g\nFNDA:2,f\nFNDA:0,g\n" +
			"BRDA:3,0,0,1\nBRDA:3,0,1,-\nBRDA:10,0,0,0\n" +
			"DA:1,5\nDA:2,0\nDA:10,1\nLF:99\nLH:99\nend_of_record\n",
	)
	got := Emit(Canonicalize(records, "/ws"))
	want := "SF:/ws/a.rs\n" +
		"FN:10,f\nFN:20,g\n" +
		"FNDA:2,f\nFNDA:0,g\n" +
		"FNF:2\nFNH:1\n" +
		"BRDA:3,0,0,1\nBRDA:3,0,1,-\nBRDA:10,0,0,0\n" +
		"BRF:3\nBRH:1\n" +
		"DA:1,5\nDA:2,0\nDA:10,1\n" +
		"LF:3\nLH:2\n" +
		"end_of_record\n"
	if got != want {
		t.Fatalf("mismatch:\n got: %q\nwant: %q", got, want)
	}
	if Emit(nil) != "" {
		t.Fatalf("empty emit should be empty")
	}
}

func TestLineSetsAndSubset(t *testing.T) {
	records, _ := ParseRecords(
		"SF:/ws/a.rs\nDA:1,1\nDA:2,0\nend_of_record\n",
	)
	canon := Canonicalize(records, "/ws")
	total, covered := LineSets(canon)
	if len(total) != 2 || len(covered) != 1 {
		t.Fatalf("sets: %v %v", total, covered)
	}
	if !IsSubset(covered, total) || IsSubset(total, covered) {
		t.Fatalf("subset broken: %v %v", total, covered)
	}
}

func writeSource(t *testing.T, dir, name, body string) string {
	t.Helper()
	p := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

func TestApplyExclusionsGolden(t *testing.T) {
	dir := t.TempDir()
	src := writeSource(t, dir, "src.rs",
		"fn covered() {\n"+ // 1 kept
			"    alive(); // LCOV_EXCL_LINE\n"+ // 2 dropped (covered, still dropped)
			"    alive();\n"+ // 3 kept
			"    // LCOV_EXCL_START\n"+ // 4 dropped (marker line itself)
			"    dead_windows_only();\n"+ // 5 dropped
			"    also_dead(); // LCOV_EXCL_LINE\n"+ // 6 dropped
			"    // LCOV_EXCL_STOP\n"+ // 7 dropped (marker line itself)
			"    alive();\n"+ // 8 kept
			"}\n") // 9 kept
	in := "SF:" + src + "\n" +
		"FN:1,covered\nFN:5,windows_only\n" +
		"FNDA:3,covered\nFNDA:0,windows_only\nFNDA:9,orphan\n" +
		"BRDA:2,0,0,5\nBRDA:3,0,0,2\nBRDA:6,0,1,-\n" +
		"DA:1,3\nDA:2,3\nDA:3,1\nDA:4,0\nDA:5,0\nDA:6,0\nDA:7,0\nDA:8,2\nDA:9,3\n" +
		"end_of_record\n"
	records, _ := ParseRecords(in)
	filtered, err := ApplyExclusions(records, dir)
	if err != nil {
		t.Fatal(err)
	}
	want := "SF:" + src + "\n" +
		"FN:1,covered\n" +
		"FNDA:3,covered\nFNDA:9,orphan\n" +
		"FNF:2\nFNH:2\n" +
		"BRDA:3,0,0,2\n" +
		"BRF:1\nBRH:1\n" +
		"DA:1,3\nDA:3,1\nDA:8,2\nDA:9,3\n" +
		"LF:4\nLH:4\n" +
		"end_of_record\n"
	if got := Emit(filtered); got != want {
		t.Fatalf("mismatch:\n got: %q\nwant: %q", got, want)
	}
	// Input records are never mutated.
	if records[0].Rec.DA["2"] != 3 {
		t.Fatalf("input mutated: %v", records[0].Rec.DA)
	}
}

func TestApplyExclusionsErrors(t *testing.T) {
	dir := t.TempDir()
	cases := []struct {
		name string
		body string
		want string
	}{
		{"unclosed START", "a\n// LCOV_EXCL_START\nb\n", "without closing"},
		{"nested START",
			"// LCOV_EXCL_START\na\n// LCOV_EXCL_START\nb\n// LCOV_EXCL_STOP\n// LCOV_EXCL_STOP\n",
			"nested"},
		{"stray STOP", "a\n// LCOV_EXCL_STOP\n", "without"},
		{"START and STOP on one line", "// LCOV_EXCL_START LCOV_EXCL_STOP\n", "same line"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			src := writeSource(t, dir, "bad.rs", tc.body)
			records, _ := ParseRecords("SF:" + src + "\nDA:1,0\nend_of_record\n")
			_, err := ApplyExclusions(records, dir)
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("err = %v, want substring %q", err, tc.want)
			}
		})
	}
	t.Run("missing file fails closed", func(t *testing.T) {
		records, _ := ParseRecords("SF:" + filepath.Join(dir, "gone.rs") + "\nDA:1,0\nend_of_record\n")
		if _, err := ApplyExclusions(records, dir); err == nil {
			t.Fatal("expected error for unreadable source")
		}
	})
}

func TestBareBraceLines(t *testing.T) {
	dir := t.TempDir()
	src := writeSource(t, dir, "b.rs",
		"fn f() {\n"+ // 1 code: not bare
			"    }\n"+ // 2 bare
			"} // trailing comment\n"+ // 3 bare (comment stripped)
			"    }),\n"+ // 4 bare
			"    } else {\n"+ // 5 branch line: not bare
			"}));\n"+ // 6 bare
			"    // just a comment\n"+ // 7 comment-only: not bare
			"\n"+ // 8 empty: not bare
			"    foo();\n"+ // 9 code: not bare
			"};\n"+ // 10 bare
			"    (\n") // 11 opener: not bare (conservative)
	braces, err := BareBraceLines(src)
	if err != nil {
		t.Fatal(err)
	}
	want := map[int]bool{2: true, 3: true, 4: true, 6: true, 10: true}
	if !reflect.DeepEqual(braces, want) {
		t.Fatalf("got %v want %v", braces, want)
	}
	if _, err := BareBraceLines(filepath.Join(dir, "gone.rs")); err == nil {
		t.Fatal("expected error for unreadable source")
	}
}

func TestApplyBareBraceFilter(t *testing.T) {
	dir := t.TempDir()
	src := writeSource(t, dir, "s.rs",
		"fn f() {\n"+ // 1 DA 3: kept
			"    }\n"+ // 2 DA 0: dropped (uncoverable zero)
			"    }\n"+ // 3 DA 5: kept (covered brace stays)
			"    missed();\n"+ // 4 DA 0: kept (true miss still fails the floor)
			"}\n") // 5 DA 0: dropped
	in := "SF:" + src + "\n" +
		"DA:1,3\nDA:2,0\nDA:3,5\nDA:4,0\nDA:5,0\nDA:xx,0\n" +
		"end_of_record\n"
	records, _ := ParseRecords(in)
	filtered, err := ApplyBareBraceFilter(records, dir)
	if err != nil {
		t.Fatal(err)
	}
	want := "SF:" + src + "\n" +
		"DA:1,3\nDA:3,5\nDA:4,0\nDA:xx,0\n" +
		"LF:4\nLH:2\n" +
		"end_of_record\n"
	if got := Emit(filtered); got != want {
		t.Fatalf("mismatch:\n got: %q\nwant: %q", got, want)
	}
	// Input records are never mutated.
	if _, ok := records[0].Rec.DA["2"]; !ok {
		t.Fatalf("input mutated: %v", records[0].Rec.DA)
	}
	// Unreadable source fails closed.
	missing, _ := ParseRecords("SF:" + filepath.Join(dir, "gone.rs") + "\nDA:1,0\nend_of_record\n")
	if _, err := ApplyBareBraceFilter(missing, dir); err == nil {
		t.Fatal("expected error for unreadable source")
	}
}

func TestApplyExclusionsResolvesRelativeSF(t *testing.T) {
	dir := t.TempDir()
	writeSource(t, dir, "sub/f.rs", "alive();\ndead(); // LCOV_EXCL_LINE\n")
	records, _ := ParseRecords("SF:sub/f.rs\nDA:1,1\nDA:2,0\nend_of_record\n")
	filtered, err := ApplyExclusions(records, dir)
	if err != nil {
		t.Fatal(err)
	}
	if got, want := Emit(filtered), "SF:sub/f.rs\nDA:1,1\nLF:1\nLH:1\nend_of_record\n"; got != want {
		t.Fatalf("mismatch:\n got: %q\nwant: %q", got, want)
	}
}
