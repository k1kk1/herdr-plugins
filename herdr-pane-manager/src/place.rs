//! Remembering where a pane was, and putting it back.
//!
//! Both Gather and Undo need the same thing: a description of a pane's place
//! that survives the pane being moved somewhere else, and a way to replay it.
//! Recording has to happen *before* anything moves, because afterwards the
//! information is gone.
//!
//! A tab that emptied out while its panes were away is recreated under the
//! original name, in the original slot — Herdr closes a tab the moment its last
//! pane leaves, so "the tab is gone" is a normal outcome, not a failure.

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::layout::{Plan, Shape, Side};
use herdr_plugin_kit::Result;
use serde::{Deserialize, Serialize};

/// Where one pane was.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Origin {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// Label of the original tab, so it can be recreated under its own name.
    #[serde(default)]
    pub tab_label: Option<String>,
    /// 0-based slot of the original tab, so a recreated tab goes back in place.
    #[serde(default)]
    pub tab_index: usize,
    /// Pane it sat next to, and on which side. Absent when it was alone in
    /// its tab, which is also what tells the restore the tab has to be recreated.
    #[serde(default)]
    pub anchor: Option<String>,
    #[serde(default)]
    pub side: Option<String>,
    /// Position among the panes taken from the same tab, so they are put back
    /// in an order where each one's anchor already exists.
    #[serde(default)]
    pub order: usize,
    #[serde(default)]
    pub focused: bool,
}

impl Origin {
    pub fn side(&self) -> Side {
        self.side
            .as_deref()
            .and_then(Side::parse)
            .unwrap_or(Side::Right)
    }
}

/// Describe where a pane is now, in enough detail to put it back.
pub fn origin_of(herdr: &Herdr, pane_id: &str) -> Result<Origin> {
    let pane = herdr.pane(pane_id)?;
    let tab = herdr.tab(&pane.tab_id)?;
    let tab_index = herdr
        .tabs(&pane.workspace_id)?
        .iter()
        .position(|t| t.tab_id == pane.tab_id)
        .unwrap_or(0);

    // Reconstruct the pane's place in its tab's split tree: which pane it sits
    // beside, on which side, and how far down the rebuild order it comes.
    let shape = herdr
        .layout(&pane.tab_id)
        .ok()
        .and_then(|l| Shape::from_layout(&l.root));
    let (anchor, side, order) = match &shape {
        Some(shape) => {
            let plan = Plan::from_shape(shape);
            match plan
                .placements
                .iter()
                .position(|p| p.pane_id == pane_id)
                .map(|index| (&plan.placements[index], index))
            {
                Some((placement, index)) => (
                    Some(placement.anchor.clone()),
                    Some(placement.side.as_str().to_string()),
                    index,
                ),
                // The tab's root pane: nothing anchors it.
                None => (None, None, 0),
            }
        }
        None => (None, None, 0),
    };

    Ok(Origin {
        pane_id: pane.pane_id,
        workspace_id: pane.workspace_id,
        tab_id: pane.tab_id,
        tab_label: tab
            .label
            .filter(|l| !l.trim().chars().all(|c| c.is_ascii_digit())),
        tab_index,
        anchor,
        side,
        order,
        focused: pane.focused,
    })
}

/// Record several panes at once, skipping any that cannot be described.
///
/// A pane we cannot describe is one we could not put back, so leaving it out is
/// safer than recording a half-truth.
pub fn origins_of(herdr: &Herdr, panes: &[String]) -> Vec<Origin> {
    panes
        .iter()
        .filter_map(|pane_id| origin_of(herdr, pane_id).ok())
        .collect()
}

/// What a restore actually did.
#[derive(Debug, Default, Clone, Copy)]
pub struct Restored {
    /// Panes that went back into a tab that was still open.
    pub into_original: usize,
    /// Tabs that had to be recreated because Herdr closed them when their last
    /// pane left. They come back under the original name, in the original slot.
    pub recreated_tabs: usize,
    /// Panes that went into one of those recreated tabs.
    pub into_recreated: usize,
    /// Panes that no longer exist — closed while they were away. Nothing can be
    /// done for them, and they must not stop the rest going home.
    pub gone: usize,
}

impl Restored {
    pub fn total(self) -> usize {
        self.into_original + self.into_recreated
    }

    /// A sentence describing the outcome, for the user.
    pub fn detail(self, expected: usize) -> String {
        let panes = |n: usize| if n == 1 { "pane" } else { "panes" };
        let placed = self.total();
        let mut detail = format!("{placed} {} back in place", panes(placed));
        if self.recreated_tabs > 0 {
            detail.push_str(&format!(
                " · {} {} rebuilt under {} original name{}",
                self.recreated_tabs,
                if self.recreated_tabs == 1 { "tab" } else { "tabs" },
                if self.recreated_tabs == 1 { "its" } else { "their" },
                if self.recreated_tabs == 1 { "" } else { "s" }
            ));
        }
        if self.gone > 0 {
            detail.push_str(&format!(
                " · {} {} closed since",
                self.gone,
                panes(self.gone)
            ));
        }
        let unaccounted = expected.saturating_sub(placed + self.gone);
        if unaccounted > 0 {
            detail.push_str(&format!(" · {unaccounted} could not be moved"));
        }
        detail
    }
}

