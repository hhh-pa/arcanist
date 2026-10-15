package lcov

import (
	"reflect"
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
