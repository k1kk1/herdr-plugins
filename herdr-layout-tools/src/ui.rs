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
use crate::template;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Choice {
    Equalize,
    Zoom,
    Arrange(Arrangement),
    Apply(String),
    Save,
    Forget,
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

    let mut menu = Menu::new("レイアウト")
        .subtitle(format!(
            "{} · {} pane{}",
            tab.label.as_deref().unwrap_or("this tab"),
            panes.len(),
            if panes.len() == 1 { "" } else { "s" }
        ));

    menu.item(
        Row::item("均等")
            .hotkey("e")
            .secondary("すべての Pane を同じ大きさに"),
        Choice::Equalize,
    );
    menu.item(
        Row::item("現在の Pane を最大化")
            .hotkey("z")
            .secondary(label::pane_compact(source)),
        Choice::Zoom,
    );

    menu.row(Row::separator());
    menu.row(Row::header("並べ方"));
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

    let saved = template::load();
    if !saved.is_empty() {
        menu.row(Row::separator());
        menu.row(Row::header("保存済み"));
        for (name, layout) in &saved {
            // A layout only fits a tab with the same number of panes, so say
            // so up front rather than failing after the user picks it.
            let note = if layout.slots() == panes.len() {
                layout.describe()
            } else {
                format!("{} — needs {} here", layout.describe(), panes.len())
            };
            menu.item(
                Row::item(name.clone()).secondary(note),
                Choice::Apply(name.clone()),
            );
        }
    }

    menu.row(Row::separator());
    menu.item(
        Row::item("この配置を保存")
            .hotkey("s")
            .secondary("今の形に名前を付けて覚える"),
        Choice::Save,
    );
    if !saved.is_empty() {
        menu.item(
            Row::item("保存済み配置を削除").hotkey("d"),
            Choice::Forget,
        );
    }

    menu.row(Row::separator());
    menu.item(Row::item("やめる").hotkey("q"), Choice::Cancel);

    let Some(choice) = menu.run(term)? else {
        return Ok(None);
    };

    match choice {
        Choice::Cancel => Ok(None),
        Choice::Equalize => ops::equalize(herdr, tab_id).map(Some),
        Choice::Zoom => ops::zoom(herdr, source).map(Some),
        Choice::Arrange(arrangement) => {
            ops::arrange(herdr, tab_id, arrangement, Some(&source.pane_id)).map(Some)
        }
        Choice::Apply(name) => ops::apply_layout(herdr, tab_id, &name).map(Some),
        Choice::Save => match ask_name(term)? {
            Some(name) => ops::save_layout(herdr, tab_id, &name).map(Some),
            None => Ok(None),
        },
        Choice::Forget => match pick_saved(term)? {
            Some(name) => ops::forget_layout(&name).map(Some),
            None => Ok(None),
        },
    }
}


/// Ask what to call the layout being saved.
///
/// The picker's own query line doubles as the text field: whatever is typed
/// becomes the name, the same way Pane Manager names a new tab.
fn ask_name(term: &mut Term) -> Result<Option<String>> {
    let existing = template::load();
    let mut menu: Menu<String> = Menu::new("この配置に名前を付けて保存")
        
        .prompt("名前を入力")
        .enter("保存")
        .filterable();

    menu.item_pinned(Row::item("{query} として保存").hotkey("↵"), String::new());
    if !existing.is_empty() {
        menu.row(Row::separator());
        menu.row(Row::header("既存を置き換える"));
        for (name, layout) in &existing {
            menu.item(
                Row::item(name.clone()).secondary(layout.describe()),
                name.clone(),
            );
        }
    }

    let Some(chosen) = menu.run(term)? else {
        return Ok(None);
    };
    // The pinned row carries an empty value, meaning "use what was typed".
    let name = if chosen.is_empty() {
        menu.query().trim().to_string()
    } else {
        chosen
    };
    Ok((!name.is_empty()).then_some(name))
}

/// Choose a saved layout to delete.
fn pick_saved(term: &mut Term) -> Result<Option<String>> {
    let saved = template::load();
    let mut menu: Menu<String> = Menu::new("どの保存済み配置を削除しますか？")
        .numbered();
    for (name, layout) in &saved {
        menu.item(
            Row::item(name.clone()).secondary(layout.describe()),
            name.clone(),
        );
    }
    menu.run(term)
}
