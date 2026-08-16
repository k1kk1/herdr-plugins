//! Active Agent Gather (addendum §1–§19).
//!
//! Collects the agent panes that need attention into dedicated tabs, and puts
//! them back afterwards. It runs through the same pipeline shape as the other
//! operations — snapshot, validate, plan, execute, verify, focus — but moves
//! many panes at once, so the plan and the way back are written down first
//! (addendum §11, §18).

pub mod layout;
pub mod select;
pub mod session;

use herdr_plugin_kit::herdr::{Agent, Herdr};
use herdr_plugin_kit::label;
use herdr_plugin_kit::layout::Side;
use herdr_plugin_kit::{anyhow, bail, Outcome, Result};

use crate::config::Config;
use crate::place;
use layout::PanesPerTab;
use select::Scope;
use session::{Origin, Session};

/// Tab used to hold panes while the Gather tabs are rebuilt.
///
/// Herdr treats a move into a pane's current tab as a no-op, so reshaping a
/// tab means taking its panes out and putting them back. The holding tab
/// closes itself the moment the last pane leaves it.
const HOLDING_LABEL: &str = "Gathering…";

/// Collect the active agents into Gather tabs.
///
/// Running this while a Gather is already in place refreshes it rather than
/// nesting a second one, which is also how the existing tabs get reused
/// (addendum §8).
pub fn gather(
    herdr: &Herdr,
    config: &Config,
    per_tab: PanesPerTab,
    scope: Scope,
) -> Result<Outcome> {
    let existing = session::load();
    let wanted = active_agents(herdr, config, scope)?;

    if wanted.is_empty() {
        // Nothing to gather. If something was gathered before, this is really
        // a refresh that emptied out, so put those panes back.
        if let Some(existing) = existing {
            restore_session(herdr, existing)?;
            return Ok(Outcome::new("No agent needs attention")
                .with_detail("The gathered panes were returned to their tabs."));
        }
        return Ok(Outcome::new("No agent needs attention").with_detail(format!(
            "Looking for {} in {}.",
            config.gather.status_summary(),
            scope.label()
        )));
    }

    let mut session = existing.unwrap_or_default();

    // Panes that were gathered but no longer qualify go home first, so they
    // are not carried into the new layout (addendum §13).
    let keep: Vec<String> = wanted.iter().map(|a| a.pane_id.clone()).collect();
    let dropped: Vec<Origin> = session.origins_other_than(&keep);
    let returned = dropped.len();
    for origin in &dropped {
        session.forget(&origin.pane_id);
    }
    if !dropped.is_empty() {
        place::restore(herdr, &dropped)?;
    }

    // Record where the newcomers came from, before anything moves.
    for agent in &wanted {
        if !session.contains(&agent.pane_id) {
            match place::origin_of(herdr, &agent.pane_id) {
                Ok(origin) => session.origins.push(origin),
                // A pane we cannot describe is one we could not put back, so
                // leave it where it is rather than stranding it.
                Err(_) => continue,
            }
        }
    }

    let order: Vec<String> = wanted
        .iter()
        .map(|a| a.pane_id.clone())
        .filter(|id| session.contains(id))
        .collect();
    if order.is_empty() {
        bail!("Could not read where the active agents currently are.");
    }

    session.scope = scope.as_str().to_string();
    session.panes_per_tab = per_tab.get() as u8;
    if session.created_unix_ms == 0 {
        session.created_unix_ms = session::now_unix_ms();
    }
    // Persist before moving: a crash mid-move must still leave a way back.
    session::save(&session)?;

    // Hand the previous run's tabs over so a Refresh reuses them in place.
    let reuse = session.gather_tabs.clone();
    let tabs = build(herdr, &order, per_tab, config, &reuse)?;
    session.gather_tabs = tabs.clone();
    session::save(&session)?;

    verify_gathered(herdr, &order, &tabs)?;

    if config.gather.focus_highest_priority {
        if let Some(agent) = select::highest_priority(&wanted) {
            let _ = herdr.focus_pane(&agent.pane_id);
        }
    }

    let count = order.len();
    let tab_word = if tabs.len() == 1 { "tab" } else { "tabs" };
    let mut detail = format!(
        "{count} agent{} · {} {tab_word} · {}",
        if count == 1 { "" } else { "s" },
        tabs.len(),
        scope.label()
    );
    if returned > 0 {
        detail.push_str(&format!(" · {returned} returned"));
    }
    Ok(Outcome::new("Gathered the active agents").with_detail(detail))
}

