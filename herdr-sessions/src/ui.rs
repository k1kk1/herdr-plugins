//! The Sessions picker.
//!
//! One list of every session Herdr knows about, running and stopped together,
//! because the thing you are looking for is a *name* and you rarely remember
//! which of the two it currently is. Running ones sort first and the session
//! you are in sits at the top, marked, so the list also answers "where am I".
//!
//! Tab switches between opening a session and managing one, the same gesture
//! Navigator uses to change what it lists. Opening is the common path, so it
//! stays one keystroke.

use herdr_plugin_kit::herdr::{Herdr, Pane};
use herdr_plugin_kit::ui::{menu, Chip, Key, Menu, Row, Term};
use herdr_plugin_kit::{ui as kit_ui, Outcome, Result};

use crate::agents::{self, AgentSession, Kind};
use crate::open::Config;
use crate::resume;
use crate::session::{self, Detail, Session};

use crossterm::style::Color;

/// What the picker is listing, and what Enter does to it.
///
/// Two families under one Tab key, because "which session was that" is the
/// same question whether the answer is a Herdr session or a conversation you
/// had with an agent last Tuesday.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Herdr sessions. Enter opens one in a new terminal window.
    Open,
    /// Herdr sessions. Enter offers stop and delete.
    Manage,
    /// Past conversations with every tool at once. Enter resumes one.
    Agents,
    /// Past Claude Code conversations. Enter resumes one.
    Claude,
    /// Past Codex conversations. Enter resumes one.
    Codex,
}

/// The two families of thing this picker lists.
///
/// They are separate axes, and conflating them is what made a single Tab cycle
/// confusing: pressing Tab in the conversation list to narrow the tool would
/// eventually land you back in Herdr's own sessions, which is not narrowing
/// anything. So Tab varies the *facet* within a family, and Shift+Tab changes
/// the family.
const SESSION_FACETS: [Mode; 2] = [Mode::Open, Mode::Manage];
const AGENT_FACETS: [Mode; 3] = [Mode::Agents, Mode::Claude, Mode::Codex];

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Open => "open",
            Mode::Manage => "manage",
            Mode::Agents => "all",
            Mode::Claude => "claude",
            Mode::Codex => "codex",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" | "attach" => Some(Mode::Open),
            "manage" => Some(Mode::Manage),
            "all" | "agents" | "resume" => Some(Mode::Agents),
            "claude" => Some(Mode::Claude),
            "codex" => Some(Mode::Codex),
            _ => None,
        }
    }

    /// Whether this mode lists agent transcripts rather than Herdr sessions.
    fn lists_agents(self) -> bool {
        matches!(self, Mode::Agents | Mode::Claude | Mode::Codex)
    }

    /// The single tool this mode is narrowed to, if it is narrowed at all.
    fn agent_kind(self) -> Option<Kind> {
        match self {
            Mode::Claude => Some(Kind::Claude),
            Mode::Codex => Some(Kind::Codex),
            _ => None,
        }
    }

    /// The facets of this mode's own family, in Tab order.
    fn facets(self) -> &'static [Mode] {
        if self.lists_agents() {
            &AGENT_FACETS
        } else {
            &SESSION_FACETS
        }
    }

    /// Tab: the next facet of the same family.
    ///
    /// For conversations that is the tool filter — all, then Claude, then
    /// Codex — and it never leaves the conversation list.
    fn next_facet(self) -> Self {
        let facets = self.facets();
        let at = facets.iter().position(|m| *m == self).unwrap_or(0);
        facets[(at + 1) % facets.len()]
    }

    /// Shift+Tab: the other family, at its widest view.
    ///
    /// Always the widest rather than whatever was last looked at, so the key
    /// does the same thing every time it is pressed.
    fn other_family(self) -> Self {
        if self.lists_agents() {
            Mode::Open
        } else {
            Mode::Agents
        }
    }

    fn title(self) -> &'static str {
        match self {
            Mode::Open => "Open a session",
            Mode::Manage => "Manage sessions",
            Mode::Agents => "Resume a conversation",
            Mode::Claude => "Resume a Claude Code session",
            Mode::Codex => "Resume a Codex session",
        }
    }

    /// The line under the title.
    ///
    /// The facets themselves are drawn as a strip rather than described here;
    /// this line only has to say what Enter does and what the other key is
    /// for. Naming the facets in prose as well would say the same thing twice,
    /// and the prose is the half nobody reads.
    fn subtitle(self) -> String {
        let what = match self {
            Mode::Open => "Opens in a new terminal window",
            Mode::Manage => "Stop or delete a session",
            Mode::Agents | Mode::Claude | Mode::Codex => "Resumes here, not in a new window",
        };
        format!("{what} · Shift+Tab for {}", self.other_family().family())
    }

    /// The facet strip: every facet of this family, the current one marked.
    ///
    /// `Tab ▸` leads it so the strip explains its own key without a legend.
    fn chips(self) -> Vec<Chip> {
        std::iter::once(Chip::new("Tab ▸", false))
            .chain(
                self.facets()
                    .iter()
                    .map(|facet| Chip::new(facet.name(), *facet == self)),
            )
            .collect()
    }

    /// What the family is called, for the Shift+Tab hint.
    fn family(self) -> &'static str {
        if self.lists_agents() {
            "conversations"
        } else {
            "herdr sessions"
        }
    }
}

