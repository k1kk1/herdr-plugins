//! The Pane Manager overlay (spec §9, addendum §1–§5).
//!
//! One entry screen and four pickers. Every screen accepts arrow keys, `j`/`k`,
//! `1`..`9`, Enter, mouse, and `Esc`/`q`; the destination pickers also filter
//! as you type. Nothing here talks to Herdr directly — each flow ends in
//! [`ops::execute`], which is the same code the headless actions run
//! (addendum §13).

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::{Herdr, Pane, Workspace};
use herdr_plugin_kit::label;
use herdr_plugin_kit::layout::{Ratio, Side};
use herdr_plugin_kit::ui::{Menu, Row, Term};
use herdr_plugin_kit::{bail, Outcome, Result};

use crate::config::Config;
use crate::gather::{self, layout::PanesPerTab, select::Scope};
use crate::ops::{self, Destination, Placement, Request, Verb};
use crate::state::{Snapshot, TabEntry};
use crate::undo;

/// Which screen the process was launched into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// The Pane Manager overlay itself (`prefix+m`).
    Manager,
    Move,
    Swap,
    Merge,
}

/// What the overlay dispatched to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Move,
    Swap,
    Extract,
    Merge,
    QuickMove(usize),
    Gather,
    Restore,
    Undo,
    Cancel,
}

/// Run the interactive UI and report the outcome.
///
/// Returns `Ok(None)` when the user cancelled.
pub fn run(herdr: &Herdr, entry: Entry, source_pane: Pane) -> Result<Option<Outcome>> {
    let (config, config_warning) = Config::load_reporting();
    let mut term = Term::open()?;

    // A failed operation refreshes and offers the picker again rather than
    // dropping the user back to their shell (addendum §7).
    let result = loop {
        let snapshot = match Snapshot::capture(herdr, source_pane.clone()) {
            Ok(snapshot) => snapshot,
            Err(err) => break Err(err),
        };
        match dispatch(
            &mut term,
            herdr,
            entry,
            &snapshot,
            &config,
            config_warning.clone(),
        ) {
            Ok(outcome) => break Ok(outcome),
            Err(err) => {
                if !retry(&mut term, &err)? {
                    break Err(err);
                }
            }
        }
    };

    term.close();
    result
}

/// Show a failure and ask whether to try again with fresh state.
fn retry(term: &mut Term, err: &anyhow::Error) -> Result<bool> {
    let mut menu = Menu::new("Pane Manager")
        .subtitle("中途半端な状態にはなっていません。");
    for line in err.to_string().lines() {
        menu.row(Row::note(line.to_string()));
    }
    // `anyhow` chains the underlying Herdr API error behind the friendly text.
    for cause in err.chain().skip(1) {
        menu.row(Row::separator());
        menu.row(Row::note(cause.to_string()));
    }
    menu.row(Row::separator());
    menu.item(
        Row::item("Try again")
            .hotkey("r")
            .secondary("最新の状態で組み直す"),
        true,
    );
    menu.item(Row::item("Close").hotkey("q"), false);
    Ok(menu.run(term)?.unwrap_or(false))
}

fn dispatch(
    term: &mut Term,
    herdr: &Herdr,
    entry: Entry,
    snapshot: &Snapshot,
    config: &Config,
    config_warning: Option<String>,
) -> Result<Option<Outcome>> {
    match entry {
        Entry::Manager => manager(term, herdr, snapshot, config, config_warning),
        Entry::Move => move_flow(term, herdr, snapshot, config),
        Entry::Swap => swap_flow(term, herdr, snapshot, config),
        Entry::Merge => merge_flow(term, herdr, snapshot, config),
    }
}

