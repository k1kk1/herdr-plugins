//! Past Claude Code and Codex sessions.
//!
//! A different kind of session from the Herdr ones, and an easier one:
//! `claude --resume <id>` and `codex resume <id>` are ordinary programs, so
//! unlike `herdr session attach` they can run **inside** the session you are
//! already in. Resuming means making a pane and starting an agent in it, not
//! opening a window.
//!
//! ## Reading the transcripts
//!
//! Neither tool offers a machine-readable listing — `claude --resume` and
//! `codex resume` both open their own interactive picker — so the transcripts
//! on disk are the only source. Those are private formats, so everything here
//! reads defensively: unknown fields are ignored, a file that cannot be parsed
//! becomes a thin row rather than an error, and nothing is required except the
//! id, which is in the filename.
//!
//! ## Reading as little of them as possible
//!
//! Transcripts get big — the one this was written in is 13 MB. Two things keep
//! the listing cheap:
//!
//! 1. **Sort by modification time first, then read.** The limit caps the I/O,
//!    not just the output, so raising it from 10 to 50 costs five times as
//!    much rather than the whole history.
//! 2. **Read from the end.** Claude appends `ai-title` and `last-prompt`
//!    records as the conversation goes, so the last 64 KB holds the newest of
//!    each. Measured on that 13 MB file: title, last prompt, cwd and branch
//!    all came back from the tail alone.
//!
//! Codex writes its metadata in the first line instead, so that one is read
//! from the front and stops early.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use herdr_plugin_kit::herdr::Herdr;
use herdr_plugin_kit::{bail, Result};
use serde_json::Value;

/// How much of the end of a Claude transcript to read.
///
/// Comfortably more than one record; the 13 MB transcript this was tuned on
/// yielded every field from its last 64 KB.
const TAIL: u64 = 64 * 1024;

/// How many lines into a Codex rollout to look for the opening prompt before
/// giving up. The metadata is line 1 and the prompt is usually just behind it,
/// but the first line also carries the whole system prompt, so this is a
/// line budget rather than a byte one.
const CODEX_SCAN_LINES: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Claude,
    Codex,
}

/// Every tool whose history this can read, in the order they are listed.
pub const KINDS: [Kind; 2] = [Kind::Claude, Kind::Codex];

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
        }
    }

    /// The tool's name as it appears in a row's right-hand column.
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Claude => "(Claude)",
            Kind::Codex => "(Codex)",
        }
    }

    /// The agent name Herdr knows this tool by, matching `herdr integration`.
    pub fn agent(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }

    /// The arguments that resume `id`.
    pub fn resume_args(self, id: &str) -> Vec<String> {
        match self {
            Kind::Claude => vec!["--resume".into(), id.into()],
            Kind::Codex => vec!["resume".into(), id.into()],
        }
    }

    fn root(self) -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(match self {
            Kind::Claude => home.join(".claude/projects"),
            Kind::Codex => home.join(".codex/sessions"),
        })
    }

    /// Whether a `.jsonl` found under [`Kind::root`] is a resumable session.
    ///
    /// Both trees hold transcripts that are not conversations you can reopen,
    /// and listing one produces a row that fails when it is chosen:
    ///
    /// * Claude keeps **subagent** transcripts at
    ///   `projects/<slug>/<id>/subagents/agent-*.jsonl`. A real session sits
    ///   directly in its project directory, one level down, so depth is the
    ///   test — it does not rely on the word "subagents", which is not ours.
    /// * Codex names its rollouts `rollout-<timestamp>-<uuid>.jsonl` and keeps
    ///   other bookkeeping in the same tree.
    ///
    /// This mattered the moment the listing stopped being capped at ten: with
    /// a limit, the noise simply never reached the top of the list.
    fn accepts(self, root: &Path, path: &Path) -> bool {
        match self {
            Kind::Claude => path.parent().and_then(Path::parent) == Some(root),
            Kind::Codex => path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("rollout-")),
        }
    }
}

