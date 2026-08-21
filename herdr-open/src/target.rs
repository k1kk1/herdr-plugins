//! What "open this directory" can mean, and how each option is run.
//!
//! A target is a command line with `{dir}` somewhere in it. Keeping them as
//! data rather than as code is what lets the same plugin serve someone whose
//! editor is `code`, someone whose editor is `nvim` in a new pane, and someone
//! who mostly wants the path on the clipboard — without any of them patching
//! Rust.

use std::path::Path;
use std::process::{Command, Stdio};

use herdr_plugin_kit::{bail, Context, Outcome, Result};
use serde::Deserialize;

/// One entry in the picker: a title and the command line behind it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Stable name used by `herdr-open open <id>` and by the manifest actions.
    pub id: String,
    pub title: String,
    /// argv, with `{dir}`, `{name}` and `{parent}` filled in per run.
    pub command: Vec<String>,
    /// Single character that picks this row in the menu.
    #[serde(default)]
    pub hotkey: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Program whose presence decides whether the target is offered.
    ///
    /// Defaults to the first word of `command`, which is only wrong when the
    /// command goes through `sh -c` and the interesting program is inside the
    /// script.
    #[serde(default)]
    pub requires: Option<String>,
}

impl Target {
    fn required_program(&self) -> Option<&str> {
        self.requires
            .as_deref()
            .or_else(|| self.command.first().map(String::as_str))
            .filter(|program| !program.trim().is_empty())
    }

    /// Whether the program this target needs exists on this machine.
    ///
    /// Unavailable targets are hidden from the picker rather than shown and
    /// failing: a detached GUI launch reports nothing back, so a row that
    /// silently does nothing is the worst outcome available.
    pub fn is_available(&self) -> bool {
        match self.required_program() {
            Some(program) => on_path(program),
            None => false,
        }
    }

    /// The command line for `dir`, placeholders filled in.
    pub fn argv(&self, dir: &Path) -> Vec<String> {
        let dir = dir.display().to_string();
        let name = Path::new(&dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.clone());
        let parent = Path::new(&dir)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| dir.clone());

        self.command
            .iter()
            .map(|part| {
                part.replace("{dir}", &dir)
                    .replace("{name}", &name)
                    .replace("{parent}", &parent)
            })
            .collect()
    }

    /// Run the target on `dir`, detached.
    ///
    /// Detached because the caller is either a popup that is about to close or
    /// a headless action process that exits immediately; a child tied to
    /// either would be killed the moment the user got what they asked for.
    pub fn run(&self, dir: &Path) -> Result<Outcome> {
        let argv = self.argv(dir);
        let (program, args) = argv
            .split_first()
            .with_context(|| format!("target `{}` has an empty command", self.id))?;

        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        disinherit(&mut command);
        command
            .spawn()
            .with_context(|| format!("could not run `{program}`"))?;

        Ok(Outcome::new(self.title.clone()).with_detail(crate::dir::tilde(dir)))
    }
}

/// Strip every `HERDR_*` variable from the child's environment.
///
/// On macOS `open` hands the caller's environment to the application it
/// launches, so an editor started from a Herdr pane would come up carrying
/// `HERDR_ENV=1` and a `HERDR_PANE_ID` naming a pane it has nothing to do
/// with. Its integrated terminal would then believe it is inside Herdr — and
/// `herdr` refuses to nest. The Sessions plugin learned this the hard way;
/// the same rule applies to anything else we launch outward.
fn disinherit(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
}

/// Whether `program` is an executable on `PATH`.
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Settings for [`crate::PLUGIN_ID`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Start the picker on the repository root rather than the pane's own
    /// directory, when the pane is inside a repository.
    pub prefer_git_root: bool,
    /// Replaces the built-in list entirely when present, so a target can be
    /// removed and not just added to.
    pub target: Vec<Target>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefer_git_root: false,
            target: builtin(),
        }
    }
}

impl Config {
    pub fn load() -> (Self, Option<String>) {
        herdr_plugin_kit::config::load(crate::PLUGIN_ID)
    }

    /// Targets that can actually run here, in configured order.
    pub fn available(&self) -> Vec<Target> {
        self.target
            .iter()
            .filter(|target| target.is_available())
            .cloned()
            .collect()
    }

