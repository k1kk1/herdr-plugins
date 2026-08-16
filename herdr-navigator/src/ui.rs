//! The Navigator picker.
//!
//! One filterable list, four scopes. Typing narrows; Tab cycles the scope;
//! Enter jumps. Panes are grouped under their workspace and tab so the list
//! reads as a map of the session rather than a flat pile of names.

use herdr_plugin_kit::herdr::{Herdr, Pane};
use herdr_plugin_kit::label;
use herdr_plugin_kit::ui::{menu, Key, Menu, Row, Term};
use herdr_plugin_kit::{ui as kit_ui, Result};

/// What the picker lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Panes,
    /// Panes running a detected agent, which is usually what you are hunting.
    Agents,
    Tabs,
    Workspaces,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Panes => "panes",
            Scope::Agents => "agents",
            Scope::Tabs => "tabs",
            Scope::Workspaces => "workspaces",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Scope::Panes => "Go to pane",
            Scope::Agents => "Go to agent",
            Scope::Tabs => "Go to tab",
            Scope::Workspaces => "Go to workspace",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "panes" | "pane" => Some(Scope::Panes),
            "agents" | "agent" => Some(Scope::Agents),
            "tabs" | "tab" => Some(Scope::Tabs),
            "workspaces" | "workspace" => Some(Scope::Workspaces),
            _ => None,
        }
    }

    fn next(self) -> Self {
        match self {
            Scope::Panes => Scope::Agents,
            Scope::Agents => Scope::Tabs,
            Scope::Tabs => Scope::Workspaces,
            Scope::Workspaces => Scope::Panes,
        }
    }
}

pub fn run(herdr: &Herdr, scope: Scope, current: Option<Pane>) -> Result<()> {
    let mut term = Term::open()?;
    let result = pick(&mut term, herdr, scope, current.as_ref());

    match result {
        Ok(Some(target)) => {
            term.close();
            // Focus after the popup is gone, so the jump is what the user sees.
            jump(herdr, &target)
        }
        Ok(None) => {
            term.close();
            Ok(())
        }
        Err(err) => {
            let _ = kit_ui::show_error(&mut term, "Navigator", &err);
            term.close();
            Err(err)
        }
    }
}

/// Focus a pane, tab or workspace, whichever the id names.
fn jump(herdr: &Herdr, id: &str) -> Result<()> {
    match id.split_once(':').and_then(|(_, rest)| rest.chars().next()) {
        Some('p') => herdr.focus_pane(id),
        Some('t') => herdr.focus_tab(id),
        _ => herdr.focus_workspace(id),
    }
}

fn pick(
    term: &mut Term,
    herdr: &Herdr,
    mut scope: Scope,
    current: Option<&Pane>,
) -> Result<Option<String>> {
    loop {
        let mut menu = build(herdr, scope, current)?;

        // Tab closes the menu so the next scope's list can be built from
        // fresh Herdr state rather than from a stale in-memory copy.
        let mut switched = false;
        let selection = menu.run_with(term, |key| {
            if matches!(key, Key::Tab) {
                switched = true;
                menu::Interrupt::Close
            } else {
                menu::Interrupt::Unhandled
            }
        })?;

        if switched && selection.is_none() {
            scope = scope.next();
            continue;
        }
        return Ok(selection);
    }
}

fn build(herdr: &Herdr, scope: Scope, current: Option<&Pane>) -> Result<Menu<String>> {
    let mut menu = Menu::new(scope.title())
        .subtitle("Type to filter · Tab changes what is listed")
        .footer("type to filter · 1-9 jump · ↑↓ move · Enter jump · Tab scope · Esc cancel")
        .filterable()
        .numbered();

    let workspaces = herdr.workspaces()?;

    match scope {
        Scope::Workspaces => {
            for workspace in &workspaces {
                let name = workspace.label.clone().unwrap_or_else(|| "Workspace".into());
                let mut row = Row::item(name).glyph(
                    workspace.agent_status.glyph(),
                    kit_ui::status_color(workspace.agent_status),
                );
                if workspace.focused {
                    row = row.secondary("current");
                }
                menu.item(row, workspace.workspace_id.clone());
            }
        }
        Scope::Tabs => {
            for workspace in &workspaces {
                let tabs = herdr.tabs(&workspace.workspace_id)?;
                if tabs.is_empty() {
                    continue;
                }
                let workspace_name =
                    workspace.label.clone().unwrap_or_else(|| "Workspace".into());
                menu.row(Row::header(workspace_name.clone()));
                for (index, tab) in tabs.iter().enumerate() {
                    let panes = if tab.pane_count == 1 { "pane" } else { "panes" };
                    let row = Row::item(label::tab_display(tab, index + 1))
                        .glyph(tab.agent_status.glyph(), kit_ui::status_color(tab.agent_status))
                        .secondary(format!("{} {panes}", tab.pane_count));
                    // The workspace name is only in the header, so make it
                    // searchable on the row itself.
                    menu.item_matching(row, tab.tab_id.clone(), &workspace_name);
                }
                menu.row(Row::separator());
            }
        }
        Scope::Panes | Scope::Agents => {
            for workspace in &workspaces {
                let tabs = herdr.tabs(&workspace.workspace_id)?;
                let panes = herdr.panes(&workspace.workspace_id)?;
                let workspace_name =
                    workspace.label.clone().unwrap_or_else(|| "Workspace".into());

                for (index, tab) in tabs.iter().enumerate() {
                    let in_tab: Vec<&Pane> = panes
                        .iter()
                        .filter(|p| p.tab_id == tab.tab_id)
                        .filter(|p| scope != Scope::Agents || p.agent.is_some())
                        .collect();
                    if in_tab.is_empty() {
                        continue;
                    }

                    let tab_name = label::tab_display(tab, index + 1);
                    menu.row(Row::header(format!("{workspace_name} · {tab_name}")));
                    for pane in in_tab {
                        let mut row = kit_ui::pane_row(pane, true, true);
                        if current.is_some_and(|c| c.pane_id == pane.pane_id) {
                            row = row.secondary("current");
                        }
                        menu.item_matching(
                            row,
                            pane.pane_id.clone(),
                            &format!("{workspace_name} {tab_name}"),
                        );
                    }
                    menu.row(Row::separator());
                }
            }
        }
    }

    if menu.is_empty() {
        menu.row(Row::note(match scope {
            Scope::Agents => "No agent is running in any pane.",
            _ => "Nothing to show.",
        }));
    }

    Ok(menu)
}
