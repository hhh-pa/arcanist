package main

import (
	"archive/tar"
	"compress/gzip"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type tarEntry struct {
	name     string
	linkname string
	flag     byte
	body     string
	mode     int64
}

func writeTar(t *testing.T, path string, entries []tarEntry) {
	t.Helper()
	fh, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	gz := gzip.NewWriter(fh)
	tw := tar.NewWriter(gz)
	for _, e := range entries {
		mode := e.mode
		if mode == 0 {
			mode = 0o644
		}
		if err := tw.WriteHeader(&tar.Header{
			Name:     e.name,
			Linkname: e.linkname,
			Typeflag: e.flag,
			Mode:     mode,
			Size:     int64(len(e.body)),
		}); err != nil {
			t.Fatal(err)
		}
		if _, err := tw.Write([]byte(e.body)); err != nil {
			t.Fatal(err)
		}
	}
	if err := tw.Close(); err != nil {
		t.Fatal(err)
	}
	if err := gz.Close(); err != nil {
		t.Fatal(err)
	}
	if err := fh.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestExtractTarGzKeepsNormalTree(t *testing.T) {
	dir := t.TempDir()
	archive := filepath.Join(dir, "tool.tar.gz")
	dest := filepath.Join(dir, "out")
	writeTar(t, archive, []tarEntry{
		{name: "tool-dir/", flag: tar.TypeDir, mode: 0o755},
		{name: "tool-dir/tool", flag: tar.TypeReg, body: "binary"},
	})
	if err := extractTarGz(archive, dest); err != nil {
		t.Fatal(err)
	}
	blob, err := os.ReadFile(filepath.Join(dest, "tool-dir", "tool"))
	if err != nil || string(blob) != "binary" {
		t.Fatalf("blob = %q, %v", blob, err)
	}
}

func TestExtractTarGzSkipsDegenerateDotEntries(t *testing.T) {
	dir := t.TempDir()
	archive := filepath.Join(dir, "dot.tar.gz")
	dest := filepath.Join(dir, "out")
	writeTar(t, archive, []tarEntry{
		{name: "./", flag: tar.TypeDir, mode: 0o755},
		{name: "ok", flag: tar.TypeReg, body: "fine"},
	})
	if err := extractTarGz(archive, dest); err != nil {
		t.Fatal(err)
	}
	blob, err := os.ReadFile(filepath.Join(dest, "ok"))
	if err != nil || string(blob) != "fine" {
		t.Fatalf("blob = %q, %v", blob, err)
	}
}

func TestExtractTarGzRejectsEscapes(t *testing.T) {
	cases := []struct {
		name  string
		entry tarEntry
		want  string
	}{
		{"dotdot", tarEntry{name: "../evil", flag: tar.TypeReg, body: "x"}, "escapes"},
		{"nested-dotdot", tarEntry{name: "a/../../evil", flag: tar.TypeReg, body: "x"}, "escapes"},
		{"absolute", tarEntry{name: "/tmp/evil", flag: tar.TypeReg, body: "x"}, "absolute"},
		{
			"symlink-out",
			tarEntry{name: "link", linkname: "../evil", flag: tar.TypeSymlink},
			"symlink escapes",
		},
		{
			"symlink-absolute",
			tarEntry{name: "link", linkname: "/etc/passwd", flag: tar.TypeSymlink},
			"symlink escapes",
		},
		{
			"hardlink-out",
			tarEntry{name: "link", linkname: "../evil", flag: tar.TypeLink},
			"hardlink escapes",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			archive := filepath.Join(dir, "evil.tar.gz")
			dest := filepath.Join(dir, "out")
			writeTar(t, archive, []tarEntry{c.entry})
			err := extractTarGz(archive, dest)
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Fatalf("err = %v, want %q", err, c.want)
			}
			if _, statErr := os.Lstat(filepath.Join(dir, "evil")); !os.IsNotExist(statErr) {
				t.Fatal("escape wrote outside dest")
			}
		})
	}
}
