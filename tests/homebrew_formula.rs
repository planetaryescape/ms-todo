//! scripts/render_homebrew_formula.sh: the release workflow renders the
//! tap's formula from a release's .sha256 files (D-039: it links `mst`).

use std::path::Path;
use std::process::Command;

const PLATFORMS: [&str; 3] = ["macos-aarch64", "macos-x86_64", "linux-x86_64"];

fn render(checksums: &Path, version: &str, output: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/render_homebrew_formula.sh"))
        .arg(version)
        .arg(checksums)
        .arg(output)
        .output()
        .expect("run the render script")
}

/// A .sha256 file as `shasum -a 256` writes it, for each platform.
fn checksums(dir: &Path, version: &str) {
    for (digit, platform) in ["a", "b", "c"].iter().zip(PLATFORMS) {
        let archive = format!("ms-todo-v{version}-{platform}.tar.gz");
        std::fs::write(
            dir.join(format!("{archive}.sha256")),
            format!("{}  {archive}\n", digit.repeat(64)),
        )
        .expect("write checksum");
    }
}

#[test]
fn the_formula_has_each_platforms_checksum_and_links_mst() {
    let dir = tempfile::tempdir().expect("tempdir");
    checksums(dir.path(), "1.2.3");
    let output = dir.path().join("Formula/ms-todo.rb");
    let rendered = render(dir.path(), "v1.2.3", &output);
    assert!(rendered.status.success(), "{rendered:?}");
    let formula = std::fs::read_to_string(&output).expect("formula");
    assert!(!formula.contains("__"), "a placeholder is left: {formula}");
    assert!(formula.contains(r#"bin.install_symlink "ms-todo" => "mst""#));
    insta::assert_snapshot!(formula);
}

#[test]
fn a_missing_or_malformed_checksum_fails_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    checksums(dir.path(), "1.2.3");
    let output = dir.path().join("ms-todo.rb");
    std::fs::remove_file(dir.path().join("ms-todo-v1.2.3-linux-x86_64.tar.gz.sha256"))
        .expect("remove");
    let rendered = render(dir.path(), "1.2.3", &output);
    assert!(!rendered.status.success());
    assert!(String::from_utf8_lossy(&rendered.stderr).contains("missing checksum file"));
    assert!(!output.exists());

    checksums(dir.path(), "1.2.3");
    std::fs::write(
        dir.path().join("ms-todo-v1.2.3-macos-x86_64.tar.gz.sha256"),
        "Not Found\n",
    )
    .expect("write");
    let rendered = render(dir.path(), "1.2.3", &output);
    assert!(!rendered.status.success());
    assert!(String::from_utf8_lossy(&rendered.stderr).contains("not a sha256"));
    assert!(!output.exists());
}
