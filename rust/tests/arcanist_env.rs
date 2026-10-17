//! T1153 CORE-19: hook children inherit the classic `ARCANIST` export.
//!
//! PHP exports the command process-wide via `putenv`
//! (`scripts/arcanist.php:229` at ceb); the port re-attaches the
//! [`arc_rs::globals`] cell as an explicit environment entry on every
//! [`arc_rs::proc_env::command`] child. This binary holds a single test
//! because the command cell is process-once.

/// A spawned child observes `ARCANIST` carrying the command cell.
#[test]
fn spawned_child_observes_arcanist() {
    arc_rs::globals::set_command_name("arcanist-env-probe".to_string());
    let expected = arc_rs::globals::command_name().expect("command cell is set");
    let output = probe_output();
    assert!(
        output.status.success(),
        "probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim_end(), expected);
}

/// Run a portable probe printing the child's `ARCANIST` value.
#[cfg(unix)]
fn probe_output() -> std::process::Output {
    arc_rs::proc_env::command("sh")
        .args(["-c", "printf %s \"$ARCANIST\""])
        .output()
        .expect("spawn sh probe")
}

/// Run a portable probe printing the child's `ARCANIST` value.
#[cfg(windows)]
fn probe_output() -> std::process::Output {
    arc_rs::proc_env::command("cmd")
        .args(["/C", "echo %ARCANIST%"])
        .output()
        .expect("spawn cmd probe")
}
