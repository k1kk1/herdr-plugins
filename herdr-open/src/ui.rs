//! The Open menu.
//!
//! One screen: the directory at the top, the ways to open it underneath.
//! Nothing here changes the session — no pane moves, nothing is focused — so
//! there is nothing to confirm and nothing to undo.

use std::path::PathBuf;

use herdr_plugin_kit::herdr::Pane;
use herdr_plugin_kit::label;
use herdr_plugin_kit::ui::{menu, Key, Menu, Row, Term};
use herdr_plugin_kit::{bail, Outcome, Result};

use crate::dir;
use crate::target::{Config, Target};

pub fn run(pane: &Pane, config: &Config) -> Result<Option<Outcome>> {
    let cwd = dir::pane_dir(pane)?;
    let root = dir::git_root(&cwd);

    let mut term = Term::open()?;
    let result = pick(&mut term, pane, config, &cwd, root.clone());

    match result {
        Ok(Some((target, chosen))) => {
            term.close();
            // Launch after the popup is gone: on macOS the new window has to
            // come up over the session, not over a popup that is closing.
            target.run(&chosen).map(Some)
        }
        Ok(None) => {
            term.close();
            Ok(None)
        }
        Err(err) => {
            let _ = herdr_plugin_kit::ui::show_error(&mut term, "Open", &err);
            term.close();
            Err(err)
        }
    }
}

/// The chosen target and the directory it should act on.
type Choice = (Target, PathBuf);

fn pick(
    term: &mut Term,
    pane: &Pane,
    config: &Config,
    cwd: &PathBuf,
    root: Option<PathBuf>,
) -> Result<Option<Choice>> {
    let targets = config.available();
    if targets.is_empty() {
        bail!(
            "None of the configured targets are installed.\n\
             Add one that is:\n\n  herdr plugin config-dir {}",
            crate::PLUGIN_ID
        );
    }

    // Which directory the rows act on. `g` swaps it; the list itself does not
    // change, so the user is choosing one thing, not picking from two lists.
    let mut using_root = config.prefer_git_root && root.is_some();

    loop {
        let chosen = match (&root, using_root) {
            (Some(root), true) => root.clone(),
            _ => cwd.clone(),
        };
        let mut menu = build(pane, &targets, &chosen, root.is_some(), using_root);

        let mut toggled = false;
        let selection = menu.run_with(term, |key| {
            // `g` is reserved for the toggle, and only while there is
            // something to toggle to.
            if root.is_some() && key == Key::Char('g') {
                toggled = true;
                menu::Interrupt::Close
            } else {
                menu::Interrupt::Unhandled
            }
        })?;

        if toggled && selection.is_none() {
            using_root = !using_root;
            continue;
        }
        return Ok(selection.map(|target| (target, chosen)));
    }
}

/// Footer text, per the shared key conventions (docs/ui-conventions.md).
///
/// This menu takes no query, so `q` closes it and is advertised.
fn footer(has_root: bool) -> &'static str {
    if has_root {
        "↑↓ move · Enter open · g git root · q / Esc cancel"
    } else {
        "↑↓ move · Enter open · q / Esc cancel"
    }
}

fn build(
    pane: &Pane,
    targets: &[Target],
    chosen: &PathBuf,
    has_root: bool,
    using_root: bool,
) -> Menu<Target> {
    let mut menu = Menu::new("Open")
        .subtitle(format!(
            "{}{}",
            dir::tilde(chosen),
            if using_root { " · repository root" } else { "" }
        ))
        .footer(footer(has_root));

    for target in targets {
        let mut row = Row::item(target.title.clone());
        if let Some(hotkey) = &target.hotkey {
            // Letters rather than numbers, as in Layout Tools: this list is
            // short and fixed, so `f` always means Finder. Numbering is for
            // the lists that grow (docs/ui-conventions.md).
            row = row.hotkey(hotkey.clone());
        }
        if let Some(description) = &target.description {
            row = row.secondary(description.clone());
        }
        menu.item(row, target.clone());
    }

    menu.row(Row::separator());
    menu.row(Row::note(format!("from {}", label::pane_compact(pane))));
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        Pane {
            cwd: Some("/Users/x/src/herdr-plugins/herdr-open".into()),
            ..Default::default()
        }
    }

    #[test]
    fn every_available_target_gets_a_row() {
        let config = Config::default();
        let targets = config.available();
        let menu = build(
            &pane(),
            &targets,
            &PathBuf::from("/Users/x/src"),
            false,
            false,
        );
        assert_eq!(menu.is_empty(), targets.is_empty());
    }

    #[test]
    fn the_git_root_key_is_only_advertised_when_there_is_one() {
        assert!(footer(true).contains("g git root"));
        assert!(!footer(false).contains("git root"));
    }
}
