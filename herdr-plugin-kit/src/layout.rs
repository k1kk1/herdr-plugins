//! Split trees, and how to rebuild one out of Herdr calls.
//!
//! Herdr has no "reshape this tab" operation. `layout.apply` looks like one
//! but replaces the tab outright and kills every process in it, so the only
//! non-destructive route is to place panes one at a time with `pane.move`,
//! each splitting a pane that is already in position.
//!
//! Splitting pane `X` replaces it with `(direction X new)` — the new pane
//! becomes `X`'s sibling and `X` keeps the first branch. Everything here
//! follows from that one rule, and it is why order matters so much: a pane
//! that should span a whole column has to be split off *before* that column
//! is subdivided.

use crate::herdr::{Direction, LayoutNode};

/// Where a pane goes relative to another, as the user thinks of it.
///
/// Herdr only splits right and down; Left and Up are produced by splitting the
/// other way and then swapping the two panes, which the user never sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

impl Side {
    /// The split Herdr can actually perform for this side.
    pub fn split(self) -> Direction {
        match self {
            Side::Left | Side::Right => Direction::Right,
            Side::Up | Side::Down => Direction::Down,
        }
    }

    /// Whether the placed pane has to be swapped with its anchor afterwards.
    pub fn needs_swap(self) -> bool {
        matches!(self, Side::Left | Side::Up)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Up => "up",
            Side::Down => "down",
        }
    }

    pub fn hotkey(self) -> char {
        match self {
            Side::Left => 'h',
            Side::Right => 'l',
            Side::Up => 'k',
            Side::Down => 'j',
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "left" | "h" => Some(Side::Left),
            "right" | "r" | "l" => Some(Side::Right),
            "up" | "k" => Some(Side::Up),
            "down" | "d" | "j" => Some(Side::Down),
            _ => None,
        }
    }

    pub const ALL: [Side; 4] = [Side::Right, Side::Down, Side::Left, Side::Up];
}

/// How two panes share the space a split gives them.
///
/// The number is the share going to the pane that was already there, so
/// `SixtyForty` means "the pane I am joining keeps 60%".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ratio(pub f32);

impl Ratio {
    pub const EVEN: Ratio = Ratio(0.5);
    pub const SIXTY_FORTY: Ratio = Ratio(0.6);
    pub const FORTY_SIXTY: Ratio = Ratio(0.4);

    pub const ALL: [Ratio; 3] = [Ratio::EVEN, Ratio::SIXTY_FORTY, Ratio::FORTY_SIXTY];

    pub fn label(self) -> &'static str {
        match self {
            r if r == Ratio::SIXTY_FORTY => "60:40",
            r if r == Ratio::FORTY_SIXTY => "40:60",
            _ => "50:50",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "50:50" | "even" | "50" => Some(Ratio::EVEN),
            "60:40" | "60" => Some(Ratio::SIXTY_FORTY),
            "40:60" | "40" => Some(Ratio::FORTY_SIXTY),
            _ => None,
        }
    }

    /// The value `pane.move` wants, which is always the share of the split's
    /// *first* branch.
    ///
    /// For Right and Down the anchor stays first, so the ratio passes through.
    /// For Left and Up the two are swapped afterwards, so it must be inverted
    /// to leave the anchor with the share the user asked for.
    pub fn for_split(self, side: Side) -> f32 {
        if side.needs_swap() {
            1.0 - self.0
        } else {
            self.0
        }
    }
}

impl Default for Ratio {
    fn default() -> Self {
        Ratio::EVEN
    }
}

/// One pane's placement relative to a pane already in the tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub pane_id: String,
    /// Pane to split. Always a pane placed earlier in the plan.
    pub anchor: String,
    pub side: Side,
}

/// An arrangement: the pane that is already in place, then everyone else in
/// the order they must be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub anchor: String,
    pub placements: Vec<Placement>,
}

impl Plan {
    /// Panes in the order they are placed, anchor first.
    pub fn pane_ids(&self) -> Vec<String> {
        std::iter::once(self.anchor.clone())
            .chain(self.placements.iter().map(|p| p.pane_id.clone()))
            .collect()
    }

    /// The split tree this plan produces.
    ///
    /// Simulating lets a caller skip a rebuild that would change nothing — a
    /// rebuild briefly moves live panes between tabs, so not doing one is
    /// worth the arithmetic.
    pub fn simulate(&self) -> Shape {
        let mut tree = Shape::Pane(self.anchor.clone());
        for placement in &self.placements {
            tree.split(&placement.anchor, &placement.pane_id, placement.side);
        }
        tree
    }

    /// The plan that reproduces `shape`.
    ///
    /// This is the inverse of [`Plan::simulate`]: it recovers the sequence of
    /// splits that builds a tree, which is what lets Merge carry a tab's
    /// internal layout across into another tab instead of flattening it.
    pub fn from_shape(shape: &Shape) -> Self {
        let mut placements = Vec::new();
        emit(shape, &mut placements);
        Plan {
            anchor: shape.first_pane().to_string(),
            placements,
        }
    }
}

/// Walk a tree parent-first, recording the split that created each node.
///
/// A node's own seed is the first pane of its first branch, which is already
/// in place by the time the node is reached; the split adds the first pane of
/// its second branch beside it. Recursing afterwards subdivides each side.
fn emit(node: &Shape, out: &mut Vec<Placement>) {
    let Shape::Split {
        side,
        first,
        second,
    } = node
    else {
        return;
    };
    out.push(Placement {
        pane_id: second.first_pane().to_string(),
        anchor: first.first_pane().to_string(),
        side: *side,
    });
    emit(first, out);
    emit(second, out);
}

