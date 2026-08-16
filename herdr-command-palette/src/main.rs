//! Herdr Command Palette — one filterable list of every plugin action.
//!
//! The palette owns no commands of its own. It reads `plugin.action.list`,
//! lets you filter it, and invokes what you pick with the context of the pane
//! you were on. Anything a plugin exposes as an action shows up here for free
//! (Pane Manager spec §12).

mod ui;

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Result};

pub const PLUGIN_ID: &str = "command-palette";
/// Plugin pane entrypoint id declared in `herdr-plugin.toml`.
const UI_ENTRYPOINT: &str = "palette";

const USAGE: &str = "\
herdr-command-palette — run any plugin action from one searchable list

Usage:
  herdr-command-palette launch
      Open the palette in a Herdr plugin pane.

  herdr-command-palette ui
      Run the palette. Herdr invokes this inside the plugin pane.

  herdr-command-palette list
      Print every available action as `plugin<TAB>action<TAB>title`.

  herdr-command-palette run <plugin> <action>
      Invoke one action directly, with the focused pane as its context.
";

fn main() {
    if let Err(err) = run() {
        // Herdr captures stderr from action commands into `herdr plugin log`.
        eprintln!("command-palette: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("ui");
    let herdr = Herdr::connect()?;

    match command {
        "launch" => {
            // Resolve the pane now, while the user's pane is still focused,
            // and hand it to the popup so invoked actions act on the right one.
            let source = context::resolve_source_pane(&herdr, None)?;
            let env = [("PM_SOURCE_PANE", source.pane_id.clone())];
            herdr.open_plugin_pane(PLUGIN_ID, UI_ENTRYPOINT, &env, true)
        }
        "ui" => {
            let source = context::resolve_source_pane(&herdr, None).ok();
            ui::run(&herdr, source)
        }
        "list" => {
            for action in ui::visible_actions(&herdr)? {
                println!(
                    "{}\t{}\t{}",
                    action.plugin_id, action.action_id, action.title
                );
            }
            Ok(())
        }
        "run" => {
            let (Some(plugin), Some(action)) = (args.get(1), args.get(2)) else {
                bail!("run needs a plugin id and an action id\n\n{USAGE}");
            };
            let source = context::resolve_source_pane(&herdr, None).ok();
            let payload = context::InvocationContext::from_env().to_params(source.as_ref());
            herdr.invoke_plugin_action(plugin, action, payload)
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("herdr-command-palette {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}
