//! Choosing which agents to gather, and in what order (addendum §2, §3, §16).

use herdr_plugin_kit::herdr::{Agent, AgentStatus};

use crate::config::GatherConfig;

/// How far Gather looks for agents (addendum §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    CurrentWorkspace,
    AllWorkspaces,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::CurrentWorkspace => "workspace",
            Scope::AllWorkspaces => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::CurrentWorkspace => "Current Workspace",
            Scope::AllWorkspaces => "All Workspaces",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "workspace" | "current" | "current-workspace" | "w" => Some(Scope::CurrentWorkspace),
            "all" | "all-workspaces" | "a" => Some(Scope::AllWorkspaces),
            _ => None,
        }
    }
}

/// Agents worth gathering, most urgent first.
///
/// Selection is by status (`blocked` / `done` / `working` by default), and
/// optionally by agent kind. Panes Herdr has not detected an agent in — shells,
/// dev servers, log tails — never appear, because they are not in `agent.list`
/// at all.
///
/// Order is `blocked` → `done` → `working` → anything else, and within a status
/// the most recently changed first. `state_change_seq` makes that total and
/// stable; `pane_id` breaks the remaining ties so two runs over unchanged state
/// produce the same layout.
pub fn select(agents: &[Agent], config: &GatherConfig, workspace: Option<&str>) -> Vec<Agent> {
    let mut chosen: Vec<Agent> = agents
        .iter()
        .filter(|agent| config.statuses.iter().any(|s| *s == agent.agent_status))
        .filter(|agent| match workspace {
            Some(workspace) => agent.workspace_id == workspace,
            None => true,
        })
        .filter(|agent| kind_allowed(agent, config))
        .cloned()
        .collect();

    chosen.sort_by(|a, b| {
        a.agent_status
            .priority()
            .cmp(&b.agent_status.priority())
            .then(b.state_change_seq.cmp(&a.state_change_seq))
            .then(a.pane_id.cmp(&b.pane_id))
    });
    chosen
}

/// An empty `agents` list means every agent kind (addendum §16).
fn kind_allowed(agent: &Agent, config: &GatherConfig) -> bool {
    if config.agents.is_empty() {
        return true;
    }
    let Some(kind) = agent.agent.as_deref() else {
        return false;
    };
    config
        .agents
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(kind))
}

/// The status a Gather run should focus afterwards (addendum §15).
pub fn highest_priority(agents: &[Agent]) -> Option<&Agent> {
    agents.first()
}

/// Statuses Gather collects unless configured otherwise.
pub fn default_statuses() -> Vec<AgentStatus> {
    vec![AgentStatus::Blocked, AgentStatus::Done, AgentStatus::Working]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(pane: &str, workspace: &str, status: AgentStatus, seq: u64, kind: &str) -> Agent {
        Agent {
            pane_id: pane.into(),
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace.into(),
            agent: Some(kind.into()),
            agent_status: status,
            state_change_seq: seq,
        }
    }

    fn sample() -> Vec<Agent> {
        vec![
            agent("p1", "w1", AgentStatus::Working, 10, "claude"),
            agent("p2", "w1", AgentStatus::Idle, 20, "claude"),
            agent("p3", "w1", AgentStatus::Blocked, 30, "codex"),
            agent("p4", "w2", AgentStatus::Done, 40, "claude"),
            agent("p5", "w1", AgentStatus::Working, 50, "codex"),
            agent("p6", "w1", AgentStatus::Unknown, 60, "claude"),
        ]
    }

    fn ids(agents: &[Agent]) -> Vec<&str> {
        agents.iter().map(|a| a.pane_id.as_str()).collect()
    }

    #[test]
    fn idle_and_unknown_are_left_alone() {
        let got = select(&sample(), &GatherConfig::default(), None);
        assert!(!ids(&got).contains(&"p2"), "idle was gathered");
        assert!(!ids(&got).contains(&"p6"), "unknown was gathered");
    }

    #[test]
    fn blocked_comes_first_then_done_then_working() {
        let got = select(&sample(), &GatherConfig::default(), None);
        // p5 before p1: same status, higher state_change_seq is more recent.
        assert_eq!(ids(&got), ["p3", "p4", "p5", "p1"]);
    }

    #[test]
    fn scope_limits_the_search_to_one_workspace() {
        let got = select(&sample(), &GatherConfig::default(), Some("w1"));
        assert_eq!(ids(&got), ["p3", "p5", "p1"]);
    }

    #[test]
    fn an_agent_filter_narrows_by_kind() {
        let config = GatherConfig {
            agents: vec!["codex".into()],
            ..GatherConfig::default()
        };
        assert_eq!(ids(&select(&sample(), &config, None)), ["p3", "p5"]);
    }

    #[test]
    fn an_empty_agent_filter_means_every_kind() {
        let config = GatherConfig::default();
        assert!(config.agents.is_empty());
        assert_eq!(select(&sample(), &config, None).len(), 4);
    }

    #[test]
    fn statuses_are_configurable() {
        let config = GatherConfig {
            statuses: vec![AgentStatus::Blocked],
            ..GatherConfig::default()
        };
        assert_eq!(ids(&select(&sample(), &config, None)), ["p3"]);
    }

    #[test]
    fn the_order_is_stable_across_runs() {
        let config = GatherConfig::default();
        let first = select(&sample(), &config, None);
        let mut shuffled = sample();
        shuffled.reverse();
        let second = select(&shuffled, &config, None);
        assert_eq!(ids(&first), ids(&second));
    }

    #[test]
    fn focus_goes_to_the_most_urgent_agent() {
        let got = select(&sample(), &GatherConfig::default(), None);
        assert_eq!(highest_priority(&got).unwrap().pane_id, "p3");
        assert!(highest_priority(&[]).is_none());
    }

    #[test]
    fn scope_names_round_trip() {
        for scope in [Scope::CurrentWorkspace, Scope::AllWorkspaces] {
            assert_eq!(Scope::parse(scope.as_str()), Some(scope));
        }
    }
}
