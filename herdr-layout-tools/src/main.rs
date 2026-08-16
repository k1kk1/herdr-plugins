//! Herdr Layout Tools — arrange the panes inside one tab.
//!
//! The counterpart to Pane Manager: Pane Manager decides *which tab* a pane
//! belongs to, Layout Tools decides *where in the tab* it sits. Neither
//! reimplements the other (Pane Manager spec §2.2, §27).
//!
//! Every operation keeps the running processes alive.

mod arrange;
mod ops;
mod ui;

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Result};

use crate::arrange::Arrangement;

pub const PLUGIN_ID: &str = "layout-tools";
/// Plugin pane entrypoint id declared in `herdr-plugin.toml`.
const UI_ENTRYPOINT: &str = "layout";

const USAGE: &str = "\
herdr-layout-tools — arrange the panes inside a Herdr tab

Usage:
  herdr-layout-tools launch [--pane ID] [--tab ID]
      Open the Layout Tools menu in a Herdr plugin pane.

  herdr-layout-tools ui
      Run the menu. Herdr invokes this inside the plugin pane.

  herdr-layout-tools equalize [--tab ID]
      Give every pane in the tab the same share of space. Immediate.

  herdr-layout-tools arrange <grid|columns|rows|main-left|main-right|main-top>
                            [--pane ID] [--tab ID]
      Rearrange the tab. --pane names the pane that gets the large slot in
      the main-* arrangements; it defaults to the focused pane.

  herdr-layout-tools doctor
      Print the current tab's split tree.
";

fn main() {
    if let Err(err) = run() {
        // Herdr captures stderr from action commands into `herdr plugin log`.
        eprintln!("layout-tools: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1));
    let herdr = Herdr::connect()?;

    match args.command.as_str() {
        "launch" => launch(&herdr, &args),
        "ui" => {
            let source = context::resolve_source_pane(&herdr, args.pane.as_deref())?;
            if let Some(outcome) = ui::run(&herdr, source, args.tab.as_deref())? {
                outcome.report(&herdr);
            }
            Ok(())
        }
        "equalize" => {
            let (tab_id, _) = target(&herdr, &args)?;
            ops::equalize(&herdr, &tab_id)?.report(&herdr);
            Ok(())
        }
        "arrange" => {
            let Some(raw) = args.positional.first() else {
                bail!("arrange needs a layout name\n\n{USAGE}");
            };
            let Some(arrangement) = Arrangement::parse(raw) else {
                bail!("unknown layout `{raw}`\n\n{USAGE}");
            };
            let (tab_id, main) = target(&herdr, &args)?;
            ops::arrange(&herdr, &tab_id, arrangement, Some(&main))?.report(&herdr);
            Ok(())
        }
        "doctor" => doctor(&herdr, &args),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("herdr-layout-tools {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// The tab to act on and the pane that should get the main slot.
fn target(herdr: &Herdr, args: &Args) -> Result<(String, String)> {
    let source = context::resolve_source_pane(herdr, args.pane.as_deref())?;
    let tab_id = context::resolve_source_tab(args.tab.as_deref(), &source);
    Ok((tab_id, source.pane_id))
}

/// Open the menu in a plugin pane.
///
/// The pane in play is resolved here, before the UI pane exists and steals
/// focus, then handed on through the environment.
fn launch(herdr: &Herdr, args: &Args) -> Result<()> {
    let source = context::resolve_source_pane(herdr, args.pane.as_deref())?;
    let tab_id = context::resolve_source_tab(args.tab.as_deref(), &source);
    let env = [
        ("PM_SOURCE_PANE", source.pane_id.clone()),
        ("PM_SOURCE_TAB", tab_id),
    ];
    herdr.open_plugin_pane(PLUGIN_ID, UI_ENTRYPOINT, &env, true)
}

/// Debug view — the one place raw ids belong.
fn doctor(herdr: &Herdr, args: &Args) -> Result<()> {
    let (tab_id, main) = target(herdr, args)?;
    let layout = herdr.layout(&tab_id)?;
    println!("herdr-layout-tools {}", env!("CARGO_PKG_VERSION"));
    println!("tab        : {tab_id}");
    println!("main pane  : {main}");
    println!("panes      : {}", layout.root.pane_ids().join(", "));
    println!(
        "shape      : {}",
        arrange::Shape::from_layout(&layout.root)
            .map(|s| s.signature())
            .unwrap_or_else(|| "<unreadable>".into())
    );
    println!("splits (path, first leaves, total leaves):");
    for (path, first, total) in layout.root.splits() {
        let path: String = path
            .iter()
            .map(|second| if *second { '2' } else { '1' })
            .collect();
        let path = if path.is_empty() { "root".into() } else { path };
        println!("  {path:<8} {first}/{total}  → ratio {:.3}", first as f32 / total as f32);
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
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut command = None;
        let mut positional = Vec::new();
        let mut pane = None;
        let mut tab = None;

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--pane" => pane = args.next(),
                "--tab" => tab = args.next(),
                _ if command.is_none() => command = Some(arg),
                _ => positional.push(arg),
            }
        }

        Self {
            command: command.unwrap_or_else(|| "ui".to_string()),
            positional,
            pane,
            tab,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Args {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn command_defaults_to_ui() {
        assert_eq!(parse(&[]).command, "ui");
    }

    #[test]
    fn flags_are_pulled_out_of_the_positional_list() {
        let args = parse(&["arrange", "main-left", "--tab", "w1:t2"]);
        assert_eq!(args.command, "arrange");
        assert_eq!(args.positional, vec!["main-left"]);
        assert_eq!(args.tab.as_deref(), Some("w1:t2"));
    }

    #[test]
    fn every_arrangement_name_round_trips_through_the_cli() {
        for arrangement in Arrangement::ALL {
            let name = arrangement.title().to_lowercase().replace(' ', "-");
            assert_eq!(Arrangement::parse(&name), Some(arrangement), "{name}");
        }
    }
}
