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
        ..
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
/// Rough East-Asian-width check: how many screen columns a character takes.
///
/// Both the diagrams and the picker need it, and they must agree — a label
/// measured one way and a line padded the other is exactly how a wall ends up
/// in the wrong column.
pub fn char_width(ch: char) -> usize {
    let c = ch as u32;
    let wide = (0x1100..=0x115F).contains(&c)
        || (0x2E80..=0xA4CF).contains(&c)
        || (0xAC00..=0xD7A3).contains(&c)
        || (0xF900..=0xFAFF).contains(&c)
        || (0xFE30..=0xFE6F).contains(&c)
        || (0xFF00..=0xFF60).contains(&c)
        || (0xFFE0..=0xFFE6).contains(&c)
        || (0x1F300..=0x1FAFF).contains(&c);
    if wide {
        2
    } else {
        1
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Pane(String),
    Split {
        side: Side,
        /// Share of the space the `first` branch keeps, as Herdr reports it.
        ///
        /// The API has always sent this and the diagrams always threw it away,
        /// so a tab split 70/30 was drawn 50/50 — the arrangement right, the
        /// sizes wrong. A split this code invents rather than reads is
        /// [`EVEN`], because that is what Herdr will make.
        ratio: f32,
        first: Box<Shape>,
        second: Box<Shape>,
    },
}

/// The ratio of a split nobody has asked to be uneven.
pub const EVEN: f32 = 0.5;

impl Shape {
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

    /// The marked diagram with a short identifier centred in each pane.
    ///
    /// A split tree ordinarily only says that regions exist. Numbering those
    /// regions lets an operation preview answer which live pane will occupy
    /// each region afterwards.
    pub fn diagram_marking_labeled(
        &self,
        width: usize,
        height: usize,
        highlight: &[&str],
        labels: &[(&str, &str)],
    ) -> Vec<String> {
        let width = width.max(2);
        let height = height.max(2);
        let mut out = self.diagram_marking(width, height, highlight);
        let mut grid = vec![vec![0usize; width]; height];
        let mut next = 0usize;
        self.rasterise(&mut grid, 0, 0, width, height, &mut next);

        for (pane, label) in labels {
            let mut seen = 0usize;
            let Some(id) = self.find(pane, &mut seen) else {
                continue;
            };
            let mut min_x = width;
            let mut max_x = 0usize;
            let mut min_y = height;
            let mut max_y = 0usize;
            let mut found = false;
            for (y, row) in grid.iter().enumerate() {
                for (x, cell) in row.iter().enumerate() {
                    if *cell == id {
                        found = true;
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                        min_y = min_y.min(y);
                        max_y = max_y.max(y);
                    }
                }
            }
            // A pane squeezed out of the raster has no box to write in, and
            // the bounds are still their starting values — `max_x - min_x`
            // then underflows. Ten panes in a preview six cells tall is not a
            // hypothetical: it is what a busy tab looks like.
            if !found {
                continue;
            }

            // Budgeted by display width, not by character count. A CJK
            // character fills one cell of the line but two columns of the
            // screen, so taking `available` *characters* made the row wider
            // than every other row and pushed the walls out of line.
            // Give each pane a compact title strip at its top edge, like the
            // header of a live terminal pane. The dot also makes the active
            // pane easy to spot when the preview is too small for the legend.
            let available = 2 * (max_x - min_x + 1) - 1;
            let marker = if highlight.contains(pane) { '●' } else { '○' };
            let text: Vec<char> = std::iter::once(marker)
                .chain(label.chars())
                .collect();
            let used: usize = text.iter().map(|ch| char_width(*ch)).sum();
            if used > available {
                continue;
            }
            let row = 2 * min_y + 1;
            let start = 2 * min_x + 1;
            let mut line: Vec<char> = out[row].chars().collect();
            if start + used > line.len() {
                continue;
            }
            line.splice(start..start + used, text);
            out[row] = line.into_iter().collect();
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
            Shape::Split { side, first, second, .. } => {
                let (a, b) = match side {
                    Side::Left | Side::Up => (second, first),
                    Side::Right | Side::Down => (first, second),
                };
                a.find(id, seen).or_else(|| b.find(id, seen))
            }
        }
    }

    /// Where to cut `total` cells so the first branch gets `share` of them.
    ///
    /// Never nothing and never everything: a pane with no cells vanishes from
    /// a picture that still looks complete, which is worse than a pane drawn
    /// one cell wider than it really is.
    fn cut_cells(total: usize, share: f32) -> usize {
        // Nothing to divide: one cell cannot hold two panes, and pretending
        // otherwise leaves the second branch a negative width.
        if total <= 1 {
            return total.saturating_sub(1);
        }
        let share = if share.is_finite() {
            share.clamp(0.0, 1.0)
        } else {
            EVEN
        };
        ((total as f32 * share).round() as usize).clamp(1, total - 1)
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
            Shape::Split {
                side,
                ratio,
                first,
                second,
            } => {
                // `ratio` is the share `first` keeps. Where the two branches
                // are drawn the other way round, the leading share is what is
                // left over.
                let (a, b, share) = match side {
                    Side::Left | Side::Up => (second, first, 1.0 - *ratio),
                    Side::Right | Side::Down => (first, second, *ratio),
                };
                match side {
                    Side::Left | Side::Right => {
                        let cut = Self::cut_cells(w, share);
                        a.rasterise(grid, x, y, cut, h, next);
                        b.rasterise(grid, x + cut, y, w - cut, h, next);
                    }
                    Side::Up | Side::Down => {
                        let cut = Self::cut_cells(h, share);
                        a.rasterise(grid, x, y, w, cut, next);
                        b.rasterise(grid, x, y + cut, w, h - cut, next);
                    }
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
                    ratio: EVEN,
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
    /// Take `id` out of the tree, closing the split it was half of.
    ///
    /// Returns `None` when that pane was all there was — the tab it describes
    /// would cease to exist, and an empty box would be a lie about what
    /// happens next.
    pub fn without(&self, id: &str) -> Option<Shape> {
        match self {
            Shape::Pane(pane) => (pane != id).then(|| self.clone()),
            Shape::Split {
                side,
                ratio,
                first,
                second,
            } => match (first.without(id), second.without(id)) {
                // The surviving side takes the whole space, exactly as Herdr
                // does when a pane leaves.
                (None, Some(rest)) | (Some(rest), None) => Some(rest),
                (Some(a), Some(b)) => Some(Shape::Split {
                    side: *side,
                    ratio: *ratio,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (None, None) => None,
            },
        }
    }

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
    /// Whether every pane would get at least one cell at this size.
    ///
    /// Below it the picture is not merely cramped, it is wrong: panes vanish
    /// from a diagram that still looks complete, and the reader counts boxes
    /// that are not there. Callers draw a placeholder instead.
    pub fn renders_in(&self, width: usize, height: usize) -> bool {
        let width = width.max(2);
        let height = height.max(2);
        let mut grid = vec![vec![0usize; width]; height];
        let mut next = 0usize;
        self.rasterise(&mut grid, 0, 0, width, height, &mut next);
        let drawn: std::collections::HashSet<usize> =
            grid.iter().flatten().copied().filter(|id| *id > 0).collect();
        drawn.len() == self.pane_ids().len()
    }

    pub fn signature(&self) -> String {
        match self {
            Shape::Pane(id) => id.clone(),
            Shape::Split {
                side,
                first,
                second,
                ..
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
                ratio,
                first,
                second,
            } => Some(Shape::Split {
                side: Side::parse(direction)?,
                ratio: *ratio,
                first: Box::new(Shape::from_layout(first)?),
                second: Box::new(Shape::from_layout(second)?),
            }),
        }
    }
}

#[cfg(test)]
mod without_tests {
    use super::*;

    fn split(side: Side, a: Shape, b: Shape) -> Shape {
        Shape::Split {
            side,
            ratio: EVEN,
            first: Box::new(a),
            second: Box::new(b),
        }
    }

    #[test]
    fn the_surviving_side_takes_the_whole_space() {
        let shape = split(Side::Right, Shape::pane("a"), Shape::pane("b"));
        assert_eq!(shape.without("a"), Some(Shape::pane("b")));
        assert_eq!(shape.without("b"), Some(Shape::pane("a")));
    }

    #[test]
    fn removing_the_only_pane_leaves_nothing_rather_than_an_empty_box() {
        assert_eq!(Shape::pane("a").without("a"), None);
    }

    #[test]
    fn a_pane_that_is_not_there_changes_nothing() {
        let shape = split(Side::Down, Shape::pane("a"), Shape::pane("b"));
        assert_eq!(shape.without("zzz"), Some(shape.clone()));
    }

    #[test]
    fn only_the_split_that_held_it_collapses() {
        // (r a (d b c)) minus b is (r a c): the outer split survives.
        let inner = split(Side::Down, Shape::pane("b"), Shape::pane("c"));
        let shape = split(Side::Right, Shape::pane("a"), inner);
        assert_eq!(
            shape.without("b"),
            Some(split(Side::Right, Shape::pane("a"), Shape::pane("c")))
        );
    }
}

#[cfg(test)]
mod sketch_tests {
    use super::*;

    fn split(side: Side, first: Shape, second: Shape) -> Shape {
        Shape::Split {
            side,
            ratio: EVEN,
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
    fn labels_identify_each_pane_inside_their_own_regions() {
        let shape = split(Side::Right, Shape::pane("left"), Shape::pane("right"));
        let lines = shape.diagram_marking_labeled(
            6,
            2,
            &["left"],
            &[("left", "1"), ("right", "2")],
        );
        assert!(lines[1].contains("●1"), "{lines:?}");
        assert!(lines[1].contains("○2"), "{lines:?}");
        assert!(lines[1].find('1') < lines[1].find('2'), "{lines:?}");
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

#[cfg(test)]
mod label_width_tests {
    use super::*;

    fn width(line: &str) -> usize {
        line.chars().map(char_width).sum()
    }

    #[test]
    fn a_wide_label_does_not_push_the_walls_out_of_line() {
        let mut shape = Shape::pane("a");
        shape.split("a", "b", Side::Right);
        let lines = shape.diagram_marking_labeled(8, 3, &[], &[("a", "相手"), ("b", "p1")]);
        let expected = width(&lines[0]);
        for (row, line) in lines.iter().enumerate() {
            assert_eq!(width(line), expected, "row {row}: {line}");
        }
        assert!(lines.iter().any(|line| line.contains("相手")));
    }

    #[test]
    fn a_label_too_wide_for_its_pane_is_left_out_rather_than_cut() {
        // Half an identifier is not a shorter identifier.
        let shape = Shape::pane("a");
        let plain = shape.diagram_marking_labeled(3, 2, &[], &[]);
        let long = shape.diagram_marking_labeled(3, 2, &[], &[("a", "日本語のとても長いタイトル")]);
        assert_eq!(plain, long);
    }

    #[test]
    fn a_label_never_overwrites_the_wall_of_a_narrow_pane() {
        // A pane one cell wide has one column of interior. `p9` needs two, so
        // it is left out; writing it took the right-hand wall with it.
        let mut shape = Shape::pane("a");
        for n in 1..6 {
            shape.split(&format!("p{}", n - 1), &format!("p{n}"), Side::Right);
        }
        let labels: Vec<(String, String)> = (0..6).map(|n| (format!("p{n}"), format!("p{n}"))).collect();
        let refs: Vec<(&str, &str)> = labels.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let lines = shape.diagram_marking_labeled(6, 2, &[], &refs);
        let walls = lines[0].chars().filter(|c| "\u{252c}\u{250c}\u{2510}".contains(*c)).count();
        for line in &lines[1..lines.len() - 1] {
            assert_eq!(
                line.chars().filter(|c| *c == '\u{2502}').count(),
                walls,
                "{line}"
            );
        }
    }
}


#[cfg(test)]
mod ratio_tests {
    use super::*;

    fn widths(shape: &Shape, cells: usize) -> Vec<usize> {
        let mut grid = vec![vec![0usize; cells]; 2];
        let mut next = 0;
        shape.rasterise(&mut grid, 0, 0, cells, 2, &mut next);
        let mut counts = vec![0usize; shape.pane_ids().len() + 1];
        for cell in &grid[0] {
            counts[*cell] += 1;
        }
        counts[1..].to_vec()
    }

    fn uneven(ratio: f32) -> Shape {
        Shape::Split {
            side: Side::Right,
            ratio,
            first: Box::new(Shape::Pane("a".into())),
            second: Box::new(Shape::Pane("b".into())),
        }
    }

    #[test]
    fn a_split_is_drawn_at_the_ratio_herdr_reports() {
        // The API has always sent this and the diagrams always dropped it, so
        // a tab split 70/30 was drawn 50/50.
        assert_eq!(widths(&uneven(0.7), 10), [7, 3]);
        assert_eq!(widths(&uneven(0.25), 8), [2, 6]);
        assert_eq!(widths(&uneven(EVEN), 10), [5, 5]);
    }

    #[test]
    fn a_lopsided_split_never_leaves_a_pane_with_nothing() {
        // A pane with no cells vanishes from a picture that still looks
        // complete, which is worse than one drawn a cell too wide.
        for ratio in [0.0, 0.01, 0.99, 1.0, f32::NAN] {
            let widths = widths(&uneven(ratio), 4);
            assert!(widths.iter().all(|w| *w > 0), "{ratio}: {widths:?}");
        }
    }

    #[test]
    fn a_split_this_code_invents_is_even() {
        // Herdr will make it even, so the preview says even.
        let mut shape = Shape::pane("a");
        shape.split("a", "b", Side::Right);
        assert_eq!(widths(&shape, 10), [5, 5]);
    }

    #[test]
    fn the_signature_still_describes_the_arrangement_alone() {
        // Ratios belong in the picture, not in the text the tests compare:
        // `(r a b)` is what a reader can check at a glance.
        assert_eq!(uneven(0.7).signature(), "(r a b)");
        assert_eq!(uneven(EVEN).signature(), "(r a b)");
    }
}
