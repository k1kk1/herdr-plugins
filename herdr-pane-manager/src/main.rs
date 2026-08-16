//! Herdr Pane Manager — move, swap, extract and merge panes across tabs
//! without restarting the processes running inside them.
//!
//! One binary serves every invocation path required by spec §3.3 and
//! addendum §13:
//!
//! * `launch` — resolves the pane in play and raises the overlay. Bound to
//!   `prefix+m`, and used by the context-menu actions that need a choice.
//! * `ui`     — the overlay itself, run by Herdr inside a plugin pane.
//! * `extract` / `quick-move` / `move` / `swap` / `merge` — headless.
//!
//! All of them end in `ops::execute`, so behaviour cannot drift between paths.

mod config;
mod gather;
mod place;
mod undo;
mod ops;
mod state;
mod ui;

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::label;
use herdr_plugin_kit::layout::{Ratio, Side};
use herdr_plugin_kit::{bail, Result};

use crate::config::{Config, PLUGIN_ID};
use crate::gather::layout::PanesPerTab;
use crate::gather::select::Scope;
use crate::ops::{Destination, Placement, Request, Verb};
use crate::state::Snapshot;
use crate::ui::Entry;

/// Plugin pane entrypoint id declared in `herdr-plugin.toml`.
const UI_ENTRYPOINT: &str = "manager";

const USAGE: &str = "\
herdr-pane-manager — Herdr Pane Manager plugin

Usage:
  herdr-pane-manager launch [manager|move|swap|merge] [--pane ID] [--tab ID]
      Resolve the pane in play and open the overlay in a Herdr plugin pane.

  herdr-pane-manager ui [manager|move|swap|merge]
      Run the overlay. Herdr invokes this inside the plugin pane; the screen
      also comes from PM_SCREEN when no argument is given.

  herdr-pane-manager extract [--pane ID] [--new-workspace] [--label TEXT]
      Split the pane out into a tab, or a workspace, of its own. Immediate.

  herdr-pane-manager quick-move <1-9> [--pane ID] [--side S] [--ratio R]
      Move the pane to the tab in that slot. Immediate.

Headless forms of the operation API, for scripts and other plugins:

  herdr-pane-manager move  --tab ID [--pane ID] [--target-pane ID]
                           [--side left|right|up|down] [--ratio 50:50|60:40|40:60]
  herdr-pane-manager swap  --with ID [--pane ID]
  herdr-pane-manager merge --tab ID [--source-tab ID] [--side S] [--ratio R]
                           [--flatten]

Active Agent Gather:

  herdr-pane-manager gather [2|3|4] [--scope workspace|all]
      Collect the blocked / done / working agents into dedicated tabs, most
      urgent first. Running it again refreshes the existing Gather.

  herdr-pane-manager refresh-gather
      Rebuild the Gather from the agents' current states.

  herdr-pane-manager restore-gather
      Return every gathered pane to where it came from.

  herdr-pane-manager undo
      Reverse the last Move / Extract / Merge / Swap. One level deep.

  herdr-pane-manager doctor
      Print what the plugin can see: config, workspaces, tabs, panes.
";

