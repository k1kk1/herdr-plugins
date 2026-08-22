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
    /// Whether `shape` being `None` means "there is nothing to know".
    ///
    /// A tab of one pane has no split tree and Herdr reports none, which is a
    /// complete answer. A tab of several panes whose layout could not be read
    /// is a different thing entirely, and the two were indistinguishable: the
    /// previews fell back to drawing a single box, so a three-pane tab was
    /// pictured as one pane whenever the call failed or had not been made.
    pub layout_known: bool,
}

impl TabEntry {
    /// The pane a split lands on when nobody names one.
    ///
    /// Herdr splits the destination tab's focused pane, so that is the pane an
    /// anchorless placement really uses. Naming it explicitly is what makes
    /// `Side::Left` and `Side::Up` work at all: those are performed as a
    /// right/down split followed by a swap, and there is nothing to swap with
    /// unless the anchor is known. It is also the pane the preview has to draw
    /// against, or the picture and the move disagree.
    /// The tab's arrangement, or `None` when it could not be read.
    ///
    /// A one-pane tab answers with the trivial shape rather than with nothing:
    /// there really is only one box to draw, and that is knowable.
    pub fn layout(&self) -> Option<Shape> {
        match (&self.shape, self.layout_known) {
            (Some(shape), _) => Some(shape.clone()),
            (None, true) => self.panes.first().map(|pane| Shape::pane(&pane.pane_id)),
            (None, false) => None,
        }
    }

    pub fn split_anchor(&self) -> Option<&Pane> {
        self.panes
            .iter()
            .find(|pane| pane.focused)
            .or_else(|| self.panes.first())
    }
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
    /// Whether destinations in every workspace, including their real split
    /// trees, have been read. The landing screen only needs the source and
    /// next local tab, so it deliberately starts incomplete.
    complete: bool,
}

impl Snapshot {
    /// Assemble a snapshot from parts, for tests and for fixtures.
    ///
    /// `complete` is false: a hand-built session names only the tabs it cares
    /// about, which is exactly the state the landing screen is in.
    #[cfg(test)]
    pub fn of(workspace: Workspace, tabs: Vec<TabEntry>, source: Pane) -> Self {
        Self {
            workspace,
            tabs,
            other_workspaces: Vec::new(),
            source,
            complete: false,
        }
    }

    /// Where a Move or a Fold goes when the reader does not pick a tab.
    ///
    /// The next tab round, so repeating the key walks a pane along the
    /// workspace instead of parking it in one place. Wrapping matters: on the
    /// last tab the useful answer is the first, not "nowhere".
    ///
    /// `None` only when this is the only tab, and then there is genuinely
    /// nothing to default to — the picker offers to make one.
    pub fn next_tab(&self) -> Option<&TabEntry> {
        let here = self
            .tabs
            .iter()
            .position(|t| t.tab.tab_id == self.source.tab_id)?;
        (1..self.tabs.len())
            .map(|step| &self.tabs[(here + step) % self.tabs.len()])
            .find(|t| t.tab.tab_id != self.source.tab_id)
    }

    /// Where a Swap goes when the reader does not pick a pane.
    ///
    /// The next pane in the same tab. Cross-tab swaps are possible but never
    /// obvious, so those stay behind the picker.
    pub fn next_pane_here(&self) -> Option<&Pane> {
        let tab = self.source_tab()?;
        let here = tab
            .panes
            .iter()
            .position(|p| p.pane_id == self.source.pane_id)?;
        (1..tab.panes.len())
            .map(|step| &tab.panes[(here + step) % tab.panes.len()])
            .find(|p| p.pane_id != self.source.pane_id)
    }

    /// Take a fresh snapshot of the workspace owning `source`, plus a listing
    /// of the other workspaces' tabs as possible destinations.
    pub fn capture(herdr: &Herdr, source: Pane) -> Result<Self> {
        let workspaces = herdr.workspaces()?;
        let workspace = workspaces
            .iter()
            .find(|w| w.workspace_id == source.workspace_id)
            .cloned()
            .ok_or_else(|| anyhow!("workspace {} no longer exists", source.workspace_id))?;

        let tabs = Self::tabs_of(herdr, &workspace.workspace_id, None, &source.pane_id)?;
        let other_workspaces = workspaces
            .into_iter()
            .filter(|w| w.workspace_id != workspace.workspace_id)
            .map(|w| {
                let tabs = Self::tabs_of(herdr, &w.workspace_id, None, &source.pane_id)?;
                Ok((w, tabs))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            workspace,
            tabs,
            other_workspaces,
            source,
            complete: true,
        })
    }