/// Origins grouped by the tab they came from, each group in the order the panes
/// have to be put back so that every anchor exists by the time it is used.
pub fn by_original_tab(origins: &[Origin]) -> Vec<(String, Vec<Origin>)> {
    let mut groups: Vec<(String, Vec<Origin>)> = Vec::new();
    for origin in origins {
        match groups.iter_mut().find(|(tab, _)| *tab == origin.tab_id) {
            Some((_, panes)) => panes.push(origin.clone()),
            None => groups.push((origin.tab_id.clone(), vec![origin.clone()])),
        }
    }
    for (_, panes) in &mut groups {
        panes.sort_by_key(|o| o.order);
    }
    groups
}

/// Move panes back where they came from.
pub fn restore(herdr: &Herdr, origins: &[Origin]) -> Result<Restored> {
    let mut restored = Restored::default();

    for (tab_id, panes) in by_original_tab(origins) {
        let tab_alive = herdr.tab(&tab_id).is_ok();
        let mut destination = tab_alive.then(|| tab_id.clone());

        for origin in &panes {
            // A pane closed since the record was written cannot be put back,
            // and trying would abort the restore for everything after it.
            if herdr.pane(&origin.pane_id).is_err() {
                restored.gone += 1;
                continue;
            }

            let anchor_alive = origin
                .anchor
                .as_deref()
                .map(|anchor| herdr.pane(anchor).is_ok())
                .unwrap_or(false);

            match &destination {
                Some(target) => {
                    let anchor = origin.anchor.as_deref().filter(|_| anchor_alive);
                    herdr.move_pane_to_tab(
                        &origin.pane_id,
                        target,
                        anchor,
                        origin.side().split(),
                        false,
                    )?;
                    if origin.side().needs_swap() {
                        if let Some(anchor) = anchor {
                            let _ = herdr.swap_panes_in_tab(&origin.pane_id, anchor);
                        }
                    }
                    if tab_alive {
                        restored.into_original += 1;
                    } else {
                        restored.into_recreated += 1;
                    }
                }
                // First pane of a vanished tab: recreate the tab around it.
                None => {
                    let result = herdr.move_pane_to_new_tab(
                        &origin.pane_id,
                        origin.tab_label.as_deref(),
                        false,
                    )?;
                    if let Some(created) = result.created_tab {
                        let _ = herdr.move_tab(&created.tab_id, origin.tab_index);
                        destination = Some(created.tab_id);
                        restored.recreated_tabs += 1;
                        restored.into_recreated += 1;
                    }
                }
            }
        }
    }

    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(pane: &str, tab: &str, order: usize) -> Origin {
        Origin {
            pane_id: pane.into(),
            workspace_id: "w1".into(),
            tab_id: tab.into(),
            tab_label: Some("Agents".into()),
            tab_index: 0,
            anchor: Some("w1:p1".into()),
            side: Some("right".into()),
            order,
            focused: false,
        }
    }

    #[test]
    fn origins_group_by_the_tab_they_came_from() {
        let origins = vec![
            origin("p2", "w1:t1", 1),
            origin("p9", "w1:t3", 0),
            origin("p3", "w1:t1", 0),
        ];
        let groups = by_original_tab(&origins);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "w1:t1");
        // Within a tab, restore order is the order they were taken in.
        assert_eq!(
            groups[0]
                .1
                .iter()
                .map(|o| o.pane_id.as_str())
                .collect::<Vec<_>>(),
            ["p3", "p2"]
        );
        assert_eq!(groups[1].0, "w1:t3");
    }

    #[test]
    fn a_clean_restore_reads_as_a_plain_sentence() {
        let restored = Restored {
            into_original: 3,
            ..Restored::default()
        };
        assert_eq!(restored.detail(3), "3 panes back in place");
    }

    #[test]
    fn a_rebuilt_tab_is_reported_as_success_not_failure() {
        let restored = Restored {
            into_recreated: 2,
            recreated_tabs: 1,
            ..Restored::default()
        };
        assert_eq!(
            restored.detail(2),
            "2 panes back in place · 1 tab rebuilt under its original name"
        );
    }

    #[test]
    fn panes_closed_since_the_record_are_reported_honestly() {
        let restored = Restored {
            into_original: 2,
            gone: 1,
            ..Restored::default()
        };
        assert_eq!(
            restored.detail(3),
            "2 panes back in place · 1 pane closed since"
        );
    }

    #[test]
    fn a_missing_side_falls_back_rather_than_failing() {
        let origin: Origin = serde_json::from_str(
            r#"{"pane_id":"p1","workspace_id":"w1","tab_id":"w1:t1"}"#,
        )
        .unwrap();
        assert_eq!(origin.side(), Side::Right);
        assert!(origin.anchor.is_none());
    }
}
