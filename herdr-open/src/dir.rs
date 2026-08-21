//! Which directory a pane means.
//!
//! Herdr reports two: `cwd` is the shell's, `foreground_cwd` is the running
//! program's. They differ exactly when something long-lived is in charge of
//! the pane — an agent started elsewhere, a `cd`-ing script — and in that case
//! the foreground process is the better answer to "where am I", because it is
//! what the pane is currently showing.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use herdr_plugin_kit::herdr::Pane;
use herdr_plugin_kit::{bail, Result};

/// The directory a pane is working in.
pub fn pane_dir(pane: &Pane) -> Result<PathBuf> {
    let raw = pane
        .foreground_cwd
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(pane.cwd.as_deref())
        .filter(|s| !s.trim().is_empty());
    let Some(raw) = raw else {
        bail!("Herdr does not report a working directory for this pane");
    };
    Ok(PathBuf::from(raw))
}

/// The root of the git repository `dir` is inside, when it differs from `dir`.
///
/// Returning `None` for "the repository root is the directory itself" is
/// deliberate: the picker only offers the git root as a *second* choice, and
/// offering the same path twice would just be noise.
pub fn git_root(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    (root != dir && !root.as_os_str().is_empty()).then_some(root)
}

/// `/Users/x/src/herdr` shown the way a person writes it.
pub fn tilde(path: &Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(cwd: Option<&str>, foreground: Option<&str>) -> Pane {
        Pane {
            cwd: cwd.map(str::to_string),
            foreground_cwd: foreground.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn the_foreground_directory_wins() {
        // An agent that was started in one place and moved to another is the
        // case this exists for.
        let dir = pane_dir(&pane(Some("/a"), Some("/a/b"))).unwrap();
        assert_eq!(dir, PathBuf::from("/a/b"));
    }

    #[test]
    fn the_shell_directory_is_the_fallback() {
        assert_eq!(pane_dir(&pane(Some("/a"), None)).unwrap(), PathBuf::from("/a"));
        // Herdr sends "" rather than omitting the field in some states.
        assert_eq!(
            pane_dir(&pane(Some("/a"), Some("  "))).unwrap(),
            PathBuf::from("/a")
        );
    }

    #[test]
    fn a_pane_without_a_directory_is_an_error_not_a_guess() {
        assert!(pane_dir(&pane(None, None)).is_err());
    }

    #[test]
    fn tilde_only_shortens_the_home_prefix() {
        std::env::set_var("HOME", "/Users/x");
        assert_eq!(tilde(Path::new("/Users/x/src")), "~/src");
        assert_eq!(tilde(Path::new("/opt/homebrew")), "/opt/homebrew");
    }
}