pub fn run(
    herdr: &Herdr,
    mut mode: Mode,
    config: &Config,
    anchor: Option<Pane>,
    warning: Option<String>,
) -> Result<()> {
    let mut term = Term::open()?;

    let outcome = loop {
        // Rebuilt on every pass rather than cached: a session can start or
        // stop, and a transcript can gain a turn, while the picker is open.
        let listing = match Listing::gather(herdr, mode, config) {
            Ok(listing) => listing,
            Err(err) => {
                let _ = kit_ui::show_error(&mut term, mode.title(), &err);
                term.close();
                return Err(err);
            }
        };

        let mut menu = listing.menu(mode, warning.as_deref(), term.distinguishes_modified_enter());
        // Both keys close the menu so the next list is built from fresh state
        // rather than from an in-memory copy that may already be stale.
        let mut switch_to = None;
        let chosen = menu.run_with(&mut term, |key| match key {
            Key::Tab => {
                switch_to = Some(mode.next_facet());
                menu::Interrupt::Close
            }
            Key::BackTab => {
                switch_to = Some(mode.other_family());
                menu::Interrupt::Close
            }
            _ => menu::Interrupt::Unhandled,
        })?;

        let Some(id) = chosen else {
            if let Some(next) = switch_to {
                mode = next;
                continue;
            }
            term.close();
            return Ok(());
        };

        let result = match &listing {
            Listing::Herdr(sessions) => match sessions.iter().find(|s| s.name == id) {
                Some(session) if mode == Mode::Manage => manage(&mut term, session),
                Some(session) => open(&mut term, config, session),
                None => continue,
            },
            Listing::Agents(sessions) => match sessions.iter().find(|s| s.id == id) {
                Some(session) => {
                    // Which Enter took the row decides how much room the
                    // conversation gets; the config only sets what plain
                    // Enter means.
                    let placement = match menu.accepted_with() {
                        Key::Enter => config.resume_in,
                        key => resume::Where::from_key(key).unwrap_or(config.resume_in),
                    };
                    resume::resume(herdr, session, placement, anchor.as_ref()).map(Some)
                }
                None => continue,
            },
        };

        match result {
            // The action asked to go back to the list.
            Ok(None) => continue,
            Ok(Some(outcome)) => break outcome,
            Err(err) => {
                let _ = kit_ui::show_error(&mut term, mode.title(), &err);
                term.close();
                return Err(err);
            }
        }
    };

    // Report after the popup is gone, so the toast is not drawn over.
    term.close();
    outcome.report(herdr);
    Ok(())
}

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