/// A split tree, reduced to the parts that decide whether two layouts match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Pane(String),
    Split {
        side: Side,
        first: Box<Shape>,
        second: Box<Shape>,
    },
}

impl Shape {
    pub fn pane(id: impl Into<String>) -> Self {
        Shape::Pane(id.into())
    }

    pub fn split(&mut self, target: &str, new_pane: &str, side: Side) {
        match self {
            Shape::Pane(id) if id == target => {
                *self = Shape::Split {
                    side,
                    first: Box::new(Shape::Pane(target.to_string())),
                    second: Box::new(Shape::Pane(new_pane.to_string())),
                };
            }
            Shape::Pane(_) => {}
            Shape::Split { first, second, .. } => {
                first.split(target, new_pane, side);
                second.split(target, new_pane, side);
            }
        }
    }

    /// Leftmost/topmost pane — the one a subtree is grown from.
    pub fn first_pane(&self) -> &str {
        match self {
            Shape::Pane(id) => id,
            Shape::Split { first, .. } => first.first_pane(),
        }
    }

    pub fn pane_ids(&self) -> Vec<String> {
        match self {
            Shape::Pane(id) => vec![id.clone()],
            Shape::Split { first, second, .. } => {
                let mut out = first.pane_ids();
                out.extend(second.pane_ids());
                out
            }
        }
    }

    /// Compact form for assertions and debugging: `(r p1 (d p2 p3))`.
    pub fn signature(&self) -> String {
        match self {
            Shape::Pane(id) => id.clone(),
            Shape::Split {
                side,
                first,
                second,
            } => format!(
                "({} {} {})",
                &side.as_str()[..1],
                first.signature(),
                second.signature()
            ),
        }
    }

    /// The shape of a live tab, as reported by `layout.export`.
    ///
    /// An exported tree only ever contains Right and Down splits, because
    /// those are the only ones Herdr stores.
    pub fn from_layout(node: &LayoutNode) -> Option<Self> {
        match node {
            LayoutNode::Pane { pane_id } => pane_id.clone().map(Shape::Pane),
            LayoutNode::Split {
                direction,
                first,
                second,
                ..
            } => Some(Shape::Split {
                side: Side::parse(direction)?,
                first: Box::new(Shape::from_layout(first)?),
                second: Box::new(Shape::from_layout(second)?),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(side: Side, first: Shape, second: Shape) -> Shape {
        Shape::Split {
            side,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn grid() -> Shape {
        split(
            Side::Right,
            split(Side::Down, Shape::pane("a"), Shape::pane("b")),
            split(Side::Down, Shape::pane("c"), Shape::pane("d")),
        )
    }

    #[test]
    fn a_plan_derived_from_a_shape_rebuilds_that_shape() {
        for shape in [
            Shape::pane("a"),
            split(Side::Right, Shape::pane("a"), Shape::pane("b")),
            grid(),
            // The lopsided tree a few ad-hoc splits leave behind.
            split(
                Side::Right,
                split(
                    Side::Down,
                    split(Side::Right, Shape::pane("a"), Shape::pane("b")),
                    Shape::pane("c"),
                ),
                Shape::pane("d"),
            ),
        ] {
            let plan = Plan::from_shape(&shape);
            assert_eq!(
                plan.simulate().signature(),
                shape.signature(),
                "round trip failed for {}",
                shape.signature()
            );
        }
    }

    #[test]
    fn a_derived_plan_places_every_pane_once_and_anchors_only_on_placed_panes() {
        let plan = Plan::from_shape(&grid());
        let mut placed = vec![plan.anchor.clone()];
        for placement in &plan.placements {
            assert!(placed.contains(&placement.anchor));
            assert!(!placed.contains(&placement.pane_id));
            placed.push(placement.pane_id.clone());
        }
        placed.sort();
        assert_eq!(placed, ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_lone_pane_needs_no_placements() {
        let plan = Plan::from_shape(&Shape::pane("a"));
        assert_eq!(plan.anchor, "a");
        assert!(plan.placements.is_empty());
    }

    #[test]
    fn left_and_up_are_a_split_plus_a_swap() {
        assert_eq!(Side::Left.split(), Direction::Right);
        assert!(Side::Left.needs_swap());
        assert_eq!(Side::Up.split(), Direction::Down);
        assert!(Side::Up.needs_swap());
        assert!(!Side::Right.needs_swap());
        assert!(!Side::Down.needs_swap());
    }

    #[test]
    fn the_anchor_keeps_its_share_whichever_side_is_chosen() {
        // 60:40 always means "the pane already there keeps 60%".
        assert_eq!(Ratio::SIXTY_FORTY.for_split(Side::Right), 0.6);
        // Left swaps the two afterwards, so the stored ratio is inverted.
        assert!((Ratio::SIXTY_FORTY.for_split(Side::Left) - 0.4).abs() < f32::EPSILON);
        assert_eq!(Ratio::EVEN.for_split(Side::Up), 0.5);
    }

    #[test]
    fn ratio_labels_round_trip() {
        for ratio in Ratio::ALL {
            assert_eq!(Ratio::parse(ratio.label()), Some(ratio));
        }
    }

    #[test]
    fn every_side_round_trips_through_its_name() {
        for side in Side::ALL {
            assert_eq!(Side::parse(side.as_str()), Some(side));
        }
    }
}
