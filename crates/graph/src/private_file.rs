// Adapted from spotuify crates/spotuify-spotify/src/auth.rs (`atomic_write_mode_0600`, :1487)
// and crates/spotuify-protocol/src/paths.rs (`ensure_private_dir`, :281)
// @ d807e5e4f9d2f09878cdc22309af3589623f7785. The non-Unix fallbacks were left
// out on purpose: they are neither atomic nor 0600 (docs/blueprint/09-reuse-map.md).

use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Create `path` (and parents) and make it 0700, repairing a directory that
/// was left group- or world-readable.
pub fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "private directory path is empty",
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("{} is not a directory", path.display()),
                ));
            }
            let mode = metadata.permissions().mode();
            // Never chmod a shared sticky directory such as /tmp.
            if mode & 0o002 != 0 && mode & 0o1000 != 0 {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "refusing to chmod shared sticky directory {}",
                        path.display()
                    ),
                ));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Write `bytes` to `path` so a reader sees either the old file or the whole
/// new one, never a torn write, and the file is never readable by others,
/// not even for a moment: the temp file is created 0600, then renamed.
pub fn atomic_write_mode_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "auth file path has no parent",
        ));
    };
    ensure_private_dir(parent)?;
    let file_name = path
        .file_name()
        .map_or_else(|| "token".into(), |name| name.to_string_lossy());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Clock nanos can collide across threads writing simultaneously; the
    // per-process counter keeps concurrent writers on distinct temp paths.
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(
        ".{file_name}.{}.{nonce}.{seq}.tmp",
        std::process::id()
    ));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_a_world_readable_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("auth");
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        ensure_private_dir(&dir).expect("ensure");

        let mode = std::fs::metadata(&dir).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn replaces_the_file_whole_and_leaves_no_temp_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("auth").join("token.json");
        atomic_write_mode_0600(&path, b"first").expect("write");
        atomic_write_mode_0600(&path, b"second").expect("rewrite");

        assert_eq!(std::fs::read(&path).expect("read"), b"second");
        let entries = std::fs::read_dir(path.parent().expect("parent"))
            .expect("list")
            .count();
        assert_eq!(entries, 1, "temp files left behind");
    }
}
