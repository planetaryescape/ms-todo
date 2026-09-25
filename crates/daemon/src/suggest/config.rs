//! `[suggest]` in config.toml (D-035, D-053). Off unless `enabled = true`.
//! A bad setting turns suggestions off with the reason, for `doctor`,
//! rather than stopping the daemon: the feature is never required.
//!
//! ```toml
//! [suggest]
//! enabled = true
//! provider = "typesafe"
//! min_confidence = 0.8
//! exclude_folders = ["Archive"]
//! api_key_command = "op read 'op://Private/TypeSafe/credential'"
//! ```

use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

/// The confidence the calibration gated at (D-053): 7 of 8 right.
const DEFAULT_MIN_CONFIDENCE: f64 = 0.8;

/// What `[suggest]` says.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Setting {
    Off,
    On(Config),
    /// Why the settings can't be used.
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Config {
    pub min_confidence: f64,
    /// Lists in these folders (any case) are never suggested.
    pub exclude_folders: Vec<String>,
    /// How to get the API key when `TYPESAFE_API_KEY` isn't set.
    pub api_key_command: Option<KeyCommand>,
}

/// A command that prints the API key: one string split as a shell would
/// split it (quotes group words), or the argv itself. Never run through a
/// shell.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum KeyCommand {
    Line(String),
    Argv(Vec<String>),
}

impl KeyCommand {
    /// The program and its arguments.
    pub fn argv(&self) -> Result<Vec<String>, String> {
        let argv = match self {
            Self::Line(line) => shlex::split(line).ok_or_else(|| {
                "suggest.api_key_command has an unclosed quote or a trailing backslash".to_owned()
            })?,
            Self::Argv(argv) => argv.clone(),
        };
        if argv.first().is_none_or(|program| program.trim().is_empty()) {
            return Err("suggest.api_key_command is empty".into());
        }
        Ok(argv)
    }
}

#[derive(Deserialize, Default)]
struct File {
    suggest: Option<Section>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    provider: Provider,
    #[serde(default = "default_min_confidence")]
    min_confidence: f64,
    #[serde(default = "default_exclude_folders")]
    exclude_folders: Vec<String>,
    #[serde(default)]
    api_key_command: Option<KeyCommand>,
}

/// Only TypeSafe for now; the key keeps room for another.
#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum Provider {
    #[default]
    Typesafe,
}

fn default_min_confidence() -> f64 {
    DEFAULT_MIN_CONFIDENCE
}

fn default_exclude_folders() -> Vec<String> {
    vec!["Archive".into()]
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
        let Some(section) = file.suggest.filter(|section| section.enabled) else {
            return Ok(Self::Off);
        };
        if !(0.0..=1.0).contains(&section.min_confidence) {
            return Err("suggest.min_confidence must be from 0 to 1".into());
        }
        if let Some(command) = &section.api_key_command {
            command.argv()?;
        }
        let Provider::Typesafe = section.provider;
        Ok(Self::On(Config {
            min_confidence: section.min_confidence,
            exclude_folders: section.exclude_folders,
            api_key_command: section.api_key_command,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(raw: &str) -> Config {
        match Setting::parse(raw) {
            Ok(Setting::On(config)) => config,
            other => unreachable!("expected on, got {other:?}"),
        }
    }

    #[test]
    fn off_without_a_section_or_unless_enabled() {
        assert_eq!(Setting::parse("[tui]\ntheme = \"x\"\n"), Ok(Setting::Off));
        assert_eq!(
            Setting::parse("[suggest]\nmin_confidence = 0.5\n"),
            Ok(Setting::Off)
        );
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            Setting::load(&dir.path().join("missing.toml")),
            Setting::Off
        );
    }

    #[test]
    fn enabled_takes_the_calibrated_defaults() {
        let config = on("[suggest]\nenabled = true\n");
        assert!((config.min_confidence - 0.8).abs() < f64::EPSILON);
        assert_eq!(config.exclude_folders, ["Archive"]);
        assert_eq!(config.api_key_command, None);
    }

    #[test]
    fn every_setting_is_read() {
        let config = on(r#"
[suggest]
enabled = true
provider = "typesafe"
min_confidence = 0.9
exclude_folders = ["Archive", "Someday"]
api_key_command = "op read 'op://Environment Variables/TypeSafe API/KEY'"
"#);
        assert!((config.min_confidence - 0.9).abs() < f64::EPSILON);
        assert_eq!(config.exclude_folders, ["Archive", "Someday"]);
        assert_eq!(
            config.api_key_command.map(|command| command.argv()),
            Some(Ok(vec![
                "op".to_owned(),
                "read".into(),
                "op://Environment Variables/TypeSafe API/KEY".into()
            ]))
        );
    }

    #[test]
    fn the_key_command_splits_like_a_shell_without_one() {
        let argv = |line: &str| KeyCommand::Line(line.into()).argv();
        assert_eq!(
            argv(r#"pass show "api keys/typesafe""#),
            Ok(vec![
                "pass".into(),
                "show".into(),
                "api keys/typesafe".into()
            ])
        );
        // Shell syntax is only text: nothing here is run or expanded.
        assert_eq!(
            argv("echo $HOME; rm -rf / | cat"),
            Ok(vec![
                "echo".into(),
                "$HOME;".into(),
                "rm".into(),
                "-rf".into(),
                "/".into(),
                "|".into(),
                "cat".into()
            ])
        );
        assert!(argv("op read 'unclosed").is_err());
        assert!(argv("   ").is_err());
        let exact = KeyCommand::Argv(vec!["op".into(), "read".into(), "a b".into()]);
        assert_eq!(
            exact.argv(),
            Ok(vec!["op".into(), "read".into(), "a b".into()])
        );
        assert!(KeyCommand::Argv(Vec::new()).argv().is_err());
    }

    #[test]
    fn an_array_command_is_the_argv() {
        let config =
            on("[suggest]\nenabled = true\napi_key_command = [\"op\", \"read\", \"op://a b/c\"]\n");
        assert_eq!(
            config.api_key_command,
            Some(KeyCommand::Argv(vec![
                "op".into(),
                "read".into(),
                "op://a b/c".into()
            ]))
        );
    }

    #[test]
    fn a_bad_setting_is_named_not_ignored() {
        for (raw, says) in [
            (
                "[suggest]\nenabled = true\nmin_confidance = 0.9\n",
                "min_confidance",
            ),
            (
                "[suggest]\nenabled = true\nmin_confidence = 1.5\n",
                "from 0 to 1",
            ),
            (
                "[suggest]\nenabled = true\nprovider = \"openai\"\n",
                "openai",
            ),
            (
                "[suggest]\nenabled = true\napi_key_command = \"op 'x\"\n",
                "unclosed",
            ),
        ] {
            let error = Setting::parse(raw).expect_err(raw);
            assert!(error.contains(says), "{raw}: {error}");
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("config.toml");
        std::fs::write(&file, "[suggest\n").expect("write");
        assert!(
            matches!(Setting::load(&file), Setting::Invalid(why) if why.contains("config.toml"))
        );
    }
}
