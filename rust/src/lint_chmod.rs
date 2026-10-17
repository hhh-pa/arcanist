//! `chmod` linter: executable files must be binary or have a shebang.
//!
//! PHP reference: `src/lint/linter/ArcanistChmodLinter.php` (`CHMOD`).
//!
//! Rules (severity `warning`): when a file is executable and binary, it is
//! accepted unless its type is one that should never be executable
//! (archives, audio, fonts, images, documents, video — see the PHP mime
//! switch); when it is executable and text, it must start with a `#!`
//! shebang line.
//!
//! The executable bit is read from disk via the engine's
//! `set_active_disk_path()` path (`getFilePathOnDisk()`, project-root
//! relative, T1153 LINT-26), falling back to [`lint_bytes`]'s `path`
//! argument for direct callers, so paths that do not exist on disk are
//! silent. [`check_bytes`] runs the same rules against an explicit flag
//! and is the portable, directly testable core.
//!
//! Known limits: MIME detection is magic-byte sniffing for exactly the PHP
//! list (OLE containers report `application/msword` as the representative
//! type; MP4-family brands are accepted like PHP's `default` branch); on
//! non-Unix platforms nothing is executable, so the linter stays silent.

extern crate self as arc_rs;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use arc_rs::lint::{LintMessage, Linter};

const CODE_INVALID_EXECUTABLE: &str = "CHMOD1";
const NAME_INVALID_EXECUTABLE: &str = "Invalid Executable";
const SEVERITY_WARNING: &str = "warning";
const DESC_NEEDS_BINARY_OR_SHEBANG: &str =
    "Executable files should either be binary or contain a shebang.";

/// Linter flagging needlessly executable files (PHP `CHMOD`).
#[derive(Debug, Default)]
pub struct ChmodLinter {
    disk: RefCell<Option<PathBuf>>,
}

impl ChmodLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// True when `data` is binary, mirroring
/// `ArcanistDiffUtils::isHeuristicBinaryFile` (any NUL byte).
#[must_use]
pub fn is_binary(data: &[u8]) -> bool {
    data.contains(&0)
}

/// True when `data` starts with `#!`, mirroring the PHP `getShebang()`
/// `/^#!(.*)$/` match on the first line.
#[must_use]
pub fn has_shebang(data: &[u8]) -> bool {
    data.starts_with(b"#!")
}

/// Detect a MIME type from magic bytes for binary blobs whose kind must not
/// be executable. Returns `None` for unknown binaries (valid executables,
/// like the PHP `default` branch) and for text. Only MIME types from the
/// PHP switch are ever returned.
#[must_use]
pub fn detect_non_executable_mime(data: &[u8]) -> Option<&'static str> {
    if let Some(mime) = detect_archive_mime(data) {
        return Some(mime);
    }
    if let Some(mime) = detect_audio_mime(data) {
        return Some(mime);
    }
    if is_ftyp(data) {
        // Decisive like the PHP switch: non-audio `ftyp` brands (mp4 video,
        // 3gp, ...) accept the file without consulting later categories.
        return ftyp_audio_or_quicktime(data);
    }
    detect_font_mime(data)
        .or_else(|| detect_image_mime(data))
        .or_else(|| detect_document_mime(data))
        .or_else(|| detect_video_mime(data))
}

/// Archive magic from the PHP mime switch.
fn detect_archive_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"PK\x03\x04")
        || data.starts_with(b"PK\x05\x06")
        || data.starts_with(b"PK\x07\x08")
    {
        return Some("application/zip");
    }
    if data.starts_with(b"\x1f\x8b") {
        return Some("application/x-gzip");
    }
    if data.starts_with(b"BZh") {
        return Some("application/x-bzip2");
    }
    if data.starts_with(b"Rar!\x1a\x07") {
        return Some("application/x-rar-compressed");
    }
    if data.len() > 262 && data[257..262] == *b"ustar" {
        return Some("application/x-tar");
    }
    None
}

/// Audio magic from the PHP mime switch (ISO base-media `ftyp` boxes are
/// classified separately by [`ftyp_audio_or_quicktime`]).
fn detect_audio_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"MThd") {
        return Some("audio/midi");
    }
    if data.starts_with(b"ID3") || is_mp3_frame(data) {
        return Some("audio/mpeg");
    }
    if is_riff(data, b"WAVE") {
        return Some("audio/x-wav");
    }
    None
}

/// Font magic from the PHP mime switch.
fn detect_font_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"wOFF") {
        return Some("application/x-woff");
    }
    if data.starts_with(b"\x00\x01\x00\x00")
        || data.starts_with(b"true")
        || data.starts_with(b"typ1")
    {
        return Some("application/x-font-ttf");
    }
    if is_eot(data) {
        return Some("application/vnd.ms-fontobject");
    }
    None
}

/// Image magic from the PHP mime switch.
fn detect_image_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"FWS") || data.starts_with(b"CWS") || data.starts_with(b"ZWS") {
        return Some("application/x-shockwave-flash");
    }
    if data.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if data.starts_with(b"II*\x00") || data.starts_with(b"MM\x00*") {
        return Some("image/tiff");
    }
    if data.starts_with(b"\x00\x00\x01\x00") {
        return Some("image/x-icon");
    }
    if data.starts_with(b"BM") {
        return Some("image/x-ms-bmp");
    }
    None
}

