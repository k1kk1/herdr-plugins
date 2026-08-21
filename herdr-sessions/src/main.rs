//! Herdr Sessions — list every Herdr session and open one.
//!
//! The fifth plugin in the set, and the only one that looks *outside* the
//! session it runs in. Pane Manager, Layout Tools, Navigator and Command
//! Palette all work within one session; this one lists the others, running and
//! stopped, and opens the one you pick.
//!
//! ## Two front ends, one implementation
//!
//! The same list is served to two places:
//!
//! * a Herdr plugin popup (`ui`), for when you are already in Herdr
//! * an Alfred Script Filter (`alfred`), for when you are not
//!
//! Both end in [`open::open`], so a session opened either way lands in the
//! same kind of window.
//!
//! ## Why the list does not come over the socket
//!
//! Every session is its own server with its own socket, and the socket API has
//! no `session.list` — only `session.snapshot`, which describes the session
//! you are connected to. So the list comes from `herdr session list --json`,
//! and the per-session summaries come from dialling each session's socket in
//! turn. See [`session`].

mod agents;
mod alfred;
mod open;
mod resume;
mod session;
mod ui;

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Result};

use crate::open::Config;
use crate::ui::Mode;

pub const PLUGIN_ID: &str = "sessions";
/// Plugin pane entrypoint id declared in `herdr-plugin.toml`.
const UI_ENTRYPOINT: &str = "sessions";

const USAGE: &str = "\
herdr-sessions — list Herdr sessions and open one

Usage:
  herdr-sessions launch [open|manage|all|claude|codex]
      Open the picker in a Herdr plugin pane.

  herdr-sessions ui [open|manage|all|claude|codex]
      Run the picker. Herdr invokes this inside the plugin pane; the mode
      also comes from SESSIONS_MODE when no argument is given.

  herdr-sessions list
      Print the Herdr sessions as text, for scripts.

  herdr-sessions recent [all|claude|codex] [--limit N]
      Print past agent conversations as text, newest first. Every tool
      unless one is named; all of them unless --limit or the `recent`
      setting says otherwise.

  herdr-sessions resume [all|claude|codex] [<placement>:]<id>
      Placement is workspace, tab or split; it defaults to `resume_in`.
      Resume a conversation in this Herdr session.

  herdr-sessions open <name>
      Open a session in a new terminal window.

  herdr-sessions alfred [resume]
      Print the sessions, or past conversations, as Alfred Script Filter
      JSON.

  herdr-sessions alfred install [--force]
      Install the Alfred workflow, with absolute paths baked in.
";

