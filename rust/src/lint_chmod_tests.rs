use super::*;

use std::path::{Path, PathBuf};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-chmod-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn png() -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&[0u8; 16]);
    data
}

#[test]
fn linter_name_is_chmod() {
    assert_eq!(ChmodLinter::new().name(), "CHMOD");
}

#[test]
fn binary_means_nul_byte() {
    assert!(is_binary(b"a\0b"));
    assert!(!is_binary(b"plain text\n"));
    assert!(!is_binary(b""));
}

#[test]
fn shebang_means_hash_bang_prefix() {
    assert!(has_shebang(b"#!/bin/sh\n"));
    assert!(has_shebang(b"#!"));
    assert!(!has_shebang(b"# !/bin/sh\n"));
    assert!(!has_shebang(b"\n#!/bin/sh\n"));
    assert!(!has_shebang(b""));
}

#[test]
fn non_executable_files_are_silent() {
    assert!(check_bytes("f.sh", b"no shebang\n", false).is_empty());
    assert!(check_bytes("f.png", &png(), false).is_empty());
}

#[test]
fn executable_text_needs_a_shebang() {
    assert!(check_bytes("run.sh", b"#!/bin/sh\necho\n", true).is_empty());
    let found = check_bytes("run.sh", b"echo hi\n", true);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "run.sh");
    assert_eq!(found[0].code, "CHMOD1");
    assert_eq!(found[0].severity, "warning");
    assert_eq!(found[0].name, "Invalid Executable");
    assert_eq!((found[0].line, found[0].col), (0, 0));
    assert_eq!(
        found[0].description,
        "Executable files should either be binary or contain a shebang."
    );
}

#[test]
fn executable_unknown_binaries_are_valid() {
    // ELF binary: unknown MIME hits the PHP `default` branch (accepted).
    let mut elf = b"\x7fELF".to_vec();
    elf.extend_from_slice(&[0u8; 16]);
    assert!(check_bytes("a.out", &elf, true).is_empty());
}

#[test]
fn executable_media_types_are_flagged() {
    let mut tar = vec![0u8; 300];
    tar[257..262].copy_from_slice(b"ustar");
    let mut eot = vec![0u8; 40];
    eot[0..4].copy_from_slice(&40u32.to_le_bytes());
    eot[34..36].copy_from_slice(b"LP");
    let cases: &[(&[u8], &str)] = &[
        (b"PK\x03\x04\x00data", "application/zip"),
        (b"\x1f\x8b\x08\x00data", "application/x-gzip"),
        (b"BZh91AY&SY\x00data", "application/x-bzip2"),
        (b"Rar!\x1a\x07\x00data", "application/x-rar-compressed"),
        (&tar, "application/x-tar"),
        (b"MThd\x00\x00data", "audio/midi"),
        (b"ID3\x04\x00data", "audio/mpeg"),
        (b"\xff\xfb\x90\x00data", "audio/mpeg"),
        (b"RIFF\x00\x00\x00\x00WAVEdata", "audio/x-wav"),
        (b"\x00\x00\x00\x20ftypM4A data", "audio/mp4"),
        (b"wOFFdata\x00", "application/x-woff"),
        (b"\x00\x01\x00\x00data\x00", "application/x-font-ttf"),
        (&eot, "application/vnd.ms-fontobject"),
        (b"FWS\x09data\x00", "application/x-shockwave-flash"),
        (b"GIF89adata\x00", "image/gif"),
        (b"\xff\xd8\xff\xe0data\x00", "image/jpeg"),
        (&png(), "image/png"),
        (b"II*\x00data\x00", "image/tiff"),
        (b"MM\x00*data\x00", "image/tiff"),
        (b"\x00\x00\x01\x00data", "image/x-icon"),
        (b"BMdata\x00", "image/x-ms-bmp"),
        (
            b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1data\x00",
            "application/msword",
        ),
        (b"%PDF-1.4\x00data", "application/pdf"),
        (b"%!PS-Adobe\x00data", "application/postscript"),
        (b"{\\rtf1\x00data", "application/rtf"),
        (b"\x00\x00\x01\xb3data", "video/mpeg"),
        (b"\x00\x00\x00\x20ftypqt  data", "video/quicktime"),
        (b"FLV\x01data\x00", "video/x-flv"),
        (
            b"\x30\x26\xb2\x75\x8e\x66\xcf\x11data\x00",
            "video/x-ms-wmv",
        ),
        (b"RIFF\x00\x00\x00\x00AVI data", "video/x-msvideo"),
    ];
    for (data, mime) in cases {
        assert_eq!(detect_non_executable_mime(data), Some(*mime), "for {mime}");
        let found = check_bytes("f.bin", data, true);
        assert_eq!(found.len(), 1, "for {mime}");
        assert_eq!(found[0].code, "CHMOD1");
        assert_eq!(found[0].severity, "warning");
        assert_eq!(
            found[0].description,
            format!("'{mime}' files should not be executable.")
        );
    }
}

#[test]
fn mp4_video_brands_are_accepted_like_php_default() {
    // `video/mp4` is not in the PHP switch, so it hits `default` (valid).
    let data = b"\x00\x00\x00\x20ftypisomdata\x00";
    assert_eq!(detect_non_executable_mime(data), None);
    assert!(check_bytes("f.mp4", data, true).is_empty());
}

#[test]
fn lint_bytes_is_silent_without_disk_state() {
    // Missing paths cannot be executable, so they stay silent.
    let linter = ChmodLinter::new();
    assert!(
        linter
            .lint_bytes("definitely/missing/run.sh", b"echo hi\n")
            .is_empty()
    );
}

#[test]
fn directories_are_never_executable() {
    let tmp = TempDir::new("dirstat");
    let dir = tmp.path().to_string_lossy().into_owned();
    assert!(ChmodLinter::new().lint_bytes(&dir, b"echo hi\n").is_empty());
}

#[cfg(unix)]
#[test]
fn lint_bytes_reads_the_executable_bit_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("arc-rs-chmod-{}-{}", std::process::id(), line!()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("run.sh");
    std::fs::write(&script, b"echo hi\n").unwrap();
    let display = script.to_string_lossy().to_string();

    let linter = ChmodLinter::new();
    assert!(linter.lint_bytes(&display, b"echo hi\n").is_empty());

    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let found = linter.lint_bytes(&display, b"echo hi\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "CHMOD1");

    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        linter
            .lint_bytes(&display, b"#!/bin/sh\necho hi\n")
            .is_empty()
    );

    std::fs::remove_dir_all(&dir).ok();
}

// -( T1153 Wave 2: lint retrocompat )--------------------------------------

#[cfg(unix)]
#[test]
fn lint26_executable_bit_stats_the_engine_disk_path() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("arc-rs-chmod-{}-{}", std::process::id(), line!()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("run.sh");
    std::fs::write(&script, b"echo hi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The display path resolves to nothing from the process working
    // directory; only the engine's project-root disk path finds the
    // executable bit (PHP `getFilePathOnDisk()`).
    let linter = ChmodLinter::new();
    linter.set_active_disk_path(&script);
    let found = linter.lint_bytes("definitely-not-here-xyz/run.sh", b"echo hi\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "CHMOD1");

    std::fs::remove_dir_all(&dir).ok();
}
