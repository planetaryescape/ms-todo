// Adapted from mxr tests/workspace_boundaries.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
//
// Dependency direction for ms-todo's crates (docs/blueprint/01-architecture.md#crates).
// F1 checks what exists: `core` stays free of I/O, and `cli` reaches `graph`
// only through its public auth API. D-031's full rule (clients depend on
// neither `graph` nor `store`, and talk to the daemon protocol only) starts
// with the daemon in rung 1; then the `cli` rule becomes "no `graph` at all".

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn dependencies(manifest_path: &str) -> Vec<String> {
    let raw = std::fs::read_to_string(repo_root().join(manifest_path)).expect("read manifest");
    let manifest: toml::Table = toml::from_str(&raw).expect("parse manifest");
    manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn core_has_no_io_dependencies() {
    const FORBIDDEN: &[&str] = &[
        "tokio",
        "tokio-util",
        "reqwest",
        "hyper",
        "sqlx",
        "rusqlite",
        "fs2",
    ];
    for dependency in dependencies("crates/core/Cargo.toml") {
        assert!(
            !FORBIDDEN.contains(&dependency.as_str()),
            "crates/core must stay free of I/O, but depends on {dependency}"
        );
    }
}

// Today `auth` is graph's only public module, so visibility already enforces
// this; the check keeps it true when rung 1 adds public HTTP modules.
#[test]
fn cli_uses_only_the_public_auth_api_of_graph() {
    let mut offenders = Vec::new();
    for file in rust_files(&repo_root().join("crates/cli/src")) {
        let source = std::fs::read_to_string(&file).expect("read source");
        for (index, line) in source.lines().enumerate() {
            let mut rest = line;
            while let Some(at) = rest.find("ms_todo_graph::") {
                rest = &rest[at + "ms_todo_graph::".len()..];
                if !rest.starts_with("auth::") {
                    offenders.push(format!("{}:{}: {line}", file.display(), index + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "crates/cli may only use ms_todo_graph::auth:\n{}",
        offenders.join("\n")
    );
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}

#[test]
fn root_package_is_not_publishable() {
    let raw = std::fs::read_to_string(repo_root().join("Cargo.toml")).expect("read manifest");
    let manifest: toml::Table = toml::from_str(&raw).expect("parse manifest");
    let publish = manifest
        .get("package")
        .and_then(|package| package.get("publish"))
        .and_then(toml::Value::as_bool);
    assert_eq!(
        publish,
        Some(false),
        "ms-todo ships through GitHub Releases, not crates.io"
    );
}
