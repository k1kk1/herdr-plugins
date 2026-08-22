//! The palette itself: every plugin action in one filterable list.

use std::collections::HashMap;

use herdr_plugin_kit::herdr::{Herdr, Pane, PluginAction};
use herdr_plugin_kit::label;
use herdr_plugin_kit::ui::{menu, Chip, Key, Menu, Row, Term};
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

/// Which plugin the list is narrowed to.
///
/// Typing already filters, but a query has to be guessed at: "everything Pane
/// Manager can do" is a question about a plugin, not about a word, and the
/// answer changes as plugins are installed. Tab walks the plugins that
/// actually have actions right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Facet {
    /// Every plugin's actions, grouped under their plugin's name.
    All,
    /// One plugin's actions, with no headers to repeat its name.
    Only(String),
}

impl Facet {
    /// `All`, then one facet per plugin with actions, in the order the list
    /// shows them.
    pub fn all(actions: &[PluginAction]) -> Vec<Facet> {
        let mut out = vec![Facet::All];
        for action in actions {
            let facet = Facet::Only(action.plugin_id.clone());
            if !out.contains(&facet) {
                out.push(facet);
            }
        }
        out
    }

    fn holds(&self, action: &PluginAction) -> bool {
        match self {
            Facet::All => true,
            Facet::Only(plugin_id) => action.plugin_id == *plugin_id,
        }
    }

    fn name(&self, names: &HashMap<String, String>) -> String {
        match self {
            Facet::All => "All".to_string(),
            Facet::Only(plugin_id) => names
                .get(plugin_id)
                .cloned()
                .unwrap_or_else(|| plugin_id.clone()),
        }
    }
}

/// The facet after this one, wrapping round.
///
/// Wrapping matters: on the last plugin the useful answer is `All`, not
/// "nowhere".
pub fn next_facet(current: &Facet, facets: &[Facet]) -> Facet {
    let here = facets.iter().position(|facet| facet == current).unwrap_or(0);
    facets
        .get((here + 1) % facets.len().max(1))
        .cloned()
        .unwrap_or(Facet::All)
}

pub fn previous_facet(current: &Facet, facets: &[Facet]) -> Facet {
    let here = facets.iter().position(|facet| facet == current).unwrap_or(0);
    let back = if here == 0 { facets.len() } else { here };
    facets
        .get(back - 1)
        .cloned()
        .unwrap_or(Facet::All)
}

fn chips(current: &Facet, facets: &[Facet], names: &HashMap<String, String>) -> Vec<Chip> {
    std::iter::once(Chip::new("Tab ▸", false))
        .chain(
            facets
                .iter()
                .map(|facet| Chip::new(facet.name(names), facet == current)),
        )
        .collect()
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
    let facets = Facet::all(&actions);
    let mut facet = Facet::All;

    loop {
        let mut menu = build(&actions, &names, &facets, &facet, source);
        // Both keys close the menu so the next list is built cleanly rather
        // than by editing the one on screen.
        let mut switch_to = None;
        let chosen = menu.run_with(term, |key| match key {
            Key::Tab => {
                switch_to = Some(next_facet(&facet, &facets));
                menu::Interrupt::Close
            }
            Key::BackTab => {
                switch_to = Some(previous_facet(&facet, &facets));
                menu::Interrupt::Close
            }
            _ => menu::Interrupt::Unhandled,
        })?;

        match chosen {
            Some(picked) => return Ok(Some(picked)),
            None => match switch_to {
                Some(next) => facet = next,
                None => return Ok(None),
            },
        }
    }
}

fn build(
    actions: &[PluginAction],
    names: &HashMap<String, String>,
    facets: &[Facet],
    facet: &Facet,
    source: Option<&Pane>,
) -> Menu<(String, String)> {
    let subtitle = match source {
        Some(pane) => format!("on {}", label::pane_compact(pane)),
        None => "Type to filter".to_string(),
    };

    let mut menu = Menu::new("Command Palette")
        .subtitle(subtitle)
        .enter("run")
        .filterable()
        .numbered()
        .tab("plugin");
    if facets.len() > 1 {
        menu = menu.tabs(chips(facet, facets, names));
    }

    let mut current_plugin: Option<&str> = None;
    for action in actions.iter().filter(|action| facet.holds(action)) {
        let plugin_name = names
            .get(&action.plugin_id)
            .cloned()
            .unwrap_or_else(|| action.plugin_id.clone());

        // Narrowed to one plugin, the header would repeat the chip above it on
        // every screen and say nothing the reader has not just chosen.
        if *facet == Facet::All && current_plugin != Some(action.plugin_id.as_str()) {
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
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(plugin: &str, id: &str) -> PluginAction {
        PluginAction {
            plugin_id: plugin.into(),
            action_id: id.into(),
            title: id.into(),
            description: None,
            contexts: Vec::new(),
        }
    }

    fn sample() -> Vec<PluginAction> {
        vec![
            action("pane-manager", "move"),
            action("pane-manager", "swap"),
            action("navigator", "find"),
        ]
    }

    #[test]
    fn there_is_one_facet_per_plugin_that_has_actions() {
        // Built from the actions on screen, not from a list written here: a
        // plugin installed tomorrow gets its own chip for free, and one with
        // nothing to offer does not get an empty screen.
        assert_eq!(
            Facet::all(&sample()),
            [
                Facet::All,
                Facet::Only("pane-manager".into()),
                Facet::Only("navigator".into()),
            ]
        );
        assert_eq!(Facet::all(&[]), [Facet::All]);
    }

    #[test]
    fn tab_walks_the_facets_and_comes_back_round() {
        let facets = Facet::all(&sample());
        let mut facet = Facet::All;
        for _ in 0..facets.len() {
            facet = next_facet(&facet, &facets);
        }
        assert_eq!(facet, Facet::All, "the cycle must close");

        // Shift+Tab is the same walk backwards.
        assert_eq!(
            previous_facet(&Facet::All, &facets),
            Facet::Only("navigator".into())
        );
    }

    #[test]
    fn a_facet_keeps_only_its_own_plugins_actions() {
        let only = Facet::Only("pane-manager".into());
        let sample = sample();
        let kept: Vec<&str> = sample
            .iter()
            .filter(|action| only.holds(action))
            .map(|action| action.action_id.as_str())
            .collect();
        assert_eq!(kept, ["move", "swap"]);
        assert_eq!(sample.iter().filter(|a| Facet::All.holds(a)).count(), 3);
    }

    #[test]
    fn a_facet_is_named_after_the_plugin_rather_than_its_id() {
        let names: HashMap<String, String> =
            [("pane-manager".to_string(), "Pane Manager".to_string())]
                .into_iter()
                .collect();
        assert_eq!(Facet::Only("pane-manager".into()).name(&names), "Pane Manager");
        // Not installed, or nameless: the id is still better than a blank chip.
        assert_eq!(Facet::Only("mystery".into()).name(&names), "mystery");
        assert_eq!(Facet::All.name(&names), "All");
    }
}