/// One past conversation.
#[derive(Debug, Clone)]
pub struct AgentSession {
    pub kind: Kind,
    pub id: String,
    /// Claude's own generated heading. Codex does not write one.
    pub title: Option<String>,
    /// The last thing the user said, as a fallback heading and as context.
    pub last_prompt: Option<String>,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub modified: SystemTime,
    pub path: PathBuf,
    /// A pane in this Herdr session looks like it is this conversation.
    pub open: bool,
}

impl AgentSession {
    /// What to show as the row's name: the generated title, else the opening
    /// of the last prompt, else the id.
    pub fn heading(&self) -> String {
        if let Some(title) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return title.clone();
        }
        if let Some(prompt) = self.last_prompt.as_ref() {
            let line = first_line(prompt);
            if !line.is_empty() {
                return truncate(&line, 60);
            }
        }
        self.id.clone()
    }

    /// `~/src/herdr-plugins · main`, the second line's left half.
    pub fn where_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(cwd) = &self.cwd {
            parts.push(tilde(cwd));
        }
        if let Some(branch) = &self.branch {
            parts.push(branch.clone());
        }
        if parts.is_empty() {
            parts.push("unknown directory".into());
        }
        parts.join(" · ")
    }

    /// The last prompt, trimmed to sit on one line beside the location.
    pub fn context_line(&self) -> Option<String> {
        let prompt = self.last_prompt.as_ref()?;
        let line = first_line(prompt);
        // When there is no title the heading is already this text; repeating
        // it underneath is noise.
        if line.is_empty() || self.title.is_none() {
            return None;
        }
        Some(truncate(&line, 70))
    }

    /// Words the filter should match beyond the heading.
    pub fn searchable(&self) -> String {
        let mut words = vec![self.id.clone(), self.kind.agent().to_string()];
        if let Some(cwd) = &self.cwd {
            words.push(tilde(cwd));
        }
        if let Some(branch) = &self.branch {
            words.push(branch.clone());
        }
        if let Some(prompt) = &self.last_prompt {
            words.push(truncate(&first_line(prompt), 200));
        }
        words.join(" ")
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// The `limit` most recently touched conversations across every tool.
///
/// Each tool is capped at `limit` before the merge, because a limit is there
/// to bound the reading and one tool must not be able to starve the other of
/// its share by being noisier.
///
/// A tool with no history at all is not an error here — the point of the
/// combined view is that you do not have to care which tool it was.
pub fn list_all(limit: usize, herdr: Option<&Herdr>) -> Result<Vec<AgentSession>> {
    let mut all = Vec::new();
    let mut failures = Vec::new();
    for kind in KINDS {
        match list(kind, limit, herdr) {
            Ok(sessions) => all.extend(sessions),
            Err(err) => failures.push(format!("{}: {err}", kind.label())),
        }
    }
    // Only complain when nothing at all could be read; otherwise a machine
    // without Codex installed could never use the combined view.
    if all.is_empty() && !failures.is_empty() {
        bail!("{}", failures.join("\n"));
    }
    all.sort_by(|a, b| b.modified.cmp(&a.modified));
    all.truncate(limit);
    Ok(all)
}

/// The `limit` most recently touched conversations of one tool, newest first.
///
/// Metadata is read only for the ones that survive the limit.
pub fn list(kind: Kind, limit: usize, herdr: Option<&Herdr>) -> Result<Vec<AgentSession>> {
    let Some(root) = kind.root() else {
        bail!("HOME is not set, so there is nowhere to look for {} sessions.", kind.label());
    };
    if !root.is_dir() {
        bail!(
            "No {} history at {}.\nNothing has been recorded there yet.",
            kind.label(),
            tilde(&root)
        );
    }

    let mut files = transcripts(kind, &root);
    // Newest first, and only then read anything: the limit has to cap the
    // work, not merely the output.
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(limit);

    let open = herdr.map(open_conversations).unwrap_or_default();
    Ok(files
        .into_iter()
        .map(|(path, modified)| read(kind, path, modified, &open))
        .collect())
}

/// Every resumable transcript under `root`, with its modification time.
///
/// Claude nests one level (`projects/<slug>/<id>.jsonl`) and Codex nests three
/// (`sessions/YYYY/MM/DD/rollout-….jsonl`), so the walk itself is generic and
/// [`Kind::accepts`] decides what counts.
fn transcripts(kind: Kind, root: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "jsonl")
                && kind.accepts(root, &path)
            {
                if let Ok(modified) = meta.modified() {
                    found.push((path, modified));
                }
            }
        }
    }
    found
}

