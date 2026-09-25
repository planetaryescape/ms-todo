//! A deleted attachment's bytes, kept so `undo` can attach it again
//! (D-056). Graph deletes an attachment for good, so before the DELETE is
//! sent the daemon downloads it here: 0600 in a 0700 directory under the
//! instance's data directory, named by a hash of the delete's operation
//! ID (which comes from a client). Kept for a week, and at most 250 MB in
//! all, oldest dropped first; an undo after that is refused, saying so.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::outbox::move_job::sha256_hex;
use ms_todo_graph::private_file::{atomic_write_mode_0600, ensure_private_dir};

/// How long a deleted attachment can be brought back.
pub(crate) const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The most kept in all: ten of the largest attachments Graph takes.
const MAX_KEPT_BYTES: u64 = 250 * 1024 * 1024;

/// Where the delete `op_id` keeps its attachment's bytes.
pub(crate) fn path(root: &Path, op_id: &str) -> PathBuf {
    root.join(&sha256_hex(op_id.as_bytes())[..32])
}

/// Keep `bytes` for the delete `op_id`, then drop what's past its time or
/// over the limit.
pub(crate) async fn keep(root: &Path, op_id: &str, bytes: Vec<u8>) -> io::Result<()> {
    let root = root.to_owned();
    let file = path(&root, op_id);
    tokio::task::spawn_blocking(move || {
        ensure_private_dir(&root)?;
        atomic_write_mode_0600(&file, &bytes)?;
        sweep_now(&root, SystemTime::now())
    })
    .await
    .map_err(io::Error::other)?
}

/// Drop kept files past [`KEPT_FOR`], then the oldest while there's more
/// than the limit. Run when the daemon starts, and after each keep.
pub(crate) async fn sweep(root: &Path) {
    let root = root.to_owned();
    let swept = tokio::task::spawn_blocking(move || sweep_now(&root, SystemTime::now())).await;
    if let Ok(Err(error)) = swept {
        eprintln!("ms-todo daemon: cannot tidy the deleted attachments kept for undo: {error}");
    }
}

fn sweep_now(root: &Path, now: SystemTime) -> io::Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified()?;
        let age = now.duration_since(modified).unwrap_or_default();
        if age > KEPT_FOR {
            std::fs::remove_file(entry.path())?;
        } else {
            kept.push((modified, metadata.len(), entry.path()));
        }
    }
    kept.sort();
    let mut total: u64 = kept.iter().map(|(_, bytes, _)| bytes).sum();
    for (_, bytes, file) in kept {
        if total <= MAX_KEPT_BYTES {
            break;
        }
        std::fs::remove_file(file)?;
        total -= bytes;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_files_go_and_the_newest_stay_under_the_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let now = SystemTime::now();
        let write = |name: &str, bytes: u64, age: Duration| {
            let file = root.join(name);
            let handle = std::fs::File::create(&file).expect("create");
            handle.set_len(bytes).expect("size");
            handle.set_modified(now - age).expect("age");
        };
        let big = MAX_KEPT_BYTES / 2;
        write("expired", 1, KEPT_FOR + Duration::from_secs(60));
        write("oldest", big, Duration::from_secs(300));
        write("middle", big, Duration::from_secs(200));
        write("newest", big, Duration::from_secs(100));
        sweep_now(root, now).expect("sweep");
        let mut left: Vec<String> = std::fs::read_dir(root)
            .expect("list")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["middle", "newest"]);
    }

    #[test]
    fn each_delete_keeps_its_own_file() {
        let root = Path::new("/kept");
        assert_ne!(path(root, "op-1"), path(root, "op-2"));
        assert_eq!(path(root, "op-1"), path(root, "op-1"));
        assert!(path(root, "../x").starts_with(root));
    }
}
