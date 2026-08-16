//! Live view of the current workspace (spec §3.4).
//!
//! Nothing here is cached across operations: a snapshot is taken immediately
//! before a picker renders and again immediately before the operation runs, so
//! panes moved by the user or by an agent in between are detected rather than
//! acted on blindly.

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::{Herdr, Pane, Tab, Workspace};
use herdr_plugin_kit::layout::Shape;
use herdr_plugin_kit::{anyhow, bail, Context, Result};

/// A tab plus the panes it currently holds, in workspace order.
#[derive(Debug, Clone)]
pub struct TabEntry {
    pub tab: Tab,
    /// 1-based slot in the workspace, i.e. the Quick Move number (spec §9.4).
    pub position: usize,
    pub panes: Vec<Pane>,
    /// The tab's split tree, when Herdr could report it. Merge uses this to
    /// carry a tab's internal arrangement across (addendum §11).
    pub shape: Option<Shape>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub workspace: Workspace,
    /// Tabs of the source pane's workspace.
    pub tabs: Vec<TabEntry>,
    /// Tabs of every other workspace, for cross-workspace Move (addendum §14).
    pub other_workspaces: Vec<(Workspace, Vec<TabEntry>)>,
    /// Pane the operation acts on — the focused pane, or the pane the context
    /// menu was opened on.
    pub source: Pane,
}

impl Snapshot {
    /// Take a fresh snapshot of the workspace owning `source`, plus a listing
    /// of the other workspaces' tabs as possible destinations.
    pub fn capture(herdr: &Herdr, source: Pane) -> Result<Self> {
        let workspaces = herdr.workspaces()?;
        let workspace = workspaces
            .iter()
            .find(|w| w.workspace_id == source.workspace_id)
            .cloned()
            .ok_or_else(|| anyhow!("workspace {} no longer exists", source.workspace_id))?;

        let tabs = Self::tabs_of(herdr, &workspace.workspace_id)?;
        let other_workspaces = workspaces
            .into_iter()
            .filter(|w| w.workspace_id != workspace.workspace_id)
            .map(|w| {
                let tabs = Self::tabs_of(herdr, &w.workspace_id)?;
                Ok((w, tabs))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            workspace,
            tabs,
            other_workspaces,
            source,
        })
    }

    fn tabs_of(herdr: &Herdr, workspace_id: &str) -> Result<Vec<TabEntry>> {
        let tabs = herdr.tabs(workspace_id)?;
        let mut panes = herdr.panes(workspace_id)?;

        // The plugin's own pane is real to Herdr but must stay invisible here:
        // Pane Manager only reorganises the user's panes (spec §2.1).
        if let Some(own) = context::self_pane_id() {
            panes.retain(|p| p.pane_id != own);
        }

        Ok(tabs
            .into_iter()
            .enumerate()
            .map(|(index, tab)| {
                let in_tab: Vec<Pane> = panes
                    .iter()
                    .filter(|p| p.tab_id == tab.tab_id)
                    .cloned()
                    .collect();
                // A tab with one pane has no interesting structure, and asking
                // for its layout would be a request per tab for nothing.
                let shape = (in_tab.len() > 1)
                    .then(|| herdr.layout(&tab.tab_id).ok())
                    .flatten()
                    .and_then(|layout| Shape::from_layout(&layout.root));
                TabEntry {
                    tab,
                    position: index + 1,
                    panes: in_tab,
                    shape,
                }
            })
            .collect())
    }

    /// Re-read everything, keeping the same source pane (spec §15.1).
    pub fn refresh(&self, herdr: &Herdr) -> Result<Self> {
        let source = herdr
            .pane(&self.source.pane_id)
            .context("the pane being moved no longer exists")?;
        Snapshot::capture(herdr, source)
    }

    /// Tabs of every workspace, the source's own first.
    pub fn all_tabs(&self) -> impl Iterator<Item = (&Workspace, &TabEntry)> {
        std::iter::once((&self.workspace, &self.tabs))
            .chain(self.other_workspaces.iter().map(|(w, t)| (w, t)))
            .flat_map(|(workspace, tabs)| tabs.iter().map(move |tab| (workspace, tab)))
    }

    pub fn tab(&self, tab_id: &str) -> Option<&TabEntry> {
        self.all_tabs().map(|(_, tab)| tab).find(|t| t.tab.tab_id == tab_id)
    }

    /// Panes sharing `tab_id` with `pane_id`, excluding it.
    pub fn siblings(&self, pane_id: &str, tab_id: &str) -> Vec<String> {
        self.tab(tab_id)
            .map(|entry| {
                entry
                    .panes
                    .iter()
                    .filter(|p| p.pane_id != pane_id)
                    .map(|p| p.pane_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Tab holding the source pane.
    pub fn source_tab(&self) -> Option<&TabEntry> {
        self.tab(&self.source.tab_id)
    }

    pub fn pane(&self, pane_id: &str) -> Option<&Pane> {
        self.all_tabs()
            .flat_map(|(_, t)| t.panes.iter())
            .find(|p| p.pane_id == pane_id)
    }

    /// Tab at a 1-based Quick Move slot.
    pub fn tab_at(&self, position: usize) -> Option<&TabEntry> {
        self.tabs.iter().find(|t| t.position == position)
    }

    /// Destination candidates for Move: every tab except the source's own,
    /// across every workspace (addendum §14).
    pub fn move_destinations(&self) -> Vec<(&Workspace, &TabEntry)> {
        self.all_tabs()
            .filter(|(_, t)| t.tab.tab_id != self.source.tab_id)
            .collect()
    }

    /// Destination candidates for Merge: every tab except the current one (§8.2).
    pub fn merge_destinations(&self, source_tab_id: &str) -> Vec<(&Workspace, &TabEntry)> {
        self.all_tabs()
            .filter(|(_, t)| t.tab.tab_id != source_tab_id)
            .collect()
    }

    /// Swap candidates: every pane except the source itself. Same-tab and
    /// cross-tab panes are both offered (spec §6).
    pub fn swap_candidates(&self) -> Vec<(&Workspace, &TabEntry, &Pane)> {
        self.all_tabs()
            .flat_map(|(workspace, entry)| {
                entry.panes.iter().map(move |pane| (workspace, entry, pane))
            })
            .filter(|(_, _, pane)| pane.pane_id != self.source.pane_id)
            .collect()
    }

    /// Verify a destination tab still exists before acting on it (spec §15.1).
    pub fn require_tab(&self, tab_id: &str) -> Result<&TabEntry> {
        match self.tab(tab_id) {
            Some(entry) => Ok(entry),
            None => bail!("Destination tab no longer exists."),
        }
    }

    /// Verify a destination pane still exists before acting on it.
    pub fn require_pane(&self, pane_id: &str) -> Result<&Pane> {
        match self.pane(pane_id) {
            Some(pane) => Ok(pane),
            None => bail!("Destination pane no longer exists."),
        }
    }
}
