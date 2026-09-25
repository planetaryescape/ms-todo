// Adapted from mxr crates/daemon/tests/cli_help.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
//
// `--help` is part of the CLI contract; a change to it should be a
// deliberate snapshot update (docs/blueprint/07-cli.md#output-contract).

use assert_cmd::Command;

fn help_output(args: &[&str]) -> String {
    let output = Command::cargo_bin("ms-todo")
        .expect("ms-todo binary")
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).expect("utf8 help output");
    // Trailing whitespace varies with terminal width handling; ignore it.
    let mut normalized = stdout
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    normalized.push('\n');
    normalized
}

/// No command off a terminal (assert_cmd gives none): help on stderr and
/// exit 2, never the TUI, so a script or an agent can't block on it. The
/// global flags are still read.
#[test]
fn a_bare_command_off_a_terminal_prints_help_and_exits_2() {
    for args in [&[][..], &["--instance", "bare-test"][..]] {
        let output = Command::cargo_bin("ms-todo")
            .expect("ms-todo binary")
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .clone();
        assert!(output.stdout.is_empty(), "{args:?}");
        let help = String::from_utf8(output.stderr).expect("utf8");
        assert!(
            help.contains("Usage: ms-todo [OPTIONS] [COMMAND]"),
            "{args:?}: {help}"
        );
        assert!(help.contains("tui "), "{help}");
    }
}

#[test]
fn cli_help_snapshots_cover_all_commands() {
    let cases: &[(&str, &[&str])] = &[
        ("cli_help_root", &["--help"]),
        ("cli_help_auth", &["auth", "--help"]),
        ("cli_help_auth_login", &["auth", "login", "--help"]),
        ("cli_help_auth_status", &["auth", "status", "--help"]),
        ("cli_help_auth_logout", &["auth", "logout", "--help"]),
        ("cli_help_auth_bearer", &["auth", "bearer", "--help"]),
        ("cli_help_lists", &["lists", "--help"]),
        ("cli_help_lists_list", &["lists", "list", "--help"]),
        ("cli_help_lists_move", &["lists", "move", "--help"]),
        ("cli_help_lists_order", &["lists", "order", "--help"]),
        ("cli_help_folders", &["folders", "--help"]),
        ("cli_help_folders_list", &["folders", "list", "--help"]),
        ("cli_help_folders_rename", &["folders", "rename", "--help"]),
        ("cli_help_folders_delete", &["folders", "delete", "--help"]),
        ("cli_help_folders_order", &["folders", "order", "--help"]),
        ("cli_help_tasks", &["tasks", "--help"]),
        ("cli_help_tasks_list", &["tasks", "list", "--help"]),
        ("cli_help_tasks_add", &["tasks", "add", "--help"]),
        ("cli_help_tasks_complete", &["tasks", "complete", "--help"]),
        ("cli_help_tasks_reopen", &["tasks", "reopen", "--help"]),
        ("cli_help_tasks_edit", &["tasks", "edit", "--help"]),
        ("cli_help_tasks_delete", &["tasks", "delete", "--help"]),
        ("cli_help_tasks_links", &["tasks", "links", "--help"]),
        ("cli_help_tasks_open", &["tasks", "open", "--help"]),
        ("cli_help_search", &["search", "--help"]),
        ("cli_help_done", &["done", "--help"]),
        ("cli_help_reschedule", &["reschedule", "--help"]),
        ("cli_help_outbox", &["outbox", "--help"]),
        ("cli_help_outbox_list", &["outbox", "list", "--help"]),
        ("cli_help_outbox_retry", &["outbox", "retry", "--help"]),
        ("cli_help_outbox_discard", &["outbox", "discard", "--help"]),
        ("cli_help_undo", &["undo", "--help"]),
        ("cli_help_sync", &["sync", "--help"]),
        ("cli_help_doctor", &["doctor", "--help"]),
        ("cli_help_schema", &["schema", "--help"]),
        ("cli_help_raw", &["raw", "--help"]),
        ("cli_help_tui", &["tui", "--help"]),
        ("cli_help_daemon", &["daemon", "--help"]),
        ("cli_help_daemon_start", &["daemon", "start", "--help"]),
        ("cli_help_daemon_stop", &["daemon", "stop", "--help"]),
        ("cli_help_daemon_status", &["daemon", "status", "--help"]),
    ];
    for (name, args) in cases {
        insta::assert_snapshot!(*name, help_output(args));
    }
}

#[test]
fn version_matches_the_workspace_version() {
    let output = Command::cargo_bin("ms-todo")
        .expect("ms-todo binary")
        .arg("--version")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(output).expect("utf8"),
        format!("ms-todo {}\n", env!("CARGO_PKG_VERSION"))
    );
}
