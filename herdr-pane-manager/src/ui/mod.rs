//! The Pane Manager overlay (spec §9, addendum §1–§5).
//!
//! One entry screen and four pickers. Every screen accepts arrow keys, `j`/`k`,
//! `1`..`9`, Enter, mouse, and `Esc`/`q`; the destination pickers also filter
//! as you type. Nothing here talks to Herdr directly — each flow ends in
//! [`ops::execute`], which is the same code the headless actions run
//! (addendum §13).

use herdr_plugin_kit::context;
use herdr_plugin_kit::herdr::{Agent, Herdr, Pane, Workspace};
use herdr_plugin_kit::label;
use herdr_plugin_kit::layout::{Ratio, Shape, Side};
use herdr_plugin_kit::ui::{Key, Menu, Panel, Row, Term};
use herdr_plugin_kit::{bail, Outcome, Result};

use crate::config::Config;
use crate::gather::{self, layout::PanesPerTab, select::Scope};
use crate::ops::{self, Destination, Placement, Request, Verb};
use crate::state::{Snapshot, TabEntry};

mod preview;
use preview::*;
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
pub(crate) enum Choice {
    Move,
    Swap,
    Extract,
    Merge,
    QuickMove(usize),
    /// The same tab, but stopping to ask side, size and which pane to split.
    DetailedMove(usize),
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
        let snapshot = match if entry == Entry::Manager && !config.show_quick_move {
            Snapshot::capture_manager(herdr, source_pane.clone())
        } else {
            Snapshot::capture(herdr, source_pane.clone())
        } {
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
        Entry::Move => {
            move_flow(term, herdr, snapshot, config, config.default_action.detailed(false))
        }
        Entry::Swap => swap_flow(term, herdr, snapshot, config),
        Entry::Merge => merge_flow(term, herdr, snapshot, config),
    }
}

/// The key line, worded for whichever way round the settings have it.
fn manager_footer(modified_enter: bool, config: &Config) -> String {
    let (plain, shifted) = match config.default_action {
        crate::config::DefaultAction::Quick => ("すぐ移動", "位置を指定"),
        crate::config::DefaultAction::Detailed => ("位置を指定", "すぐ移動"),
    };
    // Shift+letter needs no keyboard protocol; Shift+Enter does. Only promise
    // the one that will actually arrive.
    let shift_key = if modified_enter {
        "Shift+Enter / Shift+英字"
    } else {
        "Shift+英字"
    };
    // `1-9` only earns its place in the line when there are numbered rows.
    let digits = if config.show_quick_move { "1-9・" } else { "" };
    format!("{digits}英字・Enter {plain}  ·  {shift_key} {shifted}  ·  Esc 閉じる（先の画面では戻る）")
}

/// The overlay (spec §9.2, addendum §1).
///
/// Re-enters itself whenever a screen below is left without doing anything, so
/// Esc down there means "back" rather than "give up" — the reader who opens
/// `Move to…`, looks at the tabs and changes their mind lands where they
/// started rather than with the overlay shut.
fn manager(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
    config_warning: Option<String>,
) -> Result<Option<Outcome>> {
    loop {
        match manager_once(term, herdr, snapshot, config, config_warning.clone())? {
            Step::Done(outcome) => return Ok(Some(outcome)),
            Step::Back => continue,
            Step::Close => return Ok(None),
        }
    }
}

/// How one pass through the overlay ended.
enum Step {
    /// An operation ran; report it and finish.
    Done(Outcome),
    /// A screen below was cancelled. Show the menu again.
    Back,
    /// Esc or Cancel on the menu itself. Nothing left to go back to.
    Close,
}