/// The two kinds of thing this picker lists.
enum Listing {
    Herdr(Vec<Session>),
    Agents(Vec<AgentSession>),
}

impl Listing {
    fn gather(herdr: &Herdr, mode: Mode, config: &Config) -> Result<Self> {
        if !mode.lists_agents() {
            return session::list().map(Listing::Herdr);
        }
        match mode.agent_kind() {
            Some(kind) => agents::list(kind, config.recent(), Some(herdr)),
            None => agents::list_all(config.recent(), Some(herdr)),
        }
        .map(Listing::Agents)
    }

    /// The menu, whose value is the name (Herdr) or id (agent) of a row.
    fn menu(&self, mode: Mode, warning: Option<&str>, modified_enter: bool) -> Menu<String> {
        let mut menu = Menu::new(mode.title())
            .subtitle(mode.subtitle())
            .tabs(mode.chips())
            .footer(footer(mode, modified_enter))
            .filterable()
            .numbered();

        if matches!(self, Listing::Agents(_)) {
            menu = menu.accept_also(&resume::Where::MODIFIED);
        }

        if let Some(warning) = warning {
            menu.row(Row::note(warning.to_string()));
            menu.row(Row::separator());
        }

        match self {
            Listing::Herdr(sessions) => {
                for session in sessions {
                    let detail = session::detail(session);
                    menu.item_matching(
                        row(session, &detail),
                        session.name.clone(),
                        &detail.names.join(" "),
                    );
                }
                if menu.is_empty() {
                    menu.row(Row::note("No sessions yet."));
                    menu.row(Row::note(
                        "`herdr --session <name>` starts one; it will show up here.",
                    ));
                }
            }
            Listing::Agents(sessions) => {
                // Naming the tool on every row is noise when the list is one
                // tool already, and the point of the list when it is not.
                let show_tool = mode.agent_kind().is_none();
                for session in sessions {
                    menu.item_matching(
                        agent_row(session, show_tool),
                        session.id.clone(),
                        &session.searchable(),
                    );
                }
                if menu.is_empty() {
                    menu.row(Row::note("No conversations recorded yet."));
                }
            }
        }
        menu
    }
}

/// The key line at the bottom.
///
/// The agent modes spell out all three Enter keys, but only where the terminal
/// can actually tell them apart — advertising Shift+Enter somewhere it arrives
/// as plain Enter would be a lie the user only discovers by trying it.
fn footer(mode: Mode, modified_enter: bool) -> String {
    let common = "type to filter · 1-9 pick · ↑↓ move";
    match (mode.lists_agents(), modified_enter) {
        (true, true) => format!(
            "{common} · Enter new workspace · Shift+Enter new tab · Opt+Enter split · Esc cancel"
        ),
        (true, false) => {
            format!("{common} · Enter new workspace · Opt+Enter split · Esc cancel")
        }
        _ => format!("{common} · Enter choose · Esc cancel"),
    }
}

fn agent_row(session: &AgentSession, show_tool: bool) -> Row {
    let mut trailing = vec![session::ago(session.modified)];
    if session.open {
        // Deliberately hedged. Herdr does not expose the agent's session id,
        // so this is a directory-and-title match, not an identity.
        trailing.push("looks open".into());
    }

    let mut second = vec![session.where_line()];
    if let Some(context) = session.context_line() {
        second.push(context);
    }

    let mut row = Row::item(session.heading())
        .secondary(trailing.join(" · "))
        .detail(Some(second.join("  —  ")));
    if show_tool {
        // A column of its own on the right: the headings stay flush left
        // where they are read, and the tool still reads straight down.
        row = row.trailing(session.kind.tag());
    }
    row
}

