//! Drives the real binary with HOME and the XDG directories pointed at a
//! temp dir, so it never sees the developer's own sign-in.

use assert_cmd::Command;
use serde_json::Value;

fn ms_todo(home: &tempfile::TempDir) -> Command {
    let mut command = Command::cargo_bin("ms-todo").expect("ms-todo binary");
    command
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env_remove("MS_TODO_INSTANCE");
    command
}

#[test]
fn status_when_signed_out_exits_4_and_says_how_to_sign_in() {
    let home = tempfile::tempdir().expect("tempdir");

    let assert = ms_todo(&home)
        .args(["--format", "table", "auth", "status"])
        .assert()
        .code(4);

    let output = assert.get_output();
    assert!(output.stdout.is_empty(), "errors never go to stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("run `ms-todo auth login`"), "{stderr}");
}

#[test]
fn status_when_signed_out_in_json_writes_the_error_envelope_to_stderr() {
    let home = tempfile::tempdir().expect("tempdir");

    let assert = ms_todo(&home)
        .args(["--format", "json", "auth", "status"])
        .assert()
        .code(4);

    let output = assert.get_output();
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).expect("json error");
    assert_eq!(error["error"]["kind"], "auth_required");
    let message = error["error"]["message"].as_str().expect("message");
    assert!(message.contains("ms-todo auth login"), "{message}");
}

#[test]
fn piped_output_defaults_to_json() {
    let home = tempfile::tempdir().expect("tempdir");

    // assert_cmd captures stdout, so it is not a terminal.
    let assert = ms_todo(&home).args(["auth", "status"]).assert().code(4);

    let error: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json error");
    assert_eq!(error["error"]["kind"], "auth_required");
}

#[test]
fn logout_when_signed_out_succeeds_and_reports_nothing_removed() {
    let home = tempfile::tempdir().expect("tempdir");

    let assert = ms_todo(&home)
        .args(["--format", "json", "auth", "logout"])
        .assert()
        .success();

    let body: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(body["signed_in"], false);
    assert_eq!(body["removed"], false);
    // A target/ build uses the dev instance.
    let token_path = body["token_path"].as_str().expect("path");
    assert!(token_path.contains("ms-todo-dev"), "{token_path}");
}

#[test]
fn instance_flag_selects_a_separate_data_directory() {
    let home = tempfile::tempdir().expect("tempdir");

    let assert = ms_todo(&home)
        .args([
            "--format",
            "json",
            "--instance",
            "scratch",
            "auth",
            "logout",
        ])
        .assert()
        .success();

    let body: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
    let token_path = body["token_path"].as_str().expect("path");
    assert!(
        token_path.contains("ms-todo-scratch/auth/token.json"),
        "{token_path}"
    );
}

#[test]
fn invalid_instance_name_is_invalid_input() {
    let home = tempfile::tempdir().expect("tempdir");

    ms_todo(&home)
        .args([
            "--format",
            "json",
            "--instance",
            "../escape",
            "auth",
            "status",
        ])
        .assert()
        .code(2);
}
