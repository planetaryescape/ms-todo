//! `ms-todo daemon install|uninstall` (D-065): start the daemon when the
//! user logs in, so nags fire without a client starting it first. On macOS
//! a launchd agent, `~/Library/LaunchAgents/com.planetaryescape.ms-todo.plist`;
//! on Linux a systemd user unit, `~/.config/systemd/user/ms-todo.service`,
//! enabled by its link in `default.target.wants`, as `systemctl --user
//! enable` makes it. Either runs this binary's `daemon run` for the
//! default instance, and starts it again if it crashes but not after a
//! clean stop (`daemon stop`, or a second daemon that finds the first).
//!
//! Only files are written or removed: no `launchctl` or `systemctl`, so
//! nothing changes in the running session, and a test with its own HOME
//! touches nothing else. The answer says how to start it now.

use std::collections::BTreeMap;
use std::io::ErrorKind as IoErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use ms_todo_core::{ErrorKind, Instance, Paths};
use serde::Serialize;

use crate::error::CliError;
use crate::output::Render;

pub const LAUNCHD_LABEL: &str = "com.planetaryescape.ms-todo";
const SYSTEMD_UNIT: &str = "ms-todo.service";

/// Where ms-todo's files are, as the installing shell has them: carried
/// into the service, so the daemon at login reads the same config and
/// data (`ms_todo_core::Paths` reads each).
const PATH_ENV: [&str; 4] = [
    ms_todo_core::CONFIG_DIR_ENV,
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
];

/// Where the login item is, and whether it's there.
#[derive(Serialize)]
pub struct ServiceState {
    /// Whether the daemon starts at login.
    pub installed: bool,
    /// `launchd` or `systemd`.
    pub manager: &'static str,
    /// The file that starts it.
    pub path: String,
    /// Whether this command changed anything; absent in `doctor`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<bool>,
    /// How to start or stop it now rather than at the next login; absent
    /// in `doctor`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub now: Option<String>,
    /// install only: the variables written into the service's
    /// environment, from the installing shell's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<BTreeMap<String, String>>,
}

impl Render for ServiceState {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let state = match (self.installed, self.changed) {
            (true, Some(false)) => "starts at login (already installed)",
            (true, _) => "starts at login",
            (false, Some(false)) => "doesn't start at login (wasn't installed)",
            (false, _) => "doesn't start at login",
        };
        let mut rows = vec![
            ("Login", state.to_owned()),
            ("Manager", self.manager.to_owned()),
            ("File", self.path.clone()),
        ];
        if let Some(environment) = self.environment.as_ref().filter(|env| !env.is_empty()) {
            let pairs: Vec<String> = environment
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            rows.push(("Environment", pairs.join(" ")));
        }
        if let Some(now) = &self.now {
            rows.push(("Now", now.clone()));
        }
        rows
    }
}

/// How this system starts things at login.
struct Manager {
    name: &'static str,
    /// The file that describes the daemon.
    file: PathBuf,
    /// A link whose presence enables it (systemd's `wants`), if needed.
    enabled_by: Option<PathBuf>,
}

impl Manager {
    fn here() -> Result<Self, CliError> {
        let home = home_dir()?;
        if cfg!(target_os = "macos") {
            return Ok(Self {
                name: "launchd",
                file: home
                    .join("Library/LaunchAgents")
                    .join(format!("{LAUNCHD_LABEL}.plist")),
                enabled_by: None,
            });
        }
        if cfg!(target_os = "linux") {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .unwrap_or_else(|| home.join(".config"));
            let units = config.join("systemd/user");
            return Ok(Self {
                name: "systemd",
                file: units.join(SYSTEMD_UNIT),
                enabled_by: Some(units.join("default.target.wants").join(SYSTEMD_UNIT)),
            });
        }
        Err(CliError::message(
            ErrorKind::Unsupported,
            "starting the daemon at login is built for macOS (launchd) and Linux (systemd) only"
                .into(),
        ))
    }

    fn installed(&self) -> bool {
        let marker = self.enabled_by.as_deref().unwrap_or(&self.file);
        marker.symlink_metadata().is_ok() && self.file.is_file()
    }

    fn state(&self, changed: Option<bool>, now: Option<String>) -> ServiceState {
        ServiceState {
            installed: self.installed(),
            manager: self.name,
            path: self.file.display().to_string(),
            changed,
            now,
            environment: None,
        }
    }
}

fn home_dir() -> Result<PathBuf, CliError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .ok_or_else(|| {
            CliError::message(ErrorKind::Internal, "HOME isn't set to a directory".into())
        })
}

