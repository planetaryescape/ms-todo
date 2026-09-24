//! `mst` is an official alias of `ms-todo`: a symlink to the same binary
//! (D-039). Help and usage show the name the user typed, and the daemon
//! still starts when the CLI was run through the symlink.

mod support;

use support::Env;

fn mst(env: &Env) -> std::path::PathBuf {
    let link = env.home.path().join("bin").join("mst");
    std::fs::create_dir_all(link.parent().expect("bin dir")).expect("mkdir");
    std::os::unix::fs::symlink(assert_cmd::cargo::cargo_bin("ms-todo"), &link).expect("symlink");
    link
}

fn stdout(env: &Env, program: &std::path::Path, args: &[&str]) -> String {
    let output = env
        .cmd_at(program)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("utf8")
}

#[test]
fn version_through_the_alias_names_the_product() {
    let env = Env::new();
    let mst = mst(&env);
    assert_eq!(
        stdout(&env, &mst, &["--version"]).trim(),
        format!("ms-todo {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_through_the_alias_shows_the_typed_name() {
    let env = Env::new();
    let mst = mst(&env);
    let root = stdout(&env, &mst, &["--help"]);
    assert!(root.contains("Usage: mst [OPTIONS] <COMMAND>"), "{root}");
    let tasks = stdout(&env, &mst, &["tasks", "--help"]);
    assert!(
        tasks.contains("Usage: mst tasks [OPTIONS] <COMMAND>"),
        "{tasks}"
    );
}

#[test]
fn the_daemon_starts_and_stops_through_the_alias() {
    let env = Env::new();
    let mst = mst(&env);
    let started: serde_json::Value = serde_json::from_str(&stdout(
        &env,
        &mst,
        &["--format", "json", "daemon", "start"],
    ))
    .expect("json");
    assert_eq!(started["running"], true);
    assert_eq!(started["ready"], true);
    // The alias and the full name reach the same instance and daemon.
    assert_eq!(env.json(&["daemon", "status"])["pid"], started["pid"]);
    let stopped: serde_json::Value =
        serde_json::from_str(&stdout(&env, &mst, &["--format", "json", "daemon", "stop"]))
            .expect("json");
    assert_eq!(
        env.json(&["daemon", "status"])["running"],
        false,
        "{stopped}"
    );
}