fn read(
    kind: Kind,
    path: PathBuf,
    modified: SystemTime,
    open: &HashMap<String, String>,
) -> AgentSession {
    let mut session = match kind {
        Kind::Claude => read_claude(&path),
        Kind::Codex => read_codex(&path),
    };
    session.kind = kind;
    session.modified = modified;
    // The id is in the filename for both tools, which is the one piece of
    // metadata that survives a transcript we cannot parse at all.
    if session.id.is_empty() {
        session.id = id_from_filename(&path);
    }
    session.open = is_open(&session, open);
    session.path = path;
    session
}

impl Default for AgentSession {
    fn default() -> Self {
        Self {
            kind: Kind::Claude,
            id: String::new(),
            title: None,
            last_prompt: None,
            cwd: None,
            branch: None,
            modified: SystemTime::UNIX_EPOCH,
            path: PathBuf::new(),
            open: false,
        }
    }
}

/// `rollout-2026-08-13T15-13-47-<uuid>.jsonl` and `<uuid>.jsonl` both end in
/// the id, so take the last dash-separated UUID-shaped run.
fn id_from_filename(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() >= 5 {
        let tail = &parts[parts.len() - 5..];
        if tail[0].len() == 8 && tail[4].len() == 12 {
            return tail.join("-");
        }
    }
    stem.to_string()
}

/// Claude's transcript, read from the end.
fn read_claude(path: &Path) -> AgentSession {
    let mut session = AgentSession::default();
    for record in tail_records(path) {
        let Some(kind) = record.get("type").and_then(Value::as_str) else {
            continue;
        };
        match kind {
            // Later records win: these are appended as the conversation goes,
            // so the last one in the file is the current answer.
            "ai-title" => session.title = string(&record, "aiTitle"),
            "last-prompt" => session.last_prompt = string(&record, "lastPrompt"),
            _ => {}
        }
        if let Some(id) = string(&record, "sessionId") {
            session.id = id;
        }
        if let Some(cwd) = string(&record, "cwd") {
            session.cwd = Some(PathBuf::from(cwd));
        }
        if let Some(branch) = string(&record, "gitBranch") {
            session.branch = Some(branch);
        }
    }
    session
}

/// The parsed records in the last [`TAIL`] bytes of a file.
///
/// The first line of that window is almost certainly cut in half, so it is
/// dropped. Anything else that fails to parse is skipped too — one truncated
/// record is not a reason to show nothing.
fn tail_records(path: &Path) -> Vec<Value> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let partial = size > TAIL;
    if partial && file.seek(SeekFrom::End(-(TAIL as i64))).is_err() {
        return Vec::new();
    }

    let mut buffer = Vec::new();
    if file.take(TAIL + 1).read_to_end(&mut buffer).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buffer);
    text.lines()
        .skip(usize::from(partial))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Codex's rollout, read from the front.
