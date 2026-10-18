//! Shared integration-test helpers: a process-serialized HOME/USERPROFILE
//! override. This is the only place in the workspace that mutates the
//! process environment (unsafe in edition 2024), keeping the geiger
//! baseline minimal.

use std::path::{Path, PathBuf};

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores saved HOME/USERPROFILE on drop; removes the dir when owned.
pub struct HomeGuard {
    owned: Option<PathBuf>,
    saved_home: Option<std::ffi::OsString>,
    saved_profile: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

fn install(home: &Path, owned: bool) -> HomeGuard {
    let lock = ENV_LOCK.lock().unwrap();
    let saved_home = std::env::var_os("HOME");
    let saved_profile = std::env::var_os("USERPROFILE");
    // SAFETY: the process-wide lock serializes every env mutation and
    // read in this test binary, so no other thread can observe a
    // half-updated environment.
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
    }
    HomeGuard {
        owned: owned.then(|| home.to_path_buf()),
        saved_home,
        saved_profile,
        _lock: lock,
    }
}

/// Point HOME/USERPROFILE at `dir` (borrowed, left in place).
#[allow(
    dead_code,
    reason = "shared helper; each test binary uses one constructor"
)]
pub fn with_temp_home(dir: &Path) -> HomeGuard {
    install(dir, false)
}

/// Point HOME/USERPROFILE at `home`, removing it on drop.
#[allow(
    dead_code,
    reason = "shared helper; each test binary uses one constructor"
)]
pub fn with_owned_home(home: PathBuf) -> HomeGuard {
    install(&home, true)
}

#[allow(dead_code, reason = "shared helper; only CWD-pinning binaries use it")]
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores the saved working directory on drop (panic-safe).
/// Serializes CWD mutation per test binary: each binary holds at most
/// one CWD-mutating test (the `run_uses_current_dir` contract pin),
/// and every sibling was audited CWD-immune (absolute dirs, explicit
/// `.current_dir` on spawns) as of T1176. Any future CWD-dependent
/// test must go through [`with_current_dir`]. Only land_workflow's
/// contract test holds both this and `ENV_LOCK`, always CWD-then-ENV
/// (guard declared first), and no other test holds both — so no lock
/// order can invert. The locks stay separate: sharing `ENV_LOCK` would
/// self-deadlock that test (`Mutex` is not reentrant).
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
        // SAFETY: still holding the process-wide env lock.
        unsafe {
            match &self.saved_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.saved_profile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
        if let Some(owned) = &self.owned {
            std::fs::remove_dir_all(owned).ok();
        }
    }
}
