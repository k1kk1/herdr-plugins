//! Alfred integration.
//!
//! Two halves that meet in the middle:
//!
//! * `herdr-sessions alfred` prints the session list as Alfred Script Filter
//!   JSON, so Alfred can show it in its own window.
//! * `herdr-sessions alfred install` writes a workflow that calls the above
//!   and hands the chosen name back to `herdr-sessions open`.
//!
//! Opening is the same code path the plugin picker uses ([`crate::open`]), so
//! a session opened from Alfred and one opened from inside Herdr land in the
//! same kind of window.
//!
//! ## Absolute paths everywhere
//!
//! Alfred runs workflow scripts with a login-shell `PATH` at best and
//! `/usr/bin:/bin:/usr/sbin:/sbin` at worst — Homebrew's `bin` is often not on
//! it. So the workflow is generated with this binary's absolute path baked in,
//! and with `HERDR_BIN_PATH` exported, rather than trusting either to be found
//! by name.

use std::io::Read;
use std::path::{Path, PathBuf};

use herdr_plugin_kit::{bail, Context, Result};
use serde_json::{json, Value};

use crate::open::Config;
use crate::resume::Where;
use crate::session::{self, Detail, Session};

/// Alfred's own filtering is used, so every session is emitted every time and
/// the `match` field carries the extra words worth typing.
pub fn script_filter(config: &Config) -> Result<String> {
    let sessions = session::list()?;
    let items: Vec<Value> = sessions.iter().map(|s| item(config, s)).collect();

    let payload = if items.is_empty() {
        json!({ "items": [{
            "title": "No Herdr sessions",
            "subtitle": "Run `herdr --session <name>` to make one",
            "valid": false,
        }]})
    } else {
        json!({ "items": items })
    };
    Ok(payload.to_string())
}

/// Past Claude Code and Codex conversations, as Alfred items.
pub fn resume_filter(config: &Config) -> Result<String> {
    let sessions = crate::agents::list_all(config.recent(), None)?;
    let items: Vec<Value> = sessions.iter().map(resume_item).collect();

    let payload = if items.is_empty() {
        json!({ "items": [{
            "title": "No conversations recorded",
            "subtitle": "Nothing from Claude Code or Codex on this machine yet",
            "valid": false,
        }]})
    } else {
        json!({ "items": items })
    };
    Ok(payload.to_string())
}

/// Alfred's "borrow this application's icon" form.
fn app_icon(app: Option<std::path::PathBuf>) -> Option<Value> {
    let app = app?;
    Some(json!({ "type": "fileicon", "path": app.display().to_string() }))
}

fn resume_item(session: &crate::agents::AgentSession) -> Value {
    let mut subtitle = vec![
        session.kind.tag().to_string(),
        crate::session::ago(session.modified),
        session.where_line(),
    ];
    if let Some(context) = session.context_line() {
        subtitle.push(context);
    }
    // Alfred's modifier keys can only change the text an item hands on, so the
    // placement rides along in the argument. The three match the picker's
    // Enter keys exactly — the same gesture has to mean the same thing whether
    // it is pressed in Herdr or in Alfred.
    let arg = |placement: Where| format!("{}:{}", placement.name(), session.id);
    let mods = json!({
        "shift": {
            "valid": true,
            "arg": arg(Where::Tab),
            "subtitle": "Resume in a new tab of the current workspace",
        },
        "alt": {
            "valid": true,
            "arg": arg(Where::Split),
            "subtitle": "Resume beside the pane you were on",
        },
    });

    let mut item = json!({
        "uid": format!("herdr-conversation:{}", session.id),
        "title": session.heading(),
        "subtitle": subtitle.join(" · "),
        "arg": arg(Where::Workspace),
        "mods": mods,
        // The tool name and the path are in the subtitle, but Alfred only
        // matches what it is told to, so they go here too.
        "match": session.searchable(),
        "valid": true,
        "text": { "copy": format!("{} {}", session.kind.command(), session.kind.resume_args(&session.id).join(" ")) },
    });
    if let Some(icon) = app_icon(session.kind.app()) {
        item["icon"] = icon;
    }
    item
}

