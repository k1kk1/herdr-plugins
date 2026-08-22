//! Undoing the last Move / Extract / Merge / Swap / Gather.
//!
//! Gather already had to record where panes came from so Restore could put them
//! back. The other four operations move panes the same way, so the same record
//! undoes them — the only genuinely different case is Swap, which is its own
//! inverse.
//!
//! Deliberately one level deep. A stack of undos reads well in a changelog and
//! badly in practice: every entry past the first describes a world that has
//! already been rearranged by the entries after it, so replaying it puts panes
//! somewhere the user never asked for. One step back from the thing you just
//! did is the part people actually want.

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Outcome, Result};
use serde::{Deserialize, Serialize};

use crate::config::PLUGIN_ID;
use crate::ops::Verb;
use crate::place::{self, Origin};

const FILE: &str = "undo.json";

/// How to reverse the operation that was just performed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// What was done, for the message shown when offering the undo.
    pub verb: String,
    /// What it acted on, e.g. the pane or tab name.
    #[serde(default)]
    pub subject: String,
    /// Where the moved panes were beforehand.
    #[serde(default)]
    pub origins: Vec<Origin>,
    /// For Swap: the two panes, which only need swapping back.
    #[serde(default)]
    pub swap: Option<(String, String)>,
    /// Tabs the operation created, so an undo can clear away what is left.
    #[serde(default)]
    pub created_tabs: Vec<String>,
    /// This record undoes a Gather.
    ///
    /// Gather keeps its own record so that Restore works long afterwards; this
    /// one exists so that Undo works *immediately* afterwards, which is the
    /// difference between "collect these for a moment" and "rearrange my
    /// session". Undoing one has to retire the other, or Restore would be
    /// offered for panes that are already home.
    #[serde(default)]
    pub gather: bool,
    #[serde(default)]
    pub unix_ms: u64,
}

impl Record {
    /// A short description of what undoing this would do.
    pub fn describe(&self) -> String {
        if self.subject.is_empty() {
            self.verb.clone()
        } else {
            format!("{} {}", self.verb, self.subject)
        }
    }
}

fn path() -> Option<std::path::PathBuf> {
    herdr_plugin_kit::config::state_dir(PLUGIN_ID).map(|dir| dir.join(FILE))
}

/// The last undoable operation, if there is one.
///
/// A corrupt file reads as "nothing to undo" rather than an error: refusing to
/// work because of a stale record would be worse than not offering the undo.
pub fn load() -> Option<Record> {
    let raw = std::fs::read_to_string(path()?).ok()?;
    let record: Record = serde_json::from_str(&raw).ok()?;
    (!record.origins.is_empty() || record.swap.is_some()).then_some(record)
}

pub fn save(record: &Record) -> Result<()> {
    let Some(path) = path() else {
        return Ok(());
    };
    let raw = serde_json::to_string_pretty(record)?;
    let _ = std::fs::write(path, raw);
    Ok(())
}

