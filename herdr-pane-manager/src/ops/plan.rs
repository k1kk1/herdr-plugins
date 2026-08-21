//! Turning a user's request into a validated, executable plan.
//!
//! This is the stage that says no. By the time a plan exists, every pane and
//! tab it names has been re-read from Herdr and still exists, so the apply
//! stage can be a straight sequence of calls (addendum §7).

use herdr_plugin_kit::herdr::Pane;
use herdr_plugin_kit::label;
use herdr_plugin_kit::layout::Plan as LayoutPlan;
use herdr_plugin_kit::{bail, Result};

use super::Placement;
use crate::state::Snapshot;

/// Which of the four core operations was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Move,
    Swap,
    Extract,
    Merge,
}

impl Verb {
    /// How the operation is named to the user.
    pub fn label(self) -> &'static str {
        match self {
            Verb::Move => "Move",
            Verb::Swap => "Swap",
            Verb::Extract => "Extract",
            // Named for the menu entry it undoes, not for the internal verb.
            Verb::Merge => "Fold",
        }
    }
}

/// Where a pane or a tab's contents should end up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// An existing tab, optionally splitting a named pane inside it.
    Tab {
        tab_id: String,
        target_pane: Option<String>,
    },
    /// A tab that does not exist yet.
    NewTab { label: Option<String> },
    /// A workspace that does not exist yet.
    NewWorkspace { label: Option<String> },
    /// Another pane, for Swap.
    Pane { pane_id: String },
}

/// What the user asked for, before validation.
#[derive(Debug, Clone)]
pub struct Request {
    pub verb: Verb,
    /// Pane to act on. For Merge this only identifies the workspace.
    pub source_pane: String,
    /// Tab to act on, for Merge.
    pub source_tab: Option<String>,
    pub destination: Destination,
    pub placement: Placement,
    /// Preserve the source tab's internal split structure when merging
    /// (addendum §11).
    pub preserve_layout: bool,
}

/// A validated plan: every id in here existed a moment ago.
#[derive(Debug, Clone)]
pub struct Plan {
    pub verb: Verb,
    pub destination: Destination,
    pub placement: Placement,
    /// Panes being moved, in the order they must be placed.
    pub panes: Vec<String>,
    /// How the moved panes relate to each other once the first one has landed.
    /// Empty for a single pane, or when the source layout is not being kept.
    pub internal: Vec<herdr_plugin_kit::layout::Placement>,
    /// Tab the panes came from, for verification and for closing when emptied.
    pub source_tab: String,
    /// Pane the operation is swapping with, for Swap.
    pub swap_with: Option<Pane>,
    /// Human-readable name of the destination, for messages.
    pub destination_name: String,
    /// Human-readable name of what is being moved, for messages.
    pub subject_name: String,
}

/// Validate a request against fresh state and produce a plan.
pub fn build(snapshot: &Snapshot, request: &Request) -> Result<Plan> {
    // The source pane must still exist, and still be where we last saw it.
    let source = snapshot
        .pane(&request.source_pane)
        .cloned()
        .unwrap_or_else(|| snapshot.source.clone());
    if source.pane_id != request.source_pane {
        bail!("The pane you selected no longer exists.");
    }

    match request.verb {
        Verb::Move => build_move(snapshot, request, &source),
        Verb::Extract => build_extract(request, &source),
        Verb::Swap => build_swap(snapshot, request, &source),
        Verb::Merge => build_merge(snapshot, request, &source),
    }
}

fn build_move(snapshot: &Snapshot, request: &Request, source: &Pane) -> Result<Plan> {
    let destination_name = match &request.destination {
        Destination::Tab {
            tab_id,
            target_pane,
        } => {
            let entry = snapshot.require_tab(tab_id)?;
            if entry.tab.tab_id == source.tab_id {
                bail!(
                    "\"{}\" is already in {}.",
                    label::pane_compact(source),
                    label::tab_display(&entry.tab, entry.position)
                );
            }
            if let Some(target) = target_pane {
                let pane = snapshot.require_pane(target)?;
                if pane.tab_id != *tab_id {
                    bail!("The pane you chose to split is no longer in that tab.");
                }
            }
            label::tab_display(&entry.tab, entry.position)
        }
        Destination::NewTab { label } => label
            .clone()
            .map(|l| format!("a new tab \"{l}\""))
            .unwrap_or_else(|| "a new tab".into()),
        Destination::NewWorkspace { label } => label
            .clone()
            .map(|l| format!("a new workspace \"{l}\""))
            .unwrap_or_else(|| "a new workspace".into()),
        Destination::Pane { .. } => bail!("Move needs a tab, not a pane."),
    };

    Ok(Plan {
        verb: Verb::Move,
        destination: request.destination.clone(),
        placement: request.placement,
        panes: vec![source.pane_id.clone()],
        internal: Vec::new(),
        source_tab: source.tab_id.clone(),
        swap_with: None,
        destination_name,
        subject_name: label::pane_compact(source),
    })
}

