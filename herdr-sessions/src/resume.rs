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
pub fn anywhere(session: &AgentSession, config: &crate::open::Config) -> Result<()> {
    let outside = std::env::var_os("HERDR_ENV").is_none();

    match Herdr::connect() {
        Ok(herdr) => {
            let anchor = herdr_plugin_kit::context::resolve_source_pane(&herdr, None).ok();
            let outcome = resume(&herdr, session, config.resume_in, anchor.as_ref())?;
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
    herdr.start_agent(&pane.pane_id, session.kind.agent(), &args)?;

    let mut detail = format!("{} {}", session.kind.agent(), args.join(" "));
    if let Some(cwd) = &session.cwd {
        detail.push_str(&format!("\nin {}", cwd.display()));
    }
    Ok(
        Outcome::new(format!("Resumed “{heading}” {}", placement.describe()))
            .with_detail(detail),
    )
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
    fn the_default_placement_is_the_one_plain_enter_gives() {
        assert_eq!(Where::default(), Where::Workspace);
        assert_eq!(Where::from_key(Key::Enter), Some(Where::default()));
    }
}
