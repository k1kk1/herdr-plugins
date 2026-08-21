//! Target arrangements for the panes of one tab.
//!
//! An arrangement is described as a *plan*: the order panes should end up in,
//! and for each pane after the first, which pane it anchors on and on which
//! side. `ops::rebuild` turns a plan into Herdr calls.
//!
//! Keeping this pure makes the shapes testable without a running Herdr, which
//! matters because a wrong plan rearranges someone's live agents.

pub use herdr_plugin_kit::layout::{Placement, Plan, Shape, Side};

/// The arrangements Layout Tools offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    /// All panes side by side in one row.
    Columns,
    /// All panes stacked in one column.
    Rows,
    /// A balanced grid: roughly square, filled column by column.
    Grid,
    /// The focused pane down the left, everyone else stacked on the right.
    MainLeft,
    /// The focused pane down the right, everyone else stacked on the left.
    MainRight,
    /// The focused pane across the top, everyone else in a row below.
    MainTop,
}

impl Arrangement {
    pub fn title(self) -> &'static str {
        match self {
            Arrangement::Columns => "Columns",
            Arrangement::Rows => "Rows",
            Arrangement::Grid => "Grid",
            Arrangement::MainLeft => "Main Left",
            Arrangement::MainRight => "Main Right",
            Arrangement::MainTop => "Main Top",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Arrangement::Columns => "すべての Pane を横一列に",
            Arrangement::Rows => "すべての Pane を縦一列に",
            Arrangement::Grid => "だいたい正方形の格子に",
            Arrangement::MainLeft => "現在の Pane を左、残りを右へ積む",
            Arrangement::MainRight => "現在の Pane を右、残りを左へ積む",
            Arrangement::MainTop => "現在の Pane を上、残りを下へ並べる",
        }
    }

    pub fn hotkey(self) -> &'static str {
        match self {
            Arrangement::Columns => "c",
            Arrangement::Rows => "r",
            Arrangement::Grid => "g",
            Arrangement::MainLeft => "h",
            Arrangement::MainRight => "l",
            Arrangement::MainTop => "t",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "columns" | "column" => Some(Arrangement::Columns),
            "rows" | "row" => Some(Arrangement::Rows),
            "grid" => Some(Arrangement::Grid),
            "main-left" | "main_left" => Some(Arrangement::MainLeft),
            "main-right" | "main_right" => Some(Arrangement::MainRight),
            "main-top" | "main_top" => Some(Arrangement::MainTop),
            _ => None,
        }
    }

    pub const ALL: [Arrangement; 6] = [
        Arrangement::Grid,
        Arrangement::Columns,
        Arrangement::Rows,
        Arrangement::MainLeft,
        Arrangement::MainRight,
        Arrangement::MainTop,
    ];

    /// Build the plan for `panes`, treating `main` as the pane that should get
    /// the large slot in the Main* arrangements.
    ///
    /// `panes` is in current layout order; the relative order of the others is
    /// preserved so a rearrangement does not shuffle unrelated panes.
    pub fn plan(self, panes: &[String], main: Option<&str>) -> Option<Plan> {
        if panes.len() < 2 {
            return None;
        }
        match self {
            Arrangement::Columns => Some(chain(panes, Side::Right)),
            Arrangement::Rows => Some(chain(panes, Side::Down)),
            Arrangement::Grid => Some(grid(panes)),
            Arrangement::MainLeft => Some(main_split(panes, main, Side::Right, Side::Down)),
            Arrangement::MainRight => {
                // Same shape as Main Left with the roles of the two sides
                // reversed: the others form the first column, main the second.
                let mut plan = main_split(panes, main, Side::Right, Side::Down);
                plan = mirror(plan);
                Some(plan)
            }
            Arrangement::MainTop => Some(main_split(panes, main, Side::Down, Side::Right)),
        }
    }
}

/// `a | b | c | d` — each pane hangs off the previous one.
fn chain(panes: &[String], side: Side) -> Plan {
    Plan {
        anchor: panes[0].clone(),
        placements: panes
            .windows(2)
            .map(|pair| Placement {
                pane_id: pair[1].clone(),
                anchor: pair[0].clone(),
                side,
            })
            .collect(),
    }
}