fn manager_once(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
    config_warning: Option<String>,
) -> Result<Step> {
    let menu_partner = swap_partner(herdr, snapshot, config)
        .and_then(|id| snapshot.pane(&id).cloned());
    // Use the same agent list and selector that Gather itself uses. The
    // pane snapshot may have older status metadata, and an existing Gather
    // session only describes the previous result.
    let selected = herdr.agents().ok().map(|agents| {
        gather_names(
            &agents,
            config,
            match config.gather.scope() {
                Scope::CurrentWorkspace => Some(snapshot.workspace.workspace_id.as_str()),
                Scope::AllWorkspaces => None,
            },
        )
    });
    let exact = selected.is_some();
    let ready = selected.unwrap_or_else(|| gatherable_here(snapshot, config));
    let mut menu = manager_menu(
        snapshot,
        config,
        config_warning,
        term.distinguishes_modified_enter(),
        menu_partner.clone(),
        &ready,
        exact,
    );
    let Some(choice) = menu.run(term)? else {
        return Ok(Step::Close);
    };
    // The pane the Swap row named is the pane `s` must trade with. Asking
    // again here would be a second round trip and a second chance to disagree
    // with the sentence the reader just read.
    let partner = menu_partner.map(|pane| pane.pane_id);
    manager_choice(term, herdr, snapshot, config, choice, &menu, partner, &ready)
}

/// The landing screen, built but not run.
///
/// Separated so a test can read the rows: which of them appear, and whether a
/// row's name promises a screen the key will actually open.
fn manager_menu(
    snapshot: &Snapshot,
    config: &Config,
    config_warning: Option<String>,
    modified_enter: bool,
    partner: Option<Pane>,
    ready: &[String],
    exact: bool,
) -> Menu<Choice> {
    let mut menu = Menu::new("Pane Manager")
        .subtitle(source_line(snapshot, config))
        // The keys live here rather than beside the rows they apply to: a hint
        // repeated on every section is clutter, and one at the bottom is where
        // a reader looks for keys anyway.
        .footer(manager_footer(modified_enter, config))
        // Quick rows take Shift+Enter as "the same tab, but ask me where".
        .accept_also(&[Key::ShiftEnter])
        .no_preview("この操作は Pane の配置を変えません");

    // Quick Move first when it is shown: it is the fastest path to another
    // tab (addendum §2). Off by default — see `show_quick_move`.
    let quick: Vec<_> = if !config.show_quick_move {
        Vec::new()
    } else {
        snapshot
        .tabs
        .iter()
        .filter(|t| t.tab.tab_id != snapshot.source.tab_id && t.position <= 9)
        .map(|t| {
            (
                t.position,
                label::tab_display(&t.tab, t.position),
                tab_contents(t),
                tab_preview(t, Some(config.default_move_direction.resolve().unwrap_or(Side::Right))),
            )
        })
        .collect()
    };

    if !quick.is_empty() {
        menu.row(Row::header("Quick move current pane to"));
        for (position, name, contents, diagram) in quick {
            menu.item(
                Row::item(name)
                    .hotkey(position.to_string())
                    .detail(Some(contents))
                    .preview_of(diagram),
                Choice::QuickMove(position),
            );
        }
        menu.row(Row::separator());
    }

    // `Move to…` promises a next screen. With the default action set to
    // Quick the plain key does not open one, it moves the pane — so the
    // ellipsis is a lie there, and the row is named for what the key does.
    let more = |name: &str| match config.default_action {
        crate::config::DefaultAction::Detailed => format!("{name}…"),
        crate::config::DefaultAction::Quick => name.to_string(),
    };

    // Each row names its default target. A key that acts immediately has to
    // say what it will do before it is pressed, or it is a trap.
    let next_tab = snapshot
        .next_tab()
        .map(|t| label::tab_name(&t.tab).unwrap_or_else(|| "Tab".into()));
    // Resolved once by the caller: the row's wording, the picture beside it
    // and the key all have to name the same pane. Asking Herdr for the
    // neighbour in one place and walking the pane list in another put a
    // different pane in the sentence than the one `s` would trade with.
    let next_pane = partner.as_ref().map(label::pane_compact);
    let target = |name: &Option<String>, verb: &str, pick: &str| match name {
        Some(name) => format!("{name} {verb}"),
        None => pick.to_string(),
    };

    menu.item(
        illustrated(
            Row::item(more("Move to")).hotkey("m").secondary(target(
                &next_tab,
                "へ現在の Pane を移す",
                "新しい Tab へ現在の Pane を移す",
            )),
            operation_preview(&Choice::Move, snapshot, config),
            snapshot,
        ),
        Choice::Move,
    );
    menu.item(
        illustrated(
            Row::item(more("Swap with"))
                .hotkey("s")
                .secondary(target(&next_pane, "と入れ替える", "入れ替える相手を選ぶ")),
            match &partner {
                Some(partner) => swap_panels(snapshot, partner),
                None => Vec::new(),
            },
            snapshot,
        ),
        Choice::Swap,
    );
    menu.item(
        illustrated(
            Row::item(more("Extract"))
                .hotkey("e")
                .secondary("現在の Pane を新しい Tab へ切り出す"),
            operation_preview(&Choice::Extract, snapshot, config),
            snapshot,
        ),
        Choice::Extract,
    );
    // Fold moves this tab's panes into another one, so with no other tab
    // there is nothing the row can do. It used to offer to choose a
    // destination and then say there were none.
    if next_tab.is_some() {
        menu.item(
            illustrated(
                Row::item(more("Fold into")).hotkey("f").secondary(target(
                    &next_tab,
                    "へこの Tab 全体を畳む",
                    String::new().as_str(),
                )),
                operation_preview(&Choice::Merge, snapshot, config),
                snapshot,
            ),
            Choice::Merge,
        );
    }

    // Undo sits right under the operations it reverses, and only appears when
    // there is actually something to take back.
    if let Some(record) = undo::load() {
        menu.row(Row::separator());
        menu.item(
            Row::item("Undo")
                .hotkey("u")
                .secondary(format!("{} を取り消す", record.describe()))
                .panels(undo_panels(&record, snapshot)),
            Choice::Undo,
        );
    }

    menu.row(Row::separator());
    // Gather is listed with the operations, but it acts on the whole session
    // rather than on the current pane (addendum §9).
    let gathered = gather::session::load();
    menu.item(
        gather_offer(snapshot, config, ready, exact),
        Choice::Gather,
    );
    if gathered.is_some() {
        let collecting = gathered.as_ref().map_or(0, |session| session.origins.len());
        menu.item(
            Row::item("Restore Gathered Agents")
                .hotkey("r")
                // Named for what it undoes, because `Undo` sits four rows up
                // and the two take back different things: this one only ever
                // reverses a Gather, and only Gather.
                .secondary(format!("Gather した {collecting} 個を元の Tab へ"))
                .panels(restore_panels(snapshot)),
            Choice::Restore,
        );
    }

    menu.row(Row::separator());
    menu.item(Row::item("Cancel").hotkey("q"), Choice::Cancel);
    if let Some(warning) = config_warning {
        menu.row(Row::separator());
        menu.row(Row::note(format!("config: {warning}")));
    }

    menu
}