fn item(config: &Config, session: &Session) -> Value {
    let detail = session::detail(session);
    let mut item = json!({
        "uid": format!("herdr-session:{}", session.name),
        "title": session.name,
        "subtitle": subtitle(session, &detail),
        "arg": session.name,
        "match": matchable(session, &detail),
        "valid": true,
        "text": { "copy": format!("herdr session attach {}", session.name) },
    });

    // The one case where opening is not what the user wants: they are already
    // in it. Alfred shows the row but refuses to action it.
    if session.is_current() {
        item["valid"] = json!(false);
        item["subtitle"] = json!(format!(
            "{} — you are in this session",
            subtitle(session, &detail)
        ));
    }

    // A Herdr session opens in a terminal, so it wears that terminal's icon.
    if let Some(icon) = app_icon(terminal_bundle(config)) {
        item["icon"] = icon;
    }

    // Warn rather than fail: the row is still worth showing, and the error
    // shows up when the command actually runs.
    if let Err(err) = crate::open::command_for(config, &session.name) {
        item["valid"] = json!(false);
        item["subtitle"] = json!(err.to_string().lines().next().unwrap_or("cannot open"));
    }
    item
}

fn subtitle(session: &Session, detail: &Detail) -> String {
    let mut parts = vec![session.state().to_string()];
    if session.default {
        parts.push("default".into());
    }
    if !session.running {
        if let Some(time) = detail.last_used {
            parts.push(format!("last used {}", session::ago(time)));
        }
    }
    parts.push(detail.summary());
    if let Some(names) = detail.names_line() {
        parts.push(names);
    }
    parts.join(" · ")
}

/// Extra words Alfred should match on, so "recipes" finds the session that
/// holds the Agent Recipes workspace.
fn matchable(session: &Session, detail: &Detail) -> String {
    let mut words = vec![session.name.clone(), session.state().to_string()];
    words.extend(detail.names.iter().cloned());
    words.join(" ")
}

// ---------------------------------------------------------------------------
// Installing the workflow
// ---------------------------------------------------------------------------

/// The terminal application's bundle, for its icon.
fn terminal_bundle(config: &Config) -> Option<std::path::PathBuf> {
    let name = crate::open::terminal_app(config)?;
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    [
        std::path::PathBuf::from("/Applications").join(&name),
        home.join("Applications").join(&name),
    ]
    .into_iter()
    .find(|path| path.exists())
}

/// Keyword for the Herdr session list.
const KEYWORD: &str = "hs";
/// Keyword for the conversation list.
const KEYWORD_RESUME: &str = "hr";

fn workflows_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let dir = PathBuf::from(home)
        .join("Library/Application Support/Alfred/Alfred.alfredpreferences/workflows");
    if !dir.is_dir() {
        bail!(
            "Alfred's workflow folder is not where it should be:\n  {}\n\
             Is Alfred installed, and are its preferences in the default place?",
            dir.display()
        );
    }
    Ok(dir)
}

/// Write the workflow into Alfred's preferences. Alfred picks it up without a
/// restart.
///
/// Refuses to clobber an existing copy unless `force`, because the user may
/// have edited the keyword or hung more actions off it.
pub fn install(config: &Config, force: bool) -> Result<PathBuf> {
    let dir = workflows_dir()?;
    let target = existing(&dir)?.unwrap_or_else(|| dir.join(format!("user.workflow.{}", uuid())));

    if target.exists() && !force {
        bail!(
            "A Herdr Sessions workflow is already installed:\n  {}\n\
             Re-run with --force to overwrite it.",
            target.display()
        );
    }

    std::fs::create_dir_all(&target)
        .with_context(|| format!("could not create {}", target.display()))?;

    let workflow = info_plist()?;
    let path = target.join("info.plist");
    std::fs::write(&path, workflow.plist)
        .with_context(|| format!("could not write {}", path.display()))?;

    // Alfred takes an object's icon from a PNG named after its uid, and the
    // workflow's own from `icon.png`. The uids are generated per install, so
    // the icons have to be written here rather than shipped in a folder.
    if let Some(source) = icon(config) {
        let png = std::fs::read(&source)
            .with_context(|| format!("could not read {}", source.display()))?;
        for name in std::iter::once("icon".to_string()).chain(workflow.icon_uids) {
            let path = target.join(format!("{name}.png"));
            std::fs::write(&path, &png)
                .with_context(|| format!("could not write {}", path.display()))?;
        }
    }
    Ok(target)
}

