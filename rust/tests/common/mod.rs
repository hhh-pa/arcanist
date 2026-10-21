//! Shared integration-test helpers: fake homes through the
//! `globals::set_test_home` cell (no process-environment mutation,
//! so no `unsafe`), plus a working-directory guard.

use std::path::{Path, PathBuf};

/// Owned-home guard: resets the test-home override and removes the
/// dir on drop.
pub struct HomeGuard {
    _cell: arc_rs::globals::TestHomeGuard,
    owned: PathBuf,
}

/// Point home-derived paths at `dir` (borrowed, left in place).
#[allow(
    dead_code,
    reason = "shared helper; each test binary uses one constructor"
)]
pub fn with_temp_home(dir: &Path) -> arc_rs::globals::TestHomeGuard {
    arc_rs::globals::set_test_home(dir)
}

/// Point home-derived paths at `home`, removing it on drop.
#[allow(
    dead_code,
    reason = "shared helper; each test binary uses one constructor"
)]
pub fn with_owned_home(home: PathBuf) -> HomeGuard {
    let cell = arc_rs::globals::set_test_home(&home);
    HomeGuard {
        _cell: cell,
        owned: home,
    }
}

#[allow(dead_code, reason = "shared helper; only CWD-pinning binaries use it")]
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores the saved working directory on drop (panic-safe).
/// Serializes CWD mutation per test binary: each binary holds at most
/// one CWD-mutating test (the `run_uses_current_dir` contract pin),
/// and every sibling was audited CWD-immune (absolute dirs, explicit
/// `.current_dir` on spawns) as of T1176. Any future CWD-dependent
/// test must go through [`with_current_dir`]. Only land_workflow's
/// contract test holds both this and the test-home guard, always
/// CWD-then-home (guard declared first), and no other test holds both
/// — so no lock order can invert. The locks stay separate: sharing
/// the home cell's serial lock would self-deadlock that test (`Mutex`
/// is not reentrant).
#[allow(dead_code, reason = "shared helper; only CWD-pinning binaries use it")]
pub struct CwdGuard {
    saved: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

/// Run with `dir` as the process working directory, restoring the
/// previous one on drop (even on panic).
#[allow(dead_code, reason = "shared helper; only CWD-pinning binaries use it")]
pub fn with_current_dir(dir: &Path) -> CwdGuard {
    let lock = CWD_LOCK.lock().unwrap();
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    CwdGuard { saved, _lock: lock }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.saved);
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.owned).ok();
    }
}