/// Rebuild the Gather from the agents' current states (addendum §13).
pub fn refresh(herdr: &Herdr, config: &Config) -> Result<Outcome> {
    let Some(existing) = session::load() else {
        bail!("Nothing is gathered right now.");
    };
    let per_tab = PanesPerTab::new(existing.panes_per_tab).unwrap_or(config.gather.per_tab());
    let scope = Scope::parse(&existing.scope).unwrap_or(config.gather.scope());
    gather(herdr, config, per_tab, scope)
}

/// Put every gathered pane back where it came from (addendum §12).
pub fn restore(herdr: &Herdr) -> Result<Outcome> {
    let Some(existing) = session::load() else {
        bail!("Nothing is gathered right now.");
    };
    restore_session(herdr, existing)
}

fn restore_session(herdr: &Herdr, existing: Session) -> Result<Outcome> {
    let count = existing.origins.len();
    let restored = place::restore(herdr, &existing.origins)?;
    session::clear();

    // Gather tabs empty out as their panes leave and Herdr closes them; this
    // only catches one that survived.
    for tab_id in &existing.gather_tabs {
        if let Ok(tab) = herdr.tab(tab_id) {
            if tab.pane_count == 0 {
                let _ = herdr.close_tab(tab_id);
            }
        }
    }

    if let Some(focused) = existing.origins.iter().find(|o| o.focused) {
        let _ = herdr.focus_pane(&focused.pane_id);
    }

    Ok(Outcome::new("Returned the gathered agents").with_detail(restored.detail(count)))
}

/// How many agents a Gather would collect right now, for the menu.
pub fn count_active(herdr: &Herdr, config: &Config) -> Result<usize> {
    active_agents(herdr, config, config.gather.scope()).map(|agents| agents.len())
}

/// Active agents for a scope, most urgent first.
fn active_agents(herdr: &Herdr, config: &Config, scope: Scope) -> Result<Vec<Agent>> {
    let agents = herdr.agents()?;
    let workspace = match scope {
        Scope::AllWorkspaces => None,
        Scope::CurrentWorkspace => Some(herdr.focused_workspace()?.workspace_id),
    };
    Ok(select::select(&agents, &config.gather, workspace.as_deref()))
}


