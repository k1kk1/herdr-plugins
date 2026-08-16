//! Plugin settings loading.
//!
//! Each plugin owns its own settings struct; only the mechanics are shared.
//! Settings live in `config.toml` inside the directory Herdr reserves per
//! plugin (`herdr plugin config-dir <id>`, exported as
//! `HERDR_PLUGIN_CONFIG_DIR`). A missing or malformed file falls back to
//! defaults rather than blocking an operation.

use std::path::PathBuf;

use serde::de::DeserializeOwned;

/// Directory Herdr reserves for a plugin's configuration.
pub fn config_dir(plugin_id: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config/herdr/plugins/config")
            .join(plugin_id),
    )
}

pub fn config_path(plugin_id: &str) -> Option<PathBuf> {
    config_dir(plugin_id).map(|dir| dir.join("config.toml"))
}

/// Directory Herdr reserves for a plugin's mutable state, as opposed to its
/// user-edited configuration. Created on demand.
pub fn state_dir(plugin_id: &str) -> Option<PathBuf> {
    let dir = if let Some(dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR") {
        if dir.is_empty() {
            return None;
        }
        PathBuf::from(dir)
    } else {
        let home = std::env::var_os("HOME")?;
        PathBuf::from(home)
            .join(".local/state/herdr/plugins")
            .join(plugin_id)
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Load settings for `plugin_id`, falling back to defaults.
///
/// The returned warning describes why a present file was ignored, so a plugin
/// can surface a typo instead of silently behaving unexpectedly.
pub fn load<T: DeserializeOwned + Default>(plugin_id: &str) -> (T, Option<String>) {
    let Some(path) = config_path(plugin_id) else {
        return (T::default(), None);
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        // Not having a config file is the normal case.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (T::default(), None),
        Err(err) => {
            return (
                T::default(),
                Some(format!("could not read {}: {err}", path.display())),
            )
        }
    };
    match parse(&raw, plugin_id) {
        Ok(config) => (config, None),
        Err(err) => (
            T::default(),
            Some(format!("ignoring {}: {err}", path.display())),
        ),
    }
}

/// Accept both the documented `[<plugin-id>]` table form and bare top-level
/// keys, so a single-plugin config file does not need a header.
fn parse<T: DeserializeOwned + Default>(raw: &str, plugin_id: &str) -> Result<T, toml::de::Error> {
    let document: toml::Table = toml::from_str(raw)?;
    if let Some(table) = document.get(plugin_id) {
        return table.clone().try_into();
    }
    if document.is_empty() {
        return Ok(T::default());
    }
    document.try_into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(default, deny_unknown_fields)]
    struct Sample {
        size: u8,
        name: String,
    }

    impl Default for Sample {
        fn default() -> Self {
            Self {
                size: 3,
                name: "default".into(),
            }
        }
    }

    #[test]
    fn reads_the_table_form() {
        let got: Sample = parse("[demo]\nsize = 9\n", "demo").unwrap();
        assert_eq!(got.size, 9);
        // Unspecified keys keep their defaults.
        assert_eq!(got.name, "default");
    }

    #[test]
    fn reads_the_bare_form() {
        let got: Sample = parse("size = 4\n", "demo").unwrap();
        assert_eq!(got.size, 4);
    }

    #[test]
    fn empty_file_is_defaults() {
        assert_eq!(parse::<Sample>("", "demo").unwrap(), Sample::default());
    }

    #[test]
    fn unknown_keys_are_rejected_so_typos_surface() {
        assert!(parse::<Sample>("[demo]\nsiez = 9\n", "demo").is_err());
    }

    #[test]
    fn another_plugins_table_does_not_leak_in() {
        // A shared file holding several plugins' settings must not feed
        // `[other]` into `demo`.
        let got: Sample = parse("[demo]\nsize = 9\n[other]\nsize = 1\n", "demo").unwrap();
        assert_eq!(got.size, 9);
    }
}
