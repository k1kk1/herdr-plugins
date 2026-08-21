//! Opening a session.
//!
//! ## Why this spawns a window instead of attaching in place
//!
//! `herdr session attach <name>` takes over the terminal it runs in. Run from
//! inside a Herdr pane it is refused outright:
//!
//! ```text
//! error: nested herdr is disabled by default.
//! ```
//!
//! That is Herdr's `[experimental] allow_nested` guard, and it is off by
//! default for good reason — a nested session eats the outer session's prefix
//! key. So a session cannot be opened *inside* the session you are already in.
//! It has to be opened in a new window of the outer terminal, which is the
//! same thing you would do by hand.
//!
//! That also makes the plugin and the Alfred workflow share one implementation
//! rather than two: from Alfred there is no Herdr pane at all, and the answer
//! is still "a new terminal window".

use std::path::PathBuf;
use std::process::{Command, Stdio};

use herdr_plugin_kit::{bail, Context, Result};
use serde::Deserialize;

use crate::session::herdr_bin;

/// Settings for [`crate::PLUGIN_ID`].
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Command that opens a new terminal window running an attached session.
    ///
    /// `{herdr}` is replaced with the Herdr binary and `{session}` with the
    /// session name. Empty means "work it out from the terminal in use".
    pub command: Vec<String>,
    /// How many past Claude Code / Codex conversations to list.
    ///
    /// `None` — the default — lists every one of them. Reading is cheap
    /// because only the head or tail of each transcript is touched: 94
    /// conversations spanning 515 MB took 80 ms to summarise in full.
    ///
    /// Set it if that stops being true. It caps the work, not just the
    /// output: transcripts are sorted by modification time and only the
    /// survivors are read, so a limit of 50 costs fifty reads.
    pub recent: Option<usize>,
    /// Where a resumed conversation is put.
    pub resume_in: crate::resume::Where,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            command: Vec::new(),
            recent: None,
            resume_in: crate::resume::Where::default(),
        }
    }
}

impl Config {
    pub fn load() -> (Self, Option<String>) {
        herdr_plugin_kit::config::load(crate::PLUGIN_ID)
    }

    /// The listing limit. Unset means no limit.
    ///
    /// `recent = 0` is read as "no limit" too: nobody writes it meaning "show
    /// me nothing", and an empty picker with no explanation is the worst
    /// possible reading of a typo.
    pub fn recent(&self) -> usize {
        match self.recent {
            None | Some(0) => usize::MAX,
            Some(n) => n,
        }
    }
}

/// The command line that opens `name`, with placeholders filled in.
pub fn command_for(config: &Config, name: &str) -> Result<Vec<String>> {
    let template = if config.command.is_empty() {
        default_template()?
    } else {
        config.command.clone()
    };
    Ok(template
        .into_iter()
        .map(|part| {
            part.replace("{herdr}", &herdr_bin())
                .replace("{session}", name)
        })
        .collect())
}

/// The application bundle that Herdr is actually running inside.
///
/// Found by walking up from a live `herdr` client process until an ancestor
/// turns out to be an app bundle's executable:
///
/// ```text
/// herdr → -/bin/zsh → /usr/bin/login → /Applications/cmux.app/…/cmux
/// ```
///
/// Worth the process walk because the alternatives are wrong. `TERM_PROGRAM`
/// is unset when Alfred is the caller, and guessing from what is installed
/// picks whichever terminal happens to be listed first — which is how a user
/// running Herdr inside cmux ended up with a brand new, empty Ghostty window.
pub fn hosting_app() -> Option<PathBuf> {
    let pids = Command::new("pgrep").args(["-x", "herdr"]).output().ok()?;
    let pids = String::from_utf8_lossy(&pids.stdout);

    for pid in pids.split_whitespace() {
        let mut pid = pid.to_string();
        // Bounded: a runaway or circular tree must not spin here.
        for _ in 0..12 {
            let out = Command::new("ps")
                .args(["-o", "ppid=,comm=", "-p", &pid])
                .output()
                .ok()?;
            let line = String::from_utf8_lossy(&out.stdout);
            let Some((parent, command)) = line.trim().split_once(char::is_whitespace) else {
                break;
            };
            if let Some(bundle) = bundle_of(command.trim()) {
                return Some(bundle);
            }
            pid = parent.trim().to_string();
            if pid == "1" || pid == "0" || pid.is_empty() {
                break;
            }
        }
    }
    None
}

/// `/Applications/cmux.app/Contents/MacOS/cmux` → `/Applications/cmux.app`
fn bundle_of(command: &str) -> Option<PathBuf> {
    let at = command.find(".app/Contents/MacOS/")?;
    Some(PathBuf::from(&command[..at + 4]))
}

/// The application the open command launches, if it names one.
///
/// Used to bring the terminal forward after doing something to the session
/// that is already running in it. Derived from the command rather than
/// configured separately so the two cannot disagree.
pub fn terminal_app(config: &Config) -> Option<String> {
    command_for(config, "")
        .ok()?
        .into_iter()
        .find(|part| part.ends_with(".app"))
}

