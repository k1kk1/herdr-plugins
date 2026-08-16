//! The palette itself: every plugin action in one filterable list.

use std::collections::HashMap;

use herdr_plugin_kit::herdr::{Herdr, Pane, PluginAction};
use herdr_plugin_kit::label;
use herdr_plugin_kit::ui::{Menu, Row, Term};
use herdr_plugin_kit::{context, ui as kit_ui, Result};

use crate::PLUGIN_ID;

/// Actions worth offering: everything except the palette's own entry, which
/// would only reopen the window the user is already looking at.
pub fn visible_actions(herdr: &Herdr) -> Result<Vec<PluginAction>> {
    let enabled: Vec<String> = herdr
        .installed_plugins()?
        .into_iter()
        .filter(|p| p.enabled)
        .map(|p| p.plugin_id)
        .collect();

    let mut actions: Vec<PluginAction> = herdr
        .plugin_actions()?
        .into_iter()
        .filter(|a| a.plugin_id != PLUGIN_ID)
        .filter(|a| enabled.contains(&a.plugin_id))
        .collect();

    // Group by plugin, then keep each plugin's declared action order.
    actions.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
    Ok(actions)
}

pub fn run(herdr: &Herdr, source: Option<Pane>) -> Result<()> {
    let mut term = Term::open()?;
    let result = pick(&mut term, herdr, source.as_ref());

    match result {
        Ok(Some((plugin_id, action_id))) => {
            // Close first: the action may open a popup of its own, and two
            // popups at once would fight over the screen.
            term.close();
            let payload = context::InvocationContext::from_env().to_params(source.as_ref());
            herdr.invoke_plugin_action(&plugin_id, &action_id, payload)
        }
        Ok(None) => {
            term.close();
            Ok(())
        }
        Err(err) => {
            let _ = kit_ui::show_error(&mut term, "Command Palette", &err);
            term.close();
            Err(err)
        }
    }
}

fn pick(
    term: &mut Term,
    herdr: &Herdr,
    source: Option<&Pane>,
) -> Result<Option<(String, String)>> {
    let actions = visible_actions(herdr)?;
    let names: HashMap<String, String> = herdr
        .installed_plugins()?
        .into_iter()
        .map(|p| (p.plugin_id, p.name))
        .collect();

    let subtitle = match source {
        Some(pane) => format!("on {}", label::pane_compact(pane)),
        None => "Type to filter".to_string(),
    };

    let mut menu = Menu::new("Command Palette")
        .subtitle(subtitle)
        .footer("↑↓ move · Enter run · Esc cancel")
        .filterable();

    let mut current_plugin: Option<&str> = None;
    for action in &actions {
        let plugin_name = names
            .get(&action.plugin_id)
            .cloned()
            .unwrap_or_else(|| action.plugin_id.clone());

        if current_plugin != Some(action.plugin_id.as_str()) {
            if current_plugin.is_some() {
                menu.row(Row::separator());
            }
            menu.row(Row::header(plugin_name.clone()));
            current_plugin = Some(&action.plugin_id);
        }

        let mut row = Row::item(action.title.clone());
        if let Some(description) = &action.description {
            row = row.detail(Some(description.clone()));
        }
        // The plugin name lives in the header, so `pane mo` still has to find
        // "Pane Manager: Move to Tab...".
        menu.item_matching(
            row,
            (action.plugin_id.clone(), action.action_id.clone()),
            &format!("{plugin_name} {}", action.action_id),
        );
    }

    if menu.is_empty() {
        menu.row(Row::note(
            "No other plugin exposes an action. Install one with `herdr plugin link`.",
        ));
    }

    menu.run(term)
}