/// Describe what Gather does; the live selection belongs in its preview.
fn gather_offer(
    snapshot: &Snapshot,
    config: &Config,
    ready: &[String],
    exact: bool,
) -> Row {
    let known = exact || config.gather.scope() == Scope::CurrentWorkspace;
    let limit = config.gather.per_tab().get();
    let scope = match config.gather.scope() {
        Scope::CurrentWorkspace => "この Workspace",
        Scope::AllWorkspaces => "全 Workspace",
    };
    let note = format!("{scope} の Agent を更新が新しい順に最大 {limit} Pane、1つの Tab へ集める");
    let panels = if known && !ready.is_empty() {
        gather_panels(ready, Some(snapshot), config)
    } else {
        Vec::new()
    };
    Row::item("Gather Active Agents")
        .hotkey("g")
        .secondary(note)
        .panels(panels)
}

/// Act on the row the reader picked.
fn manager_choice(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
    choice: Choice,
    menu: &Menu<Choice>,
    partner: Option<String>,
    ready: &[String],
) -> Result<Step> {
    // Shift means "that, but let me say where" — on a quick row it names the
    // tab and stops to ask, on `Move to…` it carries the same intent into the
    // flow so the placement step appears without asking for Shift twice.
    // The plain key stays the fast path it has always been.
    let detailed = config
        .default_action
        .detailed(menu.accepted_with() == Key::ShiftEnter);
    let choice = match (choice, detailed) {
        (Choice::QuickMove(position), true) => Choice::DetailedMove(position),
        (choice, _) => choice,
    };

    if choice == Choice::Cancel {
        return Ok(Step::Close);
    }

    // A flow that returns nothing was cancelled on its own screen; that is a
    // step back, not the end of the session.
    let outcome = match choice {
        Choice::Cancel => unreachable!("handled above"),
        Choice::Undo => undo::undo(herdr).map(Some),
        Choice::Gather => {
            if menu.accepted_with() == Key::ShiftEnter {
                gather_flow(term, herdr, config, ready)
            } else {
                gather::gather(herdr, config, config.gather.per_tab(), config.gather.scope())
                    .map(Some)
            }
        }
        Choice::Restore => gather::restore(herdr).map(Some),
        // Without Shift these run straight away, using the default target the
        // row already names. A lone tab defaults to a new destination; a lone
        // pane still has nothing to trade with, so only Swap needs a picker.
        Choice::Move => match (detailed, snapshot.next_tab()) {
            (false, Some(tab)) => {
                let tab_id = tab.tab.tab_id.clone();
                run_request(
                    herdr,
                    snapshot,
                    config,
                    Request {
                        verb: Verb::Move,
                        source_pane: snapshot.source.pane_id.clone(),
                        source_tab: None,
                        destination: Destination::Tab {
                            tab_id,
                            target_pane: None,
                        },
                        placement: config.quick_placement(),
                        preserve_layout: false,
                    },
                )
            }
            (false, None) => run_request(
                herdr,
                snapshot,
                config,
                Request {
                    // A one-tab Move is an Extract. Keeping the operation's
                    // real verb makes its undo record and outcome accurate.
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
            _ => {
                let full = snapshot.full(herdr)?;
                move_flow(term, herdr, &full, config, detailed)
            }
        },
        Choice::Swap => match (detailed, partner) {
            (false, Some(target)) => {
                run_request(
                    herdr,
                    snapshot,
                    config,
                    Request {
                        verb: Verb::Swap,
                        source_pane: snapshot.source.pane_id.clone(),
                        source_tab: None,
                        destination: Destination::Pane { pane_id: target },
                        placement: config.quick_placement(),
                        preserve_layout: false,
                    },
                )
            }
            _ => {
                let full = snapshot.full(herdr)?;
                swap_flow(term, herdr, &full, config)
            }
        },
        Choice::Merge => match (detailed, snapshot.next_tab()) {
            (false, Some(tab)) => {
                let tab_id = tab.tab.tab_id.clone();
                run_request(
                    herdr,
                    snapshot,
                    config,
                    Request {
                        verb: Verb::Merge,
                        source_pane: snapshot.source.pane_id.clone(),
                        source_tab: Some(snapshot.source.tab_id.clone()),
                        destination: Destination::Tab {
                            tab_id,
                            target_pane: None,
                        },
                        placement: config.quick_placement(),
                        preserve_layout: config.preserve_merge_layout,
                    },
                )
            }
            _ => {
                let full = snapshot.full(herdr)?;
                merge_flow(term, herdr, &full, config)
            }
        },
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
        Choice::DetailedMove(position) => {
            let full = snapshot.full(herdr)?;
            let Some(tab) = full.tab_at(position) else {
                bail!("This workspace has no tab {position}.");
            };
            let tab_id = tab.tab.tab_id.clone();
            detailed_move_into(term, herdr, &full, config, &tab_id)
        }
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
    }?;

    Ok(match outcome {
        Some(outcome) => Step::Done(outcome),
        None => Step::Back,
    })
}

/// Gather picker (addendum §9). `prefix+m → g → 4` never reaches the scope
/// step, because a number picks the group size and runs immediately (§10).
fn gather_flow(
    term: &mut Term,
    herdr: &Herdr,
    config: &Config,
    names: &[String],
) -> Result<Option<Outcome>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pick {
        PaneCount(u8),
        Scope(Scope),
    }

    let default_scope = config.gather.scope();
    let agents = herdr.agents().ok();
    let workspace = herdr.focused_workspace().ok().map(|w| w.workspace_id);
    let default_names = agents.as_ref().map(|agents| {
        gather_names(
            agents,
            config,
            if default_scope == Scope::CurrentWorkspace {
                workspace.as_deref()
            } else {
                None
            },
        )
    });
    let default_names = if default_scope == Scope::CurrentWorkspace && workspace.is_none() {
        None
    } else {
        default_names
    };
    let default_names = default_names.as_deref().unwrap_or(names);
    let default_count = config.gather.per_tab();
    let mut menu = Menu::new("Gather Active Agents")
        .enter("gather")
        .subtitle(format!(
            "更新が新しい Agent から最大 {} Pane を1つの Tab へ · {} · {}",
            default_count.get(),
            config.gather.status_summary(),
            default_scope.label()
        ));

    let mut counts = PanesPerTab::ALL;
    counts.sort_by_key(|count| *count != default_count);
    for per_tab in counts {
        let size = per_tab.get();
        menu.item(
            Row::item(format!("{size} panes in one tab"))
                .hotkey(size.to_string())
                .secondary(if per_tab == config.gather.per_tab() {
                    "default"
                } else {
                    ""
                })
                .panels(if default_names.is_empty() {
                    Vec::new()
                } else {
                    gather_size_panels(size, default_names, config)
                }),
            Pick::PaneCount(size as u8),
        );
    }

    menu.row(Row::separator());
    menu.row(Row::header("Scope"));
    for scope in [Scope::CurrentWorkspace, Scope::AllWorkspaces] {
        let scope_names = agents.as_ref().and_then(|agents| {
            let workspace = match scope {
                Scope::CurrentWorkspace => Some(workspace.as_deref()?),
                Scope::AllWorkspaces => None,
            };
            Some(gather_names(agents, config, workspace))
        });
        let scope_names = if scope == default_scope {
            Some(scope_names.as_deref().unwrap_or(default_names))
        } else {
            scope_names.as_deref()
        };
        menu.item(
            Row::item(scope.label())
                .hotkey(if scope == Scope::CurrentWorkspace { "w" } else { "a" })
                .secondary(if scope == default_scope { "default" } else { "" })
                .panels(scope_names.filter(|names| !names.is_empty()).map_or_else(
                    Vec::new,
                    |names| gather_size_panels(config.gather.per_tab().get(), names, config),
                )),
            Pick::Scope(scope),
        );
    }

    let Some(pick) = menu.run(term)? else {
        return Ok(None);
    };

    match pick {
        // A size runs straight away with the configured scope.
        Pick::PaneCount(size) => {
            let per_tab = PanesPerTab::new(size).unwrap_or_else(|| config.gather.per_tab());
            gather::gather(herdr, config, per_tab, default_scope).map(Some)
        }
        // A scope runs with the configured size.
        Pick::Scope(scope) => gather::gather(herdr, config, config.gather.per_tab(), scope).map(Some),
    }
}

fn gather_names(agents: &[Agent], config: &Config, workspace: Option<&str>) -> Vec<String> {
    gather::select::select(agents, &config.gather, workspace)
        .iter()
        .map(|agent| short_pane_id(&agent.pane_id))
        .collect()
}

/// What a destination picker returned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pick {
    Tab(String),
    NewTab,
    NewWorkspace,
}

