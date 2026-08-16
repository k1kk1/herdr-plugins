//! Human-facing naming rules.
//!
//! Spec §3.2 forbids showing raw Herdr IDs (`w1:p3`) anywhere but Debug /
//! Details, so every list entry is built from the richest identity Herdr can
//! give us, in a fixed priority order.

use crate::herdr::{Pane, Tab};

/// Last path component of a cwd, e.g. `/Users/x/src/mushi-battle` → `mushi-battle`.
pub fn cwd_basename(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?.trim_end_matches('/');
    if cwd.is_empty() {
        return None;
    }
    let base = cwd.rsplit('/').next()?;
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
    }
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value.map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Agent name in title case: `claude` → `Claude`.
pub fn agent_name(pane: &Pane) -> Option<String> {
    let raw = non_empty(pane.display_agent.as_ref()).or_else(|| non_empty(pane.agent.as_ref()))?;
    let mut chars = raw.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().collect::<String>() + chars.as_str())
}

/// Title stripped of the agent's own status glyph, as reported by Herdr.
pub fn terminal_title(pane: &Pane) -> Option<String> {
    non_empty(pane.terminal_title_stripped.as_ref())
        .or_else(|| non_empty(pane.terminal_title.as_ref()))
}

/// Project-ish name for a pane: its own cwd, falling back to the foreground cwd.
pub fn project(pane: &Pane) -> Option<String> {
    cwd_basename(pane.cwd.as_deref()).or_else(|| cwd_basename(pane.foreground_cwd.as_deref()))
}

/// Headline for a pane (spec §3.2 priority 1–4).
pub fn pane_primary(pane: &Pane) -> String {
    non_empty(pane.label.as_ref())
        .or_else(|| agent_name(pane))
        .or_else(|| project(pane))
        .or_else(|| terminal_title(pane))
        // Last resort only: an ID is better than an unpickable blank row.
        .unwrap_or_else(|| pane.pane_id.clone())
}

/// Compact single-line form used in menus: `Claude · mushi-battle` (spec §8.3).
pub fn pane_compact(pane: &Pane) -> String {
    let primary = pane_primary(pane);
    match project(pane) {
        Some(project) if project != primary => format!("{primary} · {project}"),
        _ => primary,
    }
}

/// Optional third line: what the pane is currently doing (spec §8.3).
pub fn pane_detail(pane: &Pane) -> Option<String> {
    let title = terminal_title(pane)?;
    let primary = pane_primary(pane);
    if title == primary || Some(&title) == project(pane).as_ref() {
        None
    } else {
        Some(title)
    }
}

/// Auto-generated label for a tab created by Extract (spec §5.1).
///
/// Returns `None` when nothing better than Herdr's own `Tab <number>` default
/// is available, in which case the caller should let Herdr name the tab.
pub fn new_tab_label(pane: &Pane) -> Option<String> {
    non_empty(pane.label.as_ref())
        .or_else(|| terminal_title(pane))
        .or_else(|| project(pane))
        .or_else(|| agent_name(pane))
}

/// Whether a tab's label is just its auto-assigned number.
fn label_is_numeric(tab: &Tab) -> bool {
    match tab.label.as_deref().map(str::trim) {
        Some(label) => label.is_empty() || label.chars().all(|c| c.is_ascii_digit()),
        None => true,
    }
}

/// `Tab 2: Agents`, or plain `Tab 2` for an unnamed tab.
///
/// `position` is the 1-based slot in the workspace's tab list — the same number
/// the user types for Quick Move (spec §9.4).
pub fn tab_display(tab: &Tab, position: usize) -> String {
    if label_is_numeric(tab) {
        format!("Tab {position}")
    } else {
        format!("Tab {position}: {}", tab.label.as_deref().unwrap_or_default().trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::AgentStatus;

    fn pane() -> Pane {
        Pane {
            pane_id: "w1:p1".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            focused: false,
            label: None,
            agent: None,
            display_agent: None,
            agent_status: AgentStatus::Unknown,
            cwd: None,
            foreground_cwd: None,
            terminal_title: None,
            terminal_title_stripped: None,
        }
    }

    fn tab(label: Option<&str>) -> Tab {
        Tab {
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            label: label.map(str::to_string),
            pane_count: 1,
            focused: false,
            agent_status: AgentStatus::Unknown,
        }
    }

    #[test]
    fn cwd_basename_handles_trailing_slash_and_root() {
        assert_eq!(cwd_basename(Some("/a/b/mushi-battle/")).as_deref(), Some("mushi-battle"));
        assert_eq!(cwd_basename(Some("/")), None);
        assert_eq!(cwd_basename(None), None);
    }

    #[test]
    fn pane_label_wins_over_agent() {
        let mut p = pane();
        p.agent = Some("claude".into());
        p.label = Some("Music".into());
        assert_eq!(pane_primary(&p), "Music");
    }

    #[test]
    fn agent_is_title_cased_and_paired_with_project() {
        let mut p = pane();
        p.agent = Some("claude".into());
        p.cwd = Some("/Users/x/src/mushi-battle".into());
        assert_eq!(pane_compact(&p), "Claude · mushi-battle");
    }

    #[test]
    fn compact_does_not_repeat_the_project_name() {
        let mut p = pane();
        p.cwd = Some("/Users/x/src/agent-usage".into());
        assert_eq!(pane_compact(&p), "agent-usage");
    }

    #[test]
    fn new_tab_label_follows_spec_priority() {
        let mut p = pane();
        assert_eq!(new_tab_label(&p), None);

        p.agent = Some("codex".into());
        assert_eq!(new_tab_label(&p).as_deref(), Some("Codex"));

        p.cwd = Some("/Users/x/src/ComposerSketch".into());
        assert_eq!(new_tab_label(&p).as_deref(), Some("ComposerSketch"));

        p.terminal_title_stripped = Some("build watch".into());
        assert_eq!(new_tab_label(&p).as_deref(), Some("build watch"));

        p.label = Some("Music".into());
        assert_eq!(new_tab_label(&p).as_deref(), Some("Music"));
    }

    #[test]
    fn numeric_tab_labels_are_treated_as_unnamed() {
        assert_eq!(tab_display(&tab(Some("2")), 2), "Tab 2");
        assert_eq!(tab_display(&tab(None), 3), "Tab 3");
        assert_eq!(tab_display(&tab(Some("Agents")), 1), "Tab 1: Agents");
    }
}