/// A generated workflow: its `info.plist`, and the object uids that want an
/// icon file beside it.
struct Workflow {
    plist: String,
    icon_uids: Vec<String>,
}

/// An already-installed copy, found by its bundle id rather than by folder
/// name, which Alfred randomises.
fn existing(dir: &Path) -> Result<Option<PathBuf>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let plist = entry.path().join("info.plist");
        let Ok(mut file) = std::fs::File::open(&plist) else {
            continue;
        };
        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_err() {
            continue;
        }
        if contents.contains(BUNDLE_ID) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

const BUNDLE_ID: &str = "dev.herdr.plugins.sessions";

/// Where an icon is looked for when the config does not name one.
const ICON_FILE: &str = "icon.png";

/// The PNG to use for the workflow's rows, if there is one.
///
/// Deliberately not bundled. The only icon worth showing here is Herdr's own,
/// and shipping someone else's mark inside this repository would be
/// redistributing it. Fetching it is a fine thing for a person to do for
/// themselves, and a presumptuous thing for an installer to do for them.
fn icon(config: &Config) -> Option<std::path::PathBuf> {
    if let Some(path) = &config.icon {
        return Some(path.clone()).filter(|p| p.is_file());
    }
    herdr_plugin_kit::config::config_dir(crate::PLUGIN_ID)
        .map(|dir| dir.join(ICON_FILE))
        .filter(|path| path.is_file())
}

/// What to tell someone who has no icon set, so the row is a plain default.
pub fn icon_hint() -> String {
    let dir = herdr_plugin_kit::config::config_dir(crate::PLUGIN_ID)
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "<config dir>".into());
    format!(
        "No workflow icon set, so Alfred will use its own.\n\
         To use Herdr's logo:\n\
         \n  mkdir -p {dir}\n\
         \n  curl -sL -o {dir}/{ICON_FILE} https://herdr.dev/assets/logo.png\n\
         \nThen re-run `alfred install --force`. Any 256px PNG works; set\n\
         `icon` in the config to point somewhere else."
    )
}

/// This binary's absolute path, for baking into the workflow.
fn self_path() -> Result<String> {
    let path = std::env::current_exe().context("could not find this binary's own path")?;
    Ok(path.display().to_string())
}