/// Move picker (spec §9.3, addendum §4, §5).
/// Move the current pane into a named tab, asking everything the settings
/// leave open — which pane to split, which side, how much space.
///
/// The tail of [`move_flow`] once a destination is known, reached instead by
/// holding Shift on a quick row: the same tab, deliberately rather than fast.
fn detailed_move_into(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
    tab_id: &str,
) -> Result<Option<Outcome>> {
    let target_pane = match choose_target_pane(term, snapshot, tab_id, config)? {
        Some(target) => target,
        None => return Ok(None),
    };

    let tab = snapshot.tab(tab_id);
    let anchor = target_pane
        .clone()
        .or_else(|| tab.and_then(|t| t.split_anchor().map(|p| p.pane_id.clone())));
    let lone;
    let preview = match (tab.and_then(|t| t.shape.as_ref()), anchor.as_deref()) {
        (Some(shape), Some(anchor)) => Some((shape, anchor)),
        (None, Some(anchor)) => {
            lone = Some(Shape::pane(anchor));
            lone.as_ref().map(|shape| (shape, anchor))
        }
        _ => None,
    };

    let Some(placement) = ask_placement(term, config, "Move current pane", preview)? else {
        return Ok(None);
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
                tab_id: tab_id.to_string(),
                target_pane,
            },
            placement,
            preserve_layout: false,
        },
    )
}

