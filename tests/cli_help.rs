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

#[test]
fn cli_help_snapshots_cover_all_commands() {
    let cases: &[(&str, &[&str])] = &[
        ("cli_help_root", &["--help"]),
        ("cli_help_auth", &["auth", "--help"]),
        ("cli_help_auth_login", &["auth", "login", "--help"]),
        ("cli_help_auth_status", &["auth", "status", "--help"]),
        ("cli_help_auth_logout", &["auth", "logout", "--help"]),
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
