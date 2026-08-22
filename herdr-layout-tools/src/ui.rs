//! The Layout Tools menu.
//!
//! One screen: pick an arrangement, or equalize what is already there.
//! Everything runs immediately — these are non-destructive layout changes, so
//! there is nothing to confirm.

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::{Herdr, Pane};
use herdr_plugin_kit::label;
use herdr_plugin_kit::ui::{Menu, Panel, Row, Term};
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

#[derive(Debug, Clone)]
struct PreviewPane {
    id: String,
    number: String,
    label: String,
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
    let preview_panes = preview_panes(herdr, source, &panes);
    let current = Shape::from_layout(&layout.root);
    // Zoom is a toggle, so its preview has to describe the state *after* the
    // key is taken, rather than merely showing the current split tree.
    let zoom_preview = if layout.zoomed {
        current
            .clone()
            .map(|shape| (shape, vec![source.pane_id.clone()]))
    } else {
        Some((Shape::pane(&source.pane_id), vec![source.pane_id.clone()]))
    };
    let zoom_current = if layout.zoomed {
        Some(Shape::pane(&source.pane_id))
    } else {
        current.clone()
    };
    let pane_legend = preview_panes
        .iter()
        .map(|pane| format!("{} {}", pane.number, short_label(&pane.label)))
        .collect::<Vec<_>>()
        .join("  ·  ");

    let mut menu = Menu::new("Layout Tools")
        .subtitle(format!(
            "{} · {} pane{}{}",
            tab.label.as_deref().unwrap_or("this tab"),
            panes.len(),
            if panes.len() == 1 { "" } else { "s" },
            if pane_legend.is_empty() {
                String::new()
            } else {
                format!(" · {pane_legend}")
            }
        ));

    menu.item(
        Row::item("Equalize")
            .hotkey("e")
            .secondary("すべての Pane を同じ大きさに")
            .panels(transition_panels(
                current.as_ref(),
                current
                    .clone()
                    .map(|shape| (shape, vec![source.pane_id.clone()])),
                &preview_panes,
                &source.pane_id,
                "均等化後",
            )),
        Choice::Equalize,
    );
    menu.item(
        Row::item("Zoom current pane")
            .hotkey("z")
            .secondary(if layout.zoomed {
                "分割表示へ戻す".to_string()
            } else {
                label::pane_compact(source)
            })
            .panels(transition_panels(
                zoom_current.as_ref(),
                zoom_preview,
                &preview_panes,
                &source.pane_id,
                "実行後",
            )),
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
                    .secondary(note)
                    .panels(transition_panels(
                        current.as_ref(),
                        arrangement_preview(arrangement, &panes, &source.pane_id),
                        &preview_panes,
                        &source.pane_id,
                        "実行後",
                    )),
                Choice::Arrange(arrangement),
            );
    }

    let saved = template::load();
    if !saved.is_empty() {
        menu.row(Row::separator());
        menu.row(Row::header("Saved"));
        for (name, layout) in &saved {
            // A layout only fits a tab with the same number of panes, so say
            // so up front rather than failing after the user picks it.
            let note = if layout.slots() == panes.len() {
                layout.describe()
            } else {
                format!("{} — needs {} here", layout.describe(), panes.len())
            };
            menu.item(
                Row::item(name.clone())
                    .secondary(note)
                    .panels(transition_panels(
                        current.as_ref(),
                        saved_preview(layout, &panes, &source.pane_id),
                        &preview_panes,
                        &source.pane_id,
                        "実行後",
                    )),
                Choice::Apply(name.clone()),
            );
        }
    }

    menu.row(Row::separator());
    menu.item(
        Row::item("Save this layout")
            .hotkey("s")
            .secondary("今の形に名前を付けて覚える")
            .panels(current_panel(
                current.as_ref(),
                &preview_panes,
                &source.pane_id,
                "保存する形",
            )),
        Choice::Save,
    );
    if !saved.is_empty() {
        menu.item(
            Row::item("Delete a saved layout").hotkey("d"),
            Choice::Forget,
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

/// The panes in the tab, numbered in layout order. The number is stable while
/// the menu is open and remains legible even in a small preview cell.
fn preview_panes(herdr: &Herdr, source: &Pane, pane_ids: &[String]) -> Vec<PreviewPane> {
    let panes = herdr.panes(&source.workspace_id).unwrap_or_default();
    pane_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let number = (index + 1).to_string();
            let pane = panes
                .iter()
                .find(|pane| pane.pane_id == *id)
                .or_else(|| (source.pane_id == *id).then_some(source));
            let label = pane
                .map(label::pane_primary)
                .unwrap_or_else(|| format!("Pane {number}"));
            PreviewPane {
                id: id.clone(),
                number,
                label,
            }
        })
        .collect()
}