/// The overlay (spec §9.2, addendum §1).
fn manager(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
    config_warning: Option<String>,
) -> Result<Option<Outcome>> {
    let mut menu = Menu::new("Pane Manager")
        .subtitle(source_line(snapshot, config));

    // Quick Move first: it is the fastest path, and the reason to open this
    // at all (addendum §2).
    let quick: Vec<_> = snapshot
        .tabs
        .iter()
        .filter(|t| t.tab.tab_id != snapshot.source.tab_id && t.position <= 9)
        .map(|t| {
            (
                t.position,
                label::tab_display(&t.tab, t.position),
                t.tab.pane_count,
            )
        })
        .collect();

    if !quick.is_empty() {
        menu.row(Row::header("Quick move current pane to"));
        for (position, name, pane_count) in quick {
            let panes = if pane_count == 1 { "pane" } else { "panes" };
            menu.item(
                Row::item(name)
                    .hotkey(position.to_string())
                    .secondary(format!("{pane_count} {panes}")),
                Choice::QuickMove(position),
            );
        }
        menu.row(Row::separator());
    }

    menu.item(
        Row::item("Move to…")
            .hotkey("m")
            .secondary("この Pane を別の Tab へ移す"),
        Choice::Move,
    );
    menu.item(
        Row::item("Swap with…")
            .hotkey("s")
            .secondary("別の Pane と入れ替える"),
        Choice::Swap,
    );
    menu.item(
        Row::item("Extract…")
            .hotkey("e")
            .secondary("独立した Tab へ切り出す"),
        Choice::Extract,
    );
    menu.item(
        Row::item("Fold into…")
            .hotkey("f")
            .secondary("この Tab を別の Tab へ畳む"),
        Choice::Merge,
    );

    // Undo sits right under the operations it reverses, and only appears when
    // there is actually something to take back.
    if let Some(record) = undo::load() {
        menu.row(Row::separator());
        menu.item(
            Row::item("Undo")
                .hotkey("u")
                .secondary(format!("{} を取り消す", record.describe())),
            Choice::Undo,
        );
    }

    menu.row(Row::separator());
    // Gather is listed with the operations, but it acts on the whole session
    // rather than on the current pane (addendum §9).
    menu.item(
        Row::item("Gather Active Agents")
            .hotkey("g")
            .secondary(gather_summary(herdr, config)),
        Choice::Gather,
    );
    if gather::session::load().is_some() {
        menu.item(
            Row::item("Restore Gathered Agents")
                .hotkey("r")
                .secondary("元の場所へ戻す"),
            Choice::Restore,
        );
    }

    menu.row(Row::separator());
    menu.item(Row::item("Cancel").hotkey("q"), Choice::Cancel);
    if let Some(warning) = config_warning {
        menu.row(Row::separator());
        menu.row(Row::note(format!("config: {warning}")));
    }

    let Some(choice) = menu.run(term)? else {
        return Ok(None);
    };

    match choice {
        Choice::Cancel => Ok(None),
        Choice::Undo => undo::undo(herdr).map(Some),
        Choice::Gather => gather_flow(term, herdr, config),
        Choice::Restore => gather::restore(herdr).map(Some),
        Choice::Move => move_flow(term, herdr, snapshot, config),
        Choice::Swap => swap_flow(term, herdr, snapshot, config),
        Choice::Merge => merge_flow(term, herdr, snapshot, config),
        // Immediate, no confirmation (spec §9.6).
        Choice::Extract => run_request(
            herdr,
            snapshot,
            config,
            Request {
                verb: Verb::Extract,
                source_pane: snapshot.source.pane_id.clone(),
                source_tab: None,
                destination: Destination::NewTab {
                    label: config.new_tab_label(&snapshot.source, None),
                },
                placement: Placement::default(),
                preserve_layout: false,
            },
        ),
        // Quick Move never asks anything further (addendum §2).
        Choice::QuickMove(position) => {
            let Some(tab) = snapshot.tab_at(position) else {
                bail!("This workspace has no tab {position}.");
            };
            run_request(
                herdr,
                snapshot,
                config,
                Request {
                    verb: Verb::Move,
                    source_pane: snapshot.source.pane_id.clone(),
                    source_tab: None,
                    destination: Destination::Tab {
                        tab_id: tab.tab.tab_id.clone(),
                        target_pane: None,
                    },
                    placement: config.quick_placement(),
                    preserve_layout: false,
                },
            )
        }
    }
}

/// One line saying what a Gather would collect right now.
fn gather_summary(herdr: &Herdr, config: &Config) -> String {
    match gather::session::load() {
        Some(existing) => format!("refresh · {} gathered", existing.origins.len()),
        None => match gather::count_active(herdr, config) {
            Ok(0) => "nothing needs attention".to_string(),
            Ok(n) => format!("{n} agent{}", if n == 1 { "" } else { "s" }),
            Err(_) => config.gather.status_summary(),
        },
    }
}

