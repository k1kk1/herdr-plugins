//! Standard Gather layouts (addendum §4, §5, §6).
//!
//! A Gather tab holds two, three or four agent panes in a fixed arrangement,
//! filled in priority order. These are deliberately not the Layout Tools
//! arrangements: Gather always puts the most urgent agent in the largest or
//! first slot, which is a different rule from "make a balanced grid".

use herdr_plugin_kit::layout::{Placement, Plan, Side};

/// How many agent panes go in one Gather tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanesPerTab(u8);

impl PanesPerTab {
    pub const DEFAULT: PanesPerTab = PanesPerTab(4);

    /// Only 2, 3 and 4 have a defined layout.
    pub fn new(value: u8) -> Option<Self> {
        matches!(value, 2 | 3 | 4).then_some(PanesPerTab(value))
    }

    pub fn get(self) -> usize {
        self.0 as usize
    }

    pub const ALL: [PanesPerTab; 3] = [PanesPerTab(2), PanesPerTab(3), PanesPerTab(4)];
}

impl Default for PanesPerTab {
    fn default() -> Self {
        PanesPerTab::DEFAULT
    }
}

/// Split `panes` into per-tab groups, keeping priority order.
///
/// The last group takes the remainder, so seven agents at four per tab become
/// four and three rather than four, two and one.
pub fn chunk(panes: &[String], per_tab: PanesPerTab) -> Vec<Vec<String>> {
    panes
        .chunks(per_tab.get())
        .map(|chunk| chunk.to_vec())
        .collect()
}

/// The arrangement for one Gather tab, in priority order.
///
/// * 2 — side by side.
/// * 3 — the top agent down the left, the other two stacked on the right.
/// * 4 — a 2×2 grid filled left to right, top to bottom.
///
/// One pane needs no plan; anything larger than four is not produced by
/// [`chunk`], but is handled as a row so nothing is ever dropped.
pub fn plan(panes: &[String]) -> Option<Plan> {
    let anchor = panes.first()?.clone();
    let placements = match panes.len() {
        0 | 1 => Vec::new(),
        2 => vec![place(&panes[1], &panes[0], Side::Right)],
        3 => vec![
            place(&panes[1], &panes[0], Side::Right),
            place(&panes[2], &panes[1], Side::Down),
        ],
        // The top-right pane is split off before either column is divided, or
        // it would only span half the tab's height.
        4 => vec![
            place(&panes[1], &panes[0], Side::Right),
            place(&panes[2], &panes[0], Side::Down),
            place(&panes[3], &panes[1], Side::Down),
        ],
        _ => panes
            .windows(2)
            .map(|pair| place(&pair[1], &pair[0], Side::Right))
            .collect(),
    };
    Some(Plan { anchor, placements })
}

fn place(pane_id: &str, anchor: &str, side: Side) -> Placement {
    Placement {
        pane_id: pane_id.to_string(),
        anchor: anchor.to_string(),
        side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("a{i}")).collect()
    }

    fn shape(n: usize) -> String {
        plan(&ids(n)).unwrap().simulate().signature()
    }

    #[test]
    fn only_two_three_and_four_are_valid_sizes() {
        assert_eq!(PanesPerTab::new(4).map(PanesPerTab::get), Some(4));
        assert_eq!(PanesPerTab::new(2).map(PanesPerTab::get), Some(2));
        assert_eq!(PanesPerTab::new(1), None);
        assert_eq!(PanesPerTab::new(5), None);
        assert_eq!(PanesPerTab::default().get(), 4);
    }

    #[test]
    fn seven_agents_at_four_per_tab_become_four_and_three() {
        let groups = chunk(&ids(7), PanesPerTab::new(4).unwrap());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], ["a1", "a2", "a3", "a4"]);
        assert_eq!(groups[1], ["a5", "a6", "a7"]);
    }

    #[test]
    fn chunking_preserves_priority_order() {
        let groups = chunk(&ids(6), PanesPerTab::new(2).unwrap());
        let flat: Vec<String> = groups.into_iter().flatten().collect();
        assert_eq!(flat, ids(6));
    }

    #[test]
    fn two_panes_sit_side_by_side() {
        assert_eq!(shape(2), "(r a1 a2)");
    }

    #[test]
    fn three_panes_give_the_top_agent_the_whole_left_side() {
        assert_eq!(shape(3), "(r a1 (d a2 a3))");
    }

    #[test]
    fn four_panes_form_a_grid_filled_left_to_right() {
        // a1 a2
        // a3 a4
        assert_eq!(shape(4), "(r (d a1 a3) (d a2 a4))");
    }

    #[test]
    fn every_layout_places_every_pane_once_anchored_on_a_placed_pane() {
        for n in 1..=4 {
            let panes = ids(n);
            let plan = plan(&panes).unwrap();
            let mut placed = vec![plan.anchor.clone()];
            for placement in &plan.placements {
                assert!(
                    placed.contains(&placement.anchor),
                    "{n} panes: {} anchors on unplaced {}",
                    placement.pane_id,
                    placement.anchor
                );
                placed.push(placement.pane_id.clone());
            }
            placed.sort();
            assert_eq!(placed, panes, "{n} panes");
        }
    }

    #[test]
    fn a_single_agent_needs_no_splits() {
        let plan = plan(&ids(1)).unwrap();
        assert_eq!(plan.anchor, "a1");
        assert!(plan.placements.is_empty());
    }

    #[test]
    fn no_panes_means_no_plan() {
        assert!(plan(&[]).is_none());
    }
}
