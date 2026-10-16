//! Process-global option overrides, set once by `main` from the global
//! flags and read deep in commands that cannot take parameters.
//!
//! Each channel is a `main`-set cell with the historical process
//! environment as fallback, so spawned binaries, ambient shells, and
//! unit tests (which never call the setters) all keep working. The
//! cells exist because `std::env::set_var` is `unsafe` in edition 2024
//! while shipped code keeps zero `unsafe` (geiger gate); reads stay on
//! the safe `std::env::var` path.
//!
//! Setters are `main`-only (binary entry + dispatch). They ignore
//! repeat sets: `main` runs once per process.

use std::sync::OnceLock;

static TRACE: OnceLock<bool> = OnceLock::new();
static ANSI: OnceLock<bool> = OnceLock::new();
static CONDUIT_URI: OnceLock<String> = OnceLock::new();
static CONDUIT_TOKEN: OnceLock<String> = OnceLock::new();
static ANONYMOUS: OnceLock<bool> = OnceLock::new();
static LOAD_OPTIONS: OnceLock<String> = OnceLock::new();
static COMMAND_NAME: OnceLock<String> = OnceLock::new();

/// Record `--trace` for [`trace_enabled`]. `main`-only.
pub fn set_trace(value: bool) {
    let _ = TRACE.set(value);
}

/// Record `--ansi`/`--no-ansi` for [`ansi_override`]. `main`-only.
pub fn set_ansi(value: bool) {
    let _ = ANSI.set(value);
}

/// Record the global `--conduit-uri` for [`conduit_uri`]. `main`-only.
pub fn set_conduit_uri(value: String) {
    let _ = CONDUIT_URI.set(value);
}

/// Record the global `--conduit-token` for [`conduit_token`]. `main`-only.
pub fn set_conduit_token(value: String) {
    let _ = CONDUIT_TOKEN.set(value);
}

/// Record `--anonymous` for [`anonymous`]. `main`-only.
pub fn set_anonymous(value: bool) {
    let _ = ANONYMOUS.set(value);
}

/// Record the global `--config`/`--config-file`/`--arcrc-file` JSON
/// document for [`load_options`]. `main`-only.
pub fn set_load_options(value: String) {
    let _ = LOAD_OPTIONS.set(value);
}

/// Record the resolved classic command name (the `ARCANIST` export,
/// `scripts/arcanist.php:229`). `main`-only.
///
/// Known gap: hook children do not inherit it yet — propagating it
/// would need an explicit `.env()` at every spawn site, since the
/// process environment cannot be written without `unsafe`.
pub fn set_command_name(value: String) {
    let _ = COMMAND_NAME.set(value);
}

/// Effective `--trace` switch: the `main` cell, else `ARC_TRACE`.
#[must_use]
pub fn trace_enabled() -> bool {
    if let Some(value) = TRACE.get() {
        return *value;
    }
    matches!(std::env::var("ARC_TRACE").as_deref(), Ok("1"))
}

/// Effective `--ansi`/`--no-ansi` override: the `main` cell, else
/// `ARC_ANSI` (`"1"`/`"0"`), else `None` for TTY probing.
#[must_use]
pub fn ansi_override() -> Option<bool> {
    if let Some(value) = ANSI.get() {
        return Some(*value);
    }
    match std::env::var("ARC_ANSI").as_deref() {
        Ok("0") => Some(false),
        Ok("1") => Some(true),
        _ => None,
    }
}

/// Effective global `--conduit-uri`: the `main` cell, else
/// `ARC_GLOBAL_CONDUIT_URI`.
#[must_use]
pub fn conduit_uri() -> Option<String> {
    CONDUIT_URI
        .get()
        .cloned()
        .or_else(|| std::env::var("ARC_GLOBAL_CONDUIT_URI").ok())
}

/// Effective global `--conduit-token`: the `main` cell, else
/// `ARC_GLOBAL_CONDUIT_TOKEN`.
#[must_use]
pub fn conduit_token() -> Option<String> {
    CONDUIT_TOKEN
        .get()
        .cloned()
        .or_else(|| std::env::var("ARC_GLOBAL_CONDUIT_TOKEN").ok())
}

/// Effective `--anonymous` switch: the `main` cell, else `ARC_ANONYMOUS`.
#[must_use]
pub fn anonymous() -> bool {
    if let Some(value) = ANONYMOUS.get() {
        return *value;
    }
    matches!(std::env::var("ARC_ANONYMOUS").as_deref(), Ok("1"))
}

/// Effective global `--config` document: the `main` cell, else
/// `ARC_GLOBAL_LOAD_OPTIONS`.
#[must_use]
pub fn load_options() -> Option<String> {
    LOAD_OPTIONS
        .get()
        .cloned()
        .or_else(|| std::env::var("ARC_GLOBAL_LOAD_OPTIONS").ok())
}

/// The resolved classic command name, for future hook propagation (see
/// [`set_command_name`]).
#[allow(dead_code)]
#[must_use]
pub fn command_name() -> Option<String> {
    COMMAND_NAME.get().cloned()
}
