//! The operation layer (spec §22, addendum §13).
//!
//! Every invocation path — context menu, keyboard overlay, plugin action,
//! headless CLI — goes through [`execute`]. Nothing else calls Herdr to move a
//! pane, so behaviour cannot drift between paths.
//!
//! The pipeline is:
//!
//! ```text
//! Request → refresh topology → validate → plan → apply → verify → focus
//! ```
//!
//! Refreshing and validating immediately before applying is what makes the
//! plugin safe to leave a picker open in: the user, or an agent, may have
//! moved something in the meantime (addendum §7). Verifying afterwards is what
//! stops a successful-looking API response being reported as a completed move
//! when it was not (addendum §8).

mod apply;
mod plan;
mod verify;

pub use plan::{Destination, Request, Verb};

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::layout::{Ratio, Side};
use herdr_plugin_kit::{Outcome, Result};

use crate::config::Config;
use crate::state::Snapshot;

/// Placement options shared by every operation that puts a pane somewhere.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub side: Side,
    pub ratio: Ratio,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            side: Side::Right,
            ratio: Ratio::EVEN,
        }
    }
}

/// Run one request end to end.
pub fn execute(
    herdr: &Herdr,
    snapshot: &Snapshot,
    request: &Request,
    config: &Config,
) -> Result<Outcome> {
    // 1. Refresh: the snapshot behind the picker may be seconds old.
    let fresh = snapshot.refresh(herdr)?;

    // 2. Validate: everything the plan will touch must still exist, and the
    //    session must not have been reorganised underneath us.
    let plan = plan::build(&fresh, request)?;

    // 3. Record the way back, while the panes are still where they started.
    //    Afterwards this information no longer exists anywhere.
    let mut record = crate::undo::capture(
        herdr,
        plan.verb,
        &plan.subject_name,
        &plan.panes,
        plan.swap_with
            .as_ref()
            .map(|other| (plan.panes[0].clone(), other.pane_id.clone())),
    );

    // 4. Apply.
    let applied = apply::run(herdr, &fresh, &plan)?;

    // 5. Verify against freshly read state rather than the API's own word.
    verify::check(herdr, &plan, &applied)?;

    // A tab the operation brought into being is empty once the panes go home,
    // so the undo should clear it away rather than leave a husk behind.
    if matches!(
        plan.destination,
        Destination::NewTab { .. } | Destination::NewWorkspace { .. }
    ) {
        if let Some(tab_id) = applied.landed_in.as_ref() {
            record.created_tabs.push(tab_id.clone());
        }
    }
    // Only offer an undo for an operation that actually completed.
    let _ = crate::undo::save(&record);

    // 6. Focus follows the operation unless the user turned that off.
    if config.focus_after_operation {
        if let Some(pane_id) = applied.focus.as_deref() {
            let _ = herdr.focus_pane(pane_id);
        }
    }

    Ok(applied.outcome)
}
