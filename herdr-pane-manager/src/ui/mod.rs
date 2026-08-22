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
use herdr_plugin_kit::layout::{Plan as LayoutPlan, Ratio, Shape, Side};
use herdr_plugin_kit::ui::{Key, Menu, Panel, Row, Term};
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
fn manager_footer(term: &Term, config: &Config) -> String {
    let (plain, shifted) = match config.default_action {
        crate::config::DefaultAction::Quick => ("すぐ移動", "位置を指定"),
        crate::config::DefaultAction::Detailed => ("位置を指定", "すぐ移動"),
    };
    // Shift+letter needs no keyboard protocol; Shift+Enter does. Only promise
    // the one that will actually arrive.
    let shift_key = if term.distinguishes_modified_enter() {
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
    let mut menu = Menu::new("Pane Manager")
        .subtitle(source_line(snapshot, config))
        // The keys live here rather than beside the rows they apply to: a hint
        // repeated on every section is clutter, and one at the bottom is where
        // a reader looks for keys anyway.
        .footer(manager_footer(term, config))
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

    // Each row names its default target. A key that acts immediately has to
    // say what it will do before it is pressed, or it is a trap.
    let next_tab = snapshot
        .next_tab()
        .map(|t| label::tab_name(&t.tab).unwrap_or_else(|| "Tab".into()));
    // Resolved once: the row's wording, the picture beside it and the key all
    // have to name the same pane. Asking Herdr for the neighbour in one place
    // and walking the pane list in another put a different pane in the
    // sentence than the one `s` would actually trade with.
    let partner = swap_partner(herdr, snapshot, config)
        .and_then(|id| snapshot.pane(&id).cloned());
    let next_pane = partner.as_ref().map(label::pane_compact);
    let target = |name: &Option<String>, verb: &str, pick: &str| match name {
        Some(name) => format!("{name} {verb}"),
        None => pick.to_string(),
    };

    menu.item(
        illustrated(
            Row::item("Move to…").hotkey("m").secondary(target(
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
            Row::item("Swap with…")
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
            Row::item("Extract…")
                .hotkey("e")
                .secondary("現在の Pane を新しい Tab へ切り出す"),
            operation_preview(&Choice::Extract, snapshot, config),
            snapshot,
        ),
        Choice::Extract,
    );
    menu.item(
        illustrated(
            Row::item("Fold into…").hotkey("f").secondary(target(
                &next_tab,
                "へこの Tab 全体を畳む",
                "別 Tab がないため、畳む先を選ぶ",
            )),
            operation_preview(&Choice::Merge, snapshot, config),
            snapshot,
        ),
        Choice::Merge,
    );

    // Undo sits right under the operations it reverses, and only appears when
    // there is actually something to take back.
    if let Some(record) = undo::load() {
        menu.row(Row::separator());
        menu.item(
            Row::item("Undo")
                .hotkey("u")
                .secondary(format!("{} を取り消す", record.describe()))
                .panels(undo_panels(&record, &snapshot.source.pane_id)),
            Choice::Undo,
        );
    }

    menu.row(Row::separator());
    // Gather is listed with the operations, but it acts on the whole session
    // rather than on the current pane (addendum §9).
    let gathered = gather::session::load();
    // Gather collects by *status*, so the number of panes on screen is not the
    // number it will take: an idle agent is not gathered. The row has to say
    // which, or a picture with one box in it while two agents are running
    // reads as a bug rather than as the answer.
    //
    // Counted from this snapshot, which covers the current workspace. With
    // `scope = "all"` that is not the whole story, and the row goes back to
    // promising nothing rather than naming a number it cannot stand behind.
    let local_scope = config.gather.scope() == Scope::CurrentWorkspace;
    let ready = gatherable_here(snapshot, config);
    let (collecting, gather_note) = match (&gathered, local_scope) {
        (Some(existing), _) => (
            existing.origins.len(),
            format!("refresh · {} gathered", existing.origins.len()),
        ),
        (None, true) if ready.is_empty() => (
            0,
            format!("いま対象の Agent はいません · {}", config.gather.status_summary()),
        ),
        (None, true) => (
            0,
            format!("{} 個を集めます · {}", ready.len(), config.gather.status_summary()),
        ),
        (None, false) => (0, "選択すると対象を確認します".to_string()),
    };

    menu.item(
        Row::item("Gather Active Agents")
            .hotkey("g")
            .secondary(gather_note)
            .panels(gather_panels(collecting, &ready, config)),
        Choice::Gather,
    );
    if gathered.is_some() {
        menu.item(
            Row::item("Restore Gathered Agents")
                .hotkey("r")
                // Named for what it undoes, because `Undo` sits four rows up
                // and the two take back different things: this one only ever
                // reverses a Gather, and only Gather.
                .secondary(format!("Gather した {collecting} 個を元の Tab へ"))
                .panels(restore_panels(collecting, config)),
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
        return Ok(Step::Close);
    };

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
        Choice::Gather => gather_flow(term, herdr, config, &gatherable_here(snapshot, config)),
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
        Choice::Swap => match (detailed, swap_partner(herdr, snapshot, config)) {
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
        PerTab(u8),
        Scope(Scope),
    }

    let default_scope = config.gather.scope();
    let mut menu = Menu::new("Gather Active Agents")
        .subtitle(format!(
            "対応が必要な Agent を1つの Tab へ集めます · {} · {}",
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
                })
                .panels(gather_size_panels(size, names, config)),
            Pick::PerTab(size as u8),
        );
    }

    menu.row(Row::separator());
    menu.row(Row::header("Scope"));
    for scope in [Scope::CurrentWorkspace, Scope::AllWorkspaces] {
        menu.item(
            Row::item(scope.label())
                .hotkey(if scope == Scope::CurrentWorkspace { "w" } else { "a" })
                .secondary(if scope == default_scope { "default" } else { "" })
                // The scope changes which agents are collected, not how they
                // are arranged; the picture is the configured size either way,
                // and keeping one there stops the area blinking between rows.
                .panels(gather_size_panels(config.gather.per_tab().get(), names, config)),
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

/// The name of the tab the reader is in.
fn here_name(snapshot: &Snapshot) -> String {
    snapshot
        .source_tab()
        .map(tab_number)
        .unwrap_or_else(|| "この Tab".into())
}

/// The source tab as it is right now, with the pane that is about to move
/// filled in.
///
/// The left half of a pair reads as "before" — there is an arrow between the
/// two — so it has to show the layout the reader is looking at, not a
/// half-applied version of the operation. Which pane leaves is said by the
/// fill; where it lands is the panel on the right.
fn here_now(snapshot: &Snapshot) -> Panel {
    let caption = here_name(snapshot);
    let Some(here) = snapshot.source_tab().and_then(TabEntry::layout) else {
        return Panel::unreadable(caption);
    };
    let labels: Vec<(String, String)> = snapshot
        .source_tab()
        .map(|tab| {
            tab.panes
                .iter()
                .map(|p| (p.pane_id.clone(), pane_number(p)))
                .collect()
        })
        .unwrap_or_default();
    Panel::new(caption, here)
        .marking(vec![snapshot.source.pane_id.clone()])
        .labeling(labels)
}

/// A stand-in for a destination not chosen yet: one pane, with the moved one
/// arriving beside it on the configured side.
fn arriving_panel(caption: &str, config: &Config) -> Panel {
    let mut shape = Shape::pane(ELSEWHERE);
    let side = config.default_move_direction.resolve().unwrap_or(Side::Right);
    shape.split(ELSEWHERE, ARRIVING, side);
    Panel::new(caption, shape).marking(vec![ARRIVING.to_string()])
}

/// The pictures for each operation in the menu: what the session looks like
/// *afterwards*, not what it looks like now.
///
/// Drawn as a pair wherever a pane crosses between tabs, because that is the
/// part a single diagram cannot show — the previous version drew the current
/// tab for Move, Swap and Extract alike, so three different operations were
/// illustrated by the same unchanging picture.
fn operation_preview(choice: &Choice, snapshot: &Snapshot, config: &Config) -> Vec<Panel> {
    let source = snapshot.source.pane_id.clone();
    match choice {
        Choice::Move => vec![
            here_now(snapshot),
            match snapshot.next_tab() {
                Some(tab) => real_destination(tab, snapshot, config),
                // With no existing destination, plain Move creates a tab.
                // Preview the same concrete result that `m` will make.
                None => Panel::new(
                    config
                        .new_tab_label(&snapshot.source, None)
                        .unwrap_or_else(|| "新しい Tab".into()),
                    Shape::pane(&source),
                )
                .marking(vec![source.clone()])
                .labeling(vec![(source.clone(), pane_number(&snapshot.source))])
                .behind(here_name(snapshot)),
            },
        ],
        Choice::Extract => vec![
            here_now(snapshot),
            // A tab that does not exist yet has no number, so the caption
            // carries the name it will be given instead — which is the part
            // the reader will actually recognise on the tab bar afterwards.
            Panel::new(
                config
                    .new_tab_label(&snapshot.source, None)
                    .unwrap_or_else(|| "新しい Tab".into()),
                Shape::pane(&source),
            )
            .marking(vec![source.clone()])
            .labeling(vec![(source.clone(), pane_number(&snapshot.source))])
            // A tab created from a project directory takes its name, so both
            // captions read the same word. The sheet behind is the tab it is
            // being cut out of, named, which says "a different tab" where the
            // name on its own cannot.
            .behind(here_name(snapshot)),
        ],
        // Swap changes neither tab's shape — the two panes trade places. The
        // picture has to show an exchange, so the same two boxes appear on
        // both sides with the names moved across; a single highlighted box
        // would read as a move, which is the wrong operation.
        // Swap is drawn by `swap_panels`, which needs the pane actually being
        // traded with. The menu resolves that once and calls it directly; this
        // arm covers the callers that have not, and falls back to the next
        // pane in the tab.
        Choice::Swap => match snapshot.next_pane_here() {
            Some(partner) => swap_panels(snapshot, &partner.clone()),
            None => Vec::new(),
        },
        // Fold empties this tab into another one.
        // Fold moves *every* pane of this tab, so both sides have to show all
        // of them. An empty box on the left said only "something closes",
        // which is the least interesting part of the operation.
        Choice::Merge => {
            let Some(here) = snapshot.source_tab() else {
                return Vec::new();
            };
            let moving: Vec<String> = here.panes.iter().map(|p| p.pane_id.clone()).collect();
            let labels: Vec<(String, String)> = here
                .panes
                .iter()
                .map(|p| (p.pane_id.clone(), pane_number(p)))
                .collect();
            let shape = here
                .shape
                .clone()
                .unwrap_or_else(|| Shape::pane(&snapshot.source.pane_id));

            // Fold takes the whole tab, but the fill answers a narrower
            // question: where does the pane I am sitting in end up? Painting
            // every pane blue makes the reader hunt for their own.
            let left = Panel::new(tab_number(here), shape)
                .marking(vec![source.clone()])
                .labeling(labels.clone());

            match snapshot.next_tab() {
                Some(tab) => vec![
                    left,
                    folded_into(tab, &moving, &labels, here.shape.as_ref(), config)
                        .marking(vec![source.clone()]),
                ],
                None => vec![left],
            }
        }
        _ => Vec::new(),
    }
}

/// The shape of one Gather tab holding `panes` agents.
///
/// Built from the real planner rather than a hand-drawn approximation, so the
/// picture cannot drift away from what Gather actually produces.
fn gathered_shape(panes: usize) -> Option<(Shape, Vec<String>)> {
    let ids: Vec<String> = (0..panes.max(1)).map(|i| format!("{ARRIVING}{i}")).collect();
    let plan = gather::layout::plan(&ids)?;
    Some((plan.simulate(), ids))
}

/// How many tabs a Gather of `panes` agents would fill, for the caption.
fn gather_caption(panes: usize, config: &Config, label: &str) -> String {
    let per_tab = config.gather.per_tab().get();
    let tabs = panes.div_ceil(per_tab.max(1));
    if tabs > 1 {
        format!("{label} ×{tabs}")
    } else {
        label.to_string()
    }
}

/// How many agents a Gather would collect, counted from the snapshot the menu
/// already holds.
///
/// An estimate: `scope = "all"` reaches workspaces this snapshot never read.
/// It costs nothing — the panes are already here, and the status and kind
/// filters are the same ones `gather::select` applies — and drawing a full
/// four-pane tab when the reader can see two agents on screen was worse than
/// being approximately right.
fn gatherable_here(snapshot: &Snapshot, config: &Config) -> Vec<String> {
    let gather = &config.gather;
    let mut agents: Vec<&Pane> = snapshot
        .tabs
        .iter()
        .flat_map(|tab| tab.panes.iter())
        .filter(|pane| {
            let Some(kind) = pane.agent.as_deref() else {
                return false;
            };
            gather.agents.is_empty() || gather.agents.iter().any(|a| a.eq_ignore_ascii_case(kind))
        })
        .collect();

    // Only the agents a Gather would really take. Padding the list out with
    // idle ones when nothing is busy draws a tab Gather will not make; the row
    // beside the picture says how many there are, which is the honest way to
    // explain a single box while two agents are running.
    agents.retain(|pane| gather.statuses.iter().any(|s| *s == pane.agent_status));

    // Priority order, the same rule `gather::select` sorts by, so the box a
    // name lands in is the box that pane will land in.
    agents.sort_by(|a, b| {
        a.agent_status
            .priority()
            .cmp(&b.agent_status.priority())
            .then(a.pane_id.cmp(&b.pane_id))
    });
    agents.iter().map(|pane| pane_number(pane)).collect()
}

/// One Gather tab drawn at an explicit size, for the rows that choose it.
fn gather_size_panels(size: usize, names: &[String], config: &Config) -> Vec<Panel> {
    let Some((shape, ids)) = gathered_shape(size) else {
        return Vec::new();
    };
    vec![Panel::new(config.gather.tab_label.clone(), shape).labeling(named(&ids, names))]
}

/// Put the agents' own pane names into the boxes, in priority order.
///
/// A Gather tab drawn as empty rectangles says how many panes there will be
/// and nothing about which. The names are the only part that answers "is that
/// the agent I mean?", and they are the same `p5`-style ids every other
/// preview uses.
fn named(slots: &[String], names: &[String]) -> Vec<(String, String)> {
    slots
        .iter()
        .zip(names)
        .map(|(slot, name)| (slot.clone(), name.clone()))
        .collect()
}

fn gather_panels(panes: usize, names: &[String], config: &Config) -> Vec<Panel> {
    // Only the first tab is drawn; the caption carries the rest.
    let per_tab = config.gather.per_tab().get();
    // No Gather session yet, so the count comes from the panes on screen: a
    // tab drawn in four when only two agents are running is a picture of
    // something that will not happen. `open` is an estimate, and zero means
    // even that failed — then a full tab is the only honest sketch left.
    let panes = match (panes, names.len()) {
        // Neither a gathered session nor a countable agent: the scope reaches
        // past this snapshot, so the picture falls back to the shape of a full
        // tab — how the panes will be arranged, which is what a diagram is for.
        (0, 0) => per_tab,
        (0, open) => open,
        (panes, _) => panes,
    };
    let Some((shape, ids)) = gathered_shape(panes.min(per_tab)) else {
        return Vec::new();
    };
    // One panel: the tab that will exist afterwards, under its real name. The
    // panes come from several tabs at once, so the left-hand side has no one
    // name to carry.
    // Nothing is filled: the fill means "this is where you end up", and a
    // Gather collects agents wherever they are — the pane the reader is
    // sitting in is usually not one of them.
    // More than one tab's worth, so the picture is two sheets — and now that
    // the names live in the frames, both can say what they are. One tab stays
    // one sheet: stacking it would promise a second tab that never appears.
    let tabs = panes.div_ceil(per_tab.max(1));
    let panel = Panel::new(gather_caption(panes, config, &config.gather.tab_label), shape)
        .labeling(named(&ids, names));
    vec![if tabs > 1 {
        panel.behind(config.gather.tab_label.clone())
    } else {
        panel
    }]
}

/// Where undoing would put the panes back.
///
/// Drawn from the record's own origins, so the caption names the tab they
/// return to and the boxes carry the panes that return. A Swap has no origins
/// — it is its own inverse — so that one keeps the wordless empty preview.
fn undo_panels(record: &undo::Record, active: &str) -> Vec<Panel> {
    if record.origins.is_empty() {
        return Vec::new();
    }
    let ids: Vec<String> = record
        .origins
        .iter()
        .map(|origin| origin.pane_id.clone())
        .collect();
    let plan = crate::gather::layout::plan(&ids);
    let Some(shape) = plan.map(|plan| plan.simulate()) else {
        return Vec::new();
    };

    let mut homes: Vec<String> = record
        .origins
        .iter()
        .filter_map(|origin| origin.tab_label.clone())
        .collect();
    homes.dedup();
    let caption = match homes.len() {
        0 => "元の Tab".to_string(),
        1..=2 => homes.join(" · "),
        _ => format!("{} +{}", homes[..2].join(" · "), homes.len() - 2),
    };

    let labels: Vec<(String, String)> = record
        .origins
        .iter()
        .map(|origin| {
            let short = origin
                .pane_id
                .split_once(':')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_else(|| origin.pane_id.clone());
            (origin.pane_id.clone(), short)
        })
        .collect();

    // Filled only when the reader's own pane is one of the ones going back.
    // An Undo of somebody else's Fold moves panes, but not this one.
    let marked = ids
        .iter()
        .filter(|id| *id == active)
        .cloned()
        .collect::<Vec<_>>();

    vec![Panel::new(caption, shape)
        .marking(marked)
        .labeling(labels)]
}

fn restore_panels(panes: usize, config: &Config) -> Vec<Panel> {
    let per_tab = config.gather.per_tab().get();
    let Some((shape, ids)) = gathered_shape(panes.min(per_tab)) else {
        return Vec::new();
    };
    let Some(session) = gather::session::load() else {
        return Vec::new();
    };

    // The gathered panes are recorded, so these are the real names rather than
    // a guess from the current workspace.
    let names: Vec<String> = session
        .origins
        .iter()
        .map(|origin| short_pane_id(&origin.pane_id))
        .collect();

    let mut home: Vec<String> = session
        .origins
        .iter()
        .filter_map(|origin| origin.tab_label.clone())
        .collect();
    home.dedup();
    let back = match home.len() {
        0 => "元の Tab".to_string(),
        1..=2 => home.join(" · "),
        _ => format!("{} +{}", home[..2].join(" · "), home.len() - 2),
    };

    // Left is where the panes are now, right is where they go. The dashed
    // "will not exist" box used to sit on the right, which said the tab the
    // panes are going *back* to disappears — the exact opposite of what
    // Restore does. The tab that closes is the Active Agents one on the left,
    // and its panes leaving is what says so.
    let mut panels = vec![Panel::new(
        gather_caption(panes, config, &config.gather.tab_label),
        shape,
    )
    .labeling(named(&ids, &names))];

    let returning: Vec<String> = session
        .origins
        .iter()
        .map(|origin| origin.pane_id.clone())
        .collect();
    if let Some(home_shape) = gather::layout::plan(&returning).map(|plan| plan.simulate()) {
        panels.push(
            Panel::new(back, home_shape).labeling(
                returning
                    .iter()
                    .map(|id| (id.clone(), short_pane_id(id)))
                    .collect(),
            ),
        );
    }
    panels
}

/// Herdr's own short name for a pane: `w2N:p5` reads as `p5`.
fn short_pane_id(pane_id: &str) -> String {
    pane_id
        .split_once(':')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_else(|| pane_id.to_string())
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

/// Herdr's own name for a pane — the `p5` half of `w2N:p5`.
///
/// Used inside diagrams instead of the agent name, for two reasons. It is
/// unique: two panes both running Codex are told apart by `p2` and `p5`, where
/// two boxes both saying "codex" are not. And it is ASCII, so its width in
/// columns equals its length in characters — a Japanese label is half as many
/// characters as it is columns wide, and a diagram drawn from character counts
/// puts the walls in the wrong place.
fn pane_number(pane: &Pane) -> String {
    pane.pane_id
        .split_once(':')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_else(|| pane.pane_id.clone())
}

/// The destination tab, captioned with its Herdr number and named inside.
///
/// Labels rather than plain boxes because "which pane goes where" is the
/// question, and a diagram of anonymous rectangles cannot answer it. The words
/// stay short — one per box — so the picture is still a picture.
fn destination_panels(tab: &TabEntry, arriving: Option<Side>, source: Option<&Pane>) -> Vec<Panel> {
    let id = source.map(|p| p.pane_id.as_str()).unwrap_or(ARRIVING);
    let Some((shape, marked)) = tab_preview_of(tab, arriving, id) else {
        return vec![Panel::unreadable(tab_number(tab))];
    };
    let mut labels: Vec<(String, String)> = tab
        .panes
        .iter()
        .map(|pane| (pane.pane_id.clone(), pane_number(pane)))
        .collect();
    if let Some(source) = source {
        labels.push((source.pane_id.clone(), pane_number(source)));
    }
    vec![
        Panel::new(tab_number(tab), shape)
            .marking(marked)
            .labeling(labels),
    ]
}

/// What the tab bar calls this tab.
///
/// Its name, or its position when it has none — the two things Herdr actually
/// prints across the top of the screen. The Herdr id (`tM`, `tN`) was tried
/// here and dropped: it is base36 and internal, so `t1` reads like a tab
/// number while the real values are letters the reader has never seen. The
/// pane labels stay as `p5`, because inside a box there is no shorter true
/// name and the CLI speaks them.
///
/// A tab that does not exist yet is captioned by name alone; the sheet behind
/// it names the tab it comes from, which is what tells two identical project
/// names apart.
fn tab_number(tab: &TabEntry) -> String {
    match tab.tab.label.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => tab.position.to_string(),
    }
}

/// The destination tab with every folded pane added to it.
fn folded_into(
    tab: &TabEntry,
    moving: &[String],
    labels: &[(String, String)],
    source_shape: Option<&Shape>,
    config: &Config,
) -> Panel {
    let side = config.default_move_direction.resolve().unwrap_or(Side::Right);
    let Some(anchor) = tab.split_anchor().map(|p| p.pane_id.clone()) else {
        return arriving_panel(&tab_number(tab), config);
    };
    let Some(mut shape) = tab.layout() else {
        return Panel::unreadable(tab_number(tab));
    };

    // The real Merge lands the source tab's first pane beside the destination
    // and then replays the source tab's *own* splits between the rest, so a
    // stacked pair arrives stacked. Chaining them all along one side instead
    // drew three columns for a layout that will not have three columns, which
    // is a picture of the wrong thing.
    let plan = source_shape.map(LayoutPlan::from_shape);
    let order: Vec<String> = match &plan {
        Some(plan) => plan.pane_ids(),
        None => moving.to_vec(),
    };
    match crate::ops::three_pane_column(tab.panes.len(), &order).or_else(|| {
        plan.as_ref().map(|plan| plan.placements.clone())
    }) {
        Some(placements) => {
            if let Some(first) = order.first() {
                shape.split(&anchor, first, side);
            }
            for placement in &placements {
                shape.split(&placement.anchor, &placement.pane_id, placement.side);
            }
        }
        None => {
            let mut at = anchor;
            for pane in moving {
                shape.split(&at, pane, side);
                at = pane.clone();
            }
        }
    }

    let mut all: Vec<(String, String)> = tab
        .panes
        .iter()
        .map(|p| (p.pane_id.clone(), pane_number(p)))
        .collect();
    all.extend(labels.iter().cloned());

    Panel::new(tab_number(tab), shape)
        .marking(moving.to_vec())
        .labeling(all)
}

/// The destination tab as it would look with the pane in it, named.
fn real_destination(tab: &TabEntry, snapshot: &Snapshot, config: &Config) -> Panel {
    let side = config.default_move_direction.resolve().unwrap_or(Side::Right);
    match destination_panels(tab, Some(side), Some(&snapshot.source)).pop() {
        Some(panel) => panel,
        None => arriving_panel(&tab_number(tab), config),
    }
}

/// Stand-in id for a pane that is already in the destination.
const ELSEWHERE: &str = "\u{1}elsewhere";

/// The destination tab, and the pane that would arrive in it.
///
/// When `arriving` is set the shape is the tab **after** the move. The side is
/// already decided by then — it comes from the settings, not from a later
/// question — so there is nothing speculative about it.
fn tab_preview(tab: &TabEntry, arriving: Option<Side>) -> Option<(Shape, Vec<String>)> {
    tab_preview_of(tab, arriving, ARRIVING)
}

/// The same, for a pane whose real id is known.
///
/// Using the real id rather than the stand-in is what lets a test hold the
/// picture and the operation to the same answer: both then describe the tab in
/// Herdr's own names, and two signatures can simply be compared.
fn tab_preview_of(
    tab: &TabEntry,
    arriving: Option<Side>,
    id: &str,
) -> Option<(Shape, Vec<String>)> {
    let anchor = tab.split_anchor().map(|p| p.pane_id.clone())?;
    let mut shape = tab.layout()?;
    match arriving {
        Some(side) => {
            shape.split(&anchor, id, side);
            Some((shape, vec![id.to_string()]))
        }
        None => Some((shape, Vec::new())),
    }
}

/// Stand-in id for the pane being placed. Starts with a control character so
/// it cannot collide with a real pane id.
const ARRIVING: &str = "\u{1}arriving";

/// What is running in a destination tab.
///
/// The shape says how the tab is divided; this says what is in it, which is
/// the other half of "is this the tab I mean?".
/// `p1: Herdr pane manager… | claude` — Herdr's name for the pane, what is
/// happening in it, and which agent.
///
/// The number leads because it is what the diagram above says; the title and
/// the agent follow because they are what a person recognises.
/// Name every pane the picture mentions, under the picture.
///
/// The boxes can only carry `p5`: a conversation title is long and usually
/// CJK, and writing one inside a box pushes its walls out of true. Underneath,
/// each name has a line to itself — which is the arrangement the reader asked
/// for, a diagram with a list beside it rather than a diagram made of words.
fn legend(panels: &[Panel], snapshot: &Snapshot) -> Vec<String> {
    /// Enough to name the panes in a picture without crowding out the picture.
    const MOST: usize = 4;
    let mut seen: Vec<&str> = Vec::new();
    for panel in panels {
        for (id, _) in &panel.labels {
            if !seen.iter().any(|known| *known == id.as_str()) {
                seen.push(id);
            }
        }
    }
    seen.iter()
        .filter_map(|id| snapshot.pane(id))
        .take(MOST)
        .map(pane_line)
        .collect()
}

/// A row whose picture comes with the names of what is in it.
fn illustrated(row: Row, panels: Vec<Panel>, snapshot: &Snapshot) -> Row {
    let lines = legend(&panels, snapshot);
    row.panels(panels).legend(lines)
}

fn pane_line(pane: &Pane) -> String {
    let mut line = pane_number(pane);
    line.push(':');
    line.push(' ');
    line.push_str(&label::pane_compact(pane));
    if let Some(agent) = pane.display_agent.as_ref().or(pane.agent.as_ref()) {
        line.push_str(" | ");
        line.push_str(agent);
    }
    line
}

fn tab_contents(tab: &TabEntry) -> String {
    let names: Vec<String> = tab
        .panes
        .iter()
        .take(4)
        .map(pane_line)
        .collect();
    let mut line = names.join(" · ");
    if tab.panes.len() > names.len() {
        line.push_str(&format!(" +{}", tab.panes.len() - names.len()));
    }
    line
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

/// The destination tab with `arriving` split in beside one named pane.
///
/// The picture a "Split next to…" row stands for: not the tab as it is, but
/// the tab this row would produce. Built from the tab's real shape with the
/// real pane ids, so it is the same arrangement the move will make.
fn split_beside(tab: &TabEntry, target: &str, arriving: &Pane, side: Side) -> Vec<Panel> {
    let Some(mut shape) = tab.layout() else {
        return vec![Panel::unreadable(tab_number(tab))];
    };
    shape.split(target, &arriving.pane_id, side);

    let mut labels: Vec<(String, String)> = tab
        .panes
        .iter()
        .map(|pane| (pane.pane_id.clone(), pane_number(pane)))
        .collect();
    labels.push((arriving.pane_id.clone(), pane_number(arriving)));

    vec![Panel::new(tab_number(tab), shape)
        .marking(vec![arriving.pane_id.clone()])
        .labeling(labels)]
}

/// The tabs a Swap would leave behind.
///
/// Two panes trade places, so within one tab the shape does not change at all
/// and only the names move; across two tabs both are redrawn, because a pane
/// leaves each of them and another arrives in its place. The fill follows the
/// rule the rest of the previews use: it marks where the reader's own pane
/// ends up.
fn swap_panels(snapshot: &Snapshot, target: &Pane) -> Vec<Panel> {
    let source = &snapshot.source;
    let named = |tab: &TabEntry, swap: &(String, String)| -> Vec<(String, String)> {
        tab.panes
            .iter()
            .map(|pane| {
                let name = if pane.pane_id == swap.0 {
                    swap.1.clone()
                } else {
                    pane_number(pane)
                };
                (pane.pane_id.clone(), name)
            })
            .collect()
    };
    let shape_of = |tab: &TabEntry| -> Option<Shape> { tab.layout() };

    let Some(here) = snapshot.source_tab() else {
        return Vec::new();
    };
    let Some(there) = snapshot.tab(&target.tab_id) else {
        return Vec::new();
    };

    if here.tab.tab_id == there.tab.tab_id {
        let Some(shape) = shape_of(here) else {
            return vec![Panel::unreadable(tab_number(here))];
        };
        let mut labels = named(here, &(source.pane_id.clone(), pane_number(target)));
        for (id, name) in &mut labels {
            if *id == target.pane_id {
                *name = pane_number(source);
            }
        }
        return vec![Panel::new(tab_number(here), shape)
            .marking(vec![target.pane_id.clone()])
            .labeling(labels)];
    }

    let (Some(left), Some(right)) = (shape_of(here), shape_of(there)) else {
        return vec![
            Panel::unreadable(tab_number(here)),
            Panel::unreadable(tab_number(there)),
        ];
    };
    vec![
        // The reader's pane has left this one, so nothing here is filled.
        Panel::new(tab_number(here), left)
            .labeling(named(here, &(source.pane_id.clone(), pane_number(target)))),
        Panel::new(tab_number(there), right)
            .marking(vec![target.pane_id.clone()])
            .labeling(named(there, &(target.pane_id.clone(), pane_number(source)))),
    ]
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

/// Side and size, asked only when the settings leave them open (§4.1, §12).
/// What the destination tab would look like with the pane added on `side`.
///
/// Built by applying the split to the tab's real shape, so the preview is the
/// same computation the move itself will perform rather than a drawing that
/// merely resembles it.
fn placement_preview(shape: &Shape, target: &str, side: Side) -> (Shape, Vec<String>) {
    let mut after = shape.clone();
    after.split(target, ARRIVING, side);
    (after, vec![ARRIVING.to_string()])
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

#[cfg(test)]
mod preview_tests {
    use super::*;

    /// A Gather panel fills nothing — the reader's own pane is not one of the
    /// agents being collected — so its size is read off the diagram.
    fn panes_in(panel: &Panel) -> usize {
        panel
            .shape
            .as_ref()
            .map(|shape| shape.pane_ids().len())
            .unwrap_or(0)
    }

    fn signature(panes: usize) -> String {
        gathered_shape(panes).unwrap().0.signature()
    }

    #[test]
    fn the_gather_preview_is_drawn_by_the_real_planner() {
        // Same arrangements `gather::layout::plan` documents. Built from the
        // planner rather than sketched, so the picture cannot drift away from
        // what Gather actually produces.
        let id = |n: usize| format!("{ARRIVING}{n}");
        // Two side by side.
        assert_eq!(signature(2), format!("(r {} {})", id(0), id(1)));
        // Four as a 2x2 grid: the top-right pane is split off before either
        // column is divided, or it would only span half the tab's height.
        assert_eq!(
            signature(4),
            format!("(r (d {} {}) (d {} {}))", id(0), id(2), id(1), id(3))
        );
    }

    #[test]
    fn every_pane_in_the_preview_is_marked() {
        for panes in 1..=4 {
            let (shape, marked) = gathered_shape(panes).unwrap();
            assert_eq!(marked.len(), panes);
            assert_eq!(shape.pane_ids().len(), panes);
        }
    }

    #[test]
    fn the_caption_counts_tabs_only_when_there_is_more_than_one() {
        let mut config = Config::default();
        config.gather.max_panes_per_tab = 2;
        assert_eq!(gather_caption(2, &config, "A"), "A");
        assert_eq!(gather_caption(5, &config, "A"), "A ×3");
    }

    #[test]
    fn an_uncounted_gather_still_draws_the_shape_it_would_make() {
        // Opening this menu deliberately does not count agents — that walks
        // every workspace. A picture of the arrangement costs nothing and is
        // the part worth showing, so "not counted" must not mean "not drawn".
        let mut config = Config::default();
        config.gather.max_panes_per_tab = 4;
        let panels = gather_panels(0, &[], &config);
        assert_eq!(panels.len(), 1);
        assert_eq!(panes_in(&panels[0]), 4, "a full tab is drawn");
        assert_eq!(panels[0].caption, config.gather.tab_label);
    }

    #[test]
    fn a_gather_of_two_agents_is_drawn_as_two_panes() {
        // Four boxes when two agents are running is a picture of something
        // that will not happen. The count comes from the panes on screen.
        let config = Config::default();
        let panels = gather_panels(0, &["p1".to_string(), "p2".to_string()], &config);
        assert_eq!(panes_in(&panels[0]), 2);
        // Three keep the top agent's column full height, with the other two
        // stacked beside it.
        let panels = gather_panels(0, &["p1".to_string(), "p2".to_string(), "p3".to_string()], &config);
        assert_eq!(panes_in(&panels[0]), 3);
        assert_eq!(signature(3), {
            let id = |n: usize| format!("{ARRIVING}{n}");
            format!("(r {} (d {} {}))", id(0), id(1), id(2))
        });
    }

    #[test]
    fn a_gather_never_draws_more_than_one_tab_holds() {
        let mut config = Config::default();
        config.gather.max_panes_per_tab = 2;
        // Six agents fill three tabs; the picture shows the first one, and
        // the caption carries the rest.
        let panels = gather_panels(0, &["p1".to_string(), "p2".to_string(), "p3".to_string(), "p4".to_string(), "p5".to_string(), "p6".to_string()], &config);
        assert_eq!(panes_in(&panels[0]), 2);
        assert_eq!(panels[0].caption, format!("{} ×3", config.gather.tab_label));
    }

    fn tab_entry(tab_id: &str, label: Option<&str>, panes: &[&str], shape: Option<Shape>) -> TabEntry {
        TabEntry {
            tab: herdr_plugin_kit::herdr::Tab {
                tab_id: tab_id.to_string(),
                workspace_id: "w1".into(),
                label: label.map(str::to_string),
                pane_count: panes.len() as u32,
                focused: false,
                agent_status: Default::default(),
            },
            position: 1,
            layout_known: true,
            panes: panes
                .iter()
                .map(|id| herdr_plugin_kit::herdr::Pane {
                    pane_id: format!("w1:{id}"),
                    tab_id: tab_id.to_string(),
                    workspace_id: "w1".into(),
                    ..Default::default()
                })
                .collect(),
            shape,
        }
    }

    #[test]
    fn a_caption_reads_the_way_the_tab_bar_does() {
        // The name Herdr prints across the top, not its internal base36 id:
        // `t1` looks like a tab number, and the real ids are `tM` and `tN`.
        assert_eq!(
            tab_number(&tab_entry("w1:t1", Some("herdr-plugins"), &["p1"], None)),
            "herdr-plugins"
        );
        // No name of its own: the number the tab bar shows.
        assert_eq!(tab_number(&tab_entry("w1:t2", None, &["p1"], None)), "1");
    }

    #[test]
    fn folding_keeps_the_source_tabs_own_arrangement() {
        // Two panes stacked one above the other arrive stacked, not strung
        // out along the destination's edge.
        let mut source = Shape::pane("w1:p5");
        source.split("w1:p5", "w1:p1", Side::Down);
        let destination = tab_entry("w1:t2", None, &["pB"], None);
        let moving = vec!["w1:p5".to_string(), "w1:p1".to_string()];
        let panel = folded_into(
            &destination,
            &moving,
            &[],
            Some(&source),
            &Config::default(),
        );
        assert_eq!(
            panel.shape.unwrap().signature(),
            "(r w1:pB (d w1:p5 w1:p1))"
        );
    }

    #[test]
    fn folding_two_panes_onto_one_makes_a_column_beside_it() {
        // Three panes, whichever way the source tab was arranged: the pair
        // stacks beside the pane already there rather than becoming three
        // thin columns. Same shape Gather gives three agents.
        let mut source = Shape::pane("w1:p5");
        source.split("w1:p5", "w1:p1", Side::Right);
        let destination = tab_entry("w1:t2", None, &["pB"], None);
        let moving = vec!["w1:p5".to_string(), "w1:p1".to_string()];
        let panel = folded_into(
            &destination,
            &moving,
            &[],
            Some(&source),
            &Config::default(),
        );
        assert_eq!(
            panel.shape.unwrap().signature(),
            "(r w1:pB (d w1:p5 w1:p1))"
        );
    }

    #[test]
    fn undoing_nothing_reversible_draws_nothing() {
        // A Swap records no origins because it is its own inverse; there is
        // no "back there" to point at.
        let swap = undo::Record {
            verb: "Swap".into(),
            subject: String::new(),
            origins: Vec::new(),
            swap: Some(("w1:p1".into(), "w1:p2".into())),
            created_tabs: Vec::new(),
            gather: false,
            unix_ms: 0,
        };
        assert!(undo_panels(&swap, "w1:p1").is_empty());
    }
}

/// The specification table, executable.
///
/// One row per line: a session, an operation, and the arrangement the
/// destination tab is left in. Both the plan and the preview are held to that
/// same answer, which is the check that was missing while the picture and the
/// operation drifted apart.
#[cfg(test)]
mod spec_tests {
    use super::*;
    use crate::ops::{self, Destination, Request, Verb};
    use herdr_plugin_kit::herdr::AgentStatus;
    use crate::testkit;

    /// What the operation would really do.
    fn operation(spec: &str, verb: Verb, destination: Destination) -> String {
        let snapshot = testkit::session(spec);
        let request = Request {
            verb,
            source_pane: snapshot.source.pane_id.clone(),
            source_tab: Some(snapshot.source.tab_id.clone()),
            destination,
            placement: crate::ops::Placement::default(),
            preserve_layout: true,
        };
        let plan = ops::build(&snapshot, &request).expect("the request is valid");
        testkit::outcome(&snapshot, &plan)
    }

    /// What the picture says it would do: the last panel is where things land.
    fn preview(spec: &str, choice: Choice) -> String {
        let snapshot = testkit::session(spec);
        let panels = operation_preview(&choice, &snapshot, &Config::default());
        panels
            .last()
            .and_then(|panel| panel.shape.as_ref())
            .map(Shape::signature)
            .unwrap_or_default()
    }

    fn into_tab(id: &str) -> Destination {
        Destination::Tab {
            tab_id: testkit::pane(id),
            target_pane: None,
        }
    }

    #[test]
    fn move_puts_the_pane_beside_the_destinations_first_one() {
        let spec = "t1: p5 | p1* ; t2: pB";
        assert_eq!(
            operation(spec, Verb::Move, into_tab("t2")),
            "(r w1:pB w1:p1)"
        );
        assert_eq!(preview(spec, Choice::Move), "(r w1:pB w1:p1)");
    }

    #[test]
    fn move_into_a_split_tab_splits_the_pane_it_lands_on() {
        let spec = "t1: p1* ; t2: pB | pC";
        assert_eq!(
            operation(spec, Verb::Move, into_tab("t2")),
            "(r (r w1:pB w1:p1) w1:pC)"
        );
        assert_eq!(preview(spec, Choice::Move), "(r (r w1:pB w1:p1) w1:pC)");
    }

    #[test]
    fn extract_leaves_the_pane_alone_in_its_own_tab() {
        let spec = "t1: p5 | p1*";
        assert_eq!(
            operation(
                spec,
                Verb::Extract,
                Destination::NewTab { label: None }
            ),
            "w1:p1"
        );
        assert_eq!(preview(spec, Choice::Extract), "w1:p1");
    }

    #[test]
    fn folding_two_onto_one_stacks_the_pair_beside_it() {
        // The three-pane rule, from the spec table: whichever way the source
        // tab was arranged, three panes end up as one column and a stack.
        for spec in ["t1: p5 | p1* ; t2: pB", "t1: p5 / p1* ; t2: pB"] {
            assert_eq!(
                operation(spec, Verb::Merge, into_tab("t2")),
                "(r w1:pB (d w1:p5 w1:p1))",
                "{spec}"
            );
            assert_eq!(preview(spec, Choice::Merge), "(r w1:pB (d w1:p5 w1:p1))", "{spec}");
        }
    }

    #[test]
    fn folding_more_than_two_keeps_the_source_tabs_own_splits() {
        // Four panes is past the three-pane rule, so the source tab's
        // arrangement is carried across instead.
        let spec = "t1: p5 | (p1* / pC) ; t2: pB";
        assert_eq!(
            operation(spec, Verb::Merge, into_tab("t2")),
            "(r w1:pB (r w1:p5 (d w1:p1 w1:pC)))"
        );
        assert_eq!(
            preview(spec, Choice::Merge),
            "(r w1:pB (r w1:p5 (d w1:p1 w1:pC)))"
        );
    }

    #[test]
    fn folding_into_a_tab_that_already_has_two_panes_keeps_both_arrangements() {
        let spec = "t1: p5 / p1* ; t2: pB | pD";
        assert_eq!(
            operation(spec, Verb::Merge, into_tab("t2")),
            "(r (r w1:pB (d w1:p5 w1:p1)) w1:pD)"
        );
        assert_eq!(
            preview(spec, Choice::Merge),
            "(r (r w1:pB (d w1:p5 w1:p1)) w1:pD)"
        );
    }

    #[test]
    fn the_fill_marks_the_pane_the_reader_is_sitting_in() {
        // Fold moves a whole tab, but only one of those panes is the one the
        // reader is in, and that is the one the fill answers for.
        let snapshot = testkit::session("t1: p5 | p1* ; t2: pB");
        let panels = operation_preview(&Choice::Merge, &snapshot, &Config::default());
        for panel in &panels {
            assert_eq!(panel.marked, vec!["w1:p1".to_string()]);
        }
    }

    /// The signature of a row's picture, for the detail screens.
    fn drawn(panels: &[Panel]) -> Vec<String> {
        panels
            .iter()
            .map(|panel| {
                panel
                    .shape
                    .as_ref()
                    .map(Shape::signature)
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn split_next_to_draws_the_tab_each_row_would_make() {
        // "Split next to…" is a list of arrangements, and every row makes a
        // different one. Without a picture the rows are indistinguishable.
        let snapshot = testkit::session("t1: p1* ; t2: pB | pC");
        let tab = testkit::tab(&snapshot, "t2");
        assert_eq!(
            drawn(&split_beside(&tab, &testkit::pane("pB"), &snapshot.source, Side::Right)),
            ["(r (r w1:pB w1:p1) w1:pC)"]
        );
        assert_eq!(
            drawn(&split_beside(&tab, &testkit::pane("pC"), &snapshot.source, Side::Down)),
            ["(r w1:pB (d w1:pC w1:p1))"]
        );
    }

    #[test]
    fn split_next_to_fills_the_pane_that_arrives() {
        let snapshot = testkit::session("t1: p1* ; t2: pB | pC");
        let tab = testkit::tab(&snapshot, "t2");
        let panels = split_beside(&tab, &testkit::pane("pB"), &snapshot.source, Side::Right);
        assert_eq!(panels[0].marked, vec![testkit::pane("p1")]);
        assert_eq!(panels[0].caption, "2");
    }

    #[test]
    fn a_swap_inside_one_tab_keeps_the_shape_and_trades_the_names() {
        let snapshot = testkit::session("t1: p5 | p1*");
        let target = snapshot.pane(&testkit::pane("p5")).unwrap().clone();
        let panels = swap_panels(&snapshot, &target);
        // One tab, one picture: nothing moves between tabs.
        assert_eq!(drawn(&panels), ["(r w1:p5 w1:p1)"]);
        // p5's slot now reads p1 and p1's slot reads p5.
        assert_eq!(
            panels[0].labels,
            vec![
                (testkit::pane("p5"), "p1".to_string()),
                (testkit::pane("p1"), "p5".to_string()),
            ]
        );
        // The fill follows the reader into p5's old slot.
        assert_eq!(panels[0].marked, vec![testkit::pane("p5")]);
    }

    #[test]
    fn a_swap_across_tabs_redraws_both_of_them() {
        let snapshot = testkit::session("t1: p5 | p1* ; t2: pB");
        let target = snapshot.pane(&testkit::pane("pB")).unwrap().clone();
        let panels = swap_panels(&snapshot, &target);
        assert_eq!(drawn(&panels), ["(r w1:p5 w1:p1)", "w1:pB"]);
        assert_eq!(panels[0].caption, "1");
        assert_eq!(panels[1].caption, "2");
        // The reader's pane leaves t1, so nothing there is filled; it lands in
        // t2, and that is what the fill marks.
        assert!(panels[0].marked.is_empty());
        assert_eq!(panels[1].marked, vec![testkit::pane("pB")]);
        // t1's old slot now holds pB, and t2 holds p1.
        assert!(panels[0]
            .labels
            .contains(&(testkit::pane("p1"), "pB".to_string())));
        assert_eq!(
            panels[1].labels,
            vec![(testkit::pane("pB"), "p1".to_string())]
        );
    }

    #[test]
    fn every_gather_size_row_draws_the_size_it_offers() {
        let config = Config::default();
        let id = |n: usize| format!("{ARRIVING}{n}");
        assert_eq!(
            drawn(&gather_size_panels(2, &["p5".to_string(), "p1".to_string()], &config)),
            [format!("(r {} {})", id(0), id(1))]
        );
        assert_eq!(
            drawn(&gather_size_panels(3, &[], &config)),
            [format!("(r {} (d {} {}))", id(0), id(1), id(2))]
        );
        assert_eq!(
            drawn(&gather_size_panels(4, &[], &config)),
            [format!("(r (d {} {}) (d {} {}))", id(0), id(2), id(1), id(3))]
        );
        // Still no fill: the reader's pane is not one of the agents.
        assert!(gather_size_panels(4, &[], &config)[0].marked.is_empty());
    }

    #[test]
    fn a_fold_row_draws_the_same_arrangement_the_landing_screen_does() {
        // Two screens reach the same Fold, and they must not disagree — the
        // row in "Fold into…" and the picture on the menu behind it are the
        // same call.
        let snapshot = testkit::session("t1: p5 | p1* ; t2: pB");
        let here = snapshot.source_tab().unwrap();
        let moving: Vec<String> = here.panes.iter().map(|p| p.pane_id.clone()).collect();
        let labels: Vec<(String, String)> = here
            .panes
            .iter()
            .map(|p| (p.pane_id.clone(), pane_number(p)))
            .collect();
        let row = folded_into(
            &testkit::tab(&snapshot, "t2"),
            &moving,
            &labels,
            here.shape.as_ref(),
            &Config::default(),
        );
        assert_eq!(
            row.shape.as_ref().map(Shape::signature).unwrap(),
            preview("t1: p5 | p1* ; t2: pB", Choice::Merge)
        );
    }

    #[test]
    fn a_move_names_the_pane_it_will_split() {
        // Left and Up are a right/down split followed by a swap, and the swap
        // needs an anchor. Leaving it to Herdr meant a Left move quietly
        // landed on the right while the preview drew it on the left.
        let snapshot = testkit::session("t1: p1* ; t2: pB | pC");
        let request = Request {
            verb: Verb::Move,
            source_pane: snapshot.source.pane_id.clone(),
            source_tab: None,
            destination: into_tab("t2"),
            placement: crate::ops::Placement {
                side: Side::Left,
                ..Default::default()
            },
            preserve_layout: false,
        };
        let plan = ops::build(&snapshot, &request).unwrap();
        assert_eq!(
            plan.destination,
            Destination::Tab {
                tab_id: testkit::pane("t2"),
                // The destination tab's focused pane, or its first — the pane
                // Herdr would have picked, now written down.
                target_pane: Some(testkit::pane("pB")),
            }
        );
    }

    #[test]
    fn a_move_splits_the_focused_pane_of_the_destination() {
        let mut snapshot = testkit::session("t1: p1* ; t2: pB | pC");
        let t2 = snapshot
            .tabs
            .iter_mut()
            .find(|tab| tab.tab.tab_id == testkit::pane("t2"))
            .unwrap();
        t2.panes[1].focused = true;
        let request = Request {
            verb: Verb::Move,
            source_pane: snapshot.source.pane_id.clone(),
            source_tab: None,
            destination: into_tab("t2"),
            placement: crate::ops::Placement::default(),
            preserve_layout: false,
        };
        let plan = ops::build(&snapshot, &request).unwrap();
        assert_eq!(
            plan.destination,
            Destination::Tab {
                tab_id: testkit::pane("t2"),
                target_pane: Some(testkit::pane("pC")),
            }
        );
        // And the picture is drawn against that same pane.
        assert_eq!(
            testkit::outcome(&snapshot, &plan),
            "(r w1:pB (r w1:pC w1:p1))"
        );
        let drawn = operation_preview(&Choice::Move, &snapshot, &Config::default());
        assert_eq!(
            drawn
                .last()
                .and_then(|panel| panel.shape.as_ref())
                .map(Shape::signature)
                .unwrap(),
            "(r w1:pB (r w1:pC w1:p1))"
        );
    }

    #[test]
    fn the_legend_names_every_pane_the_picture_mentions() {
        let mut snapshot = testkit::session("t1: p5 | p1* ; t2: pB");
        snapshot.tabs[0].panes[0].agent = Some("codex".into());
        snapshot.tabs[0].panes[0].label = Some("review".into());
        snapshot.tabs[0].panes[1].agent = Some("claude".into());
        snapshot.tabs[0].panes[1].label = Some("pane manager".into());

        let panels = operation_preview(&Choice::Merge, &snapshot, &Config::default());
        let lines = legend(&panels, &snapshot);
        // Every pane once, in the order the picture introduces it, with the
        // name a person recognises rather than the id alone.
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("p5: "));
        assert!(lines[0].contains("review"));
        assert!(lines[0].ends_with("| codex"));
        assert!(lines[1].starts_with("p1: "));
        assert!(lines[1].ends_with("| claude"));
        // pB has no agent, so it is named without one.
        assert!(lines[2].starts_with("pB: "));
        assert!(!lines[2].contains('|'));
    }

    #[test]
    fn the_legend_skips_panes_that_are_only_placeholders() {
        // A Gather draws slots, not panes that exist yet; there is nothing to
        // look up and nothing to say.
        let snapshot = testkit::session("t1: p1*");
        let panels = gather_panels(0, &["p1".into()], &Config::default());
        assert!(legend(&panels, &snapshot).is_empty());
    }

    #[test]
    fn a_tab_of_ten_panes_neither_crashes_nor_overflows() {
        // Ten panes in a preview a few rows tall: the rasteriser drops some of
        // them, and the label placement used to underflow on a pane with no
        // cells at all. The picture says the count instead, and every line
        // stays inside the pane.
        let spec = "t1 herdr-plugins: p0 | p1* | p2 / p3 | p4 / p5 | p6 / p7 | p8 / p9 ; t2: pB";
        let mut snapshot = testkit::session(spec);
        for (n, pane) in snapshot.tabs[0].panes.iter_mut().enumerate() {
            pane.agent = Some("codex".into());
            pane.label = Some(format!("会話タイトル {n} のとても長い名前"));
        }
        for choice in [Choice::Move, Choice::Extract, Choice::Merge, Choice::Swap] {
            let panels = operation_preview(&choice, &snapshot, &Config::default());
            let mut preview = herdr_plugin_kit::ui::Preview::new(panels.clone());
            preview.legend = legend(&panels, &snapshot);
            for room in 5..20 {
                let lines = herdr_plugin_kit::ui::preview_lines_for_test(&preview, room, 90);
                assert!(lines.len() <= room, "{choice:?} at {room}: {}", lines.len());
            }
        }
    }

    #[test]
    fn a_tab_whose_layout_could_not_be_read_is_not_guessed_at() {
        // The old fallback drew one box for a tab of any size whenever the
        // layout call failed, so a three-pane tab was pictured as one pane.
        let mut snapshot = testkit::session("t1: p5 | p1* ; t2: pB | pC");
        for tab in &mut snapshot.tabs {
            tab.shape = None;
            tab.layout_known = false;
        }
        let panels = operation_preview(&Choice::Move, &snapshot, &Config::default());
        assert!(panels.iter().all(|panel| panel.unreadable));
        assert!(panels.iter().all(|panel| panel.shape.is_none()));
        assert_eq!(panels[0].caption, "1");
    }

    #[test]
    fn a_tab_of_one_pane_is_known_rather_than_unreadable() {
        // Herdr reports no split tree for a single pane, and that is a
        // complete answer — not a failure to answer.
        let snapshot = testkit::session("t1: p1* ; t2: pB");
        let panels = operation_preview(&Choice::Move, &snapshot, &Config::default());
        assert!(panels.iter().all(|panel| !panel.unreadable));
        assert_eq!(drawn(&panels), ["w1:p1", "(r w1:pB w1:p1)"]);
    }

    #[test]
    fn a_new_tab_is_drawn_in_front_of_the_one_it_comes_from() {
        let snapshot = testkit::session("t1 herdr-plugins: p5 | p1*");
        let panels = operation_preview(&Choice::Extract, &snapshot, &Config::default());
        let new_tab = panels.last().unwrap();
        assert!(new_tab.stacked);
        // The sheet behind carries the tab being cut from, so two frames with
        // the same project name are still told apart.
        assert_eq!(new_tab.behind.as_deref(), Some("herdr-plugins"));
    }

    #[test]
    fn a_gather_that_fills_two_tabs_is_drawn_as_two_sheets() {
        let mut config = Config::default();
        config.gather.max_panes_per_tab = 2;
        // Two agents: one tab, one sheet.
        assert!(!gather_panels(0, &["p1".into(), "p5".into()], &config)[0].stacked);
        // Five: three tabs, so the sheet behind is a tab that will exist.
        let many: Vec<String> = (1..=5).map(|n| format!("p{n}")).collect();
        let panel = &gather_panels(0, &many, &config)[0];
        assert!(panel.stacked);
        assert_eq!(panel.behind.as_deref(), Some(config.gather.tab_label.as_str()));
    }

    #[test]
    fn a_gather_counts_every_pane_with_an_agent_in_it() {
        // Two agents on screen, one of them idle: both are collected. Idle is
        // what Claude and Codex report while they wait for you, and dropping
        // it made the count flicker between one and two as the agents worked.
        let mut snapshot = testkit::session("t1: p1* | p5");
        snapshot.tabs[0].panes[0].agent = Some("claude".into());
        snapshot.tabs[0].panes[0].agent_status = AgentStatus::Working;
        snapshot.tabs[0].panes[1].agent = Some("codex".into());
        snapshot.tabs[0].panes[1].agent_status = AgentStatus::Idle;
        assert_eq!(gatherable_here(&snapshot, &Config::default()), ["p1", "p5"]);

        // A pane with no agent at all is never a candidate.
        snapshot.tabs[0].panes[1].agent = None;
        assert_eq!(gatherable_here(&snapshot, &Config::default()), ["p1"]);
    }

    #[test]
    fn a_gather_writes_the_agents_own_names_into_the_boxes() {
        // Empty rectangles say how many panes there will be and nothing about
        // which. The names are the part that answers "is that the agent I
        // mean?".
        let config = Config::default();
        let names = vec!["p5".to_string(), "p1".to_string()];
        let panels = gather_panels(0, &names, &config);
        assert_eq!(
            panels[0]
                .labels
                .iter()
                .map(|(_, name)| name.clone())
                .collect::<Vec<_>>(),
            names
        );
        // Boxes past the last known agent stay blank rather than inventing a
        // name for them.
        let panels = gather_size_panels(4, &names, &config);
        assert_eq!(panels[0].labels.len(), 2);
    }

    #[test]
    fn a_gather_fills_nothing() {
        // The reader's pane is not one of the agents being collected.
        let panels = gather_panels(0, &[], &Config::default());
        assert!(panels.iter().all(|panel| panel.marked.is_empty()));
    }
}