fn read_codex(path: &Path) -> AgentSession {
    use std::io::{BufRead, BufReader};

    let mut session = AgentSession::default();
    let Ok(file) = std::fs::File::open(path) else {
        return session;
    };

    for line in BufReader::new(file).lines().take(CODEX_SCAN_LINES) {
        let Ok(line) = line else { break };
        let Ok(record): std::result::Result<Value, _> = serde_json::from_str(&line) else {
            continue;
        };
        let payload = record.get("payload");
        match record.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(payload) = payload {
                    if let Some(id) = string(payload, "session_id") {
                        session.id = id;
                    }
                    if let Some(cwd) = string(payload, "cwd") {
                        session.cwd = Some(PathBuf::from(cwd));
                    }
                }
            }
            // Codex writes no title, so the opening prompt stands in for one.
            // It appears in one of two shapes depending on the version that
            // wrote the rollout, and both are still on disk here.
            Some("event_msg") => {
                let Some(payload) = payload else { continue };
                if payload.get("type").and_then(Value::as_str) == Some("user_message") {
                    session.last_prompt = string(payload, "message");
                    break;
                }
            }
            Some("response_item") => {
                let Some(payload) = payload else { continue };
                if payload.get("type").and_then(Value::as_str) != Some("message")
                    || payload.get("role").and_then(Value::as_str) != Some("user")
                {
                    continue;
                }
                if let Some(text) = user_text(payload) {
                    session.last_prompt = Some(text);
                    break;
                }
            }
            _ => {}
        }
    }
    session
}

/// The first real sentence out of a Codex `message` item.
///
/// Codex opens a conversation by feeding itself context as user-role
/// messages: the environment as `<environment_context>…`, and the project's
/// `AGENTS.md` verbatim under its own heading. Neither is something the user
/// said, and a row headed "# AGENTS.md instructions for /Users/…" identifies
/// the project but not the conversation.
fn user_text(payload: &Value) -> Option<String> {
    payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|part| string(part, "text"))
        .find(|text| !injected(text))
}

fn injected(text: &str) -> bool {
    text.starts_with('<') || text.starts_with("# AGENTS.md")
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Which of them are already on screen
// ---------------------------------------------------------------------------

/// Panes running an agent, as `cwd -> title`.
///
/// Herdr records an agent's session id internally (`pane.report_agent_session`)
/// but does not put it on `pane.get` or `agent.list`, so there is no id to
/// compare against. Matching on directory *and* title is the closest thing
/// available; the UI says "looks open" rather than "is open" for that reason.
fn open_conversations(herdr: &Herdr) -> HashMap<String, String> {
    herdr
        .agents()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|agent| {
            let cwd = agent.cwd.clone()?;
            Some((cwd, agent.terminal_title_stripped.clone().unwrap_or_default()))
        })
        .collect()
}

