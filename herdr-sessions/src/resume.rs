//! Bringing a past Claude Code or Codex conversation back onto the screen.
//!
//! Two steps, both ordinary Herdr calls:
//!
//! 1. make somewhere to put it — a tab, or a split of the current pane
//! 2. `agent.start` in that pane with the tool's own resume arguments
//!
//! No new window is involved. `claude --resume` is just a program, so unlike
//! `herdr session attach` it runs perfectly well inside the session you are
//! already in.

use std::time::{Duration, Instant};

use herdr_plugin_kit::herdr::{Direction, Herdr, Pane};
use herdr_plugin_kit::ui::Key;
use herdr_plugin_kit::{Outcome, Result};
use serde::Deserialize;

use crate::agents::AgentSession;

/// Where a resumed conversation is put.
///
/// The three are graded by how much room the conversation gets, and that is
/// the order the Enter keys go in: plain Enter gives it the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Where {
    /// A workspace of its own, named after the conversation.
    #[default]
    Workspace,
    /// A new tab in the workspace you are in.
    Tab,
    /// A split of the pane you are on, in the tab you are in.
    Split,
}

impl Where {
    /// How the outcome message ends: "Resumed “x” <this>".
    pub fn describe(self) -> &'static str {
        match self {
            Where::Workspace => "in a new workspace",
            Where::Tab => "in a new tab",
            Where::Split => "beside this pane",
        }
    }

    /// The name used on the command line and in Alfred's argument.
    pub fn name(self) -> &'static str {
        match self {
            Where::Workspace => "workspace",
            Where::Tab => "tab",
            Where::Split => "split",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "workspace" => Some(Where::Workspace),
            "tab" => Some(Where::Tab),
            "split" => Some(Where::Split),
            _ => None,
        }
    }

    /// The placement a key press asks for, if any.
    pub fn from_key(key: Key) -> Option<Self> {
        match key {
            Key::Enter => Some(Where::Workspace),
            Key::ShiftEnter => Some(Where::Tab),
            Key::AltEnter => Some(Where::Split),
            _ => None,
        }
    }

    /// The keys that are not plain Enter, for [`Menu::accept_also`].
    pub const MODIFIED: [Key; 2] = [Key::ShiftEnter, Key::AltEnter];
}

