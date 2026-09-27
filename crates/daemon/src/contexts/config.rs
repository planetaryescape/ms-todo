//! `[contexts.<name>]` in config.toml (rung 9d): which folders and lists
//! each context covers, and where a task goes in it when no list is named.
//!
//! ```toml
//! [contexts.work]
//! folders = ["Areas"]
//! lists = ["Contentful"]
//! default_list = "Contentful"
//! ```
//!
//! Read again for every request that uses a context, so an edit counts
//! at once. A bad file, or a bad context, is a warning in `ctx show` and
//! `doctor`, never a daemon that won't start.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

/// Names `ms-todo ctx` takes as words of its own, so no context can
/// have them.
const RESERVED: &[&str] = &["none", "list", "show"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Definition {
    #[serde(default)]
    pub folders: Vec<String>,
    #[serde(default)]
    pub lists: Vec<String>,
    #[serde(default)]
    pub default_list: Option<String>,
}

/// The contexts config.toml defines, by name, and what's wrong with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Config {
    pub definitions: BTreeMap<String, Definition>,
    /// Why config.toml's contexts couldn't be read at all.
    pub problem: Option<String>,
    /// Contexts left out, and why (a reserved name).
    pub rejected: Vec<String>,
}

#[derive(Deserialize, Default)]
struct File {
    #[serde(default)]
    contexts: BTreeMap<String, Definition>,
}

impl Config {
    pub fn load(config_file: &Path) -> Self {
        let raw = match std::fs::read_to_string(config_file) {
            Ok(raw) => raw,
            Err(error) if error.kind() == ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                return Self {
                    problem: Some(format!("{}: {error}", config_file.display())),
                    ..Self::default()
                };
            }
        };
        Self::parse(&raw).unwrap_or_else(|message| Self {
            problem: Some(format!("{}: {message}", config_file.display())),
            ..Self::default()
        })
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let file: File = toml::from_str(raw).map_err(|error| error.message().to_owned())?;
        let mut config = Self::default();
        for (name, definition) in file.contexts {
            if RESERVED.contains(&name.as_str()) {
                config.rejected.push(format!(
                    "context {name:?} can't be used: `ms-todo ctx {name}` means something \
                     else; rename [contexts.{name}]"
                ));
            } else {
                config.definitions.insert(name, definition);
            }
        }
        Ok(config)
    }

    /// Every context's name, in order.
    pub fn names(&self) -> Vec<String> {
        self.definitions.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_are_read_by_name_and_other_sections_ignored() {
        let config = Config::parse(
            r#"
[tui]
theme = "nord"

[contexts.work]
folders = ["Areas"]
lists = ["Contentful"]
default_list = "Contentful"

[contexts.home]
lists = ["Groceries", "Garden"]
"#,
        )
        .expect("parsed");
        assert_eq!(config.names(), ["home", "work"]);
        assert_eq!(
            config.definitions["work"],
            Definition {
                folders: vec!["Areas".into()],
                lists: vec!["Contentful".into()],
                default_list: Some("Contentful".into()),
            }
        );
        assert_eq!(config.definitions["home"].folders, Vec::<String>::new());
        assert_eq!(Config::parse("").expect("empty"), Config::default());
    }

    #[test]
    fn a_reserved_name_is_left_out_with_a_reason_and_a_typo_is_an_error() {
        let config = Config::parse("[contexts.none]\nlists = [\"A\"]\n").expect("parsed");
        assert!(config.definitions.is_empty());
        assert!(config.rejected[0].contains("rename [contexts.none]"));
        let typo = Config::parse("[contexts.work]\nlist = [\"A\"]\n").expect_err("typo");
        assert!(typo.contains("unknown field"), "{typo}");
    }

    #[test]
    fn a_missing_file_is_no_contexts_and_a_broken_one_says_where() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config.toml");
        assert_eq!(Config::load(&path), Config::default());
        std::fs::write(&path, "[contexts.work\n").expect("write");
        let broken = Config::load(&path);
        assert!(broken.definitions.is_empty());
        assert!(
            broken
                .problem
                .as_deref()
                .is_some_and(|problem| problem.contains("config.toml")),
            "{broken:?}"
        );
    }
}
