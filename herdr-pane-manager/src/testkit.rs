//! Whole sessions written as one line, for tests.
//!
//! Every bug this plugin has had in the preview came from the same place: the
//! picture and the operation were computed by different code, and nothing
//! checked that they agreed. Both sides speak in Herdr's own ids — `p1`, `t2`
//! — so a test can name a starting arrangement, name the expected one, and
//! hold the plan and the preview to the same answer.
//!
//! The notation is the arrangement itself:
//!
//! ```text
//! t1 herdr-plugins: p5 | p1* ; t2: pB
//! ```
//!
//! * `;` separates tabs, `tN` is the tab's Herdr id, the rest of the head is
//!   its name.
//! * `|` splits right, `/` splits down, and `( )` groups. `|` and `/` bind
//!   equally and associate left, so `a | b | c` is `(a | b) | c` — the same
//!   way Herdr builds a tab by splitting the pane you are in.
//! * `*` marks the focused pane, which is the one every operation acts on.
//!
//! An expected arrangement is a [`Shape::signature`], which reads the same way
//! round: `(r pB (d p5 p1))`.

use herdr_plugin_kit::herdr::{Pane, Tab, Workspace};
use herdr_plugin_kit::layout::{Shape, Side};

use crate::state::{Snapshot, TabEntry};

const WORKSPACE: &str = "w1";

/// Parse a session description into a [`Snapshot`].
///
/// Panics on a malformed description: these are test inputs, and a typo in one
/// should stop the test rather than quietly describe a different session.
pub fn session(spec: &str) -> Snapshot {
    let mut tabs = Vec::new();
    let mut source = None;

    for (index, chunk) in spec.split(';').enumerate() {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        let (head, body) = chunk
            .split_once(':')
            .unwrap_or_else(|| panic!("tab {chunk:?} needs a `tN:` head"));
        let mut head = head.trim().splitn(2, char::is_whitespace);
        let tab_id = format!("{WORKSPACE}:{}", head.next().unwrap().trim());
        let label = head.next().map(str::trim).filter(|n| !n.is_empty());

        let (shape, focused) = parse(body, &tab_id);
        if let Some(focused) = focused {
            source = Some(focused);
        }

        let panes: Vec<Pane> = shape
            .pane_ids()
            .into_iter()
            .map(|pane_id| Pane {
                tab_id: tab_id.clone(),
                workspace_id: WORKSPACE.into(),
                focused: Some(&pane_id) == source.as_ref(),
                pane_id,
                ..Default::default()
            })
            .collect();

        tabs.push(TabEntry {
            tab: Tab {
                tab_id,
                workspace_id: WORKSPACE.into(),
                label: label.map(str::to_string),
                pane_count: panes.len() as u32,
                focused: index == 0,
                agent_status: Default::default(),
            },
            position: index + 1,
            // A tab of one pane has no split tree, and Herdr does not report
            // one either — matching that keeps the fixtures honest.
            shape: (panes.len() > 1).then_some(shape),
            layout_known: true,
            panes,
        });
    }

    let source_id = source.expect("one pane must be marked `*`");
    let source = tabs
        .iter()
        .flat_map(|tab| tab.panes.iter())
        .find(|pane| pane.pane_id == source_id)
        .cloned()
        .expect("the `*` pane must be in a tab");

    Snapshot::of(
        Workspace {
            workspace_id: WORKSPACE.into(),
            label: None,
            focused: true,
            agent_status: Default::default(),
        },
        tabs,
        source,
    )
}

/// The pane ids of one tab in a session description, in layout order.
pub fn tab(snapshot: &Snapshot, id: &str) -> TabEntry {
    snapshot
        .tabs
        .iter()
        .find(|tab| tab.tab.tab_id == format!("{WORKSPACE}:{id}"))
        .cloned()
        .unwrap_or_else(|| panic!("no tab {id} in this session"))
}

/// Qualify a bare pane id the way the fixtures do.
pub fn pane(id: &str) -> String {
    format!("{WORKSPACE}:{id}")
}

fn parse(body: &str, tab_id: &str) -> (Shape, Option<String>) {
    let tokens = tokenise(body);
    let mut cursor = 0usize;
    let mut focused = None;
    let shape = expression(&tokens, &mut cursor, tab_id, &mut focused);
    assert_eq!(cursor, tokens.len(), "trailing input in {body:?}");
    (shape, focused)
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Pane(String),
    Side(Side),
    Open,
    Close,
}