fn main() {
    if let Err(err) = run() {
        // Herdr captures stderr from action commands into `herdr plugin log`.
        eprintln!("sessions: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("ui");
    let (config, warning) = Config::load();

    match command {
        // The Herdr paths need the socket; the Alfred and CLI paths must work
        // without it, because Alfred runs them from an ordinary shell.
        "launch" => {
            let herdr = Herdr::connect()?;
            let env = [("SESSIONS_MODE", mode_arg(&args).name().to_string())];
            herdr.open_plugin_pane(PLUGIN_ID, UI_ENTRYPOINT, &env, true)
        }
        "ui" => {
            let herdr = Herdr::connect()?;
            // Best-effort: the picker still works when nothing sensible is
            // focused, it just falls back to a tab for split placements.
            let anchor = herdr_plugin_kit::context::resolve_source_pane(&herdr, None).ok();
            ui::run(&herdr, mode_arg(&args), &config, anchor, warning)
        }
        "list" => list(),
        "recent" => {
            let kind = agent_kind_arg(&args)?;
            let limit = limit_arg(&args).unwrap_or_else(|| config.recent());
            for session in agent_sessions(kind, limit, None)? {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    session.kind.agent(),
                    session.id,
                    session.heading(),
                    session.where_line(),
                    session::ago(session.modified)
                );
            }
            Ok(())
        }
        "resume" => {
            // The tool is optional: an id is unique across both, so
            // `resume <id>` means the same thing as `resume all <id>`.
            let (kind, target) = match args.get(2) {
                Some(target) => (agent_kind_arg(&args)?, target),
                None => (None, args.get(1).filter(|a| !a.is_empty()).ok_or_else(|| {
                    herdr_plugin_kit::anyhow!("resume needs a session id\n\n{USAGE}")
                })?),
            };
            // Alfred carries the placement in the argument, because a Script
            // Filter's modifier keys can only change the text they hand on.
            let (placement, id) = split_placement(target, config.resume_in)?;
            let Some(session) = agent_sessions(kind, usize::MAX, None)?
                .into_iter()
                .find(|s| s.id == *id)
            else {
                let scope = kind.map_or("any tool's", |k| k.label());
                bail!("no {scope} session with id `{id}`");
            };
            resume::anywhere(&session, &config, placement)
        }
        "open" => {
            let Some(name) = args.get(1) else {
                bail!("open needs a session name\n\n{USAGE}");
            };
            let argv = open::open(&config, name)?;
            println!("{}", argv.join(" "));
            if let Some(note) = open::foreign_terminal_note(&config) {
                println!("{note}");
            }
            Ok(())
        }
        "alfred" => match args.get(1).map(String::as_str) {
            Some("install") => {
                let force = args.iter().any(|a| a == "--force");
                let path = alfred::install(&config, force)?;
                println!("Installed the Alfred workflow at {}", path.display());
                println!("Alfred: `hs` for Herdr sessions, `hr` for past conversations.");
                if !path.join("icon.png").is_file() {
                    println!("\n{}", alfred::icon_hint());
                }
                Ok(())
            }
            Some("resume") => {
                println!("{}", alfred::resume_filter(&config)?);
                Ok(())
            }
            // A Script Filter passes the typed query as $1; Alfred does the
            // filtering itself, so anything else here is simply ignored.
            _ => {
                println!("{}", alfred::script_filter(&config)?);
                Ok(())
            }
        },
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("herdr-sessions {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// Split a `<placement>:<id>` argument, defaulting when there is no prefix.
///
/// Ids are UUIDs and never contain a colon, so the split is unambiguous.
fn split_placement(
    target: &str,
    default: resume::Where,
) -> Result<(resume::Where, String)> {
    let Some((head, rest)) = target.split_once(':') else {
        return Ok((default, target.to_string()));
    };
    let Some(placement) = resume::Where::parse(head) else {
        bail!("unknown placement `{head}` — expected workspace, tab or split");
    };
    Ok((placement, rest.to_string()))
}

/// The agent a subcommand means. `None` means every one of them.
fn agent_kind_arg(args: &[String]) -> Result<Option<agents::Kind>> {
    match args.get(1).map(String::as_str) {
        Some("claude") => Ok(Some(agents::Kind::Claude)),
        Some("codex") => Ok(Some(agents::Kind::Codex)),
        Some("all") | None => Ok(None),
        Some(other) => bail!("unknown agent `{other}` — expected all, claude or codex"),
    }
}

/// Conversations of one tool, or of every tool when none is named.
fn agent_sessions(
    kind: Option<agents::Kind>,
    limit: usize,
    herdr: Option<&herdr_plugin_kit::herdr::Herdr>,
) -> Result<Vec<agents::AgentSession>> {
    match kind {
        Some(kind) => agents::list(kind, limit, herdr),
        None => agents::list_all(limit, herdr),
    }
}

/// `--limit N`, for scripting a different number than the config's.
fn limit_arg(args: &[String]) -> Option<usize> {
    let at = args.iter().position(|a| a == "--limit")?;
    args.get(at + 1)?.parse().ok()
}

/// Mode from the argument, else `SESSIONS_MODE`, else open.
fn mode_arg(args: &[String]) -> Mode {
    args.get(1)
        .and_then(|raw| Mode::parse(raw))
        .or_else(|| {
            std::env::var("SESSIONS_MODE")
                .ok()
                .and_then(|raw| Mode::parse(&raw))
        })
        .unwrap_or(Mode::Open)
}

/// Text form of the picker's contents: name, state, and what is inside.
fn list() -> Result<()> {
    for entry in session::list()? {
        let detail = session::detail(&entry);
        println!(
            "{}\t{}\t{}{}",
            entry.name,
            entry.state(),
            detail.summary(),
            if entry.is_current() {
                "\tcurrent"
            } else {
                ""
            }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_placement_prefix_is_optional_and_a_bare_id_keeps_the_default() {
        let id = "3ebd1a0d-395a-4eb0-815c-9c6184d88d67";
        assert_eq!(
            split_placement(id, resume::Where::Workspace).unwrap(),
            (resume::Where::Workspace, id.to_string())
        );
        assert_eq!(
            split_placement(&format!("split:{id}"), resume::Where::Workspace).unwrap(),
            (resume::Where::Split, id.to_string())
        );
        assert!(split_placement(&format!("nowhere:{id}"), resume::Where::Tab).is_err());
    }

    #[test]
    fn the_default_mode_is_the_common_case() {
        assert_eq!(mode_arg(&args(&["ui"])), Mode::Open);
    }

    #[test]
    fn the_mode_comes_from_the_argument() {
        assert_eq!(mode_arg(&args(&["ui", "manage"])), Mode::Manage);
    }

    #[test]
    fn an_unknown_mode_falls_back_rather_than_failing() {
        assert_eq!(mode_arg(&args(&["ui", "nonsense"])), Mode::Open);
    }
}
