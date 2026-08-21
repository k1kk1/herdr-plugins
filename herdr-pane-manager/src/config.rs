//! User settings (spec §20, addendum §6, §11, §12).

use herdr_plugin_kit::herdr::{AgentStatus, Pane};
use herdr_plugin_kit::label;
use herdr_plugin_kit::layout::{Ratio, Side};
use serde::Deserialize;

use crate::ops::Placement;

pub const PLUGIN_ID: &str = "pane-manager";

/// `default_move_direction` (spec §4.1, addendum §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideSetting {
    Left,
    Right,
    Up,
    Down,
    /// Always show the direction step in pickers.
    Ask,
}

impl SideSetting {
    /// Resolved side, or `None` when the user must be asked.
    pub fn resolve(self) -> Option<Side> {
        match self {
            SideSetting::Left => Some(Side::Left),
            SideSetting::Right => Some(Side::Right),
            SideSetting::Up => Some(Side::Up),
            SideSetting::Down => Some(Side::Down),
            SideSetting::Ask => None,
        }
    }
}

/// `default_split_ratio` (addendum §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum RatioSetting {
    #[serde(rename = "50:50")]
    Even,
    #[serde(rename = "60:40")]
    SixtyForty,
    #[serde(rename = "40:60")]
    FortySixty,
    /// Ask which of the three to use.
    #[serde(rename = "ask")]
    Ask,
}

impl RatioSetting {
    pub fn resolve(self) -> Ratio {
        match self {
            RatioSetting::SixtyForty => Ratio::SIXTY_FORTY,
            RatioSetting::FortySixty => Ratio::FORTY_SIXTY,
            _ => Ratio::EVEN,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Where a moved pane lands: `left` / `right` / `up` / `down` / `ask`.
    pub default_move_direction: SideSetting,
    /// How the space is shared: `"50:50"` / `"60:40"` / `"40:60"` / `"ask"`.
    pub default_split_ratio: RatioSetting,
    /// Focus the pane an operation acted on when it finishes (addendum §6).
    pub focus_after_operation: bool,
    /// Always ask which pane in the destination tab to split (spec §4.1).
    pub advanced_move: bool,
    /// Keep the source tab's split structure when merging (addendum §11).
    pub preserve_merge_layout: bool,
    /// Auto-name tabs created by Extract (spec §5.1).
    pub auto_name_new_tab: bool,
    /// Show the agent state glyph and label in pickers (spec §13).
    pub show_agent_state: bool,
    /// Show the terminal title as a third line in pickers (spec §8.3).
    pub show_terminal_title: bool,
    /// Ask before merging a tab (spec §20; off by default per §14.1).
    pub confirm_merge: bool,
    /// Reveal `w1:p3`-style IDs in pickers (spec §3.2 — Debug only).
    pub show_ids: bool,
    /// What choosing a row does when no modifier is held.
    pub default_action: DefaultAction,
    /// Active Agent Gather (addendum §2, §4, §7, §15, §16).
    pub gather: GatherConfig,
}

/// Which of the two outcomes an unmodified Enter (or hotkey) gives.
///
/// Shift always gives *the other one*, so both stay one keystroke away
/// whichever is set. Someone who nearly always wants to say where the pane
/// lands should not have to hold Shift every time to get it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultAction {
    /// Move straight away, using the configured direction and ratio.
    #[default]
    Quick,
    /// Stop and ask where the pane should land.
    Detailed,
}

impl DefaultAction {
    /// Whether the placement step should run, given whether Shift was held.
    ///
    /// Exclusive-or: the setting says what the plain key does, and Shift means
    /// "not that".
    pub fn detailed(self, shift_held: bool) -> bool {
        (self == DefaultAction::Detailed) != shift_held
    }
}

/// `[pane-manager.gather]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GatherConfig {
    /// Agent states worth collecting. `idle` and `unknown` are left out by
    /// default, and panes with no detected agent are never candidates.
    pub statuses: Vec<AgentStatus>,
    /// Agents per Gather tab: 2, 3 or 4.
    pub max_panes_per_tab: u8,
    /// How far to look: `"workspace"` or `"all"`.
    pub scope: String,
    /// Focus the most urgent agent once the Gather finishes.
    pub focus_highest_priority: bool,
    /// Restrict to certain agent kinds, e.g. `["codex", "claude"]`.
    /// Empty means every kind.
    pub agents: Vec<String>,
    /// Name of the tabs Gather creates.
    pub tab_label: String,
}

