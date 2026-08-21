pub mod menu;
pub mod term;

pub use menu::{Interrupt, Menu};
pub use term::{Chip, Key, Preview, Row, RowKind, Term, View};

use crossterm::style::Color;

use crate::herdr::{AgentStatus, Pane};
use crate::label;

/// Colour for an agent state glyph.
pub fn status_color(status: AgentStatus) -> Color {
    match status {
        AgentStatus::Working => Color::Green,
        AgentStatus::Blocked => Color::Yellow,
        AgentStatus::Done => Color::Cyan,
        AgentStatus::Idle | AgentStatus::Unknown => Color::DarkGrey,
    }
}

/// Standard picker row for a pane: state glyph, name, and what it is doing.
///
/// Shared so a pane looks identical in Pane Manager, Navigator and any other
/// plugin that has to show one.
pub fn pane_row(pane: &Pane, show_state: bool, show_title: bool) -> Row {
    let mut row = Row::item(label::pane_compact(pane));
    if show_state {
        row = row
            .glyph(pane.agent_status.glyph(), status_color(pane.agent_status))
            .secondary(pane.agent_status.label().to_string());
    }
    if show_title {
        row = row.detail(label::pane_detail(pane));
    }
    row
}

/// Show a non-destructive error and wait for a key.
pub fn show_error(term: &mut Term, title: &str, err: &anyhow::Error) -> anyhow::Result<()> {
    let mut rows: Vec<Row> = err
        .to_string()
        .lines()
        .map(|line| Row::note(line.to_string()))
        .collect();
    // `anyhow` chains the underlying Herdr API error behind the friendly text.
    for cause in err.chain().skip(1) {
        rows.push(Row::separator());
        rows.push(Row::note(cause.to_string()));
    }

    let view = View::new(title)
        .subtitle("Nothing was left half-done.")
        .rows(rows)
        .accent(Color::Red)
        .footer("Press any key to close");
    term.render(&view)?;
    term.key()?;
    Ok(())
}