/// Draw one real tab before and after an operation, using the same pane labels
/// on both sides. This is the same visual grammar Pane Manager uses for a pane
/// crossing between tabs; here the arrow describes one tab changing shape.
fn transition_panels(
    current: Option<&Shape>,
    after: Option<(Shape, Vec<String>)>,
    panes: &[PreviewPane],
    current_pane: &str,
    after_caption: &str,
) -> Vec<Panel> {
    let (Some(current), Some((after, marked))) = (current, after) else {
        return Vec::new();
    };
    let labels: Vec<(String, String)> = panes
        .iter()
        .map(|pane| (pane.id.clone(), pane.number.clone()))
        .collect();
    vec![
        Panel::new("現在", current.clone())
            .marking(vec![current_pane.to_string()])
            .labeling(labels.clone()),
        Panel::new(after_caption, after)
            .marking(marked)
            .labeling(labels),
    ]
}

/// A single unchanged layout, for Save. Showing an identical right-hand panel
/// would imply that Save rearranges the tab even though it only writes a file.
fn current_panel(
    current: Option<&Shape>,
    panes: &[PreviewPane],
    current_pane: &str,
    caption: &str,
) -> Vec<Panel> {
    let Some(current) = current else {
        return Vec::new();
    };
    let labels = panes
        .iter()
        .map(|pane| (pane.id.clone(), pane.number.clone()))
        .collect();
    vec![Panel::new(caption, current.clone())
        .marking(vec![current_pane.to_string()])
        .labeling(labels)]
}

fn short_label(label: &str) -> String {
    const LIMIT: usize = 12;
    let mut text: String = label.chars().take(LIMIT).collect();
    if label.chars().nth(LIMIT).is_some() {
        text.push('…');
    }
    text
}

/// Diagram the target arrangement before any pane is moved.
///
/// `Plan::simulate` is also the shape that `ops::rebuild` produces, so the
/// picker cannot drift into showing a decorative diagram that differs from
/// the shortcuts' actual result.
fn arrangement_preview(
    arrangement: Arrangement,
    panes: &[String],
    current_pane: &str,
) -> Option<(Shape, Vec<String>)> {
    arrangement
        .plan(panes, Some(current_pane))
        .map(|plan| (plan.simulate(), vec![current_pane.to_string()]))
}

/// Diagram a saved layout with this tab's panes filled into its slots.
///
/// A mismatched saved layout has no valid after-state; the row's existing
/// "needs N here" note remains the useful explanation in that case.
fn saved_preview(
    layout: &template::Template,
    panes: &[String],
    current_pane: &str,
) -> Option<(Shape, Vec<String>)> {
    layout
        .plan(panes)
        .ok()
        .map(|plan| (plan.simulate(), vec![current_pane.to_string()]))
}

/// Ask what to call the layout being saved.
///
/// The picker's own query line doubles as the text field: whatever is typed
/// becomes the name, the same way Pane Manager names a new tab.
fn ask_name(term: &mut Term) -> Result<Option<String>> {
    let existing = template::load();
    let mut menu: Menu<String> = Menu::new("Save this layout as")
        .subtitle("この Tab の分割の形を、名前を付けて覚えます")
        .prompt("type a name")
        .enter("save")
        .filterable();

    menu.item_pinned(Row::item("Save as {query}").hotkey("↵"), String::new());
    if !existing.is_empty() {
        menu.row(Row::separator());
        menu.row(Row::header("Replace an existing one"));
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
    let mut menu: Menu<String> = Menu::new("Delete a saved layout")
        .subtitle("選んだ保存済みレイアウトを削除します。今の画面は変わりません")
        .numbered();
    for (name, layout) in &saved {
        menu.item(
            Row::item(name.clone()).secondary(layout.describe()),
            name.clone(),
        );
    }
    menu.run(term)
}
