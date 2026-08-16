//! Saved layouts.
//!
//! A template is the shape of a tab with the panes taken out: the split tree,
//! the direction of each split, and how the space is divided. Applying one puts
//! the tab's current panes into those slots, in their existing order.
//!
//! Pane ids are deliberately **not** stored. They belong to the panes that
//! happened to be there when the template was saved, and those are gone by the
//! time anyone reapplies it — a saved layout has to describe a shape, not a
//! particular set of terminals.

use std::collections::BTreeMap;

use herdr_plugin_kit::herdr::LayoutNode;
use herdr_plugin_kit::layout::{Placement, Plan, Side};
use herdr_plugin_kit::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::PLUGIN_ID;

const FILE: &str = "layouts.json";

/// One node of a saved layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Template {
    /// A slot a pane goes into.
    Slot,
    Split {
        side: String,
        /// Share of the space kept by `first`.
        ratio: f32,
        first: Box<Template>,
        second: Box<Template>,
    },
}

impl Template {
    /// Read the shape of a tab, discarding which panes were in it.
    pub fn from_layout(node: &LayoutNode) -> Self {
        match node {
            LayoutNode::Pane { .. } => Template::Slot,
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => Template::Split {
                side: direction.clone(),
                ratio: *ratio,
                first: Box::new(Template::from_layout(first)),
                second: Box::new(Template::from_layout(second)),
            },
        }
    }

    /// How many panes this layout holds.
    pub fn slots(&self) -> usize {
        match self {
            Template::Slot => 1,
            Template::Split { first, second, .. } => first.slots() + second.slots(),
        }
    }

    fn side(&self) -> Side {
        match self {
            Template::Slot => Side::Right,
            Template::Split { side, .. } => Side::parse(side).unwrap_or(Side::Right),
        }
    }

    /// The build order for `panes`, filling slots left to right, top to bottom.
    ///
    /// Mirrors `Plan::from_shape`: the pane that heads a branch has to be split
    /// off before that branch is subdivided, or it will not span its side.
    pub fn plan(&self, panes: &[String]) -> Result<Plan> {
        if panes.len() != self.slots() {
            bail!(
                "This layout holds {} pane{}, but the tab has {}.",
                self.slots(),
                if self.slots() == 1 { "" } else { "s" },
                panes.len()
            );
        }
        let anchor = panes
            .first()
            .cloned()
            .context("a layout needs at least one pane")?;
        let mut placements = Vec::new();
        self.emit(panes, &mut placements);
        Ok(Plan { anchor, placements })
    }

    /// Walk parent-first, so every anchor exists by the time it is used.
    fn emit(&self, panes: &[String], out: &mut Vec<Placement>) {
        let Template::Split {
            first,
            second,
            side,
            ..
        } = self
        else {
            return;
        };
        let cut = first.slots();
        let (left, right) = panes.split_at(cut);
        out.push(Placement {
            pane_id: right[0].clone(),
            anchor: left[0].clone(),
            side: Side::parse(side).unwrap_or(Side::Right),
        });
        first.emit(left, out);
        second.emit(right, out);
    }

    /// Every split's path from the root, paired with its ratio, so the
    /// proportions can be restored after the panes are in place.
    pub fn ratios(&self) -> Vec<(Vec<bool>, f32)> {
        let mut out = Vec::new();
        self.walk(&mut Vec::new(), &mut out);
        out
    }

    fn walk(&self, path: &mut Vec<bool>, out: &mut Vec<(Vec<bool>, f32)>) {
        let Template::Split {
            ratio,
            first,
            second,
            ..
        } = self
        else {
            return;
        };
        out.push((path.clone(), *ratio));
        path.push(false);
        first.walk(path, out);
        path.pop();
        path.push(true);
        second.walk(path, out);
        path.pop();
    }

