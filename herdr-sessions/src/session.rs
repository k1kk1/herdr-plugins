//! Finding out what sessions exist, and what is inside them.
//!
//! A Herdr session is a whole server with its own socket, and **no session's
//! API can see any other** — there is no `session.list` method, only
//! `session.snapshot` for the one you are connected to. So the list itself
//! comes from the `herdr` CLI, which reads it off disk, and the summaries come
//! from dialling each session's own socket afterwards.
//!
//! A stopped session has no socket to dial, so its summary is read from the
//! `session.json` the server left behind. That file is the same thing the
//! server will restore from, which is what makes it worth showing: it is a
//! preview of what attaching would bring back.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

/// One entry of `herdr session list --json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub name: String,
    #[serde(default)]
    pub running: bool,
    /// Herdr's own flag for the unnamed session you get from a bare `herdr`.
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub session_dir: Option<PathBuf>,
    #[serde(default)]
    pub socket_path: Option<PathBuf>,
}

impl Session {
    /// Whether this is the session the caller is running inside.
    ///
    /// Compared by socket path rather than by name: `HERDR_SESSION` is not set
    /// in every pane, but `HERDR_SOCKET_PATH` always is, and the socket is
    /// what actually identifies a server.
    ///
    /// The `HERDR_ENV` check is not redundant. Run from Alfred there is no
    /// Herdr pane at all, and `socket_path()` still answers — with its default
    /// of `~/.config/herdr/herdr.sock`. Without this guard the default session
    /// would come up marked "you are here" and refuse to open, from a launcher
    /// where the user is demonstrably not in any session.
    pub fn is_current(&self) -> bool {
        if std::env::var_os("HERDR_ENV").is_none() {
            return false;
        }
        let (Some(theirs), Ok(ours)) = (
            self.socket_path.as_ref(),
            herdr_plugin_kit::herdr::socket_path(),
        ) else {
            return false;
        };
        same_path(theirs, &ours)
    }

    pub fn state(&self) -> &'static str {
        if self.running {
            "running"
        } else {
            "stopped"
        }
    }
}

/// Compare paths by their resolved form, so a symlinked config directory does
/// not make the current session look like somebody else's.
fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The `herdr` binary to shell out to, as an absolute path.
///
/// Herdr exports `HERDR_BIN_PATH` to panes it starts, which is the version
/// that owns the sessions we are about to list. Falling back to `PATH` covers
/// being run from an ordinary shell, which is how Alfred calls us.
///
/// Resolved to an absolute path rather than left as a bare name, because both
/// things that consume it hand it to a process that does **not** inherit this
/// one's `PATH`: a GUI terminal started through `open`, and an Alfred workflow
/// script. Homebrew's `bin` is on neither. A bare `herdr` there fails with
/// "command not found" inside a window that then vanishes.
pub fn herdr_bin() -> String {
    let named = std::env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "herdr".to_string());

    if named.contains('/') {
        return named;
    }
    which(&named).unwrap_or(named)
}

/// Look a command up on `PATH` ourselves. `/usr/bin/which` is not guaranteed
/// to be reached by name in the environments this runs in either.
fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|meta| meta.is_file())
                .unwrap_or(false)
        })
        .map(|found| found.display().to_string())
}

/// Every session Herdr knows about, running and stopped alike.
///
/// Sorted with the running ones first and the current session at the very top,
/// because the two questions a session list answers are "where am I" and
/// "what else is alive".
pub fn list() -> Result<Vec<Session>> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        sessions: Vec<Session>,
    }

    let output = Command::new(herdr_bin())
        .args(["session", "list", "--json"])
        .output()
        .with_context(|| format!("could not run `{} session list`", herdr_bin()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`herdr session list` failed: {}",
            stderr.trim().lines().next().unwrap_or("no output")
        );
    }

    let envelope: Envelope = serde_json::from_slice(&output.stdout)
        .context("could not read the output of `herdr session list --json`")?;

    let mut sessions = envelope.sessions;
    sessions.sort_by_key(|s| {
        (
            !s.is_current(),
            !s.running,
            s.name.to_lowercase(),
        )
    });
    Ok(sessions)
}

// ---------------------------------------------------------------------------
// What is inside a session
// ---------------------------------------------------------------------------