/// `doctor`'s view: whether it's installed, or `None` where it can't be.
pub fn status() -> Option<ServiceState> {
    Manager::here()
        .ok()
        .map(|manager| manager.state(None, None))
}

/// `daemon install`: write the login item, or leave it if it's already
/// as it would be written.
pub fn install(paths: &Paths) -> Result<ServiceState, CliError> {
    if paths.instance != Instance::Default {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            format!(
                "the daemon started at login is the installed one (the default instance), not \
                 {:?}; run the installed ms-todo, or pass `--instance default`",
                paths.instance.label()
            ),
        ));
    }
    let manager = Manager::here()?;
    let program = program()?;
    let log = paths.daemon_log_file();
    prepare_log(&log)?;
    let environment: BTreeMap<String, String> = PATH_ENV
        .iter()
        .filter_map(|name| {
            let value = std::env::var(name).ok().filter(|value| !value.is_empty())?;
            Some(((*name).to_owned(), value))
        })
        .collect();
    let contents = match manager.name {
        "launchd" => launchd_plist(&program, &log, &environment),
        _ => systemd_unit(&program, &log, &environment),
    };
    let mut changed = write_if_different(&manager.file, &contents)?;
    if let Some(link) = &manager.enabled_by {
        changed |= link_to(link, &manager.file)?;
    }
    let now = match manager.name {
        "launchd" => format!(
            "to start it now: launchctl bootstrap gui/$(id -u) {}",
            shell_quoted(&manager.file)
        ),
        _ => "to start it now: systemctl --user daemon-reload && systemctl --user start ms-todo"
            .to_owned(),
    };
    Ok(ServiceState {
        environment: Some(environment),
        ..manager.state(Some(changed), Some(now))
    })
}

/// `daemon uninstall`: remove the login item, if it's there.
pub fn uninstall() -> Result<ServiceState, CliError> {
    let manager = Manager::here()?;
    let mut changed = false;
    if let Some(link) = &manager.enabled_by {
        changed |= remove(link)?;
    }
    changed |= remove(&manager.file)?;
    let now = match manager.name {
        "launchd" => {
            format!("if it was started from it: launchctl bootout gui/$(id -u)/{LAUNCHD_LABEL}")
        }
        _ => "if it was started from it: systemctl --user stop ms-todo && systemctl --user \
              daemon-reload"
            .to_owned(),
    };
    Ok(manager.state(Some(changed), Some(now)))
}

/// This binary, by a path that outlives an upgrade: Homebrew runs it from
/// `<prefix>/Cellar/ms-todo/<version>/bin`, which the next upgrade
/// deletes, while `<prefix>/opt/ms-todo/bin` follows the installed version.
fn program() -> Result<PathBuf, CliError> {
    let exe = std::env::current_exe()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    Ok(homebrew_opt(&exe).filter(|opt| opt.exists()).unwrap_or(exe))
}

fn homebrew_opt(exe: &Path) -> Option<PathBuf> {
    let parts: Vec<Component<'_>> = exe.components().collect();
    let cellar = parts
        .iter()
        .rposition(|part| part.as_os_str() == "Cellar")?;
    // <prefix>/Cellar/<formula>/<version>/<the rest>
    let formula = parts.get(cellar + 1)?;
    let rest = parts.get(cellar + 3..).filter(|rest| !rest.is_empty())?;
    let mut opt: PathBuf = parts[..cellar].iter().collect();
    opt.push("opt");
    opt.push(formula);
    opt.extend(rest);
    Some(opt)
}

/// The log the daemon writes to, made as a client's auto-start makes it
/// (a 0700 directory, a 0600 file), so the service manager needn't.
fn prepare_log(log: &Path) -> Result<(), CliError> {
    crate::daemon_client::open_log(log).map(drop)
}

fn launchd_plist(program: &Path, log: &Path, environment: &BTreeMap<String, String>) -> String {
    let program = xml_escaped(&program.display().to_string());
    let log = xml_escaped(&log.display().to_string());
    let environment = if environment.is_empty() {
        String::new()
    } else {
        let pairs: String = environment
            .iter()
            .map(|(key, value)| {
                format!(
                    "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                    xml_escaped(key),
                    xml_escaped(value)
                )
            })
            .collect();
        format!("\t<key>EnvironmentVariables</key>\n\t<dict>\n{pairs}\t</dict>\n")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by `ms-todo daemon install`; `ms-todo daemon uninstall` removes it. -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LAUNCHD_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
		<string>daemon</string>
		<string>run</string>
		<string>--instance</string>
		<string>default</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
{environment}</dict>
</plist>
"#
    )
}

