//! Applying an arrangement to a live tab.
//!
//! Two mechanisms, both non-destructive:
//!
//! * **Ratios** — `layout.set_split_ratio` adjusts a split's proportions
//!   without touching any pane. Equalize is nothing but this.
//! * **Shape** — Herdr has no "restructure this tab" call. `layout.apply`
//!   looks like one but replaces the tab outright, killing every process in
//!   it, so it is never used here. Instead panes are parked in a holding tab
//!   and moved back in the order that builds the target tree; `pane.move`
//!   keeps pane ids, and with them the shells and agents running inside.

use herdr_plugin_kit::herdr::{Direction, Herdr, Layout};
use herdr_plugin_kit::{anyhow, Context, Outcome, Result};

use crate::arrange::{Arrangement, Plan, Shape};

/// Even out every split in a tab so all panes get the same area.
///
/// A split's ratio is the share of space its first branch takes, so an even
/// layout wants `leaves(first) / leaves(split)` — not 0.5, which would only be
/// right for perfectly balanced trees.
pub fn equalize(herdr: &Herdr, tab_id: &str) -> Result<Outcome> {
    let layout = herdr
        .layout(tab_id)
        .with_context(|| "Could not read the layout of this tab.")?;

    let splits = layout.root.splits();
    if splits.is_empty() {
        return Ok(Outcome::new("This tab has only one pane."));
    }

    for (path, first_leaves, total_leaves) in &splits {
        let ratio = *first_leaves as f32 / *total_leaves as f32;
        herdr
            .set_split_ratio(tab_id, path, ratio)
            .with_context(|| "Could not resize the panes in this tab.")?;
    }

    let panes = layout.root.leaf_count();
    Ok(Outcome::new("Equalized the layout").with_detail(format!("{panes} panes")))
}

/// Rearrange a tab's panes into `arrangement`.
pub fn arrange(
    herdr: &Herdr,
    tab_id: &str,
    arrangement: Arrangement,
    main: Option<&str>,
) -> Result<Outcome> {
    let layout = herdr
        .layout(tab_id)
        .with_context(|| "Could not read the layout of this tab.")?;
    let panes = layout.root.pane_ids();

    let Some(plan) = arrangement.plan(&panes, main) else {
        return Ok(Outcome::new(format!(
            "{} needs at least two panes in the tab.",
            arrangement.title()
        )));
    };

    let count = panes.len();
    let already_arranged = Shape::from_layout(&layout.root)
        .map(|current| current == plan.simulate())
        .unwrap_or(false);

    if !already_arranged {
        rebuild(herdr, tab_id, &plan, &layout)?;
    }

    // A rebuilt tab starts out with every split at 0.5, which for a nested
    // chain is visibly lopsided, so an arrangement always ends level.
    let _ = equalize(herdr, tab_id);

    if let Some(focused) = &layout.focused_pane_id {
        let _ = herdr.focus_pane(focused);
    }

    Ok(Outcome::new(format!("Arranged as {}", arrangement.title()))
        .with_detail(format!("{count} panes · {}", arrangement.description())))
}

/// Move panes out and back to rebuild the tab's split tree.
///
/// The plan's anchor never leaves, which keeps the tab alive: Herdr closes a
/// tab the moment its last pane departs.
fn rebuild(herdr: &Herdr, tab_id: &str, plan: &Plan, before: &Layout) -> Result<()> {
    let mut holding: Option<String> = None;

    for pane_id in plan.pane_ids().iter().skip(1) {
        let result = match &holding {
            None => herdr.move_pane_to_new_tab(pane_id, Some("Rearranging…"), false),
            Some(tab) => herdr.move_pane_to_tab(pane_id, tab, None, Direction::Right, false),
        };
        let result = result.map_err(|err| restore(herdr, tab_id, before, err))?;

        if holding.is_none() {
            holding = result
                .created_tab
                .map(|tab| tab.tab_id)
                .ok_or_else(|| anyhow!("Herdr did not report the holding tab it created."))?
                .into();
        }
    }

    for placement in &plan.placements {
        herdr
            .move_pane_to_tab(
                &placement.pane_id,
                tab_id,
                Some(&placement.anchor),
                placement.side.split(),
                false,
            )
            .map_err(|err| restore(herdr, tab_id, before, err))?;
    }

    // Herdr closes the holding tab once it empties; this only covers the case
    // where something was left behind.
    if let Some(tab) = holding {
        if let Ok(info) = herdr.tab(&tab) {
            if info.pane_count == 0 {
                let _ = herdr.close_tab(&tab);
            }
        }
    }

    Ok(())
}

/// Put the panes back the way they were after a failed rebuild.
///
/// Best-effort: the point is that no pane is left stranded in a holding tab,
/// even if the exact proportions are lost.
fn restore(herdr: &Herdr, tab_id: &str, before: &Layout, cause: anyhow::Error) -> anyhow::Error {
    let Some(plan) = rebuild_plan_from(before) else {
        return anyhow!("{cause}\nThe layout could not be restored automatically.");
    };

    let mut failed = false;
    for placement in &plan.placements {
        if herdr
            .move_pane_to_tab(
                &placement.pane_id,
                tab_id,
                Some(&placement.anchor),
                placement.side.split(),
                false,
            )
            .is_err()
        {
            failed = true;
        }
    }

    if failed {
        anyhow!("{cause}\nSome panes could not be put back. Check the tab list.")
    } else {
        anyhow!("{cause}\nThe previous layout was restored.")
    }
}

/// Describe an existing layout as a plan, so it can be rebuilt after a failure.
fn rebuild_plan_from(layout: &Layout) -> Option<Plan> {
    let shape = Shape::from_layout(&layout.root)?;
    if shape.pane_ids().len() < 2 {
        return None;
    }
    // Deriving the plan from the shape restores the original nesting exactly,
    // not just the original set of panes.
    Some(Plan::from_shape(&shape))
}