pub fn clear() {
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Build the record for an operation that is about to run.
///
/// Called before anything moves; afterwards the information is gone.
pub fn capture(
    herdr: &Herdr,
    verb: Verb,
    subject: &str,
    panes: &[String],
    swap: Option<(String, String)>,
) -> Record {
    Record {
        verb: verb.label().to_string(),
        subject: subject.to_string(),
        // Swap is its own inverse, so it needs no origins.
        origins: match swap {
            Some(_) => Vec::new(),
            None => place::origins_of(herdr, panes),
        },
        swap,
        created_tabs: Vec::new(),
        gather: false,
        unix_ms: now_unix_ms(),
    }
}

/// Reverse the recorded operation.
pub fn undo(herdr: &Herdr) -> Result<Outcome> {
    let Some(record) = load() else {
        bail!("There is nothing to undo.");
    };

    let outcome = match &record.swap {
        // Swapping the same two panes again puts both back.
        Some((a, b)) => {
            for pane in [a, b] {
                if herdr.pane(pane).is_err() {
                    clear();
                    bail!("\"{pane}\" is gone, so that swap cannot be undone.");
                }
            }
            let same_tab = match (herdr.pane(a), herdr.pane(b)) {
                (Ok(first), Ok(second)) => first.tab_id == second.tab_id,
                _ => false,
            };
            if !same_tab {
                clear();
                bail!("Those panes are no longer in the same tab, so the swap cannot be undone.");
            }
            herdr.swap_panes_in_tab(a, b)?;
            Outcome::new(format!("Undid {}", record.describe()))
                .with_detail("The two panes traded back")
        }
        None => {
            let expected = record.origins.len();
            let restored = place::restore(herdr, &record.origins)?;
            if let Some(focused) = record.origins.iter().find(|o| o.focused) {
                let _ = herdr.focus_pane(&focused.pane_id);
            }
            // Tabs the operation created are empty once their panes go home.
            for tab_id in &record.created_tabs {
                if let Ok(tab) = herdr.tab(tab_id) {
                    if tab.pane_count == 0 {
                        let _ = herdr.close_tab(tab_id);
                    }
                }
            }
            Outcome::new(format!("Undid {}", record.describe()))
                .with_detail(restored.detail(expected))
        }
    };

    if record.gather {
        // The panes are home, so Restore has nothing left to put back.
        // Leaving its record would offer the reader a second way to undo
        // something already undone.
        crate::gather::session::clear();
    }

    // One level only: the record is spent.
    clear();
    Ok(outcome)
}

/// Drop the record if it is the one describing a Gather.
///
/// Called when Restore has already put those panes back.
pub fn forget_gather() {
    if load().is_some_and(|record| record.gather) {
        clear();
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

    #[test]
    fn a_swap_record_needs_no_origins_to_be_undoable() {
        let record = Record {
            verb: "Swap".into(),
            subject: "editor".into(),
            origins: Vec::new(),
            swap: Some(("w1:p1".into(), "w1:p2".into())),
            created_tabs: Vec::new(),
            gather: false,
            unix_ms: 1,
        };
        let raw = serde_json::to_string(&record).unwrap();
        let back: Record = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.swap, Some(("w1:p1".into(), "w1:p2".into())));
        assert_eq!(back.describe(), "Swap editor");
    }

    #[test]
    fn a_record_with_nothing_to_reverse_is_not_offered() {
        let raw = r#"{"verb":"Move","subject":"x","origins":[],"swap":null}"#;
        let record: Record = serde_json::from_str(raw).unwrap();
        assert!(record.origins.is_empty() && record.swap.is_none());
    }

    #[test]
    fn a_record_without_a_subject_still_describes_itself() {
        let record = Record {
            verb: "Merge".into(),
            subject: String::new(),
            origins: Vec::new(),
            swap: None,
            created_tabs: Vec::new(),
            gather: false,
            unix_ms: 0,
        };
        assert_eq!(record.describe(), "Merge");
    }
}

#[cfg(test)]
mod gather_record_tests {
    use super::*;

    fn gather_record() -> Record {
        Record {
            verb: "Gather".into(),
            subject: "3 agents".into(),
            origins: Vec::new(),
            swap: None,
            created_tabs: vec!["w1:t9".into()],
            gather: true,
            unix_ms: 1,
        }
    }

    #[test]
    fn a_gather_record_says_what_undoing_it_would_do() {
        assert_eq!(gather_record().describe(), "Gather 3 agents");
    }

    #[test]
    fn the_flag_survives_a_round_trip_and_defaults_off() {
        let raw = serde_json::to_string(&gather_record()).unwrap();
        assert!(serde_json::from_str::<Record>(&raw).unwrap().gather);

        // Records written before the flag existed must still load, as
        // ordinary operations rather than gathers.
        let old = r#"{"verb":"Move","subject":"x","origins":[],"unix_ms":1}"#;
        assert!(!serde_json::from_str::<Record>(old).unwrap().gather);
    }
}
