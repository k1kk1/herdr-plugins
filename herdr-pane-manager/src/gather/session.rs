//! Remembering where gathered panes came from (addendum §11, §12).
//!
//! Gather physically moves live panes, so the way back has to be written down
//! before anything moves — and it has to outlive the process, because Restore
//! runs from a separate invocation. The record lives in the plugin's state
//! directory as JSON.

use std::path::PathBuf;


use herdr_plugin_kit::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::PLUGIN_ID;

const FILE: &str = "gather-session.json";

pub use crate::place::Origin;

/// One Gather run, as it stands right now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    /// Where each gathered pane came from.
    pub origins: Vec<Origin>,
    /// Tabs Gather created, so Refresh can reuse them and Restore can tidy up.
    #[serde(default)]
    pub gather_tabs: Vec<String>,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub panes_per_tab: u8,
    #[serde(default)]
    pub created_unix_ms: u64,
}

impl Session {
    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }

    pub fn pane_ids(&self) -> Vec<String> {
        self.origins.iter().map(|o| o.pane_id.clone()).collect()
    }

    pub fn contains(&self, pane_id: &str) -> bool {
        self.origins.iter().any(|o| o.pane_id == pane_id)
    }

    /// Origins for panes that are no longer wanted, in record order.
    pub fn origins_other_than(&self, keep: &[String]) -> Vec<Origin> {
        self.origins
            .iter()
            .filter(|o| !keep.contains(&o.pane_id))
            .cloned()
            .collect()
    }

    /// Drop a pane from the record, e.g. once it has been put back.
    pub fn forget(&mut self, pane_id: &str) {
        self.origins.retain(|o| o.pane_id != pane_id);
    }

}

fn path() -> Option<PathBuf> {
    herdr_plugin_kit::config::state_dir(PLUGIN_ID).map(|dir| dir.join(FILE))
}

/// The current Gather session, or `None` when nothing is gathered.
///
/// A corrupt file is treated as "nothing gathered" rather than an error: the
/// worst case is that Restore cannot help, and refusing to gather because of a
/// stale file would be worse.
pub fn load() -> Option<Session> {
    let raw = std::fs::read_to_string(path()?).ok()?;
    let session: Session = serde_json::from_str(&raw).ok()?;
    (!session.is_empty()).then_some(session)
}

pub fn save(session: &Session) -> Result<()> {
    let Some(path) = path() else {
        return Ok(());
    };
    if session.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let raw = serde_json::to_string_pretty(session)?;
    std::fs::write(&path, raw)
        .with_context(|| format!("could not record the Gather session at {}", path.display()))
}

pub fn clear() {
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(pane: &str, tab: &str, order: usize) -> Origin {
        Origin {
            pane_id: pane.into(),
            workspace_id: "w1".into(),
            tab_id: tab.into(),
            tab_label: Some("Agents".into()),
            tab_index: 0,
            anchor: Some("w1:p1".into()),
            side: Some("right".into()),
            order,
            focused: false,
        }
    }

    #[test]
    fn forgetting_a_pane_removes_only_that_pane() {
        let mut session = Session {
            origins: vec![origin("p2", "w1:t1", 0), origin("p3", "w1:t1", 1)],
            ..Session::default()
        };
        session.forget("p2");
        assert_eq!(session.pane_ids(), ["p3"]);
        assert!(!session.contains("p2"));
        assert!(session.contains("p3"));
    }

    #[test]
    fn an_emptied_session_reports_itself_empty() {
        let mut session = Session {
            origins: vec![origin("p2", "w1:t1", 0)],
            ..Session::default()
        };
        assert!(!session.is_empty());
        session.forget("p2");
        assert!(session.is_empty());
    }

    #[test]
    fn a_session_survives_a_round_trip_through_json() {
        let session = Session {
            origins: vec![origin("p2", "w1:t1", 0)],
            gather_tabs: vec!["w1:t9".into()],
            scope: "workspace".into(),
            panes_per_tab: 4,
            created_unix_ms: 1,
        };
        let raw = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.pane_ids(), session.pane_ids());
        assert_eq!(back.gather_tabs, session.gather_tabs);
        assert_eq!(back.origins[0].side(), herdr_plugin_kit::layout::Side::Right);
    }

    #[test]
    fn dropping_panes_keeps_only_the_ones_no_longer_wanted() {
        let session = Session {
            origins: vec![origin("p2", "w1:t1", 0), origin("p3", "w1:t1", 1)],
            ..Session::default()
        };
        let dropped = session.origins_other_than(&["p3".into()]);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].pane_id, "p2");
    }
}