/// Gather picker (addendum §9). `prefix+m → g → 4` never reaches the scope
/// step, because a number picks the group size and runs immediately (§10).
fn gather_flow(term: &mut Term, herdr: &Herdr, config: &Config) -> Result<Option<Outcome>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pick {
        PerTab(u8),
        Scope(Scope),
    }

    let default_scope = config.gather.scope();
    let mut menu = Menu::new("Gather Active Agents")
        .subtitle(format!(
            "{} · {}",
            config.gather.status_summary(),
            default_scope.label()
        ));

    for per_tab in PanesPerTab::ALL {
        let size = per_tab.get();
        menu.item(
            Row::item(format!("{size} panes / tab"))
                .hotkey(size.to_string())
                .secondary(if per_tab == config.gather.per_tab() {
                    "default"
                } else {
                    ""
                }),
            Pick::PerTab(size as u8),
        );
    }

    menu.row(Row::separator());
    menu.row(Row::header("Scope"));
    for scope in [Scope::CurrentWorkspace, Scope::AllWorkspaces] {
        menu.item(
            Row::item(scope.label())
                .hotkey(if scope == Scope::CurrentWorkspace { "w" } else { "a" })
                .secondary(if scope == default_scope { "default" } else { "" }),
            Pick::Scope(scope),
        );
    }

    let Some(pick) = menu.run(term)? else {
        return Ok(None);
    };

    match pick {
        // A size runs straight away with the configured scope.
        Pick::PerTab(size) => {
            let per_tab = PanesPerTab::new(size).unwrap_or_else(|| config.gather.per_tab());
            gather::gather(herdr, config, per_tab, default_scope).map(Some)
        }
        // A scope runs with the configured size.
        Pick::Scope(scope) => gather::gather(herdr, config, config.gather.per_tab(), scope).map(Some),
    }
}

/// What a destination picker returned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    Tab(String),
    NewTab,
    NewWorkspace,
}

/// Move picker (spec §9.3, addendum §4, §5).
fn move_flow(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
) -> Result<Option<Outcome>> {
    let mut menu = destination_menu(
        "Move current pane to",
        source_line(snapshot, config),
        snapshot.move_destinations(),
        snapshot,
        true,
    );
    let picked = menu.run(term)?;
    // Whatever was typed becomes the name of a newly created tab or
    // workspace (addendum §5).
    let query = menu.query().trim().to_string();

    let Some(picked) = picked else {
        return Ok(None);
    };

    let destination = match picked {
        Pick::Tab(tab_id) => {
            // Advanced Move: name the pane to split, rather than letting Herdr
            // pick the destination tab's focused one (spec §4.1).
            let target_pane = if config.advanced_move {
                match choose_target_pane(term, snapshot, &tab_id, config)? {
                    Some(target) => target,
                    None => return Ok(None),
                }
            } else {
                None
            };
            Destination::Tab {
                tab_id,
                target_pane,
            }
        }
        Pick::NewTab => Destination::NewTab {
            label: config.new_tab_label(&snapshot.source, non_empty(&query)),
        },
        Pick::NewWorkspace => Destination::NewWorkspace {
            label: config.new_tab_label(&snapshot.source, non_empty(&query)),
        },
    };

    // Side and size only mean something when joining an existing tab.
    let (verb, placement) = match &destination {
        Destination::Tab { .. } => {
            let Some(placement) = ask_placement(term, config, "Move current pane")? else {
                return Ok(None);
            };
            (Verb::Move, placement)
        }
        _ => (Verb::Extract, Placement::default()),
    };

    run_request(
        herdr,
        snapshot,
        config,
        Request {
            verb,
            source_pane: snapshot.source.pane_id.clone(),
            source_tab: None,
            destination,
            placement,
            preserve_layout: false,
        },
    )
}

