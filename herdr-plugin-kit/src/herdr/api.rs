//! Typed wrappers over the raw socket calls.
//!
//! Each function maps to exactly one Herdr API method, so the request shapes
//! stay in one place and the plugins read as ordinary Rust.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::client::Herdr;
use super::model::{
    Agent, Direction, InstalledPlugin, Layout, MoveResult, Pane, PluginAction, Tab, Workspace,
};

impl Herdr {
    // ----- reads -------------------------------------------------------

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        self.call_field("workspace.list", json!({}), "workspaces")
    }

    /// The focused workspace, falling back to the first one.
    pub fn focused_workspace(&self) -> Result<Workspace> {
        let workspaces = self.workspaces()?;
        workspaces
            .iter()
            .find(|w| w.focused)
            .or_else(|| workspaces.first())
            .cloned()
            .ok_or_else(|| anyhow!("no Herdr workspace is open"))
    }

    /// Tabs of a workspace, in sidebar order.
    pub fn tabs(&self, workspace_id: &str) -> Result<Vec<Tab>> {
        self.call_field("tab.list", json!({ "workspace_id": workspace_id }), "tabs")
    }

    /// Tabs of every workspace, in sidebar order.
    pub fn all_tabs(&self) -> Result<Vec<Tab>> {
        let mut out = Vec::new();
        for workspace in self.workspaces()? {
            out.extend(self.tabs(&workspace.workspace_id)?);
        }
        Ok(out)
    }

    /// Panes of a workspace.
    pub fn panes(&self, workspace_id: &str) -> Result<Vec<Pane>> {
        self.call_field("pane.list", json!({ "workspace_id": workspace_id }), "panes")
    }

    /// Panes of every workspace.
    pub fn all_panes(&self) -> Result<Vec<Pane>> {
        let mut out = Vec::new();
        for workspace in self.workspaces()? {
            out.extend(self.panes(&workspace.workspace_id)?);
        }
        Ok(out)
    }

    /// Every pane Herdr has detected an agent in, across all workspaces.
    pub fn agents(&self) -> Result<Vec<Agent>> {
        self.call_field("agent.list", json!({}), "agents")
    }

    pub fn pane(&self, pane_id: &str) -> Result<Pane> {
        self.call_field("pane.get", json!({ "pane_id": pane_id }), "pane")
    }

    pub fn tab(&self, tab_id: &str) -> Result<Tab> {
        self.call_field("tab.get", json!({ "tab_id": tab_id }), "tab")
    }

    /// The split tree of a tab.
    pub fn layout(&self, tab_id: &str) -> Result<Layout> {
        self.call_field("layout.export", json!({ "tab_id": tab_id }), "layout")
    }

    // ----- pane placement ----------------------------------------------

    /// Move a pane into an existing tab.
    ///
    /// `target_pane` selects which pane in the destination tab is split; when
    /// `None`, Herdr splits that tab's own focused pane.
    ///
    /// Note that Herdr treats a move into the pane's *current* tab as a no-op,
    /// so this cannot be used to reshape a tab in place.
    pub fn move_pane_to_tab(
        &self,
        pane_id: &str,
        tab_id: &str,
        target_pane: Option<&str>,
        direction: Direction,
        focus: bool,
    ) -> Result<MoveResult> {
        self.move_pane_to_tab_with_ratio(pane_id, tab_id, target_pane, direction, None, focus)
    }

    /// As [`Herdr::move_pane_to_tab`], with an explicit split ratio — the
    /// share of the space kept by the split's first branch, i.e. the pane
    /// being split.
    pub fn move_pane_to_tab_with_ratio(
        &self,
        pane_id: &str,
        tab_id: &str,
        target_pane: Option<&str>,
        direction: Direction,
        ratio: Option<f32>,
        focus: bool,
    ) -> Result<MoveResult> {
        let mut destination = json!({
            "type": "tab",
            "tab_id": tab_id,
            "split": direction.as_str(),
        });
        if let Some(target) = target_pane {
            destination["target_pane_id"] = json!(target);
        }
        if let Some(ratio) = ratio {
            destination["ratio"] = json!(ratio);
        }
        self.call_field(
            "pane.move",
            json!({ "pane_id": pane_id, "destination": destination, "focus": focus }),
            "move_result",
        )
    }

    /// Move a pane into a brand new tab.
    pub fn move_pane_to_new_tab(
        &self,
        pane_id: &str,
        label: Option<&str>,
        focus: bool,
    ) -> Result<MoveResult> {
        let mut destination = json!({ "type": "new_tab" });
        if let Some(label) = label {
            destination["label"] = json!(label);
        }
        self.call_field(
            "pane.move",
            json!({ "pane_id": pane_id, "destination": destination, "focus": focus }),
            "move_result",
        )
    }

    /// Swap two panes inside one tab.
    ///
    /// Herdr's `pane.swap` is same-tab only; it answers `changed: false` with
    /// `reason: "cross_tab"` otherwise. Returns whether anything changed.
    pub fn swap_panes_in_tab(&self, source: &str, target: &str) -> Result<bool> {
        let result = self.call(
            "pane.swap",
            json!({ "source_pane_id": source, "target_pane_id": target }),
        )?;
        let swap = result
            .get("swap")
            .ok_or_else(|| anyhow!("pane.swap returned no result"))?;
        if swap.get("changed").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(true);
        }
        match swap.get("reason").and_then(Value::as_str) {
            Some("cross_tab") => Ok(false),
            Some(reason) => Err(anyhow!("Herdr could not swap the panes ({reason})")),
            None => Ok(false),
        }
    }

    /// Set the ratio of one split, addressed by its path from the tab's root
    /// (`false` = first branch, `true` = second).
    pub fn set_split_ratio(&self, tab_id: &str, path: &[bool], ratio: f32) -> Result<()> {
        self.call(
            "layout.set_split_ratio",
            json!({ "tab_id": tab_id, "path": path, "ratio": ratio }),
        )
        .map(|_| ())
    }

    pub fn zoom_pane(&self, pane_id: &str, mode: &str) -> Result<()> {
        self.call("pane.zoom", json!({ "pane_id": pane_id, "mode": mode }))
            .map(|_| ())
    }

    // ----- focus and tabs ----------------------------------------------

    /// Move a pane into a brand new workspace of its own.
    pub fn move_pane_to_new_workspace(
        &self,
        pane_id: &str,
        label: Option<&str>,
        focus: bool,
    ) -> Result<MoveResult> {
        let mut destination = json!({ "type": "new_workspace" });
        if let Some(label) = label {
            destination["label"] = json!(label);
            destination["tab_label"] = json!(label);
        }
        self.call_field(
            "pane.move",
            json!({ "pane_id": pane_id, "destination": destination, "focus": focus }),
            "move_result",
        )
    }

    pub fn focus_pane(&self, pane_id: &str) -> Result<()> {
        self.call("pane.focus", json!({ "pane_id": pane_id }))
            .map(|_| ())
    }

    pub fn focus_tab(&self, tab_id: &str) -> Result<()> {
        self.call("tab.focus", json!({ "tab_id": tab_id }))
            .map(|_| ())
    }

    pub fn focus_workspace(&self, workspace_id: &str) -> Result<()> {
        self.call("workspace.focus", json!({ "workspace_id": workspace_id }))
            .map(|_| ())
    }

    pub fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()> {
        self.call("tab.rename", json!({ "tab_id": tab_id, "label": label }))
            .map(|_| ())
    }

    pub fn close_tab(&self, tab_id: &str) -> Result<()> {
        self.call("tab.close", json!({ "tab_id": tab_id })).map(|_| ())
    }

    /// Move a tab to a 0-based slot in its workspace's tab bar.
    pub fn move_tab(&self, tab_id: &str, insert_index: usize) -> Result<()> {
        self.call(
            "tab.move",
            json!({ "tab_id": tab_id, "insert_index": insert_index }),
        )
        .map(|_| ())
    }

    // ----- plugins ------------------------------------------------------

    pub fn installed_plugins(&self) -> Result<Vec<InstalledPlugin>> {
        self.call_field("plugin.list", json!({}), "plugins")
    }

    /// Every action of every installed plugin.
    pub fn plugin_actions(&self) -> Result<Vec<PluginAction>> {
        self.call_field("plugin.action.list", json!({}), "actions")
    }

    /// Run a plugin action, telling it what was focused when the user chose it.
    pub fn invoke_plugin_action(
        &self,
        plugin_id: &str,
        action_id: &str,
        context: Value,
    ) -> Result<()> {
        self.call(
            "plugin.action.invoke",
            json!({ "plugin_id": plugin_id, "action_id": action_id, "context": context }),
        )
        .map(|_| ())
    }

    /// Open one of a plugin's declared panes.
    pub fn open_plugin_pane(
        &self,
        plugin_id: &str,
        entrypoint: &str,
        env: &[(&str, String)],
        focus: bool,
    ) -> Result<()> {
        let env: serde_json::Map<String, Value> = env
            .iter()
            .map(|(k, v)| ((*k).to_string(), json!(v)))
            .collect();
        self.call(
            "plugin.pane.open",
            json!({
                "plugin_id": plugin_id,
                "entrypoint": entrypoint,
                "env": env,
                "focus": focus,
            }),
        )
        .map(|_| ())
    }

    // ----- feedback -----------------------------------------------------

    /// Best-effort toast. Never fails the surrounding operation.
    pub fn notify(&self, title: &str, body: Option<&str>) {
        let mut params = json!({ "title": title });
        if let Some(body) = body {
            params["body"] = json!(body);
        }
        let _ = self.call("notification.show", params);
    }
}
