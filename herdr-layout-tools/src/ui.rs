//! The Layout Tools menu.
//!
//! One screen: pick an arrangement, or equalize what is already there.
//! Everything runs immediately — these are non-destructive layout changes, so
//! there is nothing to confirm.

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::{Herdr, Pane};
use herdr_plugin_kit::label;
use herdr_plugin_kit::ui::{Menu, Row, Term};
use herdr_plugin_kit::{Outcome, Result};

use crate::arrange::{Arrangement, Shape};
use crate::ops;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Equalize,
    Zoom,
    Arrange(Arrangement),
    Cancel,
}

pub fn run(herdr: &Herdr, source: Pane, tab_override: Option<&str>) -> Result<Option<Outcome>> {
    let tab_id = context::resolve_source_tab(tab_override, &source);
    let mut term = Term::open()?;
    let result = menu(&mut term, herdr, &source, &tab_id);

    // Report failures inside the popup, where the user is still looking.
    match result {
        Ok(outcome) => {
            term.close();
            Ok(outcome)
        }
        Err(err) => {
            let _ = herdr_plugin_kit::ui::show_error(&mut term, "Layout Tools", &err);
            term.close();
            Err(err)
        }
    }
}

fn menu(
    term: &mut Term,
    herdr: &Herdr,
    source: &Pane,
    tab_id: &str,
) -> Result<Option<Outcome>> {
    let layout = herdr.layout(tab_id)?;
    let tab = herdr.tab(tab_id)?;
    let panes = layout.root.pane_ids();
    let current = Shape::from_layout(&layout.root);

    let mut menu = Menu::new("Layout Tools")
        .subtitle(format!(
            "{} · {} pane{}",
            tab.label.as_deref().unwrap_or("this tab"),
            panes.len(),
            if panes.len() == 1 { "" } else { "s" }
        ))
        .footer("↑↓ move · Enter select · q / Esc cancel");

    menu.item(
        Row::item("Equalize")
            .hotkey("e")
            .secondary("give every pane the same space"),
        Choice::Equalize,
    );
    menu.item(
        Row::item("Zoom current pane")
            .hotkey("z")
            .secondary(label::pane_compact(source)),
        Choice::Zoom,
    );

    menu.row(Row::separator());
    menu.row(Row::header("Arrange"));
    for arrangement in Arrangement::ALL {
        // Mark the arrangement the tab is already in, so the menu doubles as
        // a read-out of the current layout.
        let applied = current.as_ref().is_some_and(|shape| {
            arrangement
                .plan(&panes, Some(&source.pane_id))
                .is_some_and(|plan| plan.simulate() == *shape)
        });
        let note = if applied {
            "current".to_string()
        } else {
            arrangement.description().to_string()
        };
        menu.item(
            Row::item(arrangement.title())
                .hotkey(arrangement.hotkey())
                .secondary(note),
            Choice::Arrange(arrangement),
        );
    }

    menu.row(Row::separator());
    menu.item(Row::item("Cancel").hotkey("q"), Choice::Cancel);

    let Some(choice) = menu.run(term)? else {
        return Ok(None);
    };

    match choice {
        Choice::Cancel => Ok(None),
        Choice::Equalize => ops::equalize(herdr, tab_id).map(Some),
        Choice::Zoom => {
            herdr.zoom_pane(&source.pane_id, "toggle")?;
            Ok(Some(Outcome::new(format!(
                "Toggled zoom on \"{}\"",
                label::pane_compact(source)
            ))))
        }
        Choice::Arrange(arrangement) => {
            ops::arrange(herdr, tab_id, arrangement, Some(&source.pane_id)).map(Some)
        }
    }
}