    /// Look a target up by id, whether or not it is available — an id named on
    /// the command line deserves a better answer than "not in the list".
    pub fn find(&self, id: &str) -> Result<&Target> {
        let Some(target) = self.target.iter().find(|target| target.id == id) else {
            let known: Vec<&str> = self.target.iter().map(|t| t.id.as_str()).collect();
            bail!("no target called `{id}`. Configured: {}", known.join(", "));
        };
        if !target.is_available() {
            bail!(
                "`{}` needs `{}`, which is not on PATH",
                target.id,
                target.required_program().unwrap_or("?")
            );
        }
        Ok(target)
    }
}

fn target(id: &str, title: &str, hotkey: &str, command: &[&str]) -> Target {
    Target {
        id: id.into(),
        title: title.into(),
        command: command.iter().map(|part| (*part).to_string()).collect(),
        hotkey: Some(hotkey.into()),
        description: None,
        requires: None,
    }
}

/// The list someone gets before they have configured anything.
///
/// Deliberately short. Three things cover what a terminal actually lacks —
/// the file manager, the editor, and the path as text — and every extra row
/// is one more thing to read before pressing `f`.
pub fn builtin() -> Vec<Target> {
    let file_manager = if cfg!(target_os = "macos") {
        target("finder", "Reveal in Finder", "f", &["open", "{dir}"])
    } else {
        target(
            "finder",
            "Open in File Manager",
            "f",
            &["xdg-open", "{dir}"],
        )
    };

    let mut targets = vec![
        file_manager,
        target("editor", "Open in VS Code", "e", &["code", "{dir}"]),
    ];
    if let Some(copy) = copy_target() {
        targets.push(copy);
    }
    targets
}

/// Put the path on the system clipboard.
///
/// The pipeline goes through `sh -c` with the path passed as `$1` rather than
/// interpolated into the script, so a directory containing a quote or a space
/// cannot turn into shell syntax.
fn copy_target() -> Option<Target> {
    let clipboard = if cfg!(target_os = "macos") {
        "pbcopy"
    } else {
        ["wl-copy", "xclip", "xsel"]
            .into_iter()
            .find(|program| on_path(program))?
    };
    let script = match clipboard {
        "xclip" => "printf %s \"$1\" | xclip -selection clipboard".to_string(),
        "xsel" => "printf %s \"$1\" | xsel --clipboard --input".to_string(),
        other => format!("printf %s \"$1\" | {other}"),
    };
    Some(Target {
        id: "copy-path".into(),
        title: "Copy Path".into(),
        command: vec!["sh".into(), "-c".into(), script, "sh".into(), "{dir}".into()],
        hotkey: Some("c".into()),
        description: None,
        requires: Some(clipboard.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Target {
        target("editor", "Open in VS Code", "e", &["code", "{dir}"])
    }

    #[test]
    fn placeholders_are_filled_in() {
        let mut t = sample();
        t.command = ["x", "{dir}", "{name}", "{parent}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            t.argv(Path::new("/Users/x/src/herdr-plugins")),
            vec!["x", "/Users/x/src/herdr-plugins", "herdr-plugins", "/Users/x/src"]
        );
    }

    #[test]
    fn a_path_is_never_reparsed_as_shell_syntax() {
        // The clipboard target is the one place a shell is involved. The path
        // travels as an argument, so quoting it wrong is not possible.
        let Some(copy) = copy_target() else { return };
        let argv = copy.argv(Path::new("/tmp/a b'c"));
        assert_eq!(argv.last().unwrap(), "/tmp/a b'c");
        assert!(argv[2].contains("\"$1\""));
    }

    #[test]
    fn requires_overrides_the_first_word() {
        let mut t = sample();
        t.command = vec!["sh".into(), "-c".into(), "true".into()];
        assert_eq!(t.required_program(), Some("sh"));
        t.requires = Some("pbcopy".into());
        assert_eq!(t.required_program(), Some("pbcopy"));
    }

    #[test]
    fn an_unknown_id_lists_what_is_configured() {
        let config = Config::default();
        let err = config.find("emacs").unwrap_err().to_string();
        assert!(err.contains("finder"), "{err}");
    }

    #[test]
    fn built_in_ids_are_unique_and_stable() {
        let targets = builtin();
        let ids: Vec<&str> = targets.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"finder"));
        assert!(ids.contains(&"editor"));
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }
}