fn row(session: &Session, detail: &Detail) -> Row {
    let (glyph, color) = if session.running {
        // Reuse the agent palette so a busy session reads the same as a busy
        // pane does everywhere else in the plugin set.
        if detail.busy > 0 {
            ('●', Color::Green)
        } else {
            ('●', Color::Cyan)
        }
    } else {
        ('○', Color::DarkGrey)
    };

    let mut trailing = vec![session.state().to_string()];
    if !session.running {
        if let Some(time) = detail.last_used {
            trailing.push(session::ago(time));
        }
    }
    if session.default {
        // The session a bare `herdr` attaches to, which is worth marking
        // because it is the one you get without asking for it.
        trailing.push("default".into());
    }
    if session.is_current() {
        trailing.push("you are here".into());
    }

    let mut second = vec![detail.summary()];
    if let Some(names) = detail.names_line() {
        second.push(names);
    }

    Row::item(session.name.clone())
        .glyph(glyph, color)
        .secondary(trailing.join(" · "))
        .detail(Some(second.join("  —  ")))
}

// ---------------------------------------------------------------------------
// What Enter does
// ---------------------------------------------------------------------------

/// `Ok(None)` means "go back to the list".
fn open(term: &mut Term, config: &Config, session: &Session) -> Result<Option<Outcome>> {
    if session.is_current() {
        // Opening the session you are already in would give you a second
        // window onto the same server, which is never what Enter meant here.
        note(
            term,
            "Already here",
            &format!("`{}` is the session this pane is in.", session.name),
        )?;
        return Ok(None);
    }

    let argv = crate::open::open(config, &session.name)?;
    Ok(Some(
        Outcome::new(format!("Opening `{}` in a new window", session.name))
            .with_detail(argv.join(" ")),
    ))
}

