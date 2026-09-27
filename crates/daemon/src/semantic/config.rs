//! `[search]` in config.toml (D-035, D-062). Semantic search is off unless
//! `semantic = true`: turning it on downloads the model, so it's opt-in. A
//! bad setting leaves it off with the reason, for `doctor`, rather than
//! stopping the daemon: the feature is never required.
//!
//! ```toml
//! [search]
//! semantic = true
//! ```

use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

/// What `[search]` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Setting {
    Off,
    On,
    /// Why the settings can't be used.
    Invalid(String),
}

#[derive(Deserialize, Default)]
struct File {
    search: Option<Section>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    #[serde(default)]
    semantic: bool,
}

impl Setting {
    pub fn load(config_file: &Path) -> Self {
        let raw = match std::fs::read_to_string(config_file) {
            Ok(raw) => raw,
            Err(error) if error.kind() == ErrorKind::NotFound => return Self::Off,
            Err(error) => return Self::Invalid(format!("{}: {error}", config_file.display())),
        };
        Self::parse(&raw).unwrap_or_else(|message| {
            Self::Invalid(format!("{}: {message}", config_file.display()))
        })
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let file: File = toml::from_str(raw).map_err(|error| error.message().to_owned())?;
        Ok(match file.search {
            Some(Section { semantic: true }) => Self::On,
            _ => Self::Off,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_unless_semantic_is_true() {
        assert_eq!(Setting::parse("[tui]\ntheme = \"x\"\n"), Ok(Setting::Off));
        assert_eq!(Setting::parse("[search]\n"), Ok(Setting::Off));
        assert_eq!(
            Setting::parse("[search]\nsemantic = false\n"),
            Ok(Setting::Off)
        );
        assert_eq!(
            Setting::parse("[search]\nsemantic = true\n"),
            Ok(Setting::On)
        );
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            Setting::load(&dir.path().join("missing.toml")),
            Setting::Off
        );
    }

    #[test]
    fn a_bad_setting_says_why() {
        assert!(Setting::parse("[search]\nsemantic = \"yes\"\n").is_err());
        assert!(Setting::parse("[search]\nsemantics = true\n").is_err());
    }
}
