//! Confirming an operation actually happened (addendum §8).
//!
//! A successful API response says the request was accepted, not that the
//! session ended up the way the user asked. Re-reading the panes afterwards
//! is cheap and turns a silent no-op into a visible error.

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Result};

use super::apply::Applied;
use super::plan::{Plan, Verb};

pub fn check(herdr: &Herdr, plan: &Plan, applied: &Applied) -> Result<()> {
    match plan.verb {
        Verb::Move | Verb::Extract | Verb::Merge => landed(herdr, plan, applied),
        Verb::Swap => traded(herdr, applied),
    }
}

/// Every moved pane is in the tab it was sent to, and out of the one it left.
fn landed(herdr: &Herdr, plan: &Plan, applied: &Applied) -> Result<()> {
    let Some(expected) = applied.landed_in.as_deref() else {
        // A new tab or workspace: Herdr chose the id, so all we can check is
        // that the pane still exists and has moved off its old tab.
        for pane_id in &applied.panes {
            let pane = herdr.pane(pane_id).map_err(|err| {
                herdr_plugin_kit::anyhow!("{} could not be verified.\n{err}", verb_label(plan.verb))
            })?;
            if pane.tab_id == plan.source_tab {
                bail!("{} could not be verified.", verb_label(plan.verb));
            }
        }
        return Ok(());
    };

    for pane_id in &applied.panes {
        let pane = herdr.pane(pane_id).map_err(|err| {
            herdr_plugin_kit::anyhow!("{} could not be verified.\n{err}", verb_label(plan.verb))
        })?;
        if pane.tab_id != expected {
            bail!(
                "{} could not be verified.\n\"{}\" is not in the destination tab.",
                verb_label(plan.verb),
                plan.subject_name
            );
        }
    }
    Ok(())
}

/// The two panes really are in each other's tabs.
fn traded(herdr: &Herdr, applied: &Applied) -> Result<()> {
    let Some((source_id, source_tab, target_id, target_tab)) = applied.swap.as_ref() else {
        return Ok(());
    };
    // A same-tab swap changes positions within one tab, which pane ids alone
    // cannot show; the API's `changed` flag already covered that case.
    if source_tab == target_tab {
        return Ok(());
    }

    let source = herdr.pane(source_id)?;
    let target = herdr.pane(target_id)?;
    if source.tab_id != *target_tab || target.tab_id != *source_tab {
        bail!("Swap could not be verified.\nPane locations were refreshed.");
    }
    Ok(())
}

/// The same name the menu entry uses, so a failure names the thing the user
/// actually pressed.
fn verb_label(verb: Verb) -> &'static str {
    verb.label()
}
