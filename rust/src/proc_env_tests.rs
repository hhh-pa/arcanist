use super::*;

#[test]
fn command_passes_program_through() {
    // No cell interaction: the program word always survives intact.
    let cmd = command("sh");
    assert_eq!(cmd.get_program(), "sh");
}

#[test]
fn command_carries_arcanist_cell_value() {
    // T1153 CORE-19: the child environment carries exactly the command
    // cell. The cell is process-once, so tolerate a value set by another
    // test and assert against whatever is stored.
    crate::globals::set_command_name("proc-env-unit-probe".to_string());
    let expected = crate::globals::command_name().expect("command cell is set");
    let cmd = command("sh");
    let seen = cmd
        .get_envs()
        .find(|(key, _)| *key == "ARCANIST")
        .and_then(|(_, value)| value.map(|v| v.to_os_string()));
    assert_eq!(seen, Some(expected.into()));
}
