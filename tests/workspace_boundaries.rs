// Adapted from mxr tests/workspace_boundaries.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
//
// Dependency direction for ms-todo's crates (docs/blueprint/01-architecture.md#crates).
// `core` stays free of I/O. Clients talk to the daemon protocol only (D-031):
// only `daemon` uses the Graph client for data, and only `daemon` touches
// the store. `cli` may still use
// `ms_todo_graph::auth`, because `auth login|status|logout` work without a
// daemon (D-033 item 4), and it never depends on `daemon`, which would bring
// the Graph client in with it.

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
        "futures-util",
        "nix",
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

#[test]
fn clients_and_the_protocol_never_depend_on_the_daemon_or_graph_data() {
    let cli = dependencies("crates/cli/Cargo.toml");
    assert!(
        !cli.iter().any(|dependency| dependency == "ms-todo-daemon"),
        "crates/cli must reach the daemon over IPC, not link it"
    );
    for dependency in dependencies("crates/protocol/Cargo.toml") {
        assert!(
            !dependency.starts_with("ms-todo-"),
            "crates/protocol must stand alone, but depends on {dependency}"
        );
    }
}

// Only `daemon` may use the Graph client; everyone else gets `auth` at most.
#[test]
fn only_the_daemon_uses_graph_beyond_auth() {
    let mut offenders = Vec::new();
    let sources = [
        "crates/cli/src",
        "crates/core/src",
        "crates/protocol/src",
        "src",
    ];
    for file in sources
        .iter()
        .flat_map(|dir| rust_files(&repo_root().join(dir)))
    {
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
        "only crates/daemon may use ms_todo_graph beyond auth:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn only_the_daemon_touches_the_store() {
    for manifest in [
        "Cargo.toml",
        "crates/cli/Cargo.toml",
        "crates/core/Cargo.toml",
        "crates/protocol/Cargo.toml",
        "crates/graph/Cargo.toml",
    ] {
        assert!(
            !dependencies(manifest)
                .iter()
                .any(|name| name == "ms-todo-store"),
            "{manifest} must not depend on ms-todo-store; clients read the cache over IPC"
        );
    }
    assert!(
        dependencies("crates/daemon/Cargo.toml")
            .iter()
            .any(|name| name == "ms-todo-store")
    );
    // The store sits under the daemon: it knows neither Graph nor the wire.
    for dependency in dependencies("crates/store/Cargo.toml") {
        assert!(
            !dependency.starts_with("ms-todo-") || dependency == "ms-todo-core",
            "crates/store may depend on ms-todo-core only, not {dependency}"
        );
    }
    let offenders: Vec<String> = [
        "crates/cli/src",
        "crates/protocol/src",
        "crates/graph/src",
        "src",
    ]
    .iter()
    .flat_map(|dir| rust_files(&repo_root().join(dir)))
    .filter(|file| {
        std::fs::read_to_string(file)
            .expect("read source")
            .contains("ms_todo_store")
    })
    .map(|file| file.display().to_string())
    .collect();
    assert!(
        offenders.is_empty(),
        "only crates/daemon may use ms_todo_store: {offenders:?}"
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
