//! The pictures: what each operation would leave behind, drawn.
//!
//! Kept apart from the screens that show them. Every preview bug this plugin
//! has had was a screen drawing its own version of an operation, so there is
//! one place per operation here and the screens call into it. `spec_tests` in
//! the parent module holds the plan and the picture to the same answer.

use herdr_plugin_kit::herdr::Pane;
use herdr_plugin_kit::label;
use herdr_plugin_kit::layout::{Plan as LayoutPlan, Shape, Side};
use herdr_plugin_kit::ui::{Panel, Row};

use super::Choice;
use crate::config::Config;
use crate::gather;
use crate::state::{Snapshot, TabEntry};
use crate::undo;

/// The name of the tab the reader is in.
pub(super) fn here_name(snapshot: &Snapshot) -> String {
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
pub(super) fn here_now(snapshot: &Snapshot) -> Panel {
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
pub(super) fn arriving_panel(caption: &str, config: &Config) -> Panel {
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
pub(super) fn operation_preview(choice: &Choice, snapshot: &Snapshot, config: &Config) -> Vec<Panel> {
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
pub(super) fn gathered_shape(panes: usize) -> Option<(Shape, Vec<String>)> {
    let ids: Vec<String> = (0..panes.max(1)).map(|i| format!("{ARRIVING}{i}")).collect();
    let plan = gather::layout::plan(&ids)?;
    Some((plan.simulate(), ids))
}

/// How many tabs a Gather of `panes` agents would fill, for the caption.
pub(super) fn gather_caption(panes: usize, config: &Config, label: &str) -> String {
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
pub(super) fn gatherable_here(snapshot: &Snapshot, config: &Config) -> Vec<String> {
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
pub(super) fn gather_size_panels(size: usize, names: &[String], config: &Config) -> Vec<Panel> {
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
pub(super) fn named(slots: &[String], names: &[String]) -> Vec<(String, String)> {
    slots
        .iter()
        .zip(names)
        .map(|(slot, name)| (slot.clone(), name.clone()))
        .collect()
}

pub(super) fn gather_panels(panes: usize, names: &[String], config: &Config) -> Vec<Panel> {
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
pub(super) fn undo_panels(record: &undo::Record, active: &str) -> Vec<Panel> {
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

pub(super) fn restore_panels(panes: usize, config: &Config) -> Vec<Panel> {
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
pub(super) fn short_pane_id(pane_id: &str) -> String {
    pane_id
        .split_once(':')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_else(|| pane_id.to_string())
}

/// Herdr's own name for a pane — the `p5` half of `w2N:p5`.
///
/// Used inside diagrams instead of the agent name, for two reasons. It is
/// unique: two panes both running Codex are told apart by `p2` and `p5`, where
/// two boxes both saying "codex" are not. And it is ASCII, so its width in
/// columns equals its length in characters — a Japanese label is half as many
/// characters as it is columns wide, and a diagram drawn from character counts
/// puts the walls in the wrong place.
pub(super) fn pane_number(pane: &Pane) -> String {
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
pub(super) fn destination_panels(tab: &TabEntry, arriving: Option<Side>, source: Option<&Pane>) -> Vec<Panel> {
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
pub(super) fn tab_number(tab: &TabEntry) -> String {
    match tab.tab.label.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => tab.position.to_string(),
    }
}

/// The destination tab with every folded pane added to it.
pub(super) fn folded_into(
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
pub(super) fn real_destination(tab: &TabEntry, snapshot: &Snapshot, config: &Config) -> Panel {
    let side = config.default_move_direction.resolve().unwrap_or(Side::Right);
    match destination_panels(tab, Some(side), Some(&snapshot.source)).pop() {
        Some(panel) => panel,
        None => arriving_panel(&tab_number(tab), config),
    }
}

/// Stand-in id for a pane that is already in the destination.
pub(super) const ELSEWHERE: &str = "\u{1}elsewhere";

/// The destination tab, and the pane that would arrive in it.
///
/// When `arriving` is set the shape is the tab **after** the move. The side is
/// already decided by then — it comes from the settings, not from a later
/// question — so there is nothing speculative about it.
pub(super) fn tab_preview(tab: &TabEntry, arriving: Option<Side>) -> Option<(Shape, Vec<String>)> {
    tab_preview_of(tab, arriving, ARRIVING)
}

/// The same, for a pane whose real id is known.
///
/// Using the real id rather than the stand-in is what lets a test hold the
/// picture and the operation to the same answer: both then describe the tab in
/// Herdr's own names, and two signatures can simply be compared.
pub(super) fn tab_preview_of(
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
pub(super) const ARRIVING: &str = "\u{1}arriving";

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
pub(super) fn legend(panels: &[Panel], snapshot: &Snapshot) -> Vec<String> {
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
    let mut lines: Vec<String> = seen
        .iter()
        .filter_map(|id| snapshot.pane(id))
        .take(MOST)
        .map(pane_line)
        .collect();

    // What the filled box means, said with the fill itself rather than with a
    // sentence: `░ p1`. The reader can see the shading; what they cannot see
    // is which pane it stands for, and the line under it then says what that
    // pane is running.
    let filled: Vec<String> = panels
        .iter()
        .flat_map(|panel| panel.marked.iter())
        .filter_map(|id| snapshot.pane(id))
        .map(|pane| pane_number(&pane.clone()))
        .collect();
    if let Some(name) = filled.first() {
        lines.insert(0, format!("{} {name}", herdr_plugin_kit::layout::HIGHLIGHT));
    }
    lines
}

/// A row whose picture comes with the names of what is in it.
pub(super) fn illustrated(row: Row, panels: Vec<Panel>, snapshot: &Snapshot) -> Row {
    let lines = legend(&panels, snapshot);
    row.panels(panels).legend(lines)
}

pub(super) fn pane_line(pane: &Pane) -> String {
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

pub(super) fn tab_contents(tab: &TabEntry) -> String {
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

/// The destination tab with `arriving` split in beside one named pane.
///
/// The picture a "Split next to…" row stands for: not the tab as it is, but
/// the tab this row would produce. Built from the tab's real shape with the
/// real pane ids, so it is the same arrangement the move will make.
pub(super) fn split_beside(tab: &TabEntry, target: &str, arriving: &Pane, side: Side) -> Vec<Panel> {
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
pub(super) fn swap_panels(snapshot: &Snapshot, target: &Pane) -> Vec<Panel> {
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

/// Side and size, asked only when the settings leave them open (§4.1, §12).
/// What the destination tab would look like with the pane added on `side`.
///
/// Built by applying the split to the tab's real shape, so the preview is the
/// same computation the move itself will perform rather than a drawing that
/// merely resembles it.
pub(super) fn placement_preview(shape: &Shape, target: &str, side: Side) -> (Shape, Vec<String>) {
    let mut after = shape.clone();
    after.split(target, ARRIVING, side);
    (after, vec![ARRIVING.to_string()])
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