//! The TypeSafe API key: `TYPESAFE_API_KEY`, else what
//! `[suggest] api_key_command` prints. The command runs as a process of
//! its own, never through a shell. The key lives in memory only: it's
//! never written to disk, logged or put in an error.

use std::process::Stdio;
use std::time::Duration;

use super::config::KeyCommand;

pub(crate) const API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// A password manager may be slow to unlock; past this it's a failure.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// A secret, which `Debug` doesn't print.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ApiKey(String);

impl ApiKey {
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_tests(key: &str) -> Self {
        Self(key.to_owned())
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ApiKey(redacted)")
    }
}

/// The key from `env` (the variable's value), else from `command`.
pub(crate) async fn resolve(
    env: Option<String>,
    command: Option<&KeyCommand>,
) -> Result<ApiKey, String> {
    if let Some(key) = env.map(|key| key.trim().to_owned())
        && !key.is_empty()
    {
        return Ok(ApiKey(key));
    }
    let Some(command) = command else {
        return Err(format!(
            "no API key: set {API_KEY_ENV} or suggest.api_key_command in config.toml"
        ));
    };
    run(&command.argv()?).await
}

/// Run `argv` directly and take the first line it prints as the key.
async fn run(argv: &[String]) -> Result<ApiKey, String> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| "suggest.api_key_command is empty".to_owned())?;
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("suggest.api_key_command couldn't start {program}: {error}"))?;
    let output = tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            format!(
                "suggest.api_key_command didn't finish in {} seconds",
                COMMAND_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| format!("suggest.api_key_command failed: {error}"))?;
    if !output.status.success() {
        // The status only: what a password manager prints, even on its
        // error stream, may hold part of a secret.
        let printed = if output.stdout.is_empty() && output.stderr.is_empty() {
            "no output"
        } else {
            "output withheld"
        };
        return Err(format!(
            "suggest.api_key_command exited with {} ({printed})",
            output.status
        ));
    }
    let key = String::from_utf8(output.stdout)
        .ok()
        .and_then(|stdout| stdout.lines().next().map(|line| line.trim().to_owned()))
        .filter(|key| !key.is_empty())
        .ok_or_else(|| "suggest.api_key_command printed no key".to_owned())?;
    Ok(ApiKey(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(command: &str) -> KeyCommand {
        KeyCommand::Line(command.into())
    }

    #[tokio::test]
    async fn the_environment_wins_over_the_command() {
        let key = resolve(Some(" from-env \n".into()), Some(&line("false")))
            .await
            .expect("key");
        assert_eq!(key.expose(), "from-env");
        let blank = resolve(Some("  ".into()), None).await.expect_err("no key");
        assert!(blank.contains(API_KEY_ENV), "{blank}");
    }

    #[tokio::test]
    async fn the_command_runs_without_a_shell() {
        // A shell would expand `$HOME` and run `echo` twice; run directly,
        // printf gets the text as it is.
        let key = resolve(None, Some(&line(r"printf '%s\n' 'sk-$HOME; echo no'")))
            .await
            .expect("key");
        assert_eq!(key.expose(), "sk-$HOME; echo no");
    }

    #[tokio::test]
    async fn the_first_line_is_the_key() {
        let key = resolve(None, Some(&line(r"printf 'sk-1\nsomething else\n'")))
            .await
            .expect("key");
        assert_eq!(key.expose(), "sk-1");
    }

    #[tokio::test]
    async fn a_failing_command_gives_its_status_never_its_output() {
        let failed = resolve(
            None,
            Some(&KeyCommand::Argv(vec![
                "sh".into(),
                "-c".into(),
                "echo sk-secret; echo sk-fake-secret-on-stderr >&2; exit 3".into(),
            ])),
        )
        .await
        .expect_err("failed");
        assert!(
            failed.contains("exit status: 3") && failed.contains("output withheld"),
            "{failed}"
        );
        assert!(!failed.contains("sk-"), "{failed}");
        let silent = resolve(None, Some(&line("false")))
            .await
            .expect_err("failed");
        assert!(silent.contains("(no output)"), "{silent}");

        let missing = resolve(None, Some(&line("ms-todo-no-such-program")))
            .await
            .expect_err("missing");
        assert!(missing.contains("couldn't start"), "{missing}");

        let silent = resolve(None, Some(&line("true")))
            .await
            .expect_err("silent");
        assert!(silent.contains("printed no key"), "{silent}");
    }

    #[test]
    fn debug_never_shows_the_key() {
        assert_eq!(format!("{:?}", ApiKey("sk-1".into())), "ApiKey(redacted)");
    }
}