/// Column-major grid: `ceil(sqrt(n))` columns, each filled top to bottom.
fn grid(panes: &[String]) -> Plan {
    let n = panes.len();
    let columns = (n as f64).sqrt().ceil() as usize;
    let rows = n.div_ceil(columns);

    // Slice the panes into columns, the last one absorbing the remainder.
    let mut chunks: Vec<&[String]> = Vec::new();
    let mut start = 0;
    for column in 0..columns {
        // Spread the shortfall across the leftmost columns.
        let take = if column < n % columns || n % columns == 0 {
            rows
        } else {
            rows - 1
        };
        let end = (start + take).min(n);
        if start < end {
            chunks.push(&panes[start..end]);
        }
        start = end;
    }

    // Every column must exist before any column is subdivided. Splitting a
    // pane nests the new pane beside it, so a column head that is split
    // downwards first would only span half its own column afterwards.
    let mut placements = Vec::new();
    for pair in chunks.windows(2) {
        placements.push(Placement {
            pane_id: pair[1][0].clone(),
            anchor: pair[0][0].clone(),
            side: Side::Right,
        });
    }
    for chunk in &chunks {
        for pair in chunk.windows(2) {
            placements.push(Placement {
                pane_id: pair[1].clone(),
                anchor: pair[0].clone(),
                side: Side::Down,
            });
        }
    }

    Plan {
        anchor: panes[0].clone(),
        placements,
    }
}

/// `main` takes one whole side; the rest stack along `stack` on the other.
fn main_split(
    panes: &[String],
    main: Option<&str>,
    split: Side,
    stack: Side,
) -> Plan {
    let main = main
        .filter(|id| panes.iter().any(|p| p == id))
        .map(str::to_string)
        .unwrap_or_else(|| panes[0].clone());
    let others: Vec<String> = panes.iter().filter(|p| **p != main).cloned().collect();

    let mut placements = vec![Placement {
        pane_id: others[0].clone(),
        anchor: main.clone(),
        side: split,
    }];
    for pair in others.windows(2) {
        placements.push(Placement {
            pane_id: pair[1].clone(),
            anchor: pair[0].clone(),
            side: stack,
        });
    }

    Plan {
        anchor: main,
        placements,
    }
}