fn main() {
    if let Err(err) = run() {
        // Herdr captures stderr from action commands into `herdr plugin log`.
        eprintln!("pane-manager: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    let herdr = Herdr::connect()?;

    match args.command.as_str() {
        "launch" => launch(&herdr, &args),
        "ui" => interactive(&herdr, &args),
        "extract" | "quick-move" | "move" | "swap" | "merge" => headless(&herdr, &args),
        "gather" | "refresh-gather" | "restore-gather" => gather_command(&herdr, &args),
        "undo" => undo::undo(&herdr).map(|outcome| outcome.report(&herdr)),
        "doctor" => doctor(&herdr, &args),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("herdr-pane-manager {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// Open the overlay in a plugin pane.
///
/// The pane in play is resolved *here*, before the overlay exists and steals
/// focus, then handed on through the environment.
fn launch(herdr: &Herdr, args: &Args) -> Result<()> {
    let source = context::resolve_source_pane(herdr, args.pane.as_deref())?;
    let tab_id = context::resolve_source_tab(args.tab.as_deref(), &source);

    let env = [
        ("PM_SCREEN", args.screen()),
        ("PM_SOURCE_PANE", source.pane_id.clone()),
        ("PM_SOURCE_TAB", tab_id),
    ];
    herdr.open_plugin_pane(PLUGIN_ID, UI_ENTRYPOINT, &env, true)
}

fn interactive(herdr: &Herdr, args: &Args) -> Result<()> {
    let entry = match args.screen().as_str() {
        "move" => Entry::Move,
        "swap" => Entry::Swap,
        "merge" => Entry::Merge,
        _ => Entry::Manager,
    };

    let source = context::resolve_source_pane(herdr, args.pane.as_deref())?;
    if let Some(outcome) = ui::run(herdr, entry, source)? {
        outcome.report(herdr);
    }
    Ok(())
}

/// The operation API (spec §22) with every argument supplied, so scripts,
/// key bindings and other plugins can drive Pane Manager without a picker.
fn headless(herdr: &Herdr, args: &Args) -> Result<()> {
    let config = Config::load();
    let source = context::resolve_source_pane(herdr, args.pane.as_deref())?;
    let snapshot = Snapshot::capture(herdr, source)?;

    // A headless path cannot ask, so an "ask" setting falls back to the
    // documented default unless the caller was explicit.
    let placement = Placement {
        side: args
            .side
            .or_else(|| config.default_move_direction.resolve())
            .unwrap_or(Side::Right),
        ratio: args.ratio.unwrap_or_else(|| config.ratio()),
    };

    let request = match args.command.as_str() {
        "quick-move" => {
            let Some(position) = args.positional.first().and_then(|s| s.parse::<usize>().ok())
            else {
                bail!("quick-move needs a tab number between 1 and 9");
            };
            if !(1..=9).contains(&position) {
                bail!("quick-move takes a tab number between 1 and 9, got {position}");
            }
            let Some(tab) = snapshot.tab_at(position) else {
                bail!("This workspace has no tab {position}.");
            };
            Request {
                verb: Verb::Move,
                source_pane: snapshot.source.pane_id.clone(),
                source_tab: None,
                destination: Destination::Tab {
                    tab_id: tab.tab.tab_id.clone(),
                    target_pane: None,
                },
                placement,
                preserve_layout: false,
            }
        }
        "extract" => {
            let label = config.new_tab_label(&snapshot.source, args.label.as_deref());
            Request {
                verb: Verb::Extract,
                source_pane: snapshot.source.pane_id.clone(),
                source_tab: None,
                destination: if args.new_workspace {
                    Destination::NewWorkspace { label }
                } else {
                    Destination::NewTab { label }
                },
                placement,
                preserve_layout: false,
            }
        }
        "move" => {
            let destination = match (&args.tab, args.new_workspace) {
                (Some(tab_id), _) => Destination::Tab {
                    tab_id: tab_id.clone(),
                    target_pane: args.target_pane.clone(),
                },
                (None, true) => Destination::NewWorkspace {
                    label: config.new_tab_label(&snapshot.source, args.label.as_deref()),
                },
                (None, false) => bail!("move needs --tab <tab_id> or --new-workspace"),
            };
            let verb = if matches!(destination, Destination::Tab { .. }) {
                Verb::Move
            } else {
                Verb::Extract
            };
            Request {
                verb,
                source_pane: snapshot.source.pane_id.clone(),
                source_tab: None,
                destination,
                placement,
                preserve_layout: false,
            }
        }
        "swap" => {
            let Some(target) = args.with_pane.clone() else {
                bail!("swap needs --with <pane_id>");
            };
            Request {
                verb: Verb::Swap,
                source_pane: snapshot.source.pane_id.clone(),
                source_tab: None,
                destination: Destination::Pane { pane_id: target },
                placement,
                preserve_layout: false,
            }
        }
        "merge" => {
            let Some(tab_id) = args.tab.clone() else {
                bail!("merge needs --tab <destination_tab_id>");
            };
            Request {
                verb: Verb::Merge,
                source_pane: snapshot.source.pane_id.clone(),
                source_tab: Some(
                    args.source_tab
                        .clone()
                        .unwrap_or_else(|| snapshot.source.tab_id.clone()),
                ),
                destination: Destination::Tab {
                    tab_id,
                    target_pane: args.target_pane.clone(),
                },
                placement,
                preserve_layout: config.preserve_merge_layout && !args.flatten,
            }
        }
        other => bail!("unknown command `{other}`"),
    };

    ops::execute(herdr, &snapshot, &request, &config)?.report(herdr);
    Ok(())
}

/// Active Agent Gather (addendum §17).
fn gather_command(herdr: &Herdr, args: &Args) -> Result<()> {
    let config = Config::load();

    let outcome = match args.command.as_str() {
        "restore-gather" => gather::restore(herdr)?,
        "refresh-gather" => gather::refresh(herdr, &config)?,
        _ => {
            let per_tab = match args.positional.first() {
                Some(raw) => {
                    let Some(size) = raw.parse::<u8>().ok().and_then(PanesPerTab::new) else {
                        bail!("gather takes 2, 3 or 4 panes per tab, got `{raw}`");
                    };
                    size
                }
                None => config.gather.per_tab(),
            };
            let scope = match args.scope.as_deref() {
                Some(raw) => Scope::parse(raw)
                    .ok_or_else(|| herdr_plugin_kit::anyhow!("--scope takes workspace or all"))?,
                None => config.gather.scope(),
            };
            gather::gather(herdr, &config, per_tab, scope)?
        }
    };

    outcome.report(herdr);
    Ok(())
}

/// Debug view — the one place IDs are shown by design (spec §3.2).
fn doctor(herdr: &Herdr, args: &Args) -> Result<()> {
    let (config, warning) = Config::load_reporting();
    println!("herdr-pane-manager {}", env!("CARGO_PKG_VERSION"));
    println!(
        "config file: {}",
        herdr_plugin_kit::config::config_path(PLUGIN_ID)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".into())
    );
    if let Some(warning) = warning {
        println!("config warning: {warning}");
    }
    println!("config: {config:?}");

    let source = context::resolve_source_pane(herdr, args.pane.as_deref())?;
    let snapshot = Snapshot::capture(herdr, source)?;
    println!(
        "\nsource pane {} — {}",
        snapshot.source.pane_id,
        label::pane_compact(&snapshot.source)
    );

    match crate::gather::session::load() {
        Some(session) => {
            println!(
                "\ngather session: {} pane(s), {} tab(s), scope={}, {} per tab",
                session.pane_ids().len(),
                session.gather_tabs.len(),
                session.scope,
                session.panes_per_tab
            );
            for origin in &session.origins {
                println!(
                    "      {} ← {} (anchor {:?} {:?})",
                    origin.pane_id,
                    origin.tab_id,
                    origin.anchor.as_deref().unwrap_or("-"),
                    origin.side.as_deref().unwrap_or("-")
                );
            }
        }
        None => println!("\ngather session: none"),
    }

    for (workspace, tab) in snapshot.all_tabs() {
        if tab.position == 1 {
            println!(
                "\nworkspace {} ({})",
                workspace.workspace_id,
                workspace.label.as_deref().unwrap_or("-")
            );
        }
        println!(
            "  [{}] {}  ({})  shape={}",
            tab.position,
            label::tab_display(&tab.tab, tab.position),
            tab.tab.tab_id,
            tab.shape
                .as_ref()
                .map(|s| s.signature())
                .unwrap_or_else(|| "-".into())
        );
        for pane in &tab.panes {
            println!(
                "      {} {}  ({}, {})",
                pane.agent_status.glyph(),
                label::pane_compact(pane),
                pane.pane_id,
                pane.agent_status.label()
            );
        }
    }
    Ok(())
}

/// Hand-rolled argument parsing — the surface is small and a CLI parser
/// dependency would dwarf it.
struct Args {
    command: String,
    positional: Vec<String>,
    pane: Option<String>,
    tab: Option<String>,
    /// Destination pane to split, for `move` and `merge`.
    target_pane: Option<String>,
    /// Pane to exchange with, for `swap`.
    with_pane: Option<String>,
    /// Tab whose panes are merged away, for `merge`.
    source_tab: Option<String>,
    /// Name for a tab or workspace being created.
    label: Option<String>,
    /// Create a workspace rather than a tab.
    new_workspace: bool,
    /// Merge without preserving the source tab's split structure.
    flatten: bool,
    /// Gather scope: `workspace` or `all`.
    scope: Option<String>,
    side: Option<Side>,
    ratio: Option<Ratio>,
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut command = None;
        let mut positional = Vec::new();
        let mut pane = None;
        let mut tab = None;
        let mut target_pane = None;
        let mut with_pane = None;
        let mut source_tab = None;
        let mut label = None;
        let mut new_workspace = false;
        let mut flatten = false;
        let mut scope = None;
        let mut side = None;
        let mut ratio = None;

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--pane" => pane = args.next(),
                "--tab" => tab = args.next(),
                "--target-pane" => target_pane = args.next(),
                "--with" => with_pane = args.next(),
                "--source-tab" => source_tab = args.next(),
                "--label" => label = args.next(),
                "--new-workspace" => new_workspace = true,
                "--flatten" => flatten = true,
                "--scope" => scope = args.next(),
                // `--direction` is the name the original spec used.
                "--side" | "--direction" => {
                    let raw = args.next().unwrap_or_default();
                    side = Some(Side::parse(&raw).ok_or_else(|| {
                        herdr_plugin_kit::anyhow!("--side takes left, right, up or down")
                    })?);
                }
                "--ratio" => {
                    let raw = args.next().unwrap_or_default();
                    ratio = Some(Ratio::parse(&raw).ok_or_else(|| {
                        herdr_plugin_kit::anyhow!("--ratio takes 50:50, 60:40 or 40:60")
                    })?);
                }
                _ if command.is_none() => command = Some(arg),
                _ => positional.push(arg),
            }
        }

        Ok(Self {
            command: command.unwrap_or_else(|| "ui".to_string()),
            positional,
            pane,
            tab,
            target_pane,
            with_pane,
            source_tab,
            label,
            new_workspace,
            flatten,
            scope,
            side,
            ratio,
        })
    }

    /// Screen name from the argument, else `PM_SCREEN`, else the overlay.
    fn screen(&self) -> String {
        self.positional
            .first()
            .cloned()
            .or_else(|| std::env::var("PM_SCREEN").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "manager".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Args {
        Args::parse(args.iter().map(|s| s.to_string())).unwrap()
    }

    #[test]
    fn command_defaults_to_ui() {
        assert_eq!(parse(&[]).command, "ui");
    }

    #[test]
    fn flags_are_pulled_out_of_the_positional_list() {
        let args = parse(&["launch", "swap", "--pane", "w1:p3", "--tab", "w1:t2"]);
        assert_eq!(args.command, "launch");
        assert_eq!(args.positional, vec!["swap"]);
        assert_eq!(args.pane.as_deref(), Some("w1:p3"));
        assert_eq!(args.tab.as_deref(), Some("w1:t2"));
    }

    #[test]
    fn flags_may_precede_the_screen_name() {
        let args = parse(&["--pane", "w1:p3", "quick-move", "4"]);
        assert_eq!(args.command, "quick-move");
        assert_eq!(args.positional, vec!["4"]);
        assert_eq!(args.pane.as_deref(), Some("w1:p3"));
    }

    #[test]
    fn all_four_sides_are_accepted_on_the_command_line() {
        for (flag, want) in [
            ("left", Side::Left),
            ("right", Side::Right),
            ("up", Side::Up),
            ("down", Side::Down),
        ] {
            assert_eq!(parse(&["move", "--side", flag]).side, Some(want));
        }
        // The original spec's flag name still works.
        assert_eq!(
            parse(&["move", "--direction", "down"]).side,
            Some(Side::Down)
        );
    }

    #[test]
    fn ratios_are_accepted_as_written_in_the_spec() {
        assert_eq!(
            parse(&["move", "--ratio", "60:40"]).ratio,
            Some(Ratio::SIXTY_FORTY)
        );
        assert_eq!(
            parse(&["move", "--ratio", "40:60"]).ratio,
            Some(Ratio::FORTY_SIXTY)
        );
        assert_eq!(parse(&["move", "--ratio", "50:50"]).ratio, Some(Ratio::EVEN));
    }

    #[test]
    fn a_bad_side_or_ratio_is_rejected_rather_than_ignored() {
        assert!(Args::parse(["move", "--side", "sideways"].iter().map(|s| s.to_string())).is_err());
        assert!(Args::parse(["move", "--ratio", "90:10"].iter().map(|s| s.to_string())).is_err());
    }

    #[test]
    fn gather_takes_a_size_and_a_scope() {
        let args = parse(&["gather", "3", "--scope", "all"]);
        assert_eq!(args.command, "gather");
        assert_eq!(args.positional, vec!["3"]);
        assert_eq!(args.scope.as_deref(), Some("all"));
        assert_eq!(Scope::parse("all"), Some(Scope::AllWorkspaces));
    }

    #[test]
    fn creation_flags_are_recognised() {
        let args = parse(&["extract", "--new-workspace", "--label", "review"]);
        assert!(args.new_workspace);
        assert_eq!(args.label.as_deref(), Some("review"));
        assert!(parse(&["merge", "--flatten"]).flatten);
    }
}