fn manage(term: &mut Term, session: &Session) -> Result<Option<Outcome>> {
    let mut menu: Menu<Action> = Menu::new(format!("Session `{}`", session.name))
        .subtitle(session.state())
        .footer("s / d · Esc back");

    if session.running {
        menu.item(
            Row::item("Stop")
                .hotkey("s")
                .secondary("panes keep their state and come back on attach"),
            Action::Stop,
        );
    } else {
        menu.item(
            Row::item("Delete")
                .hotkey("d")
                .secondary("discards the saved layout for good"),
            Action::Delete,
        );
    }

    let Some(action) = menu.run(term)? else {
        return Ok(None);
    };

    match action {
        Action::Stop => {
            if !confirm(term, &format!("Stop `{}`?", session.name))? {
                return Ok(None);
            }
            cli(&["session", "stop", &session.name])?;
            Ok(Some(Outcome::new(format!("Stopped `{}`", session.name)).with_detail(
                "Its layout is saved; attaching brings the session back.",
            )))
        }
        Action::Delete => {
            if !confirm(
                term,
                &format!("Delete `{}` and its saved layout?", session.name),
            )? {
                return Ok(None);
            }
            cli(&["session", "delete", &session.name])?;
            Ok(Some(Outcome::new(format!("Deleted `{}`", session.name))))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Stop,
    Delete,
}

/// Run a `herdr session` subcommand, surfacing what it said if it refuses.
fn cli(args: &[&str]) -> Result<()> {
    let output = std::process::Command::new(session::herdr_bin())
        .args(args)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    herdr_plugin_kit::bail!(
        "{}",
        stderr
            .trim()
            .lines()
            .next()
            .unwrap_or("the command failed with no output")
    )
}

fn confirm(term: &mut Term, question: &str) -> Result<bool> {
    let mut menu = Menu::new(question).footer("y / n · Esc cancel");
    menu.item(Row::item("Yes").hotkey("y"), true);
    menu.item(Row::item("No").hotkey("n"), false);
    Ok(menu.run(term)?.unwrap_or(false))
}

/// A dead-end message the user acknowledges, for a choice that was a no-op.
fn note(term: &mut Term, title: &str, body: &str) -> Result<()> {
    let mut menu: Menu<()> = Menu::new(title).footer("Enter back");
    menu.row(Row::note(body.to_string()));
    menu.run(term)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_never_leaves_the_family_it_started_in() {
        // The whole point of splitting the axes: narrowing the tool filter
        // must not eventually dump you into Herdr's session list.
        let mut mode = Mode::Agents;
        for _ in 0..AGENT_FACETS.len() * 2 {
            mode = mode.next_facet();
            assert!(mode.lists_agents(), "{mode:?} left the conversation list");
        }
        let mut mode = Mode::Open;
        for _ in 0..SESSION_FACETS.len() * 2 {
            mode = mode.next_facet();
            assert!(!mode.lists_agents(), "{mode:?} left the session list");
        }
    }

    #[test]
    fn tab_visits_every_facet_and_closes_the_cycle() {
        for start in [Mode::Agents, Mode::Open] {
            let facets = start.facets();
            let mut seen = vec![start];
            let mut mode = start;
            for _ in 0..facets.len() - 1 {
                mode = mode.next_facet();
                assert!(!seen.contains(&mode), "{mode:?} came round twice");
                seen.push(mode);
            }
            assert_eq!(mode.next_facet(), start, "the cycle must close");
            assert_eq!(seen.len(), facets.len());
        }
    }

    #[test]
    fn shift_tab_swaps_families_and_is_its_own_inverse() {
        assert_eq!(Mode::Agents.other_family(), Mode::Open);
        assert_eq!(Mode::Open.other_family(), Mode::Agents);
        // Pressed from a narrowed view it still lands on the other family's
        // widest one, so the key means one thing wherever it is used.
        assert_eq!(Mode::Codex.other_family(), Mode::Open);
        assert_eq!(Mode::Manage.other_family(), Mode::Agents);
    }

    fn strip(mode: Mode) -> String {
        mode.chips()
            .iter()
            .map(|c| {
                if c.active {
                    format!("[{}]", c.label)
                } else {
                    c.label.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_strip_marks_exactly_the_facet_that_is_showing() {
        assert_eq!(strip(Mode::Agents), "Tab ▸ [all] claude codex");
        assert_eq!(strip(Mode::Codex), "Tab ▸ all claude [codex]");
        assert_eq!(strip(Mode::Open), "Tab ▸ [open] manage");
        for mode in [Mode::Open, Mode::Manage, Mode::Agents, Mode::Claude, Mode::Codex] {
            assert_eq!(
                mode.chips().iter().filter(|c| c.active).count(),
                1,
                "{mode:?} must highlight exactly one facet"
            );
        }
    }

    #[test]
    fn the_subtitle_points_at_the_other_family() {
        assert!(Mode::Agents.subtitle().contains("Shift+Tab for herdr sessions"));
        assert!(Mode::Open.subtitle().contains("Shift+Tab for conversations"));
    }

    #[test]
    fn the_combined_view_lists_agents_without_naming_one() {
        // `agent_kind` is "narrowed to a single tool", not "lists agents" —
        // conflating them would make the All view fall back to Herdr sessions.
        assert!(Mode::Agents.lists_agents());
        assert!(Mode::Agents.agent_kind().is_none());

        assert_eq!(Mode::Claude.agent_kind(), Some(Kind::Claude));
        assert_eq!(Mode::Codex.agent_kind(), Some(Kind::Codex));

        for mode in [Mode::Open, Mode::Manage] {
            assert!(!mode.lists_agents());
            assert!(mode.agent_kind().is_none());
        }
    }

    #[test]
    fn all_is_reachable_by_every_name_the_manifest_uses() {
        for name in ["all", "agents", "resume"] {
            assert_eq!(Mode::parse(name), Some(Mode::Agents), "{name}");
        }
    }

    #[test]
    fn modes_parse_from_the_manifest_argument() {
        assert_eq!(Mode::parse("open"), Some(Mode::Open));
        assert_eq!(Mode::parse("Manage"), Some(Mode::Manage));
        assert_eq!(Mode::parse("claude"), Some(Mode::Claude));
        assert_eq!(Mode::parse("codex"), Some(Mode::Codex));
        assert_eq!(Mode::parse("nonsense"), None);
    }
}