fn is_open(session: &AgentSession, open: &HashMap<String, String>) -> bool {
    let (Some(cwd), Some(title)) = (&session.cwd, &session.title) else {
        return false;
    };
    open.get(&cwd.display().to_string())
        .is_some_and(|running| running == title)
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

fn tilde(path: &Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

/// The first meaningful line of a prompt.
///
/// Prompts routinely open with attachment markers — `[Image #1]` on its own
/// line, or inline before the real text. Neither identifies anything, so they
/// are dropped whichever shape they take.
fn first_line(text: &str) -> String {
    text.lines()
        .map(|line| strip_markers(line.trim()))
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Remove leading `[…]` markers from a line.
fn strip_markers(mut line: &str) -> &str {
    while line.starts_with('[') {
        let Some(end) = line.find(']') else { break };
        line = line[end + 1..].trim_start();
    }
    line
}

/// Truncate on a character boundary, which matters because these are Japanese
/// as often as not.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("herdr-sessions-test-{name}.jsonl"));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn a_claude_transcript_gives_up_its_title_location_and_last_prompt() {
        let path = temp(
            "claude",
            concat!(
                r#"{"type":"user","cwd":"/tmp/proj","gitBranch":"main","sessionId":"abc"}"#,
                "\n",
                r#"{"type":"ai-title","aiTitle":"first title"}"#,
                "\n",
                r#"{"type":"ai-title","aiTitle":"newer title"}"#,
                "\n",
                r#"{"type":"last-prompt","lastPrompt":"do the thing"}"#,
                "\n"
            ),
        );
        let session = read_claude(&path);
        // The last `ai-title` wins: they are appended as the title is revised.
        assert_eq!(session.title.as_deref(), Some("newer title"));
        assert_eq!(session.last_prompt.as_deref(), Some("do the thing"));
        assert_eq!(session.cwd, Some(PathBuf::from("/tmp/proj")));
        assert_eq!(session.branch.as_deref(), Some("main"));
        assert_eq!(session.id, "abc");
    }

    #[test]
    fn a_truncated_leading_record_does_not_sink_the_rest() {
        // Reading from the end cuts a record in half. That one is dropped and
        // everything behind it still parses.
        let path = temp(
            "partial",
            concat!(
                r#"pe":"user","cwd":"/tmp/x"}"#,
                "\n",
                r#"{"type":"ai-title","aiTitle":"survived"}"#,
                "\n"
            ),
        );
        let session = read_claude(&path);
        assert_eq!(session.title.as_deref(), Some("survived"));
    }

    #[test]
    fn a_codex_rollout_gives_up_its_metadata_and_opening_prompt() {
        let path = temp(
            "codex",
            concat!(
                r#"{"type":"session_meta","payload":{"session_id":"019f-x","cwd":"/tmp/c"}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"other"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"plan it"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"later"}}"#,
                "\n"
            ),
        );
        let session = read_codex(&path);
        assert_eq!(session.id, "019f-x");
        assert_eq!(session.cwd, Some(PathBuf::from("/tmp/c")));
        // The *opening* prompt, not the latest: it is standing in for a title.
        assert_eq!(session.last_prompt.as_deref(), Some("plan it"));
    }

    #[test]
    fn the_other_codex_transcript_shape_is_read_too() {
        // Newer rollouts record the prompt as a response_item rather than an
        // event_msg. Both versions are on disk at the same time.
        let path = temp(
            "codex2",
            concat!(
                r#"{"type":"session_meta","payload":{"session_id":"01a0","cwd":"/tmp/d"}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"system stuff"}]}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n<cwd>/tmp/d</cwd>"}]}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ビルドして反映してください"}]}}"#,
                "\n"
            ),
        );
        let session = read_codex(&path);
        assert_eq!(session.id, "01a0");
        // Neither the developer preamble nor the injected environment block.
        assert_eq!(session.last_prompt.as_deref(), Some("ビルドして反映してください"));
    }

    #[test]
    fn an_unreadable_transcript_still_yields_its_id() {
        let path = temp("junk", "not json at all\n");
        let session = read(Kind::Claude, path.clone(), SystemTime::UNIX_EPOCH, &HashMap::new());
        assert_eq!(session.id, id_from_filename(&path));
        assert!(session.title.is_none());
    }

    #[test]
    fn claude_subagent_transcripts_are_not_offered_as_sessions() {
        let root = Path::new("/h/.claude/projects");
        // A real session: one directory below the root.
        assert!(Kind::Claude.accepts(root, &root.join("-Users-x/3ebd1a0d.jsonl")));
        // A subagent's transcript, which `claude --resume` cannot open.
        assert!(!Kind::Claude
            .accepts(root, &root.join("-Users-x/3ebd1a0d/subagents/agent-a20.jsonl")));
    }

    #[test]
    fn codex_only_offers_its_rollouts() {
        let root = Path::new("/h/.codex/sessions");
        let day = root.join("2026/08/19");
        assert!(Kind::Codex.accepts(root, &day.join("rollout-2026-08-19T20-07-05-01a0.jsonl")));
        assert!(!Kind::Codex.accepts(root, &day.join("index.jsonl")));
    }

    #[test]
    fn ids_come_out_of_both_filename_shapes() {
        assert_eq!(
            id_from_filename(Path::new("/x/3ebd1a0d-395a-4eb0-815c-9c6184d88d67.jsonl")),
            "3ebd1a0d-395a-4eb0-815c-9c6184d88d67"
        );
        assert_eq!(
            id_from_filename(Path::new(
                "/x/rollout-2026-07-23T00-07-10-019f8a5d-ca5e-7b73-87fd-64a56753fbd2.jsonl"
            )),
            "019f8a5d-ca5e-7b73-87fd-64a56753fbd2"
        );
    }

    #[test]
    fn every_tool_has_its_own_tag() {
        let tags: Vec<&str> = KINDS.iter().map(|k| k.tag()).collect();
        assert_ne!(tags[0], tags[1]);
        assert!(tags.iter().all(|t| t.starts_with('(') && t.ends_with(')')));
    }

    #[test]
    fn every_kind_is_in_the_combined_listing() {
        // A tool added to `Kind` but forgotten in `KINDS` would silently never
        // appear in the All view.
        assert!(KINDS.contains(&Kind::Claude));
        assert!(KINDS.contains(&Kind::Codex));
        assert_eq!(KINDS.len(), 2);
    }

    #[test]
    fn resume_arguments_match_each_tool() {
        assert_eq!(Kind::Claude.resume_args("x"), ["--resume", "x"]);
        assert_eq!(Kind::Codex.resume_args("x"), ["resume", "x"]);
    }

    #[test]
    fn a_heading_falls_back_through_title_then_prompt_then_id() {
        let mut session = AgentSession {
            id: "the-id".into(),
            ..Default::default()
        };
        assert_eq!(session.heading(), "the-id");
        session.last_prompt = Some("[Image #1]\n\nfix the icons".into());
        // The attachment marker is not a heading.
        assert_eq!(session.heading(), "fix the icons");
        session.title = Some("Icon sizing".into());
        assert_eq!(session.heading(), "Icon sizing");
    }

    #[test]
    fn attachment_markers_are_dropped_whether_they_stand_alone_or_lead_a_line() {
        assert_eq!(first_line("[Image #1]\n\nfix the icons"), "fix the icons");
        assert_eq!(first_line("[Image #31] 悪くなさそうです"), "悪くなさそうです");
        assert_eq!(first_line("[a][b] text"), "text");
        // An unclosed bracket is text, not a marker.
        assert_eq!(first_line("[unclosed"), "[unclosed");
    }

    #[test]
    fn codex_context_that_codex_fed_itself_is_not_mistaken_for_a_prompt() {
        assert!(injected("<environment_context>\n<cwd>/x</cwd>"));
        assert!(injected("# AGENTS.md instructions for /home/you/src/x"));
        assert!(!injected("ビルドして反映してください"));
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        let text = "セッション一覧を表示するプラグイン";
        let cut = truncate(text, 5);
        assert_eq!(cut.chars().count(), 5);
        assert!(cut.ends_with('…'));
        // Would panic on a byte slice.
        assert_eq!(truncate("短い", 5), "短い");
    }

    #[test]
    fn a_session_without_a_title_does_not_repeat_its_prompt_twice() {
        let session = AgentSession {
            last_prompt: Some("do the thing".into()),
            ..Default::default()
        };
        assert_eq!(session.heading(), "do the thing");
        assert!(session.context_line().is_none());
    }

    #[test]
    fn open_needs_both_the_directory_and_the_title_to_agree() {
        let session = AgentSession {
            cwd: Some(PathBuf::from("/tmp/proj")),
            title: Some("Build it".into()),
            ..Default::default()
        };
        let mut open = HashMap::new();
        open.insert("/tmp/proj".to_string(), "Something else".to_string());
        assert!(!is_open(&session, &open));
        open.insert("/tmp/proj".to_string(), "Build it".to_string());
        assert!(is_open(&session, &open));
    }
}