    /// The data needed for Pane Manager's landing screen.
    ///
    /// Opening the manager must feel immediate. The landing screen names only
    /// the current tab and its next local neighbour, so reading every other
    /// workspace and every split tree here merely delays a choice the user may
    /// never make. Destination pickers call [`Snapshot::full`] on demand.
    pub fn capture_manager(herdr: &Herdr, source: Pane) -> Result<Self> {
        let workspaces = herdr.workspaces()?;
        let workspace = workspaces
            .iter()
            .find(|w| w.workspace_id == source.workspace_id)
            .cloned()
            .ok_or_else(|| anyhow!("workspace {} no longer exists", source.workspace_id))?;

        let mut tabs = Self::tabs_of(herdr, &workspace.workspace_id, Some(&[]), &source.pane_id)?;
        let here = tabs
            .iter()
            .position(|tab| tab.tab.tab_id == source.tab_id);
        let mut shaped = vec![source.tab_id.clone()];
        if let Some(here) = here {
            if tabs.len() > 1 {
                shaped.push(tabs[(here + 1) % tabs.len()].tab.tab_id.clone());
            }
        }
        Self::load_shapes(herdr, &mut tabs, &shaped);

        Ok(Self {
            workspace,
            tabs,
            other_workspaces: Vec::new(),
            source,
            complete: false,
        })
    }

    /// `shape_tabs = None` reads every split tree; an explicit list reads only
    /// those tabs. The latter keeps the first menu render bounded.
    fn tabs_of(
        herdr: &Herdr,
        workspace_id: &str,
        shape_tabs: Option<&[String]>,
        keep: &str,
    ) -> Result<Vec<TabEntry>> {
        let tabs = herdr.tabs(workspace_id)?;
        let mut panes = herdr.panes(workspace_id)?;

        // The plugin's own pane is real to Herdr but must stay invisible here:
        // Pane Manager only reorganises the user's panes (spec §2.1).
        //
        // Except when it is the pane being acted on. A popup is launched with
        // the invoking pane's `HERDR_PANE_ID`, so on that path "the plugin's
        // own pane" and "the pane the reader is sitting in" are the same id —
        // and dropping it hid the reader's own pane from every list built from
        // this snapshot. Split trees come from `layout` rather than from here,
        // which is why the diagrams still showed a pane the counts did not.
        if let Some(own) = context::self_pane_id() {
            panes.retain(|p| p.pane_id != own || p.pane_id == keep);
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
                let want_shape = shape_tabs
                    .map(|ids| ids.iter().any(|id| id == &tab.tab_id))
                    .unwrap_or(true);
                // A tab with one pane has no interesting structure, and asking
                // for its layout would be a request per tab for nothing.
                // A tab with one pane has no interesting structure, and asking
                // for its layout would be a request per tab for nothing.
                let simple = in_tab.len() <= 1;
                let shape = (want_shape && !simple)
                    .then(|| herdr.layout(&tab.tab_id).ok())
                    .flatten()
                    .and_then(|layout| Shape::from_layout(&layout.root));
                TabEntry {
                    tab,
                    position: index + 1,
                    layout_known: simple || shape.is_some(),
                    panes: in_tab,
                    shape,
                }
            })
            .collect())
    }

    fn load_shapes(herdr: &Herdr, tabs: &mut [TabEntry], ids: &[String]) {
        for tab in tabs {
            if tab.panes.len() > 1 && ids.iter().any(|id| id == &tab.tab.tab_id) {
                tab.shape = herdr
                    .layout(&tab.tab.tab_id)
                    .ok()
                    .and_then(|layout| Shape::from_layout(&layout.root));
                tab.layout_known = tab.shape.is_some();
            }
        }
    }

    /// Re-read everything, keeping the same source pane (spec §15.1).
    pub fn refresh(&self, herdr: &Herdr) -> Result<Self> {
        let source = herdr
            .pane(&self.source.pane_id)
            .context("the pane being moved no longer exists")?;
        Snapshot::capture(herdr, source)
    }

    /// Upgrade a landing-screen snapshot before opening a destination picker.
    pub fn full(&self, herdr: &Herdr) -> Result<Self> {
        if self.complete {
            Ok(self.clone())
        } else {
            self.refresh(herdr)
        }
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