/// Turn a Main-Left plan into a Main-Right one.
///
/// Herdr only splits right and down, so "main on the right" is built the other
/// way round: the stack's head anchors the tab, main is split off it first so
/// that it spans the full height, and only then is the stack subdivided.
fn mirror(plan: Plan) -> Plan {
    let main = plan.anchor;
    let others = plan.placements;
    let first_other = others[0].pane_id.clone();

    let mut placements = vec![Placement {
        pane_id: main,
        anchor: first_other.clone(),
        side: Side::Right,
    }];
    // The stacking placements already anchor on each other, so they carry over
    // unchanged; only the main pane's placement had to be rewritten.
    placements.extend(others.into_iter().skip(1));

    Plan {
        anchor: first_other,
        placements,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("p{i}")).collect()
    }

    fn describe(plan: &Plan) -> Vec<String> {
        plan.placements
            .iter()
            .map(|p| format!("{}->{}{}", p.pane_id, p.anchor, p.side.as_str()))
            .collect()
    }

    #[test]
    fn a_single_pane_needs_no_rearranging() {
        assert!(Arrangement::Grid.plan(&ids(1), None).is_none());
        assert!(Arrangement::Columns.plan(&[], None).is_none());
    }

    #[test]
    fn columns_chains_every_pane_to_the_right() {
        let plan = Arrangement::Columns.plan(&ids(4), None).unwrap();
        assert_eq!(plan.anchor, "p1");
        assert_eq!(
            describe(&plan),
            ["p2->p1right", "p3->p2right", "p4->p3right"]
        );
    }

    #[test]
    fn rows_chains_every_pane_downwards() {
        let plan = Arrangement::Rows.plan(&ids(3), None).unwrap();
        assert_eq!(describe(&plan), ["p2->p1down", "p3->p2down"]);
    }

    #[test]
    fn grid_of_four_is_two_by_two() {
        let plan = Arrangement::Grid.plan(&ids(4), None).unwrap();
        assert_eq!(plan.anchor, "p1");
        // Column 1 = p1/p2, column 2 = p3/p4, hung off column 1.
        assert_eq!(
            describe(&plan),
            ["p3->p1right", "p2->p1down", "p4->p3down"]
        );
    }

    #[test]
    fn grid_spreads_the_remainder_over_the_first_columns() {
        // 5 panes -> 3 columns of 2, 2, 1.
        let plan = Arrangement::Grid.plan(&ids(5), None).unwrap();
        assert_eq!(
            describe(&plan),
            ["p3->p1right", "p5->p3right", "p2->p1down", "p4->p3down"]
        );
    }

    #[test]
    fn every_arrangement_places_every_pane_exactly_once() {
        for n in 2..=9 {
            let panes = ids(n);
            for arrangement in Arrangement::ALL {
                let plan = arrangement.plan(&panes, Some("p2")).unwrap();
                let mut got = plan.pane_ids();
                got.sort();
                let mut want = panes.clone();
                want.sort();
                assert_eq!(got, want, "{:?} with {n} panes", arrangement);
            }
        }
    }

    #[test]
    fn every_placement_anchors_on_a_pane_placed_before_it() {
        for n in 2..=9 {
            let panes = ids(n);
            for arrangement in Arrangement::ALL {
                let plan = arrangement.plan(&panes, Some("p3")).unwrap();
                let mut placed = vec![plan.anchor.clone()];
                for placement in &plan.placements {
                    assert!(
                        placed.contains(&placement.anchor),
                        "{:?} with {n} panes anchors {} on unplaced {}",
                        arrangement,
                        placement.pane_id,
                        placement.anchor
                    );
                    placed.push(placement.pane_id.clone());
                }
            }
        }
    }

    #[test]
    fn main_left_gives_the_focused_pane_the_whole_left_side() {
        let plan = Arrangement::MainLeft.plan(&ids(4), Some("p3")).unwrap();
        assert_eq!(plan.anchor, "p3");
        assert_eq!(
            describe(&plan),
            ["p1->p3right", "p2->p1down", "p4->p2down"]
        );
    }

    #[test]
    fn main_right_stacks_the_others_first_then_adds_main_to_their_right() {
        let plan = Arrangement::MainRight.plan(&ids(4), Some("p3")).unwrap();
        assert_eq!(plan.anchor, "p1");
        assert_eq!(
            describe(&plan),
            ["p3->p1right", "p2->p1down", "p4->p2down"]
        );
    }

    #[test]
    fn main_top_puts_the_others_in_a_row_underneath() {
        let plan = Arrangement::MainTop.plan(&ids(3), Some("p1")).unwrap();
        assert_eq!(plan.anchor, "p1");
        assert_eq!(describe(&plan), ["p2->p1down", "p3->p2right"]);
    }

    fn shape(arrangement: Arrangement, n: usize, main: Option<&str>) -> String {
        arrangement
            .plan(&ids(n), main)
            .unwrap()
            .simulate()
            .signature()
    }

    #[test]
    fn columns_produce_a_right_nested_chain() {
        assert_eq!(shape(Arrangement::Columns, 4, None), "(r p1 (r p2 (r p3 p4)))");
    }

    #[test]
    fn grid_of_four_produces_two_columns_of_two() {
        assert_eq!(shape(Arrangement::Grid, 4, None), "(r (d p1 p2) (d p3 p4))");
    }

    #[test]
    fn main_left_produces_one_big_pane_beside_a_stack() {
        assert_eq!(
            shape(Arrangement::MainLeft, 4, Some("p3")),
            "(r p3 (d p1 (d p2 p4)))"
        );
    }

    #[test]
    fn main_right_is_the_mirror_of_main_left() {
        // The stack comes first and main hangs off its right-hand side.
        assert_eq!(
            shape(Arrangement::MainRight, 4, Some("p3")),
            "(r (d p1 (d p2 p4)) p3)"
        );
    }

    #[test]
    fn main_falls_back_to_the_first_pane_when_the_focus_is_unknown() {
        let plan = Arrangement::MainLeft.plan(&ids(3), Some("gone")).unwrap();
        assert_eq!(plan.anchor, "p1");
    }
}