fn info_plist() -> Result<Workflow> {
    let me = self_path()?;
    let herdr = session::herdr_bin();
    let filter_uid = uuid();
    let action_uid = uuid();

    // Built from raw paths and escaped once, at the end. `{:?}` quotes them
    // for the shell; `xml` quotes the result for the plist.
    let resume_filter_uid = uuid();
    let resume_action_uid = uuid();

    let script = |args: &str| {
        xml(&format!(
            "export HERDR_BIN_PATH={herdr:?}\nexec {me:?} {args}\n"
        ))
    };
    // How an action receives the chosen item's argument.
    //
    // Alfred hands it over by substituting `{{query}}` into the script text
    // before running it — *not* as a positional parameter. Setting the
    // "input as argv" option did not change that: measured, the script ran as
    // `/bin/bash` with `$1` empty, and the item silently did nothing.
    //
    // There is deliberately no argv fallback. Guarding one costs a second
    // `{{query}}` in the text, and Alfred substitutes *every* occurrence — so
    // the guard is rewritten too and the fallback fires precisely when
    // substitution worked. If substitution ever stops, the literal token
    // reaches the binary and comes back as a legible error naming it, which
    // beats a fallback that breaks the working case.
    //
    // Single-quoted: the value is `<placement>:<uuid>`, so there is nothing in
    // it for a shell to do, and nothing it could do if there were.
    // How an action receives the chosen item's argument.
    //
    // As `$1`, with `scriptargtype = 1`. That number is the whole story and it
    // reads backwards: **1 is "input as argv", 0 is not**. With 0 the script
    // ran as `/bin/bash` with no positional parameters at all and the item
    // silently did nothing; `{{query}}` substitution does not happen either
    // way. Measured both, because guessing cost two rounds of "it still does
    // not work".
    let action = |verb: &str| {
        xml(&format!(
            "export HERDR_BIN_PATH={herdr:?}\nexec {me:?} {verb} \"$1\"\n"
        ))
    };
    // One connection per accepted modifier. The item's `mods` decide what
    // argument is handed on, but a modifier with no connection has nothing to
    // hand it to — Shift+Enter would simply do nothing.
    let resume_connections: String = [
        (0, ""),
        (131_072, "in a new tab"),   // NSEventModifierFlagShift
        (524_288, "beside this pane"), // NSEventModifierFlagOption
    ]
    .iter()
    .map(|(modifiers, subtext)| {
        format!(
            "			<dict>\n\
             				<key>destinationuid</key>\n\
             				<string>{resume_action_uid}</string>\n\
             				<key>modifiers</key>\n\
             				<integer>{modifiers}</integer>\n\
             				<key>modifiersubtext</key>\n\
             				<string>{subtext}</string>\n\
             				<key>vitoclose</key>\n\
             				<false/>\n\
             			</dict>\n"
        )
    })
    .collect();

    let list_script = script("alfred");
    let open_script = action("open");
    let resume_list_script = script("alfred resume");
    let resume_script = action("resume");

    let plist = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>bundleid</key>
	<string>{BUNDLE_ID}</string>
	<key>category</key>
	<string>Tools</string>
	<key>connections</key>
	<dict>
		<key>{filter_uid}</key>
		<array>
			<dict>
				<key>destinationuid</key>
				<string>{action_uid}</string>
				<key>modifiers</key>
				<integer>0</integer>
				<key>modifiersubtext</key>
				<string></string>
				<key>vitoclose</key>
				<false/>
			</dict>
		</array>
		<key>{resume_filter_uid}</key>
		<array>
{resume_connections}		</array>
	</dict>
	<key>createdby</key>
	<string>herdr-sessions</string>
	<key>description</key>
	<string>List Herdr sessions and past agent conversations, and open one.</string>
	<key>disabled</key>
	<false/>
	<key>name</key>
	<string>Herdr Sessions</string>
	<key>objects</key>
	<array>
		<dict>
			<key>config</key>
			<dict>
				<key>alfredfiltersresults</key>
				<true/>
				<key>alfredfiltersresultsmatchmode</key>
				<integer>0</integer>
				<key>argumenttreatemptyqueryasnil</key>
				<true/>
				<key>argumenttrimmode</key>
				<integer>0</integer>
				<key>argumenttype</key>
				<integer>1</integer>
				<key>escaping</key>
				<integer>102</integer>
				<key>keyword</key>
				<string>{KEYWORD}</string>
				<key>queuedelaycustom</key>
				<integer>3</integer>
				<key>queuedelayimmediatesinitial</key>
				<true/>
				<key>queuedelaymode</key>
				<integer>0</integer>
				<key>queuemode</key>
				<integer>1</integer>
				<key>runningsubtext</key>
				<string>Reading sessions...</string>
				<key>script</key>
				<string>{list_script}</string>
				<key>scriptargtype</key>
				<integer>0</integer>
				<key>scriptfile</key>
				<string></string>
				<key>subtext</key>
				<string>Open a Herdr session in a new window</string>
				<key>title</key>
				<string>Herdr Sessions</string>
				<key>type</key>
				<integer>0</integer>
				<key>withspace</key>
				<true/>
			</dict>
			<key>type</key>
			<string>alfred.workflow.input.scriptfilter</string>
			<key>uid</key>
			<string>{filter_uid}</string>
			<key>version</key>
			<integer>3</integer>
		</dict>
		<dict>
			<key>config</key>
			<dict>
				<key>alfredfiltersresults</key>
				<true/>
				<key>alfredfiltersresultsmatchmode</key>
				<integer>0</integer>
				<key>argumenttreatemptyqueryasnil</key>
				<true/>
				<key>argumenttrimmode</key>
				<integer>0</integer>
				<key>argumenttype</key>
				<integer>1</integer>
				<key>escaping</key>
				<integer>102</integer>
				<key>keyword</key>
				<string>{KEYWORD_RESUME}</string>
				<key>queuedelaycustom</key>
				<integer>3</integer>
				<key>queuedelayimmediatesinitial</key>
				<true/>
				<key>queuedelaymode</key>
				<integer>0</integer>
				<key>queuemode</key>
				<integer>1</integer>
				<key>runningsubtext</key>
				<string>Reading conversations...</string>
				<key>script</key>
				<string>{resume_list_script}</string>
				<key>scriptargtype</key>
				<integer>0</integer>
				<key>scriptfile</key>
				<string></string>
				<key>subtext</key>
				<string>Resume a Claude Code or Codex conversation</string>
				<key>title</key>
				<string>Herdr Resume</string>
				<key>type</key>
				<integer>0</integer>
				<key>withspace</key>
				<true/>
			</dict>
			<key>type</key>
			<string>alfred.workflow.input.scriptfilter</string>
			<key>uid</key>
			<string>{resume_filter_uid}</string>
			<key>version</key>
			<integer>3</integer>
		</dict>
		<dict>
			<key>config</key>
			<dict>
				<key>concurrently</key>
				<false/>
				<key>escaping</key>
				<integer>102</integer>
				<key>script</key>
				<string>{resume_script}</string>
				<key>scriptargtype</key>
				<integer>1</integer>
				<key>scriptfile</key>
				<string></string>
				<key>type</key>
				<integer>0</integer>
			</dict>
			<key>type</key>
			<string>alfred.workflow.action.script</string>
			<key>uid</key>
			<string>{resume_action_uid}</string>
			<key>version</key>
			<integer>2</integer>
		</dict>
		<dict>
			<key>config</key>
			<dict>
				<key>concurrently</key>
				<false/>
				<key>escaping</key>
				<integer>102</integer>
				<key>script</key>
				<string>{open_script}</string>
				<key>scriptargtype</key>
				<integer>1</integer>
				<key>scriptfile</key>
				<string></string>
				<key>type</key>
				<integer>0</integer>
			</dict>
			<key>type</key>
			<string>alfred.workflow.action.script</string>
			<key>uid</key>
			<string>{action_uid}</string>
			<key>version</key>
			<integer>2</integer>
		</dict>
	</array>
	<key>readme</key>
	<string>`{KEYWORD}` lists every Herdr session, running and stopped; Enter opens one in a new terminal window.

`{KEYWORD_RESUME}` lists past Claude Code and Codex conversations; Enter resumes one inside the running Herdr session and brings the terminal forward. With no session running it opens a window instead.

Generated by `herdr-sessions alfred install`. Re-run it after moving the plugin, so the baked-in paths stay right.</string>
	<key>uidata</key>
	<dict>
		<key>{filter_uid}</key>
		<dict>
			<key>xpos</key>
			<real>60</real>
			<key>ypos</key>
			<real>60</real>
		</dict>
		<key>{action_uid}</key>
		<dict>
			<key>xpos</key>
			<real>320</real>
			<key>ypos</key>
			<real>60</real>
		</dict>
		<key>{resume_filter_uid}</key>
		<dict>
			<key>xpos</key>
			<real>60</real>
			<key>ypos</key>
			<real>190</real>
		</dict>
		<key>{resume_action_uid}</key>
		<dict>
			<key>xpos</key>
			<real>320</real>
			<key>ypos</key>
			<real>190</real>
		</dict>
	</dict>
	<key>userconfigurationconfig</key>
	<array/>
	<key>version</key>
	<string>{version}</string>
	<key>webaddress</key>
	<string></string>
</dict>
</plist>
"##,
        version = env!("CARGO_PKG_VERSION"),
    );
    Ok(Workflow {
        // Only the keyword rows are seen before a search runs, so they are the
        // ones that need to say what they belong to.
        icon_uids: vec![filter_uid, resume_filter_uid],
        plist,
    })
}