/// Resume `session` from wherever the caller happens to be.
///
/// Inside Herdr this is the same as [`resume`]. From outside — Alfred, a
/// shell — it still puts the conversation into the running session rather than
/// a window of its own, because that is where the user works; the terminal is
/// then brought forward so the result is visible. With no session running
/// there is nothing to put it into, so a new window is the answer after all.
pub fn anywhere(
    session: &AgentSession,
    config: &crate::open::Config,
    placement: Where,
) -> Result<()> {
    let outside = std::env::var_os("HERDR_ENV").is_none();

    match Herdr::connect() {
        Ok(herdr) => {
            let anchor = herdr_plugin_kit::context::resolve_source_pane(&herdr, None).ok();
            let outcome = resume(&herdr, session, placement, anchor.as_ref())?;
            if outside {
                crate::open::focus_terminal(config);
            }
            outcome.report(&herdr);
            Ok(())
        }
        Err(_) if outside => {
            let mut argv = vec![session.kind.command()];
            argv.extend(session.kind.resume_args(&session.id));
            let cwd = session.cwd.as_ref().map(|p| p.display().to_string());
            let line = crate::open::run_in_terminal(config, &argv, cwd.as_deref())?;
            println!("Herdr is not running; opened a window instead");
            println!("{}", line.join(" "));
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Resume `session`, and say what happened.
///
/// `anchor` is the pane the user was on, needed only for the split placement.
pub fn resume(
    herdr: &Herdr,
    session: &AgentSession,
    placement: Where,
    anchor: Option<&Pane>,
) -> Result<Outcome> {
    // Start the agent in the directory the conversation happened in. Without
    // this the agent lands somewhere unrelated and opens with a "do you trust
    // this folder?" prompt instead of the transcript.
    let cwd = session.cwd.as_ref().map(|p| p.display().to_string());
    let heading = session.heading();
    let label = label(&heading);

    // Splitting needs a pane to split. Falling back to a tab is better than
    // refusing: the user asked for the conversation, not for a particular
    // rectangle.
    let placement = match (placement, anchor) {
        (Where::Split, None) => Where::Tab,
        (placement, _) => placement,
    };

    let pane = match placement {
        Where::Workspace => herdr.create_workspace(&label, cwd.as_deref(), true)?,
        Where::Tab => {
            let workspace = herdr.focused_workspace()?;
            herdr.create_tab(&workspace.workspace_id, &label, cwd.as_deref(), true)?
        }
        Where::Split => {
            let target = anchor.expect("checked above").pane_id.clone();
            herdr.split_pane(&target, Direction::Right, cwd.as_deref(), true)?
        }
    };

    start(herdr, session, pane, placement, &heading)
}

fn start(
    herdr: &Herdr,
    session: &AgentSession,
    pane: Pane,
    placement: Where,
    heading: &str,
) -> Result<Outcome> {
    let args = session.kind.resume_args(&session.id);

    if let Err(err) = launch(herdr, &pane, session, &args) {
        // The pane was made for an agent that never arrived. Leaving it behind
        // would litter the session with empty shells every time something goes
        // wrong, and the user did not ask for a pane — they asked for a
        // conversation.
        let _ = herdr.close_pane(&pane.pane_id);
        return Err(err);
    }

    let mut detail = format!("{} {}", session.kind.agent(), args.join(" "));
    if let Some(cwd) = &session.cwd {
        detail.push_str(&format!("\nin {}", cwd.display()));
    }
    Ok(
        Outcome::new(format!("Resumed “{heading}” {}", placement.describe()))
            .with_detail(detail),
    )
}

/// How long to keep waiting for a freshly made pane's shell.
///
/// `pane.split` and `tab.create` return as soon as the pane exists, but the
/// shell inside it is not interactive yet, and `agent.start` refuses a pane
/// that has no prompt with `agent_pane_busy`. Measured: a pane opened in
/// `/tmp` is ready immediately, one opened in a project directory — where the
/// shell runs starship, git and direnv — is not, and needs about 200ms.
/// Generous, because the cost of waiting is nothing and the cost of giving up
/// early is the whole operation.
const SHELL_WAIT: Duration = Duration::from_secs(5);
const SHELL_POLL: Duration = Duration::from_millis(100);

/// Start the agent once its pane is ready to receive it.
fn launch(herdr: &Herdr, pane: &Pane, session: &AgentSession, args: &[String]) -> Result<()> {
    let deadline = Instant::now() + SHELL_WAIT;
    loop {
        match launch_named(herdr, pane, session, args) {
            Err(err) if is_pane_busy(&err) && Instant::now() < deadline => {
                std::thread::sleep(SHELL_POLL);
            }
            result => return result,
        }
    }
}

fn is_pane_busy(err: &anyhow::Error) -> bool {
    herdr_plugin_kit::herdr::api_error(err).is_some_and(|api| api.code == "agent_pane_busy")
}

/// Start the agent, working around Herdr's unique-name requirement.
///
/// Three names are tried, in decreasing order of how well they read and
/// increasing order of how certainly they are free:
///
/// 1. `claude` — the common case, and what a lone agent should be called
/// 2. `claude-3ebd1a0d` — taken once a second conversation is resumed
/// 3. `claude-w2B-p3` — the pane's own id, which nothing else can hold, so
///    even resuming the *same* conversation a third time still works
fn launch_named(herdr: &Herdr, pane: &Pane, session: &AgentSession, args: &[String]) -> Result<()> {
    let kind = session.kind.agent();
    let short = sanitise(&session.id);
    let unique = sanitise(&pane.pane_id);

    let names = [
        kind.to_string(),
        format!("{kind}-{short}"),
        format!("{kind}-{unique}"),
    ];
    let last = names.len() - 1;

    for (attempt, name) in names.into_iter().enumerate() {
        match herdr.start_agent(&pane.pane_id, &name, kind, args) {
            Ok(()) => return Ok(()),
            Err(err) if attempt < last && is_name_taken(&err) => continue,
            Err(err) => return Err(err),
        }
    }
    unreachable!("the last attempt returns on both branches")
}

/// Cut `text` down to what Herdr accepts inside an agent name.
///
/// Herdr's rule, learned by being refused: lowercase letters, digits, `-` and
/// `_` only, 1–32 characters, starting with a lowercase letter. Pane ids like
/// `w2B:p3` fail it twice over, on the colon and on the capital.
fn sanitise(text: &str) -> String {
    text.chars()
        .flat_map(|ch| ch.to_lowercase())
        .map(|ch| {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .take(8)
        .collect()
}

fn is_name_taken(err: &anyhow::Error) -> bool {
    herdr_plugin_kit::herdr::api_error(err).is_some_and(|api| api.code == "agent_name_taken")
}

/// A tab or workspace name short enough to read on a tab bar.
fn label(heading: &str) -> String {
    const MAX: usize = 24;
    if heading.chars().count() <= MAX {
        return heading.to_string();
    }
    let cut: String = heading.chars().take(MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_stay_inside_what_herdr_accepts() {
        // Lowercase letters, digits, - and _ only; 1-32 characters; must start
        // with a lowercase letter. Every name we build is `<kind>-<sanitised>`.
        for raw in ["w2B:p3", "3ebd1a0d-395a", "W1V:pC", "ABC"] {
            let name = format!("claude-{}", sanitise(raw));
            assert!(name.len() <= 32, "{name} is too long");
            assert!(
                name.starts_with(|c: char| c.is_ascii_lowercase()),
                "{name} must start with a letter"
            );
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
                "{name} has a character Herdr refuses"
            );
        }
        assert_eq!(sanitise("w2B:p3"), "w2b-p3");
    }

    #[test]
    fn a_label_is_cut_on_a_character_boundary() {
        let long = "セッション一覧を表示して選択すると開くプラグインを作る";
        let cut = label(long);
        assert_eq!(cut.chars().count(), 24);
        assert!(cut.ends_with('…'));
        assert_eq!(label("short"), "short");
    }

    #[test]
    fn each_enter_key_asks_for_a_different_amount_of_room() {
        assert_eq!(Where::from_key(Key::Enter), Some(Where::Workspace));
        assert_eq!(Where::from_key(Key::ShiftEnter), Some(Where::Tab));
        assert_eq!(Where::from_key(Key::AltEnter), Some(Where::Split));
        assert_eq!(Where::from_key(Key::Esc), None);
    }

    #[test]
    fn the_modified_keys_are_the_ones_that_are_not_plain_enter() {
        assert!(!Where::MODIFIED.contains(&Key::Enter));
        for key in Where::MODIFIED {
            assert!(Where::from_key(key).is_some());
        }
    }

    #[test]
    fn every_placement_survives_a_round_trip_through_its_name() {
        // Alfred passes the placement as text in the item's argument, so the
        // names have to be exactly as parseable as the keys are.
        for placement in [Where::Workspace, Where::Tab, Where::Split] {
            assert_eq!(Where::parse(placement.name()), Some(placement));
        }
        assert_eq!(Where::parse("nonsense"), None);
    }

    #[test]
    fn the_default_placement_is_the_one_plain_enter_gives() {
        assert_eq!(Where::default(), Where::Workspace);
        assert_eq!(Where::from_key(Key::Enter), Some(Where::default()));
    }
}