/// `forced_detail` is the starting intent: what the menu that opened this one
/// had already settled on, so the placement step appears without the reader
/// having to hold Shift twice. A Shift held here flips it back.
fn move_flow(
    term: &mut Term,
    herdr: &Herdr,
    snapshot: &Snapshot,
    config: &Config,
    forced_detail: bool,
) -> Result<Option<Outcome>> {
    let mut menu = destination_menu(
        "Move to…",
        format!(
            "{} を、選んだ Tab へ移します{}",
            source_line(snapshot, config),
            // Say that the Shift already held has been taken: otherwise this
            // screen looks identical either way and the key feels ignored.
            if forced_detail {
                " · 位置を指定します"
            } else if term.distinguishes_modified_enter() {
                " · Shift+Enter で位置を指定"
            } else {
                ""
            }
        ),
        snapshot.move_destinations(),
        snapshot,
        true,
        // The side is settled before the list is shown, so the drawing can
        // show the result rather than the starting point.
        |tab| vec![real_destination(tab, snapshot, config)],
    );
    let picked = menu.run(term)?;
    // `forced_detail` is the intent carried in from the menu that opened this
    // one; Shift here means "the other one" just as it does everywhere else.
    // Exclusive-or rather than or, so a Shift held now can still take the
    // decision back.
    let detailed = forced_detail != (menu.accepted_with() == Key::ShiftEnter);
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
            if detailed {
                return detailed_move_into(term, herdr, snapshot, config, &tab_id);
            }
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

    // Held outside the match so a shape built for a one-pane tab outlives the
    // borrow the preview takes of it.
    let lone;
    // Side and size only mean something when joining an existing tab.
    let (verb, placement) = match &destination {
        Destination::Tab { tab_id, target_pane } => {
            // The preview needs the tab's real shape and the pane being split.
            let target = snapshot.tab(tab_id);
            let anchor = target_pane.clone().or_else(|| {
                target.and_then(|t| t.split_anchor().map(|p| p.pane_id.clone()))
            });
            let preview = match (target.and_then(|t| t.shape.as_ref()), anchor.as_deref()) {
                (Some(shape), Some(anchor)) => Some((shape, anchor)),
                // A one-pane tab has no stored shape; make the trivial one so
                // the preview still works, which is the commonest case of all.
                (None, Some(anchor)) => {
                    lone = Some(Shape::pane(anchor));
                    lone.as_ref().map(|shape| (shape, anchor))
                }
                _ => None,
            };
            let Some(placement) = ask_placement(term, config, "Move current pane", preview)? else {
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
        "Fold into…",
        format!(
            "{source_name} の {pane_count} Pane を、選んだ Tab へまとめて移します"
        ),
        snapshot.merge_destinations(&source_tab_id),
        snapshot,
        // Merging into a brand-new tab would only rename the current one.
        false,
        // A whole tab's worth of panes arrives, arranged the way the Fold will
        // really arrange them — the same call the landing screen's picture
        // uses, so the two screens cannot disagree.
        |tab| {
            let moving: Vec<String> = source_tab
                .panes
                .iter()
                .map(|pane| pane.pane_id.clone())
                .collect();
            let labels: Vec<(String, String)> = source_tab
                .panes
                .iter()
                .map(|pane| (pane.pane_id.clone(), pane_number(pane)))
                .collect();
            vec![
                folded_into(tab, &moving, &labels, source_tab.shape.as_ref(), config)
                    .marking(vec![snapshot.source.pane_id.clone()]),
            ]
        },
    );

    let Some(Pick::Tab(destination)) = menu.run(term)? else {
        return Ok(None);
    };

    if config.confirm_merge && !confirm(term, &format!("Merge {source_name} into this tab?"))? {
        return Ok(None);
    }

    let Some(placement) = ask_placement(term, config, "Merge current tab", None)? else {
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
    let mut menu = Menu::new("Swap with…")
        .subtitle(format!(
            "{} を、選んだ Pane と位置ごと入れ替えます",
            source_line(snapshot, config)
        ))
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
        let mut row = illustrated(pane_row(pane, config), swap_panels(snapshot, pane), snapshot);
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














/// The pane a plain Swap trades with.
///
/// The neighbour in the configured direction, which is how `herdr pane swap
/// --direction` chooses. Falling back to the next pane in the tab keeps the
/// key useful in a layout where nothing sits that way — with two panes stacked
/// vertically there is no pane to the right, but there is obviously only one
/// thing the reader can mean.
fn swap_partner(herdr: &Herdr, snapshot: &Snapshot, config: &Config) -> Option<String> {
    let side = config.default_move_direction.resolve().unwrap_or(Side::Right);
    herdr
        .neighbor(&snapshot.source.pane_id, side.as_str())
        .ok()
        .flatten()
        .or_else(|| snapshot.next_pane_here().map(|p| p.pane_id.clone()))
}














/// A filterable list of tabs, grouped by workspace, optionally offering the
/// two "create it now" entries (addendum §4, §5).
fn destination_menu(
    title: &str,
    subtitle: String,
    destinations: Vec<(&Workspace, &TabEntry)>,
    snapshot: &Snapshot,
    offer_new: bool,
    // Each list draws its own consequence: Move puts one pane into the tab,
    // Fold puts a whole tab's worth. Passing the picture in rather than a side
    // keeps the row and the operation it stands for computed by one function.
    panels: impl Fn(&TabEntry) -> Vec<Panel>,
) -> Menu<Pick> {
    let mut menu = Menu::new(title)
        .subtitle(subtitle)
        .filterable()
        // Same bargain as the quick rows: Enter takes the settings, Shift+Enter
        // stops to ask. Binding it in both places means it does not matter
        // which list the reader happens to be looking at.
        .accept_also(&[Key::ShiftEnter]);

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

        let mut row = illustrated(
            Row::item(label::tab_display(&tab.tab, tab.position))
                .detail(Some(tab_contents(tab))),
            panels(tab),
            snapshot,
        );
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

    let mut menu = Menu::new("Split next to…")
        .subtitle(format!(
            "{} の、どの Pane の隣に置くかを選びます",
            label::tab_display(&tab.tab, tab.position)
        ));

    let side = config.default_move_direction.resolve().unwrap_or(Side::Right);
    // Auto lands beside whichever pane the destination tab has focused, and
    // failing that its first — the same pane Herdr would split.
    let automatic = tab.split_anchor();
    menu.item(
        Row::item("Auto")
            .hotkey("a")
            .secondary("その Tab のフォーカス中の Pane を使う")
            .panels(match automatic {
                Some(pane) => split_beside(tab, &pane.pane_id, &snapshot.source, side),
                None => Vec::new(),
            })
            .legend(legend(
                &match automatic {
                    Some(pane) => split_beside(tab, &pane.pane_id, &snapshot.source, side),
                    None => Vec::new(),
                },
                snapshot,
            )),
        None as Option<String>,
    );
    menu.row(Row::separator());
    for (index, pane) in tab.panes.iter().enumerate() {
        let mut row = illustrated(
            pane_row(pane, config),
            split_beside(tab, &pane.pane_id, &snapshot.source, side),
            snapshot,
        );
        if index < 9 {
            row = row.hotkey((index + 1).to_string());
        }
        menu.item(row, Some(pane.pane_id.clone()));
    }

    menu.run(term)
}


fn ask_placement(
    term: &mut Term,
    config: &Config,
    context: &str,
    preview: Option<(&Shape, &str)>,
) -> Result<Option<Placement>> {
    let side = match config.default_move_direction.resolve() {
        Some(side) => side,
        None => {
            let mut menu = Menu::new("Which side?")
                .subtitle(format!("{context} の、どちら側に置きますか"));
            for side in Side::ALL {
                let mut row =
                    Row::item(capitalize(side.as_str())).hotkey(side.hotkey().to_string());
                if let Some((shape, target)) = preview {
                    let (after, marked) = placement_preview(shape, target, side);
                    row = row.preview(after, marked);
                }
                menu.item(row, side);
            }
            match menu.run(term)? {
                Some(side) => side,
                None => return Ok(None),
            }
        }
    };

    let ratio = if config.ask_ratio() {
        let mut menu = Menu::new("How much space?")
            .subtitle(format!("{context} の空間を、どう分けますか"));
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


/// The specification table, executable.
///
/// One row per line: a session, an operation, and the arrangement the
/// destination tab is left in. Both the plan and the preview are held to that
/// same answer, which is the check that was missing while the picture and the
/// operation drifted apart.

#[cfg(test)]
#[path = "spec_tests.rs"]
mod spec_tests;