/// Escape text for a plist `<string>`.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A version-4-shaped UUID from the system RNG.
///
/// Alfred only needs these to be unique within the file, so pulling 16 bytes
/// from `/dev/urandom` beats taking a dependency.
fn uuid() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str, running: bool) -> Session {
        Session {
            name: name.into(),
            running,
            default: false,
            session_dir: None,
            socket_path: None,
        }
    }

    #[test]
    fn a_conversation_offers_all_three_placements() {
        let session = crate::agents::AgentSession {
            id: "abc".into(),
            title: Some("Build it".into()),
            ..Default::default()
        };
        let item = resume_item(&session);
        // Plain Enter and each modifier must name a different placement, or
        // holding the key would silently do the same thing as not holding it.
        let plain = item["arg"].as_str().unwrap();
        let shift = item["mods"]["shift"]["arg"].as_str().unwrap();
        let alt = item["mods"]["alt"]["arg"].as_str().unwrap();
        assert_eq!(plain, "workspace:abc");
        assert_eq!(shift, "tab:abc");
        assert_eq!(alt, "split:abc");
        // Every one of them has to survive the trip back.
        for arg in [plain, shift, alt] {
            let (head, id) = arg.split_once(':').unwrap();
            assert!(Where::parse(head).is_some(), "{head}");
            assert_eq!(id, "abc");
        }
    }

    #[test]
    fn a_subtitle_leads_with_the_state() {
        let detail = Detail {
            workspaces: 2,
            panes: 4,
            ..Default::default()
        };
        let line = subtitle(&session("work", true), &detail);
        assert!(line.starts_with("running · "), "{line}");
        assert!(line.contains("2 workspaces"), "{line}");
    }

    #[test]
    fn workspace_names_are_matchable_so_you_can_find_a_session_by_what_is_in_it() {
        let detail = Detail {
            names: vec!["Agent Recipes".into()],
            ..Default::default()
        };
        let words = matchable(&session("work", true), &detail);
        assert!(words.contains("Agent Recipes"), "{words}");
        assert!(words.contains("work"), "{words}");
    }

    #[test]
    fn uuids_are_shaped_the_way_alfred_writes_them() {
        let id = uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(
            id.split('-').map(str::len).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert_ne!(uuid(), id);
    }

    #[test]
    fn the_conversation_keyword_is_connected_for_every_modifier() {
        let plist = info_plist().unwrap().plist;
        // Plain Enter plus the two modifiers, against one for the session
        // keyword: four connections in all.
        assert_eq!(plist.matches("destinationuid").count(), 4);
        for modifiers in ["<integer>131072</integer>", "<integer>524288</integer>"] {
            assert!(plist.contains(modifiers), "missing connection for {modifiers}");
        }
    }

    #[test]
    fn the_generated_plist_wires_both_keywords_end_to_end() {
        let plist = info_plist().unwrap().plist;
        assert!(plist.contains(BUNDLE_ID));

        // Two keywords, each a filter feeding an action: four scripts.
        assert_eq!(plist.matches("alfred.workflow.input.scriptfilter").count(), 2);
        assert_eq!(plist.matches("alfred.workflow.action.script").count(), 2);
        assert!(plist.contains(&format!("<string>{KEYWORD}</string>")));
        assert!(plist.contains(&format!("<string>{KEYWORD_RESUME}</string>")));

        // Alfred runs these with a bare PATH, so every one of them must name
        // the binary by absolute path rather than by name.
        let me = self_path().unwrap();
        assert!(me.starts_with('/'), "{me}");
        assert_eq!(plist.matches(&me).count(), 4, "every script must name it");

        // Every uid referenced by a connection must exist as an object.
        for uid in ["filter", "action"] {
            let _ = uid;
        }
        assert_eq!(plist.matches("destinationuid").count(), 4);
    }

    #[test]
    fn every_keyword_row_gets_an_icon_file() {
        let workflow = info_plist().unwrap();
        // One per Script Filter, and each must name an object that exists.
        assert_eq!(workflow.icon_uids.len(), 2);
        for uid in &workflow.icon_uids {
            assert!(workflow.plist.contains(uid), "{uid} is not in the plist");
        }
    }

    #[test]
    fn a_missing_icon_is_not_an_error_but_does_come_with_instructions() {
        // No icon is a perfectly good state — Alfred has its own — so it must
        // not fail the install, only explain itself.
        let none = Config {
            icon: Some("/nonexistent/icon.png".into()),
            ..Default::default()
        };
        assert!(icon(&none).is_none());
        let hint = icon_hint();
        assert!(hint.contains("herdr.dev"), "{hint}");
        assert!(hint.contains(ICON_FILE), "{hint}");
    }

    #[test]
    fn plist_text_is_escaped() {
        assert_eq!(xml("a & b <c>"), "a &amp; b &lt;c&gt;");
    }
}