/// Move the chosen panes into Gather tabs and lay them out.
fn build(
    herdr: &Herdr,
    order: &[String],
    per_tab: PanesPerTab,
    config: &Config,
    reuse: &[String],
) -> Result<Vec<String>> {
    let groups = layout::chunk(order, per_tab);
    let multiple = groups.len() > 1;

    // Gather tabs from the previous run that are still open. Reusing them keeps
    // the tab's identity and its slot in the tab bar, so a Refresh does not make
    // the Active Agents tab jump to the end every time.
    let mut spare: Vec<String> = reuse
        .iter()
        .filter(|tab| herdr.tab(tab).is_ok())
        .cloned()
        .collect();

    // Each group's landing tab, and the pane that holds that tab open while the
    // rest are parked. Herdr closes a tab the moment its last pane leaves, so an
    // anchor has to stay behind or the tab we are trying to keep disappears.
    let mut anchored: Vec<Option<String>> = Vec::new();
    for group in &groups {
        let head = &group[0];
        let Some(tab) = spare.first().cloned() else {
            anchored.push(None);
            continue;
        };
        spare.remove(0);

        // Put the head pane in that tab before anything is parked.
        let head_is_home = herdr
            .pane(head)
            .map(|pane| pane.tab_id == tab)
            .unwrap_or(false);
        if !head_is_home && herdr.move_pane_to_tab(head, &tab, None, Side::Right.split(), false).is_err() {
            anchored.push(None);
            continue;
        }
        anchored.push(Some(tab));
    }

    // Park everyone except the anchors. A pane cannot be repositioned inside the
    // tab it already sits in, so it has to leave and come back.
    let parked: Vec<String> = order
        .iter()
        .filter(|pane| {
            !anchored
                .iter()
                .zip(&groups)
                .any(|(tab, group)| tab.is_some() && &&group[0] == pane)
        })
        .cloned()
        .collect();
    let holding = park(herdr, &parked)?;

    let mut tabs = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let label = if multiple {
            format!("{} {}", config.gather.tab_label, index + 1)
        } else {
            config.gather.tab_label.clone()
        };

        let tab = match anchored[index].clone() {
            Some(tab) => {
                // The group count may have changed, so the name may need to too.
                let _ = herdr.rename_tab(&tab, &label);
                tab
            }
            None => herdr
                .move_pane_to_new_tab(&group[0], Some(&label), false)?
                .created_tab
                .map(|tab| tab.tab_id)
                .ok_or_else(|| {
                    anyhow!("Herdr did not report the tab it created for the agents.")
                })?,
        };

        if let Some(plan) = layout::plan(group) {
            for placement in &plan.placements {
                herdr.move_pane_to_tab(
                    &placement.pane_id,
                    &tab,
                    Some(&placement.anchor),
                    placement.side.split(),
                    false,
                )?;
                if placement.side.needs_swap() {
                    let _ = herdr.swap_panes_in_tab(&placement.pane_id, &placement.anchor);
                }
            }
        }
        equalize(herdr, &tab);
        tabs.push(tab);
    }

    // Gather tabs left over from a run that needed more of them.
    for leftover in spare {
        if let Ok(tab) = herdr.tab(&leftover) {
            if tab.pane_count == 0 {
                let _ = herdr.close_tab(&leftover);
            }
        }
    }

    // The holding tab empties as the last group leaves and Herdr closes it.
    if let Some(holding) = holding {
        if let Ok(tab) = herdr.tab(&holding) {
            if tab.pane_count == 0 {
                let _ = herdr.close_tab(&holding);
            }
        }
    }

    Ok(tabs)
}

/// Move every pane into one temporary tab, returning its id.
fn park(herdr: &Herdr, panes: &[String]) -> Result<Option<String>> {
    let mut holding: Option<String> = None;
    for pane_id in panes {
        match &holding {
            None => {
                holding = herdr
                    .move_pane_to_new_tab(pane_id, Some(HOLDING_LABEL), false)?
                    .created_tab
                    .map(|tab| tab.tab_id);
            }
            Some(tab) => {
                herdr.move_pane_to_tab(pane_id, tab, None, Side::Right.split(), false)?;
            }
        }
    }
    Ok(holding)
}

/// Even out a freshly built Gather tab.
///
/// A rebuilt tab starts with every split at 0.5, which for the three-pane
/// layout leaves the main agent at half width instead of the intended half of
/// the whole tab. Best-effort: a wrong ratio is cosmetic.
fn equalize(herdr: &Herdr, tab_id: &str) {
    let Ok(exported) = herdr.layout(tab_id) else {
        return;
    };
    for (path, first_leaves, total_leaves) in exported.root.splits() {
        let ratio = first_leaves as f32 / total_leaves as f32;
        let _ = herdr.set_split_ratio(tab_id, &path, ratio);
    }
}

/// Confirm the panes really are in the Gather tabs (addendum §18).
fn verify_gathered(herdr: &Herdr, panes: &[String], tabs: &[String]) -> Result<()> {
    for pane_id in panes {
        let pane = herdr
            .pane(pane_id)
            .map_err(|err| anyhow!("Gather could not be verified.\n{err}"))?;
        if !tabs.contains(&pane.tab_id) {
            bail!(
                "Gather could not be verified.\n\"{}\" is not in a gathered tab.",
                label::pane_compact(&pane)
            );
        }
    }
    Ok(())
}
