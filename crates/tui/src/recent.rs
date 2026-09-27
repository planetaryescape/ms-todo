//! The palette's recent commands (D-066): the labels of the last
//! [`KEPT`] commands run from it, newest first, one per line in a small
//! file in the instance's data directory, so they survive a restart and
//! a dev build's never mix with the installed copy's.

use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many recent commands are kept and offered.
pub const KEPT: usize = 10;

/// The file's name in the instance's data directory.
pub const FILE_NAME: &str = "tui-recent-commands";

/// The labels saved at `path`, newest first; none when there's no file
/// or it can't be read, which only costs the ordering.
pub fn load(path: &Path) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(KEPT)
            .map(str::to_owned)
            .collect(),
        Err(error) => {
            if error.kind() != ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), %error, "can't read the recent commands");
            }
            Vec::new()
        }
    }
}

/// Write `labels` to `path`, beside it and renamed over it, so a crash
/// can't leave half a file. The staged file's name is this process's and
/// this save's, so two TUIs of one instance never write the same one; if
/// the rename fails while another's save is in place, that one stands.
pub fn save(path: &Path, labels: &[String]) -> std::io::Result<()> {
    static SAVES: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = labels.join("\n");
    text.push('\n');
    let save = SAVES.fetch_add(1, Ordering::Relaxed);
    let staged = path.with_extension(format!("{}.{save}.tmp", std::process::id()));
    std::fs::write(&staged, text)?;
    match std::fs::rename(&staged, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            if path.exists() { Ok(()) } else { Err(error) }
        }
    }
}

/// `labels` with `label` first, once, and at most [`KEPT`] of them.
pub fn remember(labels: &mut Vec<String>, label: &str) {
    labels.retain(|kept| kept != label);
    labels.insert(0, label.to_owned());
    labels.truncate(KEPT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_goes_first_once_and_only_ten_are_kept() {
        let mut labels = Vec::new();
        for n in 0..12 {
            remember(&mut labels, &format!("Go to {n}"));
        }
        remember(&mut labels, "Go to 5");
        assert_eq!(labels.len(), KEPT);
        assert_eq!(labels[0], "Go to 5");
        assert_eq!(labels[1], "Go to 11");
        assert_eq!(labels.iter().filter(|label| *label == "Go to 5").count(), 1);
    }

    #[test]
    fn saved_labels_load_back_and_no_file_is_none() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("instance").join(FILE_NAME);
        assert!(load(&path).is_empty());
        let labels = vec!["Sync".to_owned(), "Go to Home".to_owned()];
        save(&path, &labels).expect("saved");
        assert_eq!(load(&path), labels);
        // Saved again, with nothing staged left behind.
        save(&path, &labels[..1]).expect("saved again");
        assert_eq!(load(&path), labels[..1]);
        let left: Vec<_> = std::fs::read_dir(path.parent().expect("dir"))
            .expect("read")
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }
}
