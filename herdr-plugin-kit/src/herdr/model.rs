//! The subset of the Herdr API model the plugins read.
//!
//! Only fields actually used are declared; the API is additive, so unknown
//! fields are ignored rather than treated as errors.

use serde::{Deserialize, Serialize};

/// Agent lifecycle state (`AgentStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Working,
    Blocked,
    Done,
    Idle,
    #[serde(other)]
    Unknown,
}

impl AgentStatus {
    /// Glyph used in pickers.
    pub fn glyph(self) -> char {
        match self {
            AgentStatus::Working => '●',
            AgentStatus::Blocked => '!',
            AgentStatus::Done => '✓',
            AgentStatus::Idle => '○',
            AgentStatus::Unknown => '?',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
            AgentStatus::Idle => "idle",
            AgentStatus::Unknown => "unknown",
        }
    }

    /// Sort key putting the agents that need a human first.
    pub fn priority(self) -> u8 {
        match self {
            AgentStatus::Blocked => 0,
            AgentStatus::Done => 1,
            AgentStatus::Working => 2,
            AgentStatus::Idle => 3,
            AgentStatus::Unknown => 4,
        }
    }
}

impl Default for AgentStatus {
    fn default() -> Self {
        AgentStatus::Unknown
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub focused: bool,
    /// User-assigned pane label (`herdr pane rename`).
    #[serde(default)]
    pub label: Option<String>,
    /// Detected agent kind, e.g. `claude`, `codex`.
    #[serde(default)]
    pub agent: Option<String>,
    /// Display override reported via `pane report-metadata`.
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
    /// Display label. Unnamed tabs report their public number as a string.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub pane_count: u32,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub agent_status: AgentStatus,
}

/// A pane running a detected agent, as reported by `agent.list`.
///
/// This carries `state_change_seq`, which `pane.list` does not: a counter that
/// increases every time an agent changes state. It gives Gather a stable,
/// meaningful order within a status group — most recently changed first.
#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub state_change_seq: u64,
}

/// Result payload of `pane.move`.
#[derive(Debug, Clone, Deserialize)]
pub struct MoveResult {
    /// Present when the move created a tab, i.e. for Extract.
    #[serde(default)]
    pub created_tab: Option<Tab>,
}

/// Direction a pane is placed relative to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Right,
    Down,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Right => "right",
            Direction::Down => "down",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "right" | "r" => Some(Direction::Right),
            "down" | "d" => Some(Direction::Down),
            _ => None,
        }
    }
}

/// A tab's split tree, as returned by `layout.export`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    Pane {
        #[serde(default)]
        pane_id: Option<String>,
    },
    Split {
        direction: String,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    /// Pane ids in left-to-right, top-to-bottom order.
    pub fn pane_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<String>) {
        match self {
            LayoutNode::Pane { pane_id } => {
                if let Some(id) = pane_id {
                    out.push(id.clone());
                }
            }
            LayoutNode::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Pane { .. } => 1,
            LayoutNode::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    /// Every split in the tree, as `(path, first_leaves, total_leaves)`.
    ///
    /// A path is the sequence of branch choices from the root that
    /// `layout.set_split_ratio` expects: `false` = first, `true` = second.
    pub fn splits(&self) -> Vec<(Vec<bool>, usize, usize)> {
        let mut out = Vec::new();
        self.walk_splits(&mut Vec::new(), &mut out);
        out
    }

    fn walk_splits(&self, path: &mut Vec<bool>, out: &mut Vec<(Vec<bool>, usize, usize)>) {
        if let LayoutNode::Split { first, second, .. } = self {
            out.push((path.clone(), first.leaf_count(), self.leaf_count()));
            path.push(false);
            first.walk_splits(path, out);
            path.pop();
            path.push(true);
            second.walk_splits(path, out);
            path.pop();
        }
    }
}

/// Result payload of `layout.export`.
#[derive(Debug, Clone, Deserialize)]
pub struct Layout {
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub zoomed: bool,
    pub root: LayoutNode,
}

/// A plugin action, as reported by `plugin.action.list`.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginAction {
    pub plugin_id: String,
    pub action_id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub contexts: Vec<String>,
}

/// An installed plugin, as reported by `plugin.list`.
#[derive(Debug, Clone, Deserialize)]
pub struct InstalledPlugin {
    pub plugin_id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> LayoutNode {
        LayoutNode::Pane {
            pane_id: Some(id.to_string()),
        }
    }

    fn split(direction: &str, first: LayoutNode, second: LayoutNode) -> LayoutNode {
        LayoutNode::Split {
            direction: direction.to_string(),
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// `(r a (r b (r c d)))` — the right-nested chain a Columns rebuild makes.
    fn chain() -> LayoutNode {
        split(
            "right",
            pane("a"),
            split("right", pane("b"), split("right", pane("c"), pane("d"))),
        )
    }

    #[test]
    fn pane_ids_are_in_layout_order() {
        assert_eq!(chain().pane_ids(), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn splits_report_the_leaf_counts_equalize_needs() {
        // An even chain needs ratios 1/4, 1/3, 1/2 — not 0.5 everywhere.
        let splits = chain().splits();
        assert_eq!(splits.len(), 3);
        assert_eq!(splits[0], (vec![], 1, 4));
        assert_eq!(splits[1], (vec![true], 1, 3));
        assert_eq!(splits[2], (vec![true, true], 1, 2));
    }

    #[test]
    fn splits_walk_both_branches_of_a_balanced_tree() {
        let grid = split(
            "right",
            split("down", pane("a"), pane("b")),
            split("down", pane("c"), pane("d")),
        );
        let splits = grid.splits();
        assert_eq!(splits[0], (vec![], 2, 4));
        assert_eq!(splits[1], (vec![false], 1, 2));
        assert_eq!(splits[2], (vec![true], 1, 2));
    }

    #[test]
    fn a_lone_pane_has_no_splits() {
        assert!(pane("a").splits().is_empty());
        assert_eq!(pane("a").leaf_count(), 1);
    }
}