fn systemd_unit(program: &Path, log: &Path, environment: &BTreeMap<String, String>) -> String {
    let program = systemd_quoted(&program.display().to_string());
    let log = log.display().to_string().replace('%', "%%");
    let environment: String = environment
        .iter()
        .map(|(key, value)| {
            format!(
                "Environment={}\n",
                systemd_quoted(&format!("{key}={value}"))
            )
        })
        .collect();
    format!(
        "# Written by `ms-todo daemon install`; `ms-todo daemon uninstall` removes it.\n\
         [Unit]\n\
         Description=ms-todo daemon: Microsoft To Do sync and nag reminders\n\
         \n\
         [Service]\n\
         ExecStart={program} daemon run --instance default\n\
         {environment}\
         Restart=on-failure\n\
         StandardOutput=append:{log}\n\
         StandardError=append:{log}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

fn xml_escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A path as one argument of `ExecStart`: double-quoted, with systemd's
/// escapes for `\`, `"` and its `%` specifiers.
fn systemd_quoted(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

fn shell_quoted(path: &Path) -> String {
    let text = path.display().to_string();
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "/._-".contains(ch))
    {
        text
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// Write `contents` to `path` unless it already holds exactly that.
/// Returns whether it wrote. Written aside and renamed, so a half-written
/// file is never read.
fn write_if_different(path: &Path, contents: &str) -> Result<bool, CliError> {
    match std::fs::read_to_string(path) {
        Ok(existing) if existing == contents => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == IoErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let aside = path.with_extension("tmp");
    // launchd refuses a plist others can write: 0644.
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o644)
        .open(&aside)
        .and_then(|mut file| std::io::Write::write_all(&mut file, contents.as_bytes()))?;
    std::fs::rename(&aside, path)?;
    Ok(true)
}

/// Make `link` a symlink to `target`. Returns whether it changed.
fn link_to(link: &Path, target: &Path) -> Result<bool, CliError> {
    if std::fs::read_link(link).is_ok_and(|points| points == target) {
        return Ok(false);
    }
    remove(link)?;
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::os::unix::fs::symlink(target, link)?;
    Ok(true)
}

/// Remove `path` if it's there. Returns whether it was.
fn remove(path: &Path) -> Result<bool, CliError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == IoErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_homebrew_cellar_path_becomes_its_opt_link() {
        assert_eq!(
            homebrew_opt(Path::new("/opt/homebrew/Cellar/ms-todo/0.1.34/bin/ms-todo")),
            Some(PathBuf::from("/opt/homebrew/opt/ms-todo/bin/ms-todo"))
        );
        assert_eq!(
            homebrew_opt(Path::new("/Users/bk/.local/bin/ms-todo")),
            None
        );
        assert_eq!(
            homebrew_opt(Path::new("/opt/homebrew/Cellar/ms-todo")),
            None
        );
    }

    #[test]
    fn the_plist_runs_the_default_instance_and_restarts_only_after_a_crash() {
        let environment =
            BTreeMap::from([("MS_TODO_CONFIG_DIR".to_owned(), "/cfg & co".to_owned())]);
        let plist = launchd_plist(
            Path::new("/Apps & Tools/ms-todo"),
            Path::new("/Users/bk/Library/Application Support/ms-todo/logs/daemon.log"),
            &environment,
        );
        assert!(
            plist.contains(
                "<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>MS_TODO_CONFIG_DIR</key>\n\t\t<string>/cfg &amp; co</string>"
            ),
            "{plist}"
        );
        let bare = launchd_plist(
            Path::new("/bin/ms-todo"),
            Path::new("/log"),
            &BTreeMap::new(),
        );
        assert!(!bare.contains("EnvironmentVariables"));
        assert!(plist.contains("<string>/Apps &amp; Tools/ms-todo</string>"));
        assert!(plist.contains("<string>--instance</string>\n\t\t<string>default</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(plist.contains(&format!("<string>{LAUNCHD_LABEL}</string>")));
    }

    #[test]
    fn the_unit_quotes_the_program_and_escapes_specifiers() {
        let unit = systemd_unit(
            Path::new("/home/bk/my bin/ms-todo"),
            Path::new("/home/bk/.local/share/ms-todo/logs/daemon.log"),
            &BTreeMap::from([("XDG_DATA_HOME".to_owned(), "/home/bk/my data".to_owned())]),
        );
        assert!(
            unit.contains("Environment=\"XDG_DATA_HOME=/home/bk/my data\"\n"),
            "{unit}"
        );
        assert!(
            unit.contains("ExecStart=\"/home/bk/my bin/ms-todo\" daemon run --instance default"),
            "{unit}"
        );
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
        assert_eq!(systemd_quoted("/a%b\"c"), "\"/a%%b\\\"c\"");
    }
}
