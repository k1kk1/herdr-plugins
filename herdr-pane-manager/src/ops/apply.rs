//! Executing a validated plan, and undoing it if a later step fails.
//!
//! Herdr only splits right and down, and only swaps within one tab. Left, Up
//! and cross-tab swap are built out of those primitives here, so the rest of
//! the plugin — and the user — sees four directions and one swap
//! (addendum §9, §10).

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::layout::Side;
use herdr_plugin_kit::{anyhow, Outcome, Result};

use super::plan::{Destination, Plan, Verb};
use super::Placement;

/// What actually happened, for verification and messaging.
pub struct Applied {
    pub outcome: Outcome,
    /// Pane to focus afterwards.
    pub focus: Option<String>,
    /// Tab the moved panes should now be in.
    pub landed_in: Option<String>,
    /// Panes that were moved.
    pub panes: Vec<String>,
    /// For Swap: where each pane started, so verification can confirm they
    /// really traded places.
    pub swap: Option<(String, String, String, String)>,
}

pub fn run(herdr: &Herdr, snapshot: &crate::state::Snapshot, plan: &Plan) -> Result<Applied> {
    match plan.verb {
        Verb::Move | Verb::Extract => single(herdr, plan),
        Verb::Merge => merge(herdr, plan),
        Verb::Swap => swap(herdr, snapshot, plan),
    }
}

/// Place one pane into a tab, splitting `anchor` on the given side.
///
/// Left and Up are a right/down split followed by a swap, which leaves the
/// pane on the side the user asked for.
fn place(
    herdr: &Herdr,
    pane_id: &str,
    tab_id: &str,
    anchor: Option<&str>,
    placement: Placement,
    focus: bool,
) -> Result<()> {
    herdr.move_pane_to_tab_with_ratio(
        pane_id,
        tab_id,
        anchor,
        placement.side.split(),
        Some(placement.ratio.for_split(placement.side)),
        focus,
    )?;

    if placement.side.needs_swap() {
        // Only meaningful against a named anchor; without one Herdr chose the
        // target itself and there is nothing to flip against.
        if let Some(anchor) = anchor {
            herdr.swap_panes_in_tab(pane_id, anchor)?;
        }
    }
    Ok(())
}

fn single(herdr: &Herdr, plan: &Plan) -> Result<Applied> {
    let pane_id = plan.panes[0].clone();

    let landed_in = match &plan.destination {
        Destination::Tab {
            tab_id,
            target_pane,
        } => {
            place(
                herdr,
                &pane_id,
                tab_id,
                target_pane.as_deref(),
                plan.placement,
                false,
            )?;
            Some(tab_id.clone())
        }
        Destination::NewTab { label } => herdr
            .move_pane_to_new_tab(&pane_id, label.as_deref(), false)?
            .created_tab
            .map(|tab| tab.tab_id),
        Destination::NewWorkspace { label } => herdr
            .move_pane_to_new_workspace(&pane_id, label.as_deref(), false)?
            .created_tab
            .map(|tab| tab.tab_id),
        Destination::Pane { .. } => return Err(anyhow!("internal: pane destination in a move")),
    };

    let verb = if plan.verb == Verb::Extract {
        "Extracted"
    } else {
        "Moved"
    };

    Ok(Applied {
        outcome: Outcome::new(format!(
            "{verb} \"{}\" to {}",
            plan.subject_name, plan.destination_name
        )),
        focus: Some(pane_id.clone()),
        landed_in,
        panes: vec![pane_id],
        swap: None,
    })
}

fn merge(herdr: &Herdr, plan: &Plan) -> Result<Applied> {
    let Destination::Tab {
        tab_id,
        target_pane,
    } = &plan.destination
    else {
        return Err(anyhow!("internal: merge without a destination tab"));
    };

    // The first pane joins the destination on the side the user chose; the
    // rest reproduce the source tab's own structure around it.
    let first = plan.panes[0].clone();
    place(
        herdr,
        &first,
        tab_id,
        target_pane.as_deref(),
        plan.placement,
        false,
    )?;

    if plan.internal.is_empty() {
        // No structure to keep: chain the remainder so their order survives.
        let mut anchor = first.clone();
        for pane_id in plan.panes.iter().skip(1) {
            place(
                herdr,
                pane_id,
                tab_id,
                Some(&anchor),
                Placement {
                    side: plan.placement.side,
                    ..Placement::default()
                },
                false,
            )?;
            anchor = pane_id.clone();
        }
    } else {
        for placement in &plan.internal {
            place(
                herdr,
                &placement.pane_id,
                tab_id,
                Some(&placement.anchor),
                Placement {
                    side: placement.side,
                    ..Placement::default()
                },
                false,
            )?;
        }
    }

    // Herdr closes an emptied tab itself; this covers the case where it did not.
    if let Ok(tab) = herdr.tab(&plan.source_tab) {
        if tab.pane_count == 0 {
            let _ = herdr.close_tab(&plan.source_tab);
        }
    }

    let count = plan.panes.len();
    let panes = if count == 1 { "pane" } else { "panes" };
    Ok(Applied {
        outcome: Outcome::new(format!(
            "Merged {} into {}",
            plan.subject_name, plan.destination_name
        ))
        .with_detail(format!("{count} {panes} moved")),
        focus: plan.panes.first().cloned(),
        landed_in: Some(tab_id.clone()),
        panes: plan.panes.clone(),
        swap: None,
    })
}