/// Merge picker (spec §9.7, addendum §11).
fn merge_flow(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
) -> Result<Option<Outcome>> {
    let source_tab_id = context::resolve_source_tab(None, &snapshot.source);
    let Some(source_tab) = snapshot.tab(&source_tab_id) else {
        bail!("Source tab no longer exists.");
    };
    let source_name = label::tab_display(&source_tab.tab, source_tab.position);
    let pane_count = source_tab.panes.len();

    let mut menu = destination_menu(
        "Merge current tab into",
        format!(
            "{source_name} · {pane_count} pane{}",
            if pane_count == 1 { "" } else { "s" }
        ),
        snapshot.merge_destinations(&source_tab_id),
        snapshot,
        // Merging into a brand-new tab would only rename the current one.
        false,
    );

    let Some(Pick::Tab(destination)) = menu.run(term)? else {
        return Ok(None);
    };

    if config.confirm_merge && !confirm(term, &format!("Merge {source_name} into this tab?"))? {
        return Ok(None);
    }

    let Some(placement) = ask_placement(term, config, "Merge current tab")? else {
        return Ok(None);
    };

    run_request(
        herdr,
        snapshot,
        config,
        Request {
            verb: Verb::Merge,
            source_pane: snapshot.source.pane_id.clone(),
            source_tab: Some(source_tab_id),
            destination: Destination::Tab {
                tab_id: destination,
                target_pane: None,
            },
            placement,
            preserve_layout: config.preserve_merge_layout,
        },
    )
}

/// Swap picker (spec §9.5, addendum §10).
fn swap_flow(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
) -> Result<Option<Outcome>> {
    let mut menu = Menu::new("Swap current pane with")
        .subtitle(source_line(snapshot, config))
        .filterable();

    let candidates = snapshot.swap_candidates();
    if candidates.is_empty() {
        menu.row(Row::note("このセッションに他の Pane がありません。"));
    }

    let mut hotkey = 0usize;
    let mut current_tab: Option<String> = None;
    for (workspace, tab, pane) in candidates {
        let group = group_name(workspace, tab, snapshot);
        if current_tab.as_deref() != Some(tab.tab.tab_id.as_str()) {
            if current_tab.is_some() {
                menu.row(Row::separator());
            }
            menu.row(Row::header(group.clone()));
            current_tab = Some(tab.tab.tab_id.clone());
        }
        hotkey += 1;
        let mut row = pane_row(pane, config);
        if hotkey <= 9 {
            row = row.hotkey(hotkey.to_string());
        }
        // The group name is only in the header, so make it searchable here.
        menu.item_matching(row, pane.pane_id.clone(), &group);
    }

    let Some(target) = menu.run(term)? else {
        return Ok(None);
    };

    run_request(
        herdr,
        snapshot,
        config,
        Request {
            verb: Verb::Swap,
            source_pane: snapshot.source.pane_id.clone(),
            source_tab: None,
            destination: Destination::Pane { pane_id: target },
            placement: Placement::default(),
            preserve_layout: false,
        },
    )
}

/// A filterable list of tabs, grouped by workspace, optionally offering the
/// two "create it now" entries (addendum §4, §5).
fn destination_menu(
    title: &str,
    subtitle: String,
    destinations: Vec<(&Workspace, &TabEntry)>,
    snapshot: &Snapshot,
    offer_new: bool,
) -> Menu<Pick> {
    let mut menu = Menu::new(title)
        .subtitle(subtitle)
        .filterable();

    if destinations.is_empty() && !offer_new {
        menu.row(Row::note("このセッションに他の Tab がありません。"));
    }

    let mut current_workspace: Option<String> = None;
    for (workspace, tab) in destinations {
        let name = workspace_name(workspace);
        if current_workspace.as_deref() != Some(workspace.workspace_id.as_str()) {
            if current_workspace.is_some() {
                menu.row(Row::separator());
                menu.row(Row::header(name.clone()));
            }
            current_workspace = Some(workspace.workspace_id.clone());
        }

        let panes = if tab.tab.pane_count == 1 { "pane" } else { "panes" };
        let mut row = Row::item(label::tab_display(&tab.tab, tab.position))
            .secondary(format!("{} {panes}", tab.tab.pane_count));
        // Quick-pick numbers only make sense inside the current workspace,
        // where they match the tab numbers the user already knows.
        if workspace.workspace_id == snapshot.workspace.workspace_id && tab.position <= 9 {
            row = row.hotkey(tab.position.to_string());
        }
        menu.item_matching(row, Pick::Tab(tab.tab.tab_id.clone()), &name);
    }

    if offer_new {
        menu.row(Row::separator());
        // `{query}` is substituted as the user types, so the row reads
        // `+ New Tab "review"` once something has been entered.
        menu.item_pinned(Row::item("+ New Tab {query}").hotkey("n"), Pick::NewTab);
        menu.item_pinned(
            Row::item("+ New Workspace {query}").hotkey("w"),
            Pick::NewWorkspace,
        );
    }
    menu
}