/// Bring the terminal Herdr is running in to the front.
///
/// Only ever raises an application that is **already running**, which is why
/// the target comes from the process tree rather than from configuration or a
/// guess: `open -a` launches an app that is not running, and a fresh empty
/// terminal window is a worse outcome than doing nothing at all.
pub fn focus_terminal(_config: &Config) {
    let Some(app) = hosting_app() else {
        return;
    };
    // No `-n`: the point is to raise the window that already exists.
    let _ = Command::new("open")
        .args(["-a".as_ref(), app.as_os_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Run `argv` in a new terminal window.
///
/// The fallback for when Herdr is not running: a conversation still has to be
/// resumable, it just cannot be put into a session that does not exist.
pub fn run_in_terminal(config: &Config, argv: &[String], cwd: Option<&str>) -> Result<Vec<String>> {
    let template = command_for(config, "")?;
    // Everything up to the placeholder command is the terminal's own
    // invocation; `{herdr} session attach` is what gets replaced.
    let head: Vec<String> = template
        .into_iter()
        .take_while(|part| !part.ends_with("herdr") && part != "session")
        .collect();

    let mut line: Vec<String> = head;
    if let Some(cwd) = cwd {
        // `cd` first: the agent has to start where the conversation happened.
        line.push("sh".into());
        line.push("-c".into());
        line.push(format!("cd {} && exec {}", shell_quote(cwd), argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")));
    } else {
        line.extend(argv.iter().cloned());
    }

    let (program, rest) = line.split_first().context("the open command is empty")?;
    let mut command = Command::new(program);
    command
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    disinherit(&mut command);
    command
        .spawn()
        .with_context(|| format!("could not run `{program}`"))?;
    Ok(line)
}

/// Single-quote for `sh -c`, which is the one place here a shell is involved.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Launch the session in a new terminal window.
///
/// Detached deliberately: the spawned window outlives this process, which on
/// the plugin path is a popup that is about to close.
pub fn open(config: &Config, name: &str) -> Result<Vec<String>> {
    let argv = command_for(config, name)?;
    let (program, args) = argv.split_first().context("the open command is empty")?;

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    disinherit(&mut command);

    command
        .spawn()
        .with_context(|| format!("could not run `{program}`"))?;

    Ok(argv)
}

/// Strip every `HERDR_*` variable from the child's environment.
///
/// Without this the whole thing does not work, and the reason is not obvious:
/// on macOS `open -na` hands the caller's environment to the application it
/// launches, so a window opened from inside a Herdr pane arrives still
/// carrying `HERDR_ENV=1`. Herdr sees that, decides it is being run inside
/// itself, and refuses:
///
/// ```text
/// error: nested herdr is disabled by default.
/// ```
///
/// The window is genuinely a new top-level terminal; only the inherited
/// variables said otherwise. Clearing the prefix wholesale also drops the
/// stale `HERDR_PANE_ID` and `HERDR_SOCKET_PATH`, which would otherwise point
/// the new session's panes at *this* session's server.
fn disinherit(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
}

// ---------------------------------------------------------------------------
// Working out how to open a terminal window
// ---------------------------------------------------------------------------

/// Terminals we know the incantation for, in preference order.
///
/// On macOS a GUI terminal cannot be started by running its binary — Ghostty
/// says so itself — so every entry goes through `open -na`, which is what each
/// project documents. An empty argument list means the terminal takes a
/// command only through AppleScript, which is not worth guessing at.
const KNOWN: &[(&str, &str, &[&str])] = &[
    // (TERM_PROGRAM value, application, arguments before the command)
    ("ghostty", "Ghostty.app", &["--args", "-e"]),
    ("WezTerm", "WezTerm.app", &["--args", "start", "--"]),
    ("iTerm.app", "iTerm.app", &[]),
    ("Apple_Terminal", "Terminal.app", &[]),
];

/// The command template for the terminal the user is actually in, falling back
/// to whichever known terminal is installed.
fn default_template() -> Result<Vec<String>> {
    let attach = ["{herdr}", "session", "attach", "{session}"];

    if cfg!(not(target_os = "macos")) {
        // No `open` equivalent worth guessing at. `$TERMINAL -e cmd` is the
        // closest thing to a convention.
        let terminal = std::env::var("TERMINAL").unwrap_or_default();
        if terminal.trim().is_empty() {
            bail!("{}", NO_TERMINAL);
        }
        return Ok(std::iter::once(terminal)
            .chain(std::iter::once("-e".to_string()))
            .chain(attach.iter().map(|s| s.to_string()))
            .collect());
    }

    // The terminal Herdr is actually running in comes first, then the one this
    // process is running in, then whatever is installed. Only the first is
    // reliable when Alfred is the caller: it sets no TERM_PROGRAM, and
    // "whatever is installed" once opened an empty Ghostty window for someone
    // whose Herdr lives in cmux.
    let host = hosting_app()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().to_string()));
    let current = std::env::var("TERM_PROGRAM").unwrap_or_default();

    if let Some(host) = &host {
        if let Some((_, app, before)) = KNOWN
            .iter()
            .find(|(_, app, _)| app == host)
            .filter(|(_, _, before)| !before.is_empty())
        {
            return Ok(assemble(app, before, &attach));
        }
        // Herdr's terminal cannot be handed a command. Falling through to
        // another one still does the useful thing — a session attached in some
        // terminal beats no session — but the caller says which, because a
        // window from an application you were not using is a surprise.
    }

    let chosen = KNOWN
        .iter()
        .find(|(term, app, _)| *term == current && installed(app))
        .or_else(|| KNOWN.iter().find(|(_, app, _)| installed(app)));

    let Some((name, app, before)) = chosen else {
        bail!("{}", NO_TERMINAL);
    };

    if before.is_empty() {
        bail!("{}", cannot_drive(name));
    }

    Ok(assemble(app, before, &attach))
}

fn assemble(app: &str, before: &[&str], attach: &[&str]) -> Vec<String> {
    ["open", "-na", app]
        .into_iter()
        .chain(before.iter().copied())
        .chain(attach.iter().copied())
        .map(str::to_string)
        .collect()
}

/// Set when the session will open in a terminal other than Herdr's own.
///
/// Returned for the caller to show, rather than swallowed: a window from an
/// application the user was not using needs explaining, and the explanation is
/// also the instruction for changing it.
pub fn foreign_terminal_note(config: &Config) -> Option<String> {
    if !config.command.is_empty() {
        return None;
    }
    let host = hosting_app()?;
    let host = host.file_name()?.to_string_lossy().to_string();
    if KNOWN
        .iter()
        .any(|(_, app, before)| *app == host && !before.is_empty())
    {
        return None;
    }
    let opened = terminal_app(config)?;
    Some(format!(
        "Opened in {opened}: {host} cannot be handed a command. \
         Set `command` in the Sessions plugin config to change that."
    ))
}

#[allow(dead_code)]
fn cannot_drive(name: &str) -> String {
    format!(
        "{name} cannot be handed a command on the command line.\n\
         Set `command` in the Sessions plugin config to open it your way:\n\
         \n  herdr plugin config-dir {}\n\
         \n{}",
        crate::PLUGIN_ID,
        EXAMPLE
    )
}

fn installed(app: &str) -> bool {
    !app.is_empty() && std::path::Path::new("/Applications").join(app).exists()
}

const NO_TERMINAL: &str = "\
Could not work out how to open a new terminal window.
Set `command` in the Sessions plugin config, using {session} for the name.";

const EXAMPLE: &str = r#"command = ["open", "-na", "Ghostty.app", "--args", "-e", "{herdr}", "session", "attach", "{session}"]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_in() {
        let config = Config {
            command: ["t", "-e", "{herdr}", "attach", "{session}"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        };
        let argv = command_for(&config, "scratch").unwrap();
        assert_eq!(argv[4], "scratch");
        assert_ne!(argv[2], "{herdr}");
    }

    #[test]
    fn a_session_name_is_never_shell_interpreted() {
        // The command is spawned as argv, not through a shell, so a name with
        // shell metacharacters is inert. Pinned because the Alfred path takes
        // this name from user input.
        let config = Config {
            command: ["t", "{session}"].iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let argv = command_for(&config, "; rm -rf /").unwrap();
        assert_eq!(argv, ["t", "; rm -rf /"]);
    }

    #[test]
    fn everything_is_listed_unless_a_limit_is_set() {
        assert_eq!(Config::default().recent(), usize::MAX);
        let capped = Config { recent: Some(25), ..Default::default() };
        assert_eq!(capped.recent(), 25);
        // A `0` is a typo, not a request for an empty picker.
        let zero = Config { recent: Some(0), ..Default::default() };
        assert_eq!(zero.recent(), usize::MAX);
    }

    #[test]
    fn the_terminal_app_is_read_back_out_of_the_open_command() {
        let config = Config {
            command: ["open", "-na", "Ghostty.app", "--args", "-e", "{session}"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        };
        assert_eq!(terminal_app(&config).as_deref(), Some("Ghostty.app"));
    }

    #[test]
    fn shell_quoting_survives_a_directory_with_a_quote_in_it() {
        assert_eq!(shell_quote("/a/b"), "'/a/b'");
        assert_eq!(shell_quote("/it's"), r"'/it'\''s'");
    }

    #[test]
    fn a_configured_command_is_used_verbatim() {
        let config = Config {
            command: vec!["my-terminal".into(), "{session}".into()],
            ..Default::default()
        };
        assert_eq!(command_for(&config, "x").unwrap()[0], "my-terminal");
    }
}