/// Exchange two panes, in the same tab or across tabs.
///
/// Cross-tab swap is two moves. They are ordered so neither tab is ever
/// emptied mid-flight, because Herdr closes an emptied tab and that would
/// destroy the destination the second move needs.
fn swap(herdr: &Herdr, snapshot: &crate::state::Snapshot, plan: &Plan) -> Result<Applied> {
    let source_id = plan.panes[0].clone();
    let target = plan
        .swap_with
        .clone()
        .ok_or_else(|| anyhow!("internal: swap without a target"))?;
    let source_tab = plan.source_tab.clone();
    let target_tab = target.tab_id.clone();

    let outcome = Outcome::new(format!(
        "Swapped \"{}\" with \"{}\"",
        plan.subject_name, plan.destination_name
    ));
    let applied = |outcome: Outcome| Applied {
        outcome,
        focus: Some(source_id.clone()),
        landed_in: Some(target_tab.clone()),
        panes: vec![source_id.clone()],
        swap: Some((
            source_id.clone(),
            source_tab.clone(),
            target.pane_id.clone(),
            target_tab.clone(),
        )),
    };

    if source_tab == target_tab {
        if herdr.swap_panes_in_tab(&source_id, &target.pane_id)? {
            return Ok(applied(outcome));
        }
        // Herdr said cross-tab for panes we believe share a tab: our view is
        // stale, so stop rather than guess.
        return Err(anyhow!(
            "Swap could not be completed.\nPane locations were refreshed."
        ));
    }

    let source_siblings = snapshot.siblings(&source_id, &source_tab);
    let target_siblings = snapshot.siblings(&target.pane_id, &target_tab);
    let source_slot = snapshot.tab(&source_tab).map(|t| t.position - 1);
    let source_label = snapshot
        .tab(&source_tab)
        .and_then(|t| t.tab.label.clone())
        .filter(|l| !l.trim().chars().all(|c| c.is_ascii_digit()));

    let log = &mut Vec::new();
    let step = |log: &mut Vec<(String, String, Option<String>)>,
                pane: &str,
                from_tab: &str,
                to_tab: &str,
                anchor: Option<&str>,
                undo_anchor: Option<&str>|
     -> Result<()> {
        herdr
            .move_pane_to_tab(pane, to_tab, anchor, Side::Right.split(), false)
            .map_err(|err| anyhow!("Swap could not be completed.\n{err}"))?;
        log.push((
            pane.to_string(),
            from_tab.to_string(),
            undo_anchor.map(str::to_string),
        ));
        Ok(())
    };

    let mut recreated = false;
    let result = (|| -> Result<()> {
        match (source_siblings.first(), target_siblings.first()) {
            // Both tabs keep a pane throughout: anchor each move on a sibling.
            (Some(anchor_a), Some(anchor_b)) => {
                step(log, &source_id, &source_tab, &target_tab, Some(anchor_b), Some(anchor_a))?;
                step(log, &target.pane_id, &target_tab, &source_tab, Some(anchor_a), Some(anchor_b))?;
            }
            // The source is alone: bring the target over before the source
            // leaves, so the source's tab never empties.
            (None, Some(anchor_b)) => {
                step(log, &target.pane_id, &target_tab, &source_tab, Some(&source_id), Some(anchor_b))?;
                step(log, &source_id, &source_tab, &target_tab, Some(anchor_b), Some(&target.pane_id))?;
            }
            (Some(anchor_a), None) => {
                step(log, &source_id, &source_tab, &target_tab, Some(&target.pane_id), Some(anchor_a))?;
                step(log, &target.pane_id, &target_tab, &source_tab, Some(anchor_a), Some(&source_id))?;
            }
            // Both are alone. One tab has to close and be recreated; its name
            // and its slot in the tab bar are carried over, so from the
            // outside the two panes simply traded places.
            (None, None) => {
                recreated = true;
                step(log, &source_id, &source_tab, &target_tab, Some(&target.pane_id), None)?;
                let result = herdr
                    .move_pane_to_new_tab(&target.pane_id, source_label.as_deref(), false)
                    .map_err(|err| anyhow!("Swap could not be completed.\n{err}"))?;
                log.push((target.pane_id.clone(), target_tab.clone(), None));
                if let (Some(created), Some(slot)) = (result.created_tab, source_slot) {
                    // Cosmetic: a failure here leaves a correct swap with the
                    // tab at the end of the bar, not worth unwinding for.
                    let _ = herdr.move_tab(&created.tab_id, slot);
                }
            }
        }
        Ok(())
    })();

    if let Err(cause) = result {
        return Err(rollback(herdr, log, cause));
    }

    Ok(applied(if recreated {
        outcome.with_detail("Both panes were alone in their tabs, so one tab was recreated.")
    } else {
        outcome
    }))
}

/// Undo the moves recorded so far, newest first (spec §6.3, §15.3).
fn rollback(
    herdr: &Herdr,
    log: &[(String, String, Option<String>)],
    cause: anyhow::Error,
) -> anyhow::Error {
    if log.is_empty() {
        return cause;
    }

    let mut failures = Vec::new();
    for (pane_id, tab_id, anchor) in log.iter().rev() {
        let tab_alive = herdr.tab(tab_id).is_ok();
        let result = if tab_alive {
            herdr
                .move_pane_to_tab(pane_id, tab_id, anchor.as_deref(), Side::Right.split(), false)
                .map(|_| ())
        } else {
            // Its original tab is gone; a tab of its own beats being stacked
            // somewhere the user would not think to look.
            herdr.move_pane_to_new_tab(pane_id, None, false).map(|_| ())
        };
        if let Err(err) = result {
            failures.push(format!("{pane_id}: {err}"));
        }
    }

    if failures.is_empty() {
        anyhow!("{cause}\nPane locations were restored.")
    } else {
        anyhow!(
            "{cause}\nRollback was incomplete. Pane locations were refreshed.\n{}",
            failures.join("; ")
        )
    }
}
