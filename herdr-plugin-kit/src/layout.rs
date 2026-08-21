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

/// Fill for the pane a diagram is pointing at.
pub const HIGHLIGHT: char = '░';

/// The box-drawing character for a corner, from the walls meeting there.
fn junction(up: bool, down: bool, left: bool, right: bool) -> char {
    match (up, down, left, right) {
        (true, true, true, true) => '┼',
        (true, true, true, false) => '┤',
        (true, true, false, true) => '├',
        (true, true, false, false) => '│',
        (true, false, true, true) => '┴',
        (true, false, true, false) => '┘',
        (true, false, false, true) => '└',
        (false, true, true, true) => '┬',
        (false, true, true, false) => '┐',
        (false, true, false, true) => '┌',
        (false, false, true, true) => '─',
        (true, false, false, false) | (false, true, false, false) => '│',
        (false, false, true, false) | (false, false, false, true) => '─',
        (false, false, false, false) => ' ',
    }
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
    /// A one-line sketch of the arrangement, for a picker row.
    ///
    /// `▫│▫` is two panes side by side, `▫─▫` two stacked, `[▫│▫]─▫` a pair
    /// above a third. A count — "3 panes" — says how many boxes there are but
    /// nothing about where they sit, which is the part a reader is choosing
    /// between when they pick a destination.
    ///
    /// Brackets only appear around a nested group, so the common flat cases
    /// stay as light as possible.
    pub fn sketch(&self) -> String {
        self.draw(true)
    }

    /// A box diagram of the arrangement, one string per line.
    ///
    /// The sketch (`▫│[▫─▫]`) fits on a list row; this is what the tab
    /// actually looks like, for the one row a reader is pointing at.
    ///
    /// The tree is rasterised into a grid of cells, and the walls are then
    /// worked out as the *boundaries* between cells that belong to different
    /// panes. Drawing walls inside cells instead is the obvious shortcut and
    /// it produces wrong corners: a wall that starts halfway along a row has
    /// no junction to hang from.
    pub fn diagram(&self, width: usize, height: usize) -> Vec<String> {
        self.diagram_with(width, height, None)
    }

    /// The diagram with one pane shaded, for showing where something lands.
    ///
    /// The shading glyph is what the renderer colours, so "which one is new"
    /// survives even where colour does not — a printed diagram, a terminal
    /// without it, a reader who cannot tell the two colours apart.
    pub fn diagram_with(
        &self,
        width: usize,
        height: usize,
        highlight: Option<&str>,
    ) -> Vec<String> {
        self.diagram_marking(width, height, highlight.as_slice())
    }

    /// The diagram with several panes shaded — a whole tab on the move, say.
    pub fn diagram_marking(&self, width: usize, height: usize, highlight: &[&str]) -> Vec<String> {
        let width = width.max(2);
        let height = height.max(2);
        let mut grid = vec![vec![0usize; width]; height];
        let mut next = 0usize;
        self.rasterise(&mut grid, 0, 0, width, height, &mut next);

        // Which rasterised ids the highlighted panes ended up as.
        let marked: Vec<usize> = highlight
            .iter()
            .filter_map(|id| {
                let mut seen = 0usize;
                self.find(id, &mut seen)
            })
            .collect();

        // A wall stands on a boundary when the cells either side of it differ,
        // and around the whole diagram.
        let vwall = |x: usize, y: usize| -> bool {
            x == 0 || x == width || grid[y][x - 1] != grid[y][x]
        };
        let hwall = |x: usize, y: usize| -> bool {
            y == 0 || y == height || grid[y - 1][x] != grid[y][x]
        };

        // A gap between two cells of the highlighted pane belongs to it too;
        // leaving those as spaces makes a solid rectangle look like dots.
        let filled = |x: usize, y: usize| -> bool { marked.contains(&grid[y][x]) };
        let gap_h = |x: usize, y: usize| -> bool {
            x > 0 && x < width && !vwall(x, y) && filled(x - 1, y) && filled(x, y)
        };
        let gap_v = |x: usize, y: usize| -> bool {
            y > 0 && y < height && !hwall(x, y) && filled(x, y - 1) && filled(x, y)
        };

        let mut out = Vec::new();
        for y in 0..=height {
            // The line of corners and horizontal walls at this boundary.
            let mut line = String::new();
            for x in 0..=width {
                let up = y > 0 && vwall(x, y - 1);
                let down = y < height && vwall(x, y);
                let left = x > 0 && hwall(x - 1, y);
                let right = x < width && hwall(x, y);
                let corner = junction(up, down, left, right);
                // Inside the shaded pane every cell of the drawing is shaded,
                // corners and gaps included.
                line.push(if corner == ' ' && gap_v(x.min(width.saturating_sub(1)), y) && gap_h(x, y.min(height.saturating_sub(1))) {
                    HIGHLIGHT
                } else {
                    corner
                });
                if x < width {
                    line.push(if right {
                        '─'
                    } else if gap_v(x, y) {
                        HIGHLIGHT
                    } else {
                        ' '
                    });
                }
            }
            out.push(line);

            if y < height {
                let mut line = String::new();
                for x in 0..=width {
                    line.push(if vwall(x, y) {
                        '│'
                    } else if gap_h(x, y) {
                        HIGHLIGHT
                    } else {
                        ' '
                    });
                    if x < width {
                        line.push(if filled(x, y) { HIGHLIGHT } else { ' ' });
                    }
                }
                out.push(line);
            }
        }
        out
    }

    /// The rasterisation id a leaf will receive, found by the same walk order
    /// `rasterise` uses.
    fn find(&self, id: &str, seen: &mut usize) -> Option<usize> {
        match self {
            Shape::Pane(pane) => {
                *seen += 1;
                (pane == id).then_some(*seen)
            }
            Shape::Split { side, first, second } => {
                let (a, b) = match side {
                    Side::Left | Side::Up => (second, first),
                    Side::Right | Side::Down => (first, second),
                };
                a.find(id, seen).or_else(|| b.find(id, seen))
            }
        }
    }

    fn rasterise(
        &self,
        grid: &mut [Vec<usize>],
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        next: &mut usize,
    ) {
        match self {
            Shape::Pane(_) => {
                *next += 1;
                let id = *next;
                for row in grid.iter_mut().skip(y).take(h) {
                    for cell in row.iter_mut().skip(x).take(w) {
                        *cell = id;
                    }
                }
            }
            Shape::Split { side, first, second } => {
                let (a, b) = match side {
                    Side::Left | Side::Up => (second, first),
                    Side::Right | Side::Down => (first, second),
                };
                match side {
                    Side::Left | Side::Right => {
                        let cut = (w / 2).max(1).min(w.saturating_sub(1));
                        a.rasterise(grid, x, y, cut, h, next);
                        b.rasterise(grid, x + cut, y, w - cut, h, next);
                    }
                    Side::Up | Side::Down => {
                        let cut = (h / 2).max(1).min(h.saturating_sub(1));
                        a.rasterise(grid, x, y, w, cut, next);
                        b.rasterise(grid, x, y + cut, w, h - cut, next);
                    }
                }
            }
        }
    }

    fn draw(&self, top: bool) -> String {
        match self {
            Shape::Pane(_) => "▫".to_string(),
            Shape::Split { side, first, second } => {
                let joint = match side {
                    Side::Left | Side::Right => '│',
                    Side::Up | Side::Down => '─',
                };
                // `Left` and `Up` place the *new* pane before the existing
                // one, so the drawing has to swap to match what is on screen.
                let (a, b) = match side {
                    Side::Left | Side::Up => (second, first),
                    Side::Right | Side::Down => (first, second),
                };
                let body = format!("{}{joint}{}", a.draw(false), b.draw(false));
                if top {
                    body
                } else {
                    format!("[{body}]")
                }
            }
        }
    }

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
mod sketch_tests {
    use super::*;

    fn split(side: Side, first: Shape, second: Shape) -> Shape {
        Shape::Split {
            side,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// The diagram as a list of lines, which reads better in a failure than
    /// one string full of escapes.
    fn art(shape: &Shape, w: usize, h: usize) -> Vec<String> {
        shape.diagram(w, h)
    }

    #[test]
    fn a_lone_pane_is_an_empty_box() {
        assert_eq!(
            art(&Shape::pane("a"), 3, 2),
            ["┌─────┐", "│     │", "│     │", "│     │", "└─────┘"]
        );
    }

    #[test]
    fn a_side_by_side_split_gets_a_wall_with_proper_corners() {
        let shape = split(Side::Right, Shape::pane("a"), Shape::pane("b"));
        assert_eq!(
            art(&shape, 2, 2),
            ["┌─┬─┐", "│ │ │", "│ │ │", "│ │ │", "└─┴─┘"]
        );
    }

    #[test]
    fn a_wall_that_starts_halfway_hangs_off_a_tee() {
        // One pane down the left, two stacked on the right. The horizontal
        // wall exists only on the right, so the left edge stays `│` and the
        // junction in the middle is a `├` — the case that came out wrong when
        // walls were drawn inside cells rather than on boundaries.
        let shape = split(
            Side::Right,
            Shape::pane("a"),
            split(Side::Down, Shape::pane("b"), Shape::pane("c")),
        );
        assert_eq!(
            art(&shape, 4, 2),
            ["┌───┬───┐", "│   │   │", "│   ├───┤", "│   │   │", "└───┴───┘"]
        );
    }

    #[test]
    fn the_highlighted_pane_is_the_only_one_filled() {
        let shape = split(Side::Right, Shape::pane("old"), Shape::pane("new"));
        let lines = shape.diagram_with(2, 2, Some("new"));
        let filled: usize = lines.iter().map(|l| l.matches(HIGHLIGHT).count()).sum();
        // One cell wide. A diagram is `2 * height + 1` lines tall, so a
        // two-row box has three shaded rows once the gap between them is
        // filled in as well.
        assert_eq!(filled, 3, "{lines:?}");
        // And it is on the right, where `Right` puts it.
        assert!(lines[1].ends_with("░│"), "{}", lines[1]);
    }

    #[test]
    fn the_shaded_pane_is_a_solid_rectangle() {
        // The gaps between cells belong to the pane too. Leaving them blank
        // turns a filled rectangle into a field of dots.
        let shape = split(Side::Right, Shape::pane("old"), Shape::pane("new"));
        let lines = shape.diagram_with(6, 2, Some("new"));
        for line in &lines[1..lines.len() - 1] {
            // Counted in characters, not bytes: these are all multi-byte.
            let chars: Vec<char> = line.chars().collect();
            let start = chars.iter().position(|c| *c == HIGHLIGHT);
            let end = chars.iter().rposition(|c| *c == HIGHLIGHT);
            let (Some(start), Some(end)) = (start, end) else {
                panic!("nothing shaded in {line}");
            };
            assert!(
                chars[start..=end].iter().all(|c| *c == HIGHLIGHT),
                "{line} has gaps"
            );
        }
    }

    #[test]
    fn highlighting_a_pane_that_is_not_there_shades_nothing() {
        let shape = split(Side::Right, Shape::pane("a"), Shape::pane("b"));
        let lines = shape.diagram_with(2, 2, Some("absent"));
        assert!(lines.iter().all(|l| !l.contains(HIGHLIGHT)));
    }

    #[test]
    fn a_lone_pane_is_one_box() {
        assert_eq!(Shape::pane("p1").sketch(), "▫");
    }

    #[test]
    fn the_joint_says_which_way_the_split_runs() {
        let across = split(Side::Right, Shape::pane("a"), Shape::pane("b"));
        let down = split(Side::Down, Shape::pane("a"), Shape::pane("b"));
        assert_eq!(across.sketch(), "▫│▫");
        assert_eq!(down.sketch(), "▫─▫");
    }

    #[test]
    fn nesting_is_bracketed_but_the_outermost_split_is_not() {
        // main-left: one pane down the left, two stacked on the right.
        let shape = split(
            Side::Right,
            Shape::pane("a"),
            split(Side::Down, Shape::pane("b"), Shape::pane("c")),
        );
        assert_eq!(shape.sketch(), "▫│[▫─▫]");
    }

    #[test]
    fn left_and_up_are_drawn_where_they_actually_land() {
        // Splitting to the Left puts the new pane on the left of the screen,
        // so the sketch must not simply follow first/second order.
        let right = split(Side::Right, Shape::pane("old"), Shape::pane("new"));
        let left = split(Side::Left, Shape::pane("old"), Shape::pane("new"));
        assert_eq!(right.sketch(), left.sketch());
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

