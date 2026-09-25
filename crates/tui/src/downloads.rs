//! Where the TUI saves attachments: `[attachments] download_dir` in
//! config.toml (D-035), `~/Downloads` when it isn't set. A `~` at its
//! start is the home directory; anything else must be an absolute path.
//!
//! ```toml
//! [attachments]
//! download_dir = "~/Documents/To Do"
//! ```

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use toml_edit::DocumentMut;

use crate::app::attachments::{Places, expand_home};

#[derive(Debug, thiserror::Error)]
#[error("{}: {message}", path.display())]
pub struct DownloadsError {
    path: PathBuf,
    message: String,
}

/// Where attachments go, and how the TUI reads a typed path: from the
/// config at `config_file`, the directory the TUI started in, and HOME.
pub fn places(config_file: &Path) -> Result<Places, DownloadsError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let configured = read(config_file)?;
    let dir =
        download_dir(configured.as_deref(), home.as_deref()).map_err(|message| DownloadsError {
            path: config_file.to_owned(),
            message,
        })?;
    Ok(Places {
        // The daemon names what it wrote by the real path, which is what
        // the TUI checks before opening it.
        download_dir: dir.canonicalize().unwrap_or(dir),
        cwd: std::env::current_dir().unwrap_or_default(),
        home,
    })
}

/// `[attachments] download_dir`, if set.
fn read(path: &Path) -> Result<Option<String>, DownloadsError> {
    let invalid = |message: String| DownloadsError {
        path: path.to_owned(),
        message,
    };
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(invalid(error.to_string())),
    };
    let document: DocumentMut = raw.parse().map_err(|error| invalid(format!("{error}")))?;
    let Some(attachments) = document.get("attachments") else {
        return Ok(None);
    };
    let table = attachments
        .as_table_like()
        .ok_or_else(|| invalid("attachments must be a table: [attachments]".into()))?;
    let mut dir = None;
    for (key, item) in table.iter() {
        match key {
            "download_dir" => {
                let value = item.as_str().ok_or_else(|| {
                    invalid(
                        "attachments.download_dir must be a string, such as \"~/Downloads\"".into(),
                    )
                })?;
                dir = Some(value.to_owned());
            }
            other => {
                return Err(invalid(format!(
                    "attachments.{other} is not a setting; [attachments] takes download_dir"
                )));
            }
        }
    }
    Ok(dir)
}

/// The directory `configured` names, or `~/Downloads`.
fn download_dir(configured: Option<&str>, home: Option<&Path>) -> Result<PathBuf, String> {
    let configured = configured.unwrap_or("~/Downloads");
    let path = expand_home(configured, home)
        .ok_or("attachments.download_dir starts with ~, but HOME isn't set")?;
    if !path.is_absolute() {
        return Err(format!(
            "attachments.download_dir = \"{configured}\" must be an absolute path or start with ~/"
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_downloads_and_a_tilde_is_home() {
        let home = Path::new("/home/bk");
        assert_eq!(
            download_dir(None, Some(home)),
            Ok(PathBuf::from("/home/bk/Downloads"))
        );
        assert_eq!(
            download_dir(Some("~/To Do"), Some(home)),
            Ok(PathBuf::from("/home/bk/To Do"))
        );
        assert_eq!(
            download_dir(Some("/srv/files"), Some(home)),
            Ok(PathBuf::from("/srv/files"))
        );
        assert!(download_dir(Some("files"), Some(home)).is_err());
        assert!(download_dir(None, None).is_err());
    }

    #[test]
    fn the_setting_is_read_and_a_typo_is_named() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            "[tui]\ntheme = \"night-owl\"\n\n[attachments]\ndownload_dir = \"/srv/in\"\n",
        )
        .expect("write");
        assert_eq!(read(&config).expect("read").as_deref(), Some("/srv/in"));
        std::fs::write(&config, "[attachments]\ndownload_directory = \"/x\"\n").expect("write");
        let error = read(&config).expect_err("typo").to_string();
        assert!(error.contains("download_directory"), "{error}");
        assert_eq!(read(&dir.path().join("none.toml")).expect("none"), None);
    }
}
