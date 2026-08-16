//! Working out what the user was looking at when they invoked a plugin.
//!
//! A plugin process can be started from a pane context menu, from a key
//! binding, or from another plugin, and each path supplies the surrounding
//! state differently. This module funnels all of them into one answer.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::herdr::{Herdr, Pane};

/// The plugin's own pane, when it is running inside one. Never a candidate.
pub fn self_pane_id() -> Option<String> {
    non_empty_env("HERDR_PANE_ID")
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// Herdr's invocation context for plugin actions (`HERDR_PLUGIN_CONTEXT_JSON`).
///
/// This is how the pane and tab context menus say what was right-clicked.
#[derive(Debug, Default, Clone)]
pub struct InvocationContext {
    pub focused_pane_id: Option<String>,
    pub tab_id: Option<String>,
    pub workspace_id: Option<String>,
}

impl InvocationContext {
    pub fn from_env() -> Self {
        let parsed = non_empty_env("HERDR_PLUGIN_CONTEXT_JSON")
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());

        let field = |key: &str| -> Option<String> {
            parsed
                .as_ref()?
                .get(key)?
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };

        Self {
            focused_pane_id: field("focused_pane_id"),
            tab_id: field("tab_id").or_else(|| non_empty_env("HERDR_TAB_ID")),
            workspace_id: field("workspace_id").or_else(|| non_empty_env("HERDR_WORKSPACE_ID")),
        }
    }

    /// Rebuild the context to hand on when this plugin invokes another one,
    /// so the callee sees the user's pane rather than our popup.
    pub fn to_params(&self, pane: Option<&Pane>) -> Value {
        let mut context = json!({ "invocation_source": "plugin" });
        if let Some(pane) = pane {
            context["focused_pane_id"] = json!(pane.pane_id);
            context["tab_id"] = json!(pane.tab_id);
            context["workspace_id"] = json!(pane.workspace_id);
            if let Some(cwd) = &pane.cwd {
                context["focused_pane_cwd"] = json!(cwd);
            }
            if let Some(agent) = &pane.agent {
                context["focused_pane_agent"] = json!(agent);
            }
        } else {
            if let Some(id) = &self.focused_pane_id {
                context["focused_pane_id"] = json!(id);
            }
            if let Some(id) = &self.tab_id {
                context["tab_id"] = json!(id);
            }
            if let Some(id) = &self.workspace_id {
                context["workspace_id"] = json!(id);
            }
        }
        context
    }
}

/// Pane the user was on, resolved in decreasing order of confidence:
///
/// 1. an explicit `--pane` flag
/// 2. `PM_SOURCE_PANE`, set by a launcher that resolved it before the UI
///    stole focus
/// 3. the plugin invocation context, i.e. the pane that was right-clicked
/// 4. `HERDR_ACTIVE_PANE_ID`, set for `[[keys.command]]` bindings
/// 5. whichever pane Herdr currently reports as focused
///
/// The plugin's own UI pane is excluded at every step.
pub fn resolve_source_pane(herdr: &Herdr, explicit: Option<&str>) -> Result<Pane> {
    let own = self_pane_id();

    let candidates: Vec<String> = explicit
        .map(str::to_string)
        .into_iter()
        .chain(non_empty_env("PM_SOURCE_PANE"))
        .chain(InvocationContext::from_env().focused_pane_id)
        .chain(non_empty_env("HERDR_ACTIVE_PANE_ID"))
        .filter(|id| Some(id) != own.as_ref())
        .collect();

    let mut last_err = None;
    for id in candidates {
        match herdr.pane(&id) {
            Ok(pane) => return Ok(pane),
            Err(err) => last_err = Some(err),
        }
    }

    match focused_pane(herdr, own.as_deref()) {
        Ok(pane) => Ok(pane),
        Err(err) => Err(last_err.unwrap_or(err)),
    }
}

/// The focused pane of the focused workspace, ignoring the plugin's own pane.
pub fn focused_pane(herdr: &Herdr, exclude: Option<&str>) -> Result<Pane> {
    let workspace = herdr.focused_workspace()?;
    let panes = herdr.panes(&workspace.workspace_id)?;
    panes
        .iter()
        .find(|p| p.focused && Some(p.pane_id.as_str()) != exclude)
        .or_else(|| panes.iter().find(|p| Some(p.pane_id.as_str()) != exclude))
        .cloned()
        .ok_or_else(|| anyhow!("no pane is available in this workspace"))
}

/// Tab an operation should act on: an explicit flag, the tab the context menu
/// was opened on, then the source pane's own tab.
///
/// The inherited values are checked against the source pane's workspace,
/// because `PM_SOURCE_TAB` and the plugin context outlive the pane they were
/// set for and can point at a tab in a workspace that is no longer in play.
///
/// An explicit `--tab` is **not** filtered. Someone naming a tab on the command
/// line means that tab, and a caller scripting across workspaces would
/// otherwise have the argument silently swapped for the current pane's tab.
pub fn resolve_source_tab(explicit: Option<&str>, source: &Pane) -> String {
    if let Some(explicit) = explicit.filter(|id| !id.trim().is_empty()) {
        return explicit.to_string();
    }
    let same_workspace = |id: &String| id.starts_with(&format!("{}:", source.workspace_id));
    non_empty_env("PM_SOURCE_TAB")
        .or_else(|| InvocationContext::from_env().tab_id)
        .filter(same_workspace)
        .unwrap_or_else(|| source.tab_id.clone())
}

#[cfg(test)]
mod tab_resolution_tests {
    use super::*;

    fn pane(workspace: &str, tab: &str) -> Pane {
        Pane {
            pane_id: format!("{workspace}:p1"),
            workspace_id: workspace.into(),
            tab_id: tab.into(),
            ..Default::default()
        }
    }

    #[test]
    fn an_explicit_tab_wins_even_in_another_workspace() {
        // Scripting across workspaces has to be possible; silently swapping the
        // argument for the current pane's tab writes to the wrong tab.
        let source = pane("w1", "w1:t1");
        assert_eq!(resolve_source_tab(Some("w9:t3"), &source), "w9:t3");
    }

    #[test]
    fn no_argument_falls_back_to_the_panes_own_tab() {
        let source = pane("w1", "w1:t7");
        assert_eq!(resolve_source_tab(None, &source), "w1:t7");
        assert_eq!(resolve_source_tab(Some("  "), &source), "w1:t7");
    }
}