impl Default for GatherConfig {
    fn default() -> Self {
        Self {
            statuses: crate::gather::select::default_statuses(),
            max_panes_per_tab: 4,
            scope: "workspace".into(),
            focus_highest_priority: true,
            agents: Vec::new(),
            tab_label: "Active Agents".into(),
        }
    }
}

impl GatherConfig {
    /// Configured group size, falling back to 4 for an unsupported number.
    pub fn per_tab(&self) -> crate::gather::layout::PanesPerTab {
        crate::gather::layout::PanesPerTab::new(self.max_panes_per_tab).unwrap_or_default()
    }

    pub fn scope(&self) -> crate::gather::select::Scope {
        crate::gather::select::Scope::parse(&self.scope)
            .unwrap_or(crate::gather::select::Scope::CurrentWorkspace)
    }

    /// The configured statuses, for messages: `blocked, done, working`.
    pub fn status_summary(&self) -> String {
        self.statuses
            .iter()
            .map(|s| s.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_move_direction: SideSetting::Right,
            default_split_ratio: RatioSetting::Even,
            focus_after_operation: true,
            advanced_move: false,
            preserve_merge_layout: true,
            auto_name_new_tab: true,
            show_agent_state: true,
            show_terminal_title: true,
            confirm_merge: false,
            show_ids: false,
            default_action: DefaultAction::default(),
            gather: GatherConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        herdr_plugin_kit::config::load::<Self>(PLUGIN_ID).0
    }

    /// Like [`Config::load`], but also returns a warning describing why the
    /// file was ignored, so the overlay can surface a bad config.
    pub fn load_reporting() -> (Self, Option<String>) {
        herdr_plugin_kit::config::load::<Self>(PLUGIN_ID)
    }

    /// Whether the picker has to ask for a ratio.
    pub fn ask_ratio(&self) -> bool {
        self.default_split_ratio == RatioSetting::Ask
    }

    pub fn ratio(&self) -> Ratio {
        self.default_split_ratio.resolve()
    }

    /// Placement for the paths that must not ask anything: Quick Move and the
    /// headless actions (addendum §2).
    pub fn quick_placement(&self) -> Placement {
        Placement {
            side: self.default_move_direction.resolve().unwrap_or(Side::Right),
            ratio: self.ratio(),
        }
    }

    /// Name for a tab or workspace being created.
    ///
    /// Anything the user typed into the picker wins; otherwise the pane names
    /// it after itself (spec §5.1), unless auto-naming is off.
    pub fn new_tab_label(&self, pane: &Pane, typed: Option<&str>) -> Option<String> {
        if let Some(typed) = typed {
            return Some(typed.to_string());
        }
        if !self.auto_name_new_tab {
            return None;
        }
        label::new_tab_label(pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Config {
        toml::from_str::<toml::Table>(raw)
            .unwrap()
            .get(PLUGIN_ID)
            .cloned()
            .unwrap()
            .try_into()
            .unwrap()
    }

    #[test]
    fn defaults_match_the_spec() {
        let config = Config::default();
        assert_eq!(config.default_move_direction, SideSetting::Right);
        assert_eq!(config.ratio(), Ratio::EVEN);
        assert!(config.focus_after_operation);
        assert!(config.preserve_merge_layout);
        assert!(!config.confirm_merge);
        assert!(!config.show_ids);
    }

    #[test]
    fn all_four_directions_are_configurable() {
        for (name, want) in [
            ("left", Some(Side::Left)),
            ("right", Some(Side::Right)),
            ("up", Some(Side::Up)),
            ("down", Some(Side::Down)),
            ("ask", None),
        ] {
            let config = parse(&format!(
                "[pane-manager]\ndefault_move_direction = \"{name}\"\n"
            ));
            assert_eq!(config.default_move_direction.resolve(), want, "{name}");
        }
    }

    #[test]
    fn ratios_are_written_the_way_they_are_displayed() {
        let config = parse("[pane-manager]\ndefault_split_ratio = \"60:40\"\n");
        assert_eq!(config.ratio(), Ratio::SIXTY_FORTY);
        assert!(!config.ask_ratio());

        let config = parse("[pane-manager]\ndefault_split_ratio = \"ask\"\n");
        assert!(config.ask_ratio());
        // Asking still needs a value for the paths that cannot ask.
        assert_eq!(config.ratio(), Ratio::EVEN);
    }

    #[test]
    fn quick_move_never_needs_to_ask() {
        let config = parse(
            "[pane-manager]\ndefault_move_direction = \"ask\"\ndefault_split_ratio = \"ask\"\n",
        );
        let placement = config.quick_placement();
        assert_eq!(placement.side, Side::Right);
        assert_eq!(placement.ratio, Ratio::EVEN);
    }

    #[test]
    fn gather_defaults_match_the_spec() {
        let gather = Config::default().gather;
        assert_eq!(gather.statuses.len(), 3);
        assert_eq!(gather.per_tab().get(), 4);
        assert_eq!(gather.scope(), crate::gather::select::Scope::CurrentWorkspace);
        assert!(gather.focus_highest_priority);
        assert!(gather.agents.is_empty());
        assert_eq!(gather.tab_label, "Active Agents");
        assert_eq!(gather.status_summary(), "blocked, done, working");
    }

    #[test]
    fn gather_settings_live_in_their_own_table() {
        let config = parse(
            r#"
            [pane-manager]
            confirm_merge = true

            [pane-manager.gather]
            max_panes_per_tab = 2
            scope = "all"
            statuses = ["blocked"]
            agents = ["codex"]
            "#,
        );
        assert!(config.confirm_merge);
        assert_eq!(config.gather.per_tab().get(), 2);
        assert_eq!(config.gather.scope(), crate::gather::select::Scope::AllWorkspaces);
        assert_eq!(config.gather.statuses, vec![AgentStatus::Blocked]);
        assert_eq!(config.gather.agents, vec!["codex".to_string()]);
        // Untouched keys keep their defaults.
        assert!(config.gather.focus_highest_priority);
    }

    #[test]
    fn an_unsupported_group_size_falls_back_to_four() {
        let config = parse("[pane-manager]\n[pane-manager.gather]\nmax_panes_per_tab = 7\n");
        assert_eq!(config.gather.per_tab().get(), 4);
    }

    #[test]
    fn a_typed_name_wins_over_auto_naming() {
        let mut pane = pane();
        pane.label = Some("Music".into());
        let config = Config::default();
        assert_eq!(
            config.new_tab_label(&pane, Some("review")).as_deref(),
            Some("review")
        );
        assert_eq!(config.new_tab_label(&pane, None).as_deref(), Some("Music"));
    }

    #[test]
    fn auto_naming_can_be_turned_off_without_losing_typed_names() {
        let mut pane = pane();
        pane.label = Some("Music".into());
        let config = parse("[pane-manager]\nauto_name_new_tab = false\n");
        assert_eq!(config.new_tab_label(&pane, None), None);
        assert_eq!(
            config.new_tab_label(&pane, Some("review")).as_deref(),
            Some("review")
        );
    }

    fn pane() -> Pane {
        Pane {
            pane_id: "w1:p1".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            focused: false,
            label: None,
            agent: None,
            display_agent: None,
            agent_status: Default::default(),
            cwd: None,
            foreground_cwd: None,
            terminal_title: None,
            terminal_title_stripped: None,
        }
    }
}

#[cfg(test)]
mod default_action_tests {
    use super::DefaultAction;

    #[test]
    fn shift_always_gives_the_other_outcome() {
        // Whichever way round the setting is, both outcomes stay one
        // keystroke away — that is the point of making it configurable.
        assert!(!DefaultAction::Quick.detailed(false));
        assert!(DefaultAction::Quick.detailed(true));
        assert!(DefaultAction::Detailed.detailed(false));
        assert!(!DefaultAction::Detailed.detailed(true));
    }

    #[test]
    fn quick_is_the_default_so_nobody_gains_a_prompt_they_did_not_ask_for() {
        assert_eq!(DefaultAction::default(), DefaultAction::Quick);
    }
}