/// Document magic from the PHP mime switch.
fn detect_document_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        // OLE container (doc/xls/ppt); msword is the representative type.
        return Some("application/msword");
    }
    if data.starts_with(b"%PDF") {
        return Some("application/pdf");
    }
    if data.starts_with(b"%!PS") {
        return Some("application/postscript");
    }
    if data.starts_with(b"{\\rtf") {
        return Some("application/rtf");
    }
    None
}

/// Video magic from the PHP mime switch.
fn detect_video_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x00\x00\x01\xb3") {
        return Some("video/mpeg");
    }
    if data.starts_with(b"FLV") {
        return Some("video/x-flv");
    }
    if data.starts_with(b"\x30\x26\xb2\x75\x8e\x66\xcf\x11") {
        return Some("video/x-ms-wmv");
    }
    if is_riff(data, b"AVI ") {
        return Some("video/x-msvideo");
    }
    None
}

fn is_mp3_frame(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xff && data[1] & 0xe0 == 0xe0
}

/// EOT fonts carry "LP" at offset 34 and their total size as a
/// little-endian u32 at offset 0.
fn is_eot(data: &[u8]) -> bool {
    if data.len() <= 36 || data[34..36] != *b"LP" {
        return false;
    }
    let mut size = [0u8; 4];
    size.copy_from_slice(&data[0..4]);
    u32::from_le_bytes(size) as usize == data.len()
}

fn is_riff(data: &[u8], kind: &[u8; 4]) -> bool {
    data.len() >= 12 && data[0..4] == *b"RIFF" && data[8..12] == *kind
}

fn is_ftyp(data: &[u8]) -> bool {
    data.len() >= 12 && data[4..8] == *b"ftyp"
}

/// Classify an ISO base-media `ftyp` box. Audio brands map to `audio/mp4`,
/// QuickTime to `video/quicktime`; anything else (mp4 video, 3gp, …) is
/// `None`, mirroring the PHP `default` branch for MIME types outside the
/// switch.
fn ftyp_audio_or_quicktime(data: &[u8]) -> Option<&'static str> {
    let brand = &data[8..12];
    if brand == b"M4A " || brand == b"M4B " || brand == b"M4P " || brand == b"m4a " {
        Some("audio/mp4")
    } else if brand == b"qt  " {
        Some("video/quicktime")
    } else {
        None
    }
}

/// Run the chmod rules for `path`/`data` with an explicit executable flag.
/// Non-executable files are always silent.
#[must_use]
pub fn check_bytes(path: &str, data: &[u8], is_executable: bool) -> Vec<LintMessage> {
    if !is_executable {
        return Vec::new();
    }
    if is_binary(data) {
        if let Some(mime) = detect_non_executable_mime(data) {
            return vec![invalid_executable(
                path,
                &format!("'{mime}' files should not be executable."),
            )];
        }
        return Vec::new();
    }
    if has_shebang(data) {
        return Vec::new();
    }
    vec![invalid_executable(path, DESC_NEEDS_BINARY_OR_SHEBANG)]
}

fn invalid_executable(path: &str, description: &str) -> LintMessage {
    LintMessage {
        path: path.to_string(),
        // Path-level (`raiseLintAtPath()`): no line or char.
        line: 0,
        col: 0,
        code: CODE_INVALID_EXECUTABLE.to_string(),
        severity: SEVERITY_WARNING.to_string(),
        name: NAME_INVALID_EXECUTABLE.to_string(),
        description: description.to_string(),
        original: None,
        replacement: None,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    }
}

/// True when `path` exists on disk as a regular file with any executable
/// bit set. Directories, symlinks-to-dirs, missing paths, and (on non-Unix
/// platforms) everything report false.
fn is_executable_on_disk(path: &Path) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    if !meta.is_file() {
        return false;
    }
    is_executable_permissions(&meta)
}

#[cfg(unix)]
fn is_executable_permissions(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_permissions(_meta: &std::fs::Metadata) -> bool {
    false
}

impl Linter for ChmodLinter {
    fn name(&self) -> &'static str {
        "CHMOD"
    }

    fn severity_lookup_code<'a>(&self, full_code: &'a str) -> &'a str {
        full_code.strip_prefix("CHMOD").unwrap_or(full_code)
    }

    // PHP lints binaries with this linter
    // (`ArcanistChmodLinter.php:40-42`).
    fn should_lint_binary_files(&self) -> bool {
        true
    }

    fn set_active_disk_path(&self, disk: &Path) {
        *self.disk.borrow_mut() = Some(disk.to_path_buf());
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        let disk = self.disk.borrow().clone();
        let probe;
        let stat_path: &Path = match disk.as_deref() {
            Some(disk) => disk,
            None => {
                probe = PathBuf::from(path);
                &probe
            }
        };
        check_bytes(path, data, is_executable_on_disk(stat_path))
    }
}

#[cfg(test)]
#[path = "lint_chmod_tests.rs"]
mod lint_chmod_tests;