fn build_extract(request: &Request, source: &Pane) -> Result<Plan> {
    let destination_name = match &request.destination {
        Destination::NewTab { label } => label.clone().unwrap_or_else(|| "a new tab".into()),
        Destination::NewWorkspace { label } => {
            label.clone().unwrap_or_else(|| "a new workspace".into())
        }
        _ => bail!("Extract creates a new tab or workspace."),
    };

    Ok(Plan {
        verb: Verb::Extract,
        destination: request.destination.clone(),
        placement: request.placement,
        panes: vec![source.pane_id.clone()],
        internal: Vec::new(),
        source_tab: source.tab_id.clone(),
        swap_with: None,
        destination_name,
        subject_name: label::pane_compact(source),
    })
}

fn build_swap(snapshot: &Snapshot, request: &Request, source: &Pane) -> Result<Plan> {
    let Destination::Pane { pane_id } = &request.destination else {
        bail!("Swap needs another pane.");
    };
    if *pane_id == source.pane_id {
        bail!("A pane cannot be swapped with itself.");
    }
    let target = snapshot.require_pane(pane_id)?.clone();

    Ok(Plan {
        verb: Verb::Swap,
        destination: request.destination.clone(),
        placement: request.placement,
        panes: vec![source.pane_id.clone()],
        internal: Vec::new(),
        source_tab: source.tab_id.clone(),
        destination_name: label::pane_compact(&target),
        subject_name: label::pane_compact(source),
        swap_with: Some(target),
    })
}

fn build_merge(snapshot: &Snapshot, request: &Request, source: &Pane) -> Result<Plan> {
    let source_tab_id = request
        .source_tab
        .clone()
        .unwrap_or_else(|| source.tab_id.clone());
    let Some(source_tab) = snapshot.tab(&source_tab_id) else {
        bail!("Source tab no longer exists.");
    };

    let Destination::Tab { tab_id, .. } = &request.destination else {
        bail!("Merge needs a destination tab.");
    };
    if *tab_id == source_tab_id {
        bail!("A tab cannot be merged into itself.");
    }
    let destination = snapshot.require_tab(tab_id)?;

    if source_tab.panes.is_empty() {
        bail!("That tab has no panes to merge.");
    }

    // Carry the source tab's own split structure across, so a two-over-one
    // arrangement stays a two-over-one arrangement (addendum §11). Falling
    // back to layout order costs only the nesting, never a pane.
    let (panes, internal) = match (request.preserve_layout, &source_tab.shape) {
        (true, Some(shape)) => {
            let layout_plan = LayoutPlan::from_shape(shape);
            (layout_plan.pane_ids(), layout_plan.placements)
        }
        _ => (
            source_tab.panes.iter().map(|p| p.pane_id.clone()).collect(),
            Vec::new(),
        ),
    };

    Ok(Plan {
        verb: Verb::Merge,
        destination: request.destination.clone(),
        placement: request.placement,
        panes,
        internal,
        source_tab: source_tab_id,
        swap_with: None,
        destination_name: label::tab_display(&destination.tab, destination.position),
        subject_name: label::tab_display(&source_tab.tab, source_tab.position),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_plugin_kit::layout::{Shape, Side};

    /// Build a shape by replaying splits, the way Herdr does.
    fn shape_of(splits: &[(&str, &str, &str)], root: &str) -> Shape {
        let mut shape = Shape::pane(root);
        for (anchor, pane, side) in splits {
            shape.split(anchor, pane, Side::parse(side).unwrap());
        }
        shape
    }

    #[test]
    fn a_preserved_merge_replays_the_source_tabs_own_splits() {
        // The addendum's example: B|C side by side, with D across the bottom.
        let shape = shape_of(&[("b", "d", "down"), ("b", "c", "right")], "b");
        assert_eq!(shape.signature(), "(d (r b c) d)");

        let plan = LayoutPlan::from_shape(&shape);
        assert_eq!(plan.anchor, "b");
        // D is placed before C, because B has to be split downwards before it
        // is split sideways or D would only span half the width.
        assert_eq!(plan.pane_ids(), vec!["b", "d", "c"]);
        // Replaying it reproduces the arrangement rather than a flat row.
        assert_eq!(plan.simulate().signature(), shape.signature());
    }

    #[test]
    fn a_flattened_merge_keeps_the_panes_but_not_the_nesting() {
        let shape = shape_of(&[("b", "d", "down"), ("b", "c", "right")], "b");
        let mut flat = shape.pane_ids();
        flat.sort();
        let mut preserved = LayoutPlan::from_shape(&shape).pane_ids();
        preserved.sort();
        // Either way every pane comes across; only the arrangement differs.
        assert_eq!(flat, preserved);
    }
}
