//! Herdr Open — hand the current pane's working directory to something outside
//! the terminal.
//!
//! The other plugins in this set all rearrange or search the session. This one
//! is the only one that points *outward*: Finder, an editor, the clipboard.
//! It moves nothing and focuses nothing (Pane Manager spec §2.2, §27).

mod dir;
mod target;
mod ui;

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Result};

use crate::target::Config;

pub const PLUGIN_ID: &str = "open";
/// Plugin pane entrypoint id declared in `herdr-plugin.toml`.
const UI_ENTRYPOINT: &str = "open";

const USAGE: &str = "\
herdr-open — open a pane's working directory outside the terminal

Usage:
  herdr-open launch [--pane ID]
      Open the picker in a Herdr plugin pane.

  herdr-open ui [--pane ID]
      Run the picker. Herdr invokes this inside the plugin pane.

  herdr-open open <target> [--pane ID] [--git-root]
      Run one target straight away, no picker. Targets are listed by
      `herdr-open list`; the built-in ids are finder, editor and copy-path.

  herdr-open list
      Print the configured targets and whether each one is installed.

  herdr-open where [--pane ID] [--git-root]
      Print the directory the other commands would use.

Options:
  --pane ID    Act on this pane instead of the one in play.
  --git-root   Use the repository root rather than the pane's own directory.
";

fn main() {
    if let Err(err) = run() {
        // Herdr captures stderr from action commands into `herdr plugin log`.
        eprintln!("open: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1));
    let (config, warning) = Config::load();

    match args.command.as_str() {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            return Ok(());
        }
        "--version" | "-V" => {
            println!("herdr-open {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }

    let herdr = Herdr::connect()?;
    // A config typo must be visible, but it must not stop the plugin: the
    // built-in targets still work, and being told why is better than wondering
    // where the row went.
    if let Some(warning) = &warning {
        eprintln!("open: {warning}");
        herdr.notify("Open: ignoring the plugin config", Some(warning));
    }

    match args.command.as_str() {
        "launch" => {
            // Resolve the pane here, before the popup exists and takes focus.
            let source = context::resolve_source_pane(&herdr, args.pane.as_deref())?;
            let env = [("OPEN_SOURCE_PANE", source.pane_id.clone())];
            herdr.open_plugin_pane(PLUGIN_ID, UI_ENTRYPOINT, &env, true)
        }
        "ui" => {
            let source = source_pane(&herdr, &args)?;
            if let Some(outcome) = ui::run(&source, &config)? {
                outcome.report(&herdr);
            }
            Ok(())
        }
        "open" => {
            let Some(id) = args.positional.first() else {
                bail!("open needs a target\n\n{USAGE}");
            };
            let source = source_pane(&herdr, &args)?;
            let target = config.find(id)?;
            let directory = directory(&source, args.git_root || config.prefer_git_root)?;
            target.run(&directory)?.report(&herdr);
            Ok(())
        }
        "list" => {
            for target in &config.target {
                println!(
                    "{}\t{}\t{}",
                    target.id,
                    target.title,
                    if target.is_available() {
                        "installed"
                    } else {
                        "missing"
                    }
                );
            }
            Ok(())
        }
        "where" => {
            let source = source_pane(&herdr, &args)?;
            let directory = directory(&source, args.git_root || config.prefer_git_root)?;
            println!("{}", directory.display());
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// The pane in play: `--pane`, else the one the launcher resolved, else
/// whatever Herdr says is focused.
fn source_pane(herdr: &Herdr, args: &Args) -> Result<herdr_plugin_kit::herdr::Pane> {
    let explicit = args
        .pane
        .clone()
        .or_else(|| std::env::var("OPEN_SOURCE_PANE").ok())
        .filter(|id| !id.trim().is_empty());
    context::resolve_source_pane(herdr, explicit.as_deref())
}

/// The directory to act on, repository root included when asked for.
///
/// Asking for the root of something that is not a repository is not an error:
/// the pane's own directory is still the honest answer.
fn directory(
    pane: &herdr_plugin_kit::herdr::Pane,
    git_root: bool,
) -> Result<std::path::PathBuf> {
    let cwd = dir::pane_dir(pane)?;
    if git_root {
        if let Some(root) = dir::git_root(&cwd) {
            return Ok(root);
        }
    }
    Ok(cwd)
}

/// Arguments, in the same shape the other plugins parse them.
#[derive(Debug, Default)]
struct Args {
    command: String,
    positional: Vec<String>,
    pane: Option<String>,
    git_root: bool,
}

impl Args {
    fn parse(raw: impl Iterator<Item = String>) -> Self {
        let mut args = Args {
            command: "ui".into(),
            ..Default::default()
        };
        let mut rest: Vec<String> = Vec::new();
        let mut items = raw.peekable();
        let mut first = true;

        while let Some(item) = items.next() {
            match item.as_str() {
                "--pane" => args.pane = items.next(),
                "--git-root" => args.git_root = true,
                // Flags may come first; the command is the first thing that
                // is not one.
                _ if first => {
                    args.command = item;
                    first = false;
                }
                _ => rest.push(item),
            }
        }
        args.positional = rest;
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &[&str]) -> Args {
        Args::parse(raw.iter().map(|s| s.to_string()))
    }

    #[test]
    fn the_default_command_is_the_picker() {
        assert_eq!(args(&[]).command, "ui");
    }

    #[test]
    fn flags_are_taken_out_of_the_positionals() {
        let parsed = args(&["open", "editor", "--pane", "w1:p3", "--git-root"]);
        assert_eq!(parsed.command, "open");
        assert_eq!(parsed.positional, vec!["editor"]);
        assert_eq!(parsed.pane.as_deref(), Some("w1:p3"));
        assert!(parsed.git_root);
    }

    #[test]
    fn a_flag_before_the_command_does_not_eat_it() {
        let parsed = args(&["--git-root", "where"]);
        assert_eq!(parsed.command, "where");
        assert!(parsed.git_root);
    }
}