fn tokenise(body: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Token>| {
        if !word.is_empty() {
            out.push(Token::Pane(std::mem::take(word)));
        }
    };
    for ch in body.chars() {
        match ch {
            '|' => {
                flush(&mut word, &mut out);
                out.push(Token::Side(Side::Right));
            }
            '/' => {
                flush(&mut word, &mut out);
                out.push(Token::Side(Side::Down));
            }
            '(' => {
                flush(&mut word, &mut out);
                out.push(Token::Open);
            }
            ')' => {
                flush(&mut word, &mut out);
                out.push(Token::Close);
            }
            c if c.is_whitespace() => flush(&mut word, &mut out),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

fn expression(
    tokens: &[Token],
    cursor: &mut usize,
    tab_id: &str,
    focused: &mut Option<String>,
) -> Shape {
    let mut left = term(tokens, cursor, tab_id, focused);
    while let Some(Token::Side(side)) = tokens.get(*cursor) {
        let side = *side;
        *cursor += 1;
        let right = term(tokens, cursor, tab_id, focused);
        left = Shape::Split {
            side,
            ratio: herdr_plugin_kit::layout::EVEN,
            first: Box::new(left),
            second: Box::new(right),
        };
    }
    left
}

fn term(tokens: &[Token], cursor: &mut usize, tab_id: &str, focused: &mut Option<String>) -> Shape {
    match tokens.get(*cursor) {
        Some(Token::Open) => {
            *cursor += 1;
            let inner = expression(tokens, cursor, tab_id, focused);
            assert_eq!(tokens.get(*cursor), Some(&Token::Close), "unclosed `(`");
            *cursor += 1;
            inner
        }
        Some(Token::Pane(name)) => {
            *cursor += 1;
            let (name, marked) = match name.strip_suffix('*') {
                Some(bare) => (bare, true),
                None => (name.as_str(), false),
            };
            let id = format!("{WORKSPACE}:{name}");
            if marked {
                assert!(focused.is_none() || focused.as_deref() == Some(&id));
                *focused = Some(id.clone());
            }
            let _ = tab_id;
            Shape::Pane(id)
        }
        other => panic!("expected a pane, found {other:?}"),
    }
}

/// The arrangement a plan would leave in its destination tab.
///
/// A pure replay of what `ops::apply::merge` and `ops::apply::single` do, in
/// the same order: the first pane joins the destination on the chosen side,
/// then either the plan's own placements or a chain along that side. Kept next
/// to the fixtures rather than inside the tests so every test measures the
/// operation the same way.
pub fn outcome(snapshot: &Snapshot, plan: &crate::ops::Plan) -> String {
    use crate::ops::Destination;

    let mut shape = match &plan.destination {
        Destination::Tab { tab_id, .. } => {
            let entry = snapshot
                .tabs
                .iter()
                .find(|tab| tab.tab.tab_id == *tab_id)
                .expect("destination tab is in the session");
            let anchor = entry.split_anchor().expect("destination has a pane");
            let mut shape = entry
                .shape
                .clone()
                .unwrap_or_else(|| Shape::pane(&anchor.pane_id));
            let target = match &plan.destination {
                Destination::Tab {
                    target_pane: Some(pane),
                    ..
                } => pane.clone(),
                _ => anchor.pane_id.clone(),
            };
            shape.split(&target, &plan.panes[0], plan.placement.side);
            shape
        }
        // Nothing to land beside: the first pane is the tab.
        _ => Shape::pane(&plan.panes[0]),
    };

    if plan.internal.is_empty() {
        let mut anchor = plan.panes[0].clone();
        for pane_id in plan.panes.iter().skip(1) {
            shape.split(&anchor, pane_id, plan.placement.side);
            anchor = pane_id.clone();
        }
    } else {
        for placement in &plan.internal {
            shape.split(&placement.anchor, &placement.pane_id, placement.side);
        }
    }

    shape.signature()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_reads_back_as_the_arrangement_it_describes() {
        let snapshot = session("t1 herdr-plugins: p5 | p1* ; t2: pB");
        assert_eq!(snapshot.tabs.len(), 2);
        assert_eq!(snapshot.source.pane_id, "w1:p1");
        assert_eq!(
            tab(&snapshot, "t1").shape.unwrap().signature(),
            "(r w1:p5 w1:p1)"
        );
        assert_eq!(tab(&snapshot, "t1").tab.label.as_deref(), Some("herdr-plugins"));
        // One pane, so no split tree — the same as Herdr reports.
        assert!(tab(&snapshot, "t2").shape.is_none());
    }

    #[test]
    fn splits_group_left_and_parentheses_override_that() {
        let flat = session("t1: a | b | c*");
        assert_eq!(
            tab(&flat, "t1").shape.unwrap().signature(),
            "(r (r w1:a w1:b) w1:c)"
        );
        let nested = session("t1: a* | (b / c)");
        assert_eq!(
            tab(&nested, "t1").shape.unwrap().signature(),
            "(r w1:a (d w1:b w1:c))"
        );
    }
}