    /// A compact description for the picker, e.g. `4 panes · 2×2`.
    pub fn describe(&self) -> String {
        let slots = self.slots();
        let shape = match self {
            Template::Slot => "single".to_string(),
            Template::Split { .. } => {
                let across = matches!(self.side(), Side::Right | Side::Left);
                format!("{} split", if across { "vertical" } else { "horizontal" })
            }
        };
        format!(
            "{slots} pane{} · {shape}",
            if slots == 1 { "" } else { "s" }
        )
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

type Saved = BTreeMap<String, Template>;

fn path() -> Option<std::path::PathBuf> {
    herdr_plugin_kit::config::state_dir(PLUGIN_ID).map(|dir| dir.join(FILE))
}

/// Every saved layout, by name. A corrupt file reads as "none saved".
pub fn load() -> Saved {
    let Some(path) = path() else {
        return Saved::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save(name: &str, template: &Template) -> Result<()> {
    let Some(path) = path() else {
        bail!("Could not work out where to keep saved layouts.");
    };
    let mut saved = load();
    saved.insert(name.to_string(), template.clone());
    let raw = serde_json::to_string_pretty(&saved)?;
    std::fs::write(&path, raw)
        .with_context(|| format!("could not write {}", path.display()))
}

pub fn remove(name: &str) -> Result<bool> {
    let Some(path) = path() else {
        return Ok(false);
    };
    let mut saved = load();
    if saved.remove(name).is_none() {
        return Ok(false);
    }
    let raw = serde_json::to_string_pretty(&saved)?;
    std::fs::write(&path, raw)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(side: &str, ratio: f32, first: Template, second: Template) -> Template {
        Template::Split {
            side: side.into(),
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn ids(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("p{i}")).collect()
    }

    /// (r p1 (d p2 p3)) — one pane down the left, two stacked on the right.
    fn sample() -> Template {
        split(
            "right",
            0.4,
            Template::Slot,
            split("down", 0.7, Template::Slot, Template::Slot),
        )
    }

    #[test]
    fn a_saved_layout_forgets_which_panes_were_in_it() {
        let node = LayoutNode::Split {
            direction: "right".into(),
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane {
                pane_id: Some("w1:p1".into()),
            }),
            second: Box::new(LayoutNode::Pane {
                pane_id: Some("w1:p2".into()),
            }),
        };
        let template = Template::from_layout(&node);
        let raw = serde_json::to_string(&template).unwrap();
        assert!(!raw.contains("w1:p1"), "pane ids must not be stored: {raw}");
        assert_eq!(template.slots(), 2);
    }

    #[test]
    fn applying_a_layout_rebuilds_the_same_shape() {
        let plan = sample().plan(&ids(3)).unwrap();
        assert_eq!(plan.simulate().signature(), "(r p1 (d p2 p3))");
    }

    #[test]
    fn a_grid_round_trips_through_save_and_apply() {
        // (r (d p1 p3) (d p2 p4)) — the 2x2 that split order gets wrong.
        let template = split(
            "right",
            0.5,
            split("down", 0.5, Template::Slot, Template::Slot),
            split("down", 0.5, Template::Slot, Template::Slot),
        );
        // Slots fill left-to-right, top-to-bottom: p1 p2 / p3 p4 reading the
        // tree in order gives column-major ids.
        let plan = template.plan(&ids(4)).unwrap();
        assert_eq!(plan.simulate().signature(), "(r (d p1 p2) (d p3 p4))");
    }

    #[test]
    fn a_mismatched_pane_count_is_refused_with_both_numbers() {
        let err = sample().plan(&ids(2)).unwrap_err().to_string();
        assert!(err.contains('3') && err.contains('2'), "{err}");
    }

    #[test]
    fn ratios_are_addressed_by_path_from_the_root() {
        let ratios = sample().ratios();
        assert_eq!(ratios.len(), 2);
        assert_eq!(ratios[0], (vec![], 0.4));
        assert_eq!(ratios[1], (vec![true], 0.7));
    }

    #[test]
    fn a_single_pane_layout_needs_no_splits() {
        let plan = Template::Slot.plan(&ids(1)).unwrap();
        assert!(plan.placements.is_empty());
        assert!(Template::Slot.ratios().is_empty());
    }

    #[test]
    fn a_template_survives_a_round_trip_through_json() {
        let raw = serde_json::to_string(&sample()).unwrap();
        let back: Template = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, sample());
    }
}