/// A summary of a session's contents, however it had to be obtained.
#[derive(Debug, Default, Clone)]
pub struct Detail {
    pub workspaces: usize,
    pub panes: usize,
    /// Panes with a detected agent.
    pub agents: usize,
    /// Agents that are working or waiting on you, i.e. worth going back for.
    pub busy: usize,
    /// Workspace names, in order, for the second line of a picker row.
    pub names: Vec<String>,
    pub last_used: Option<SystemTime>,
    /// Set when the summary could not be obtained; the row still renders.
    pub problem: Option<String>,
}

impl Detail {
    /// The trailing summary on a picker row, e.g. `3 workspaces · 2 agents`.
    pub fn summary(&self) -> String {
        if let Some(problem) = &self.problem {
            return problem.clone();
        }
        let mut parts = vec![plural(self.workspaces, "workspace")];
        parts.push(plural(self.panes, "pane"));
        if self.agents > 0 {
            parts.push(if self.busy > 0 {
                format!("{} agents, {} busy", self.agents, self.busy)
            } else {
                plural(self.agents, "agent")
            });
        }
        parts.join(" · ")
    }

    /// The workspace names, trimmed to something that fits one line.
    pub fn names_line(&self) -> Option<String> {
        if self.names.is_empty() {
            return None;
        }
        let shown: Vec<&str> = self.names.iter().take(4).map(String::as_str).collect();
        let mut line = shown.join(" · ");
        if self.names.len() > shown.len() {
            line.push_str(&format!(" +{}", self.names.len() - shown.len()));
        }
        Some(line)
    }
}

fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Summarise a session: live over its socket if it is running, off its
/// `session.json` if it is not.
///
/// Never fails. A session that cannot be summarised is still a session you may
/// want to open, so the problem is recorded on the row instead of aborting the
/// whole list.
pub fn detail(session: &Session) -> Detail {
    let mut detail = if session.running {
        live(session).unwrap_or_else(|err| Detail {
            problem: Some(short(&err)),
            ..Default::default()
        })
    } else {
        stored(session).unwrap_or_else(|err| Detail {
            problem: Some(short(&err)),
            ..Default::default()
        })
    };
    detail.last_used = session.session_dir.as_deref().and_then(last_used);
    detail
}

fn short(err: &anyhow::Error) -> String {
    err.to_string()
        .lines()
        .next()
        .unwrap_or("could not be read")
        .to_string()
}

/// Summary of a running session, from one `session.snapshot` call.
///
/// One round trip for the whole thing: the snapshot carries workspaces, panes
/// and agents together, so listing ten sessions costs ten connections rather
/// than thirty.
fn live(session: &Session) -> Result<Detail> {
    let socket = session
        .socket_path
        .as_ref()
        .context("this session has no socket path")?;
    let client = Herdr::at(socket.clone())?;
    let snapshot = client.call("session.snapshot", serde_json::json!({}))?;
    let snapshot = snapshot
        .get("snapshot")
        .context("the snapshot came back empty")?;

    let array = |key: &str| -> &[Value] {
        snapshot
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    };

    let agents = array("agents");
    Ok(Detail {
        workspaces: array("workspaces").len(),
        panes: array("panes").len(),
        agents: agents.len(),
        busy: agents
            .iter()
            .filter(|agent| {
                matches!(
                    agent.get("agent_status").and_then(Value::as_str),
                    Some("working") | Some("blocked")
                )
            })
            .count(),
        names: array("workspaces")
            .iter()
            .filter_map(|w| w.get("label").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        ..Default::default()
    })
}

/// Summary of a stopped session, from the state file it left behind.
///
/// Read with `Value` rather than a typed struct on purpose: this is Herdr's
/// private on-disk format, it is versioned (`"version": 3` at the time of
/// writing), and a plugin that hard-fails when that number changes would be
/// worse than one that shows a slightly thinner row.
fn stored(session: &Session) -> Result<Detail> {
    let dir = session
        .session_dir
        .as_ref()
        .context("this session has no directory")?;
    let path = dir.join("session.json");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("no saved state at {}", path.display()))?;
    let state: Value = serde_json::from_str(&raw)
        .with_context(|| format!("could not read {}", path.display()))?;

    let workspaces = state
        .get("workspaces")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut panes = 0usize;
    for workspace in workspaces {
        for tab in workspace
            .get("tabs")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            panes += tab
                .get("panes")
                .and_then(Value::as_object)
                .map(|panes| panes.len())
                .unwrap_or(0);
        }
    }

    Ok(Detail {
        workspaces: workspaces.len(),
        panes,
        names: workspaces
            .iter()
            .filter_map(|w| w.get("custom_name").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        ..Default::default()
    })
}