/// Advanced Move: pick which pane in the destination tab gets split (§4.1).
///
/// `Ok(Some(None))` means "let Herdr choose"; `Ok(None)` means cancelled.
fn choose_target_pane(
    term: &mut Term,
    snapshot: &Snapshot,
    tab_id: &str,
    config: &Config,
) -> Result<Option<Option<String>>> {
    let Some(tab) = snapshot.tab(tab_id) else {
        bail!("Destination tab no longer exists.");
    };

    let mut menu = Menu::new("Split which pane?")
        .subtitle(label::tab_display(&tab.tab, tab.position));

    menu.item(
        Row::item("Auto")
            .hotkey("a")
            .secondary("その Tab のフォーカス中の Pane を使う"),
        None as Option<String>,
    );
    menu.row(Row::separator());
    for (index, pane) in tab.panes.iter().enumerate() {
        let mut row = pane_row(pane, config);
        if index < 9 {
            row = row.hotkey((index + 1).to_string());
        }
        menu.item(row, Some(pane.pane_id.clone()));
    }

    menu.run(term)
}

/// Side and size, asked only when the settings leave them open (§4.1, §12).
fn ask_placement(term: &mut Term, config: &Config, context: &str) -> Result<Option<Placement>> {
    let side = match config.default_move_direction.resolve() {
        Some(side) => side,
        None => {
            let mut menu = Menu::new("Place where?")
                .subtitle(context.to_string());
            for side in Side::ALL {
                menu.item(
                    Row::item(capitalize(side.as_str())).hotkey(side.hotkey().to_string()),
                    side,
                );
            }
            match menu.run(term)? {
                Some(side) => side,
                None => return Ok(None),
            }
        }
    };

    let ratio = if config.ask_ratio() {
        let mut menu = Menu::new("How much space?")
            .subtitle(context.to_string());
        for (index, ratio) in Ratio::ALL.iter().enumerate() {
            menu.item(
                Row::item(ratio.label())
                    .hotkey((index + 1).to_string())
                    .secondary("元からある Pane : 置く Pane"),
                *ratio,
            );
        }
        match menu.run(term)? {
            Some(ratio) => ratio,
            None => return Ok(None),
        }
    } else {
        config.ratio()
    };

    Ok(Some(Placement { side, ratio }))
}

fn confirm(term: &mut Term, question: &str) -> Result<bool> {
    let mut menu = Menu::new(question).enter("confirm");
    menu.item(Row::item("Yes").hotkey("y"), true);
    menu.item(Row::item("No").hotkey("n"), false);
    Ok(menu.run(term)?.unwrap_or(false))
}

fn run_request(
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
    request: Request,
) -> Result<Option<Outcome>> {
    ops::execute(herdr, snapshot, &request, config).map(Some)
}

/// One-line description of the pane an operation will act on.
fn source_line(snapshot: &Snapshot, config: &Config) -> String {
    let mut line = label::pane_compact(&snapshot.source);
    if let Some(tab) = snapshot.source_tab() {
        line.push_str(&format!(
            "  ({})",
            label::tab_display(&tab.tab, tab.position)
        ));
    }
    if config.show_ids {
        line.push_str(&format!("  [{}]", snapshot.source.pane_id));
    }
    line
}

fn workspace_name(workspace: &Workspace) -> String {
    workspace
        .label
        .clone()
        .unwrap_or_else(|| "Workspace".to_string())
}

/// Group header for a tab, qualified by workspace only when it is not the
/// user's own.
fn group_name(workspace: &Workspace, tab: &TabEntry, snapshot: &Snapshot) -> String {
    let tab_name = label::tab_display(&tab.tab, tab.position);
    if workspace.workspace_id == snapshot.workspace.workspace_id {
        tab_name
    } else {
        format!("{} · {tab_name}", workspace_name(workspace))
    }
}

/// Picker row for a pane, with the ID revealed only when `show_ids` is on.
fn pane_row(pane: &Pane, config: &Config) -> Row {
    let mut row =
        herdr_plugin_kit::ui::pane_row(pane, config.show_agent_state, config.show_terminal_title);
    if config.show_ids {
        row = row.secondary(pane.pane_id.clone());
    }
    row
}

fn non_empty(text: &str) -> Option<&str> {
    Some(text).filter(|t| !t.is_empty())
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
