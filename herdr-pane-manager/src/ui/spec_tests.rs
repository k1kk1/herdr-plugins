//! The specification table, executable.
//!
//! One row per line: a session, an operation, and the arrangement the
//! destination tab is left in. Both the plan and the preview are held to that
//! same answer, which is the check that was missing while the picture and the
//! operation drifted apart.

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

/// The rows the landing screen offers, without running it.
fn row_titles(snapshot: &Snapshot, config: &Config) -> Vec<String> {
    manager_menu(snapshot, config, None, true, snapshot.next_pane_here().cloned()).item_titles()
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
    // The shading first, saying which pane it stands for, then every pane
    // once in the order the picture introduces it — with the name a person
    // recognises rather than the id alone.
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0], "░ p1");
    assert!(lines[1].starts_with("p5: "));
    assert!(lines[1].contains("review"));
    assert!(lines[1].ends_with("| codex"));
    assert!(lines[2].starts_with("p1: "));
    assert!(lines[2].ends_with("| claude"));
    // pB has no agent, so it is named without one.
    assert!(lines[3].starts_with("pB: "));
    assert!(!lines[3].contains('|'));
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
fn a_row_only_promises_a_next_screen_when_the_key_opens_one() {
    // With the default action set to Quick the plain key moves the pane;
    // an ellipsis there says "a picker is coming" and none is.
    let snapshot = testkit::session("t1: p5 | p1* ; t2: pB");
    let quick = Config::default();
    assert_eq!(quick.default_action, crate::config::DefaultAction::Quick);
    assert!(row_titles(&snapshot, &quick)
        .iter()
        .all(|title| !title.ends_with('…')));

    let detailed = Config {
        default_action: crate::config::DefaultAction::Detailed,
        ..Config::default()
    };
    assert!(row_titles(&snapshot, &detailed)
        .iter()
        .any(|title| title == "Move to…"));
}

#[test]
fn fold_is_not_offered_when_there_is_nowhere_to_fold_into() {
    // It used to offer to choose a destination and then say there were
    // none.
    let alone = testkit::session("t1: p5 | p1*");
    assert!(!row_titles(&alone, &Config::default())
        .iter()
        .any(|title| title.starts_with("Fold")));

    let pair = testkit::session("t1: p5 | p1* ; t2: pB");
    assert!(row_titles(&pair, &Config::default())
        .iter()
        .any(|title| title.starts_with("Fold")));
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
