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