/// When the session was last written to, as a stand-in for when it was last
/// used. Herdr rewrites `session.json` as the session changes.
fn last_used(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir.join("session.json"))
        .ok()?
        .modified()
        .ok()
}

/// `3 days ago`, for a stopped session's row.
pub fn ago(time: SystemTime) -> String {
    let Ok(elapsed) = time.elapsed() else {
        return "just now".into();
    };
    let seconds = elapsed.as_secs();
    let (n, unit) = match seconds {
        0..=59 => return "just now".into(),
        60..=3599 => (seconds / 60, "minute"),
        3600..=86_399 => (seconds / 3600, "hour"),
        86_400..=2_591_999 => (seconds / 86_400, "day"),
        _ => (seconds / 2_592_000, "month"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn session(name: &str, running: bool) -> Session {
        Session {
            name: name.into(),
            running,
            default: false,
            session_dir: None,
            socket_path: None,
        }
    }

    #[test]
    fn running_sessions_sort_above_stopped_ones() {
        let mut sessions = vec![
            session("zeta", true),
            session("alpha", false),
            session("beta", true),
        ];
        sessions.sort_by_key(|s| (!s.is_current(), !s.running, s.name.to_lowercase()));
        let order: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(order, ["beta", "zeta", "alpha"]);
    }

    #[test]
    fn nothing_is_the_current_session_outside_herdr() {
        // The Alfred path runs with no Herdr environment at all, and
        // `socket_path()` still returns its default. Marking the default
        // session "you are here" there would make it unopenable.
        let mut session = session("default", true);
        session.socket_path = herdr_plugin_kit::herdr::socket_path().ok();
        assert!(session.socket_path.is_some());

        let saved = std::env::var_os("HERDR_ENV");
        std::env::remove_var("HERDR_ENV");
        let outside = session.is_current();
        if let Some(saved) = saved {
            std::env::set_var("HERDR_ENV", saved);
        }
        assert!(!outside);
    }

    #[test]
    fn a_summary_names_what_is_in_the_session() {
        let detail = Detail {
            workspaces: 3,
            panes: 1,
            agents: 2,
            busy: 1,
            ..Default::default()
        };
        assert_eq!(detail.summary(), "3 workspaces · 1 pane · 2 agents, 1 busy");
    }

    #[test]
    fn a_summary_that_could_not_be_read_says_so_instead_of_showing_zeroes() {
        // A session whose socket is wedged must not render as "0 workspaces",
        // which reads as a fact about the session rather than about us.
        let detail = Detail {
            problem: Some("could not reach the server".into()),
            ..Default::default()
        };
        assert_eq!(detail.summary(), "could not reach the server");
    }

    #[test]
    fn agents_are_left_out_when_there_are_none() {
        let detail = Detail {
            workspaces: 1,
            panes: 2,
            ..Default::default()
        };
        assert_eq!(detail.summary(), "1 workspace · 2 panes");
    }

    #[test]
    fn workspace_names_are_trimmed_with_a_count_of_the_rest() {
        let detail = Detail {
            names: ["a", "b", "c", "d", "e", "f"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        };
        assert_eq!(detail.names_line().unwrap(), "a · b · c · d +2");
    }

    #[test]
    fn no_names_means_no_second_line() {
        assert!(Detail::default().names_line().is_none());
    }

    #[test]
    fn elapsed_time_reads_as_a_phrase() {
        let now = SystemTime::now();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - Duration::from_secs(60 * 5)), "5 minutes ago");
        assert_eq!(ago(now - Duration::from_secs(3600)), "1 hour ago");
        assert_eq!(ago(now - Duration::from_secs(86_400 * 3)), "3 days ago");
    }
}
