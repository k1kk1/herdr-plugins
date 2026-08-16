//! Herdr Navigator — find and jump to any pane, tab or workspace.
//!
//! The search-and-jump half of the plugin set. Pane Manager moves panes,
//! Layout Tools arranges them; Navigator only ever changes what is *focused*,
//! never where anything lives (Pane Manager spec §2.2, §27).

mod ui;

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::label;
use herdr_plugin_kit::{bail, Result};

use crate::ui::Scope;

pub const PLUGIN_ID: &str = "navigator";
/// Plugin pane entrypoint id declared in `herdr-plugin.toml`.
const UI_ENTRYPOINT: &str = "navigator";

const USAGE: &str = "\
herdr-navigator — find and jump to any pane, tab or workspace

Usage:
  herdr-navigator launch [panes|tabs|workspaces|agents]
      Open the picker in a Herdr plugin pane.

  herdr-navigator ui [panes|tabs|workspaces|agents]
      Run the picker. Herdr invokes this inside the plugin pane; the scope
      also comes from NAV_SCOPE when no argument is given.

  herdr-navigator list [panes|tabs|workspaces|agents]
      Print the same list as text, for scripts.

  herdr-navigator focus <id>
      Focus a pane, tab or workspace by id.
";

fn main() {
    if let Err(err) = run() {
        // Herdr captures stderr from action commands into `herdr plugin log`.
        eprintln!("navigator: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("ui");
    let herdr = Herdr::connect()?;

    match command {
        "launch" => {
            let env = [("NAV_SCOPE", scope_arg(&args).name().to_string())];
            herdr.open_plugin_pane(PLUGIN_ID, UI_ENTRYPOINT, &env, true)
        }
        "ui" => {
            // Resolving the pane is best-effort: Navigator still works from a
            // context where nothing sensible is focused, it just cannot mark
            // the current pane in the list.
            let current = context::resolve_source_pane(&herdr, None).ok();
            ui::run(&herdr, scope_arg(&args), current)
        }
        "list" => list(&herdr, scope_arg(&args)),
        "focus" => {
            let Some(id) = args.get(1) else {
                bail!("focus needs an id\n\n{USAGE}");
            };
            focus(&herdr, id)
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("herdr-navigator {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// Scope from the argument, else `NAV_SCOPE`, else panes.
fn scope_arg(args: &[String]) -> Scope {
    args.get(1)
        .and_then(|raw| Scope::parse(raw))
        .or_else(|| {
            std::env::var("NAV_SCOPE")
                .ok()
                .and_then(|raw| Scope::parse(&raw))
        })
        .unwrap_or(Scope::Panes)
}

/// Focus whatever the id refers to.
///
/// Herdr ids carry their own kind (`w1`, `w1:t2`, `w1:p3`), so the caller does
/// not have to say which it means.
fn focus(herdr: &Herdr, id: &str) -> Result<()> {
    if let Some((_, rest)) = id.split_once(':') {
        match rest.chars().next() {
            Some('p') => return herdr.focus_pane(id),
            Some('t') => return herdr.focus_tab(id),
            _ => bail!("`{id}` is not a pane, tab or workspace id"),
        }
    }
    herdr.focus_workspace(id)
}

/// Text form of the picker's contents.
fn list(herdr: &Herdr, scope: Scope) -> Result<()> {
    match scope {
        Scope::Workspaces => {
            for workspace in herdr.workspaces()? {
                println!(
                    "{}\t{}\t{}",
                    workspace.workspace_id,
                    workspace.label.as_deref().unwrap_or("-"),
                    workspace.agent_status.label()
                );
            }
        }
        Scope::Tabs => {
            for (index, tab) in herdr.all_tabs()?.into_iter().enumerate() {
                println!(
                    "{}\t{}\t{} panes",
                    tab.tab_id,
                    label::tab_display(&tab, index + 1),
                    tab.pane_count
                );
            }
        }
        Scope::Panes | Scope::Agents => {
            for pane in herdr.all_panes()? {
                if scope == Scope::Agents && pane.agent.is_none() {
                    continue;
                }
                println!(
                    "{}\t{}\t{}",
                    pane.pane_id,
                    label::pane_compact(&pane),
                    pane.agent_status.label()
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn scope_defaults_to_panes() {
        assert_eq!(scope_arg(&args(&["ui"])), Scope::Panes);
    }

    #[test]
    fn scope_comes_from_the_argument() {
        assert_eq!(scope_arg(&args(&["ui", "tabs"])), Scope::Tabs);
        assert_eq!(scope_arg(&args(&["ui", "workspaces"])), Scope::Workspaces);
        assert_eq!(scope_arg(&args(&["ui", "agents"])), Scope::Agents);
    }

    #[test]
    fn an_unknown_scope_falls_back_rather_than_failing() {
        assert_eq!(scope_arg(&args(&["ui", "nonsense"])), Scope::Panes);
    }
}
