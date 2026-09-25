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

use crate::arrange::{Arrangement, LayoutSpec, Shape};
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
    let result = match std::env::var("LT_UI_MODE").as_deref() {
        Ok("save") => save_current(&mut term, herdr, &tab_id),
        Ok("saved") => saved_menu(&mut term, herdr, &source, &tab_id),
        _ => menu(&mut term, herdr, &source, &tab_id),
    };

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

fn save_current(term: &mut Term, herdr: &Herdr, tab_id: &str) -> Result<Option<Outcome>> {
    match ask_name(term)? {
        Some(name) => ops::save_layout(herdr, tab_id, &name).map(Some),
        None => Ok(None),
    }
}

/// Focused entry point for the "Saved Layouts" action. The generic menu still
/// includes this section; this view avoids making that action open above an
/// unrelated Equalize row.
fn saved_menu(
    term: &mut Term,
    herdr: &Herdr,
    source: &Pane,
    tab_id: &str,
) -> Result<Option<Outcome>> {
    let layout = herdr.layout(tab_id)?;
    let panes = layout.root.pane_ids();
    let current = Shape::from_layout(&layout.root);
    let preview_panes = preview_panes(herdr, source, &panes);
    let (saved, warning) = template::load_reporting();
    let mut menu = Menu::new("Saved Layouts")
        .subtitle("保存した形をこの Tab に適用します")
        .no_preview("この項目は Pane の配置を変えません");

    if let Some(warning) = warning {
        menu.row(Row::note(warning));
    }
    if saved.is_empty() {
        menu.row(Row::note("保存済みレイアウトはありません"));
    }
    for (name, saved_layout) in &saved {
        let note = if saved_layout.slots() == panes.len() {
            saved_layout.describe()
        } else {
            format!(
                "{} — needs {} here",
                saved_layout.describe(),
                panes.len()
            )
        };
        if saved_layout.slots() != panes.len() {
            menu.row(Row::note(name.clone()).secondary(note));
            continue;
        }
        menu.item(
            illustrated(
                Row::item(name.clone()).secondary(note),
                transition_panels(
                    current.as_ref(),
                    saved_preview(saved_layout, &panes, &source.pane_id),
                    &preview_panes,
                    &source.pane_id,
                    "実行後",
                ),
                &preview_panes,
                &source.pane_id,
            ),
            Choice::Apply(name.clone()),
        );
    }

    menu.row(Row::separator());
    menu.item(Row::item("Save this layout").hotkey("s"), Choice::Save);
    if !saved.is_empty() {
        menu.item(
            Row::item("Delete a saved layout").hotkey("d"),
            Choice::Forget,
        );
    }
    menu.item(Row::item("Cancel").hotkey("q"), Choice::Cancel);

    let Some(choice) = menu.run(term)? else {
        return Ok(None);
    };
    match choice {
        Choice::Apply(name) => ops::apply_layout(herdr, tab_id, &name).map(Some),
        Choice::Save => save_current(term, herdr, tab_id),
        Choice::Forget => match pick_saved(term)? {
            Some(name) => ops::forget_layout(&name).map(Some),
            None => Ok(None),
        },
        Choice::Cancel => Ok(None),
        _ => Ok(None),
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
    let (zoom_current, zoom_preview) =
        zoom_previews(current.as_ref(), &source.pane_id, layout.zoomed);
    let mut menu = Menu::new("Layout Tools")
        .subtitle(format!(
            "{} · {} pane{}",
            tab.label.as_deref().unwrap_or("this tab"),
            panes.len(),
            if panes.len() == 1 { "" } else { "s" }
        ))
        .no_preview("この操作は Pane の配置を変えません");

    let equalized = current
        .as_ref()
        .map(LayoutSpec::equalized_shape)
        .map(|spec| (spec.shape, vec![source.pane_id.clone()]));
    menu.item(
        illustrated(
            Row::item("Equalize")
                .hotkey("e")
                .secondary("すべての Pane を同じ大きさに"),
            transition_panels(
                current.as_ref(),
                equalized,
                &preview_panes,
                &source.pane_id,
                "均等化後",
            ),
            &preview_panes,
            &source.pane_id,
        ),
        Choice::Equalize,
    );
    menu.item(
        illustrated(
            Row::item("Zoom current pane")
                .hotkey("z")
                .secondary(if layout.zoomed {
                    "分割表示へ戻す".to_string()
                } else {
                    label::pane_compact(source)
                }),
            transition_panels(
                zoom_current.as_ref(),
                zoom_preview,
                &preview_panes,
                &source.pane_id,
                "実行後",
            ),
            &preview_panes,
            &source.pane_id,
        ),
        Choice::Zoom,
    );

    menu.row(Row::separator());
    menu.row(Row::header("Arrange"));
    for arrangement in Arrangement::ALL {
        // Mark the arrangement the tab is already in, so the menu doubles as
        // a read-out of the current layout.
        let applied = current.as_ref().is_some_and(|shape| {
            arrangement
                .spec(&panes, Some(&source.pane_id))
                .is_some_and(|spec| spec.matches(shape, 0.03))
        });
        let note = if applied {
            "current".to_string()
        } else {
            arrangement.description().to_string()
        };
        menu.item(
            illustrated(
                Row::item(arrangement.title())
                    .hotkey(arrangement.hotkey())
                    .secondary(note),
                transition_panels(
                    current.as_ref(),
                    arrangement_preview(arrangement, &panes, &source.pane_id),
                    &preview_panes,
                    &source.pane_id,
                    "実行後",
                ),
                &preview_panes,
                &source.pane_id,
            ),
            Choice::Arrange(arrangement),
        );
    }

    let (saved, saved_warning) = template::load_reporting();
    if let Some(warning) = saved_warning {
        menu.row(Row::note(warning));
    }
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
            if layout.slots() != panes.len() {
                menu.row(Row::note(name.clone()).secondary(note));
                continue;
            }
            menu.item(
                illustrated(
                    Row::item(name.clone()).secondary(note),
                    transition_panels(
                        current.as_ref(),
                        saved_preview(layout, &panes, &source.pane_id),
                        &preview_panes,
                        &source.pane_id,
                        "実行後",
                    ),
                    &preview_panes,
                    &source.pane_id,
                ),
                Choice::Apply(name.clone()),
            );
        }
    }

    menu.row(Row::separator());
    menu.item(
        illustrated(
            Row::item("Save this layout")
                .hotkey("s")
                .secondary("今の形に名前を付けて覚える"),
            current_panel(
                current.as_ref(),
                &preview_panes,
                &source.pane_id,
                "保存する形",
            ),
            &preview_panes,
            &source.pane_id,
        ),
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

/// The visible state before and after toggling Zoom.
///
/// `layout.export` always carries the full split tree, even while one pane is
/// zoomed. Turning that tree directly into the left panel made Zoom-out look
/// unchanged, so the current side must collapse to the pane that is actually
/// filling the screen.
fn zoom_previews(
    full: Option<&Shape>,
    current_pane: &str,
    zoomed: bool,
) -> (Option<Shape>, Option<(Shape, Vec<String>)>) {
    let marked = vec![current_pane.to_string()];
    if zoomed {
        (
            Some(Shape::pane(current_pane)),
            full.cloned().map(|shape| (shape, marked)),
        )
    } else {
        (
            full.cloned(),
            Some((Shape::pane(current_pane), marked)),
        )
    }
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

fn illustrated(
    row: Row,
    panels: Vec<Panel>,
    panes: &[PreviewPane],
    current_pane: &str,
) -> Row {
    let legend = panes
        .iter()
        .map(|pane| {
            let marker = if pane.id == current_pane {
                format!("{} ", herdr_plugin_kit::layout::HIGHLIGHT)
            } else {
                "  ".to_string()
            };
            format!("{marker}{}: {}", pane.number, short_label(&pane.label))
        })
        .collect();
    row.panels(panels).legend(legend)
}

/// Diagram the target arrangement before any pane is moved.
///
/// The complete spec is also what `ops::arrange` applies, including ratios.
fn arrangement_preview(
    arrangement: Arrangement,
    panes: &[String],
    current_pane: &str,
) -> Option<(Shape, Vec<String>)> {
    arrangement
        .spec(panes, Some(current_pane))
        .map(|spec| (spec.shape, vec![current_pane.to_string()]))
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
        .spec(panes)
        .ok()
        .map(|spec| (spec.shape, vec![current_pane.to_string()]))
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

#[cfg(test)]
mod preview_tests {
    use super::*;

    fn panes() -> Vec<PreviewPane> {
        vec![
            PreviewPane {
                id: "p1".into(),
                number: "1".into(),
                label: "Codex".into(),
            },
            PreviewPane {
                id: "p2".into(),
                number: "2".into(),
                label: "Claude".into(),
            },
            PreviewPane {
                id: "p3".into(),
                number: "3".into(),
                label: "Shell".into(),
            },
        ]
    }

    fn current() -> Shape {
        let mut shape = Shape::pane("p1");
        shape.split("p1", "p2", herdr_plugin_kit::layout::Side::Right);
        shape.split("p2", "p3", herdr_plugin_kit::layout::Side::Down);
        shape
    }

    #[test]
    fn an_arrangement_preview_shows_current_then_the_real_planned_result() {
        let ids = vec!["p1".to_string(), "p2".to_string(), "p3".to_string()];
        let after = arrangement_preview(Arrangement::Rows, &ids, "p2");
        let panels = transition_panels(Some(&current()), after, &panes(), "p2", "実行後");

        assert_eq!(panels.len(), 2);
        assert_eq!(panels[0].caption, "現在");
        assert_eq!(panels[1].caption, "実行後");
        assert_eq!(
            panels[0].shape.as_ref().map(Shape::signature).unwrap(),
            "(r p1 (d p2 p3))"
        );
        assert_eq!(
            panels[1].shape.as_ref().map(Shape::signature).unwrap(),
            "(d p1 (d p2 p3))"
        );
        assert_eq!(panels[0].labels, panels[1].labels);
        assert_eq!(panels[0].marked, vec!["p2".to_string()]);
        assert_eq!(panels[1].marked, vec!["p2".to_string()]);
    }

    #[test]
    fn main_preview_shows_the_same_half_width_the_operation_applies() {
        let ids = vec![
            "p1".to_string(),
            "p2".to_string(),
            "p3".to_string(),
        ];
        let (shape, _) = arrangement_preview(Arrangement::MainLeft, &ids, "p2").unwrap();
        let Shape::Split { ratio, second, .. } = shape else {
            panic!("main preview must have a root split");
        };
        assert_eq!(ratio, 0.5);
        let Shape::Split {
            ratio: secondary_ratio,
            ..
        } = *second
        else {
            panic!("secondary panes must be split");
        };
        assert_eq!(secondary_ratio, 0.5);
    }

    #[test]
    fn equalize_preview_changes_a_nested_chains_visible_ratio() {
        let before = current();
        let after = LayoutSpec::equalized_shape(&before);
        let Shape::Split { ratio, .. } = after.shape else {
            panic!("preview must have a root split");
        };
        assert!((ratio - 1.0 / 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_preview_uses_the_state_that_is_really_visible_on_each_side() {
        let full = current();
        let (before, after) = zoom_previews(Some(&full), "p2", false);
        assert_eq!(before.as_ref().map(Shape::signature).unwrap(), full.signature());
        assert_eq!(after.unwrap().0.signature(), "p2");

        let (before, after) = zoom_previews(Some(&full), "p2", true);
        assert_eq!(before.unwrap().signature(), "p2");
        assert_eq!(after.unwrap().0.signature(), full.signature());
    }

    #[test]
    fn saving_draws_one_unchanged_panel_instead_of_a_fake_transition() {
        let panels = current_panel(Some(&current()), &panes(), "p1", "保存する形");
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].caption, "保存する形");
        assert_eq!(panels[0].marked, vec!["p1".to_string()]);
    }

    #[test]
    fn an_invalid_saved_layout_draws_no_result() {
        let layout = template::Template::Slot;
        let ids = vec!["p1".to_string(), "p2".to_string(), "p3".to_string()];
        let after = saved_preview(&layout, &ids, "p1");
        assert!(transition_panels(Some(&current()), after, &panes(), "p1", "実行後").is_empty());
    }
}
