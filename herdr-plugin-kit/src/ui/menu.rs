//! Keyboard-driven single-choice list, with an optional type-to-filter mode.
//!
//! Two shapes share one implementation:
//!
//! * **Hotkey menus** — short, fixed lists where `m` / `1` picks an entry
//!   directly. Arrow keys and Enter also work so lists longer than nine
//!   entries stay reachable.
//! * **Filter pickers** — long lists (every pane, every plugin action) where
//!   typing narrows the list and Enter takes the highlighted row.

use anyhow::Result;

use super::term::{Key, Row, Term, View};

/// Result of a caller-supplied key handler.
pub enum Interrupt {
    /// The handler consumed the key; redraw and keep waiting.
    Redraw,
    /// The handler wants the menu to close with no selection, so the caller
    /// can rebuild it — a scope switch, for instance.
    Close,
    /// The handler ignored the key; fall through to normal handling.
    Unhandled,
}

/// A list where some rows carry a value and some are decoration.
pub struct Menu<T> {
    view: View,
    /// Parallel to `view.rows`: the value a row selects, if any.
    values: Vec<Option<T>>,
    /// Parallel to `view.rows`: lowercase text the filter matches against.
    haystacks: Vec<String>,
    /// Parallel to `view.rows`: rows that survive every filter, such as
    /// "create a new tab", whose text is templated with the current query.
    pinned: Vec<bool>,
    /// When set, printable keys build a query instead of picking by hotkey.
    filter: Option<String>,
    /// Number the first nine selectable rows on screen.
    numbered: bool,
}

impl<T: Clone> Menu<T> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            view: View::new(title),
            values: Vec::new(),
            haystacks: Vec::new(),
            pinned: Vec::new(),
            filter: None,
            numbered: false,
        }
    }

    pub fn subtitle(mut self, text: impl Into<String>) -> Self {
        self.view = self.view.subtitle(text);
        self
    }

    pub fn footer(mut self, text: impl Into<String>) -> Self {
        self.view = self.view.footer(text);
        self
    }

    /// Switch to type-to-filter mode. Hotkeys stop being picked directly,
    /// because every printable key belongs to the query.
    pub fn filterable(mut self) -> Self {
        self.filter = Some(String::new());
        self
    }

    /// Number the first nine selectable rows so `1`–`9` pick them.
    ///
    /// Numbering is applied to what is **on screen**, recomputed after every
    /// keystroke, so the digits keep matching the list as a filter narrows it.
    /// Assigning fixed numbers when the menu is built would leave gaps — rows
    /// 3, 7 and 20 surviving a filter would still show 3, 7 and nothing.
    pub fn numbered(mut self) -> Self {
        self.numbered = true;
        self
    }

    /// Add a decorative row (header, separator, note).
    pub fn row(&mut self, row: Row) {
        self.push(row, None, String::new());
    }

    /// Add a selectable row.
    pub fn item(&mut self, row: Row, value: T) {
        let haystack = searchable(&row);
        self.push(row, Some(value), haystack);
    }

    /// Add a selectable row with extra text the filter should match, such as
    /// a workspace name that is shown in a group header rather than the row.
    pub fn item_matching(&mut self, row: Row, value: T, extra: &str) {
        let haystack = format!("{} {}", searchable(&row), extra.to_lowercase());
        self.push(row, Some(value), haystack);
    }

    /// Add a row that is always offered, whatever the filter says.
    ///
    /// Occurrences of `{query}` in its text are replaced with the current
    /// query, so "create this" rows can name what they would create.
    pub fn item_pinned(&mut self, row: Row, value: T) {
        let haystack = searchable(&row);
        self.push(row, Some(value), haystack);
        *self.pinned.last_mut().expect("just pushed") = true;
    }

    fn push(&mut self, row: Row, value: Option<T>, haystack: String) {
        self.view.rows.push(row);
        self.values.push(value);
        self.haystacks.push(haystack);
        self.pinned.push(false);
    }

    /// Text the user has typed, for callers that use it as a default name.
    pub fn query(&self) -> &str {
        self.filter.as_deref().unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.values.iter().all(Option::is_none)
    }

    /// Row indices that survive the current query, decoration included so
    /// group headers stay attached to the rows underneath them.
    fn visible(&self) -> Vec<usize> {
        // Indexed off `values`, not `view.rows`: the render loop moves the
        // rows out of the view while it works, so `view.rows` is not a
        // reliable count here.
        let Some(query) = self.filter.as_deref().filter(|q| !q.is_empty()) else {
            return (0..self.values.len()).collect();
        };
        let query = query.to_lowercase();

        let mut keep: Vec<usize> = Vec::new();
        for (index, haystack) in self.haystacks.iter().enumerate() {
            if self.values[index].is_none() {
                continue;
            }
            if self.pinned[index] || subsequence(&query, haystack) {
                keep.push(index);
            }
        }
        keep
    }

    /// Fill `{query}` placeholders in a pinned row.
    fn render_pinned(&self, row: &Row) -> Row {
        let query = self.query().trim();
        let named = if query.is_empty() {
            "...".to_string()
        } else {
            format!("\"{query}\"")
        };
        let mut row = row.clone();
        row.primary = row.primary.replace("{query}", &named);
        row
    }

    fn selectable(&self, visible: &[usize]) -> Vec<usize> {
        visible
            .iter()
            .copied()
            .filter(|i| self.values[*i].is_some())
            .collect()
    }

    fn claims_vim_key(&self, needle: char) -> bool {
        self.view.rows.iter().enumerate().any(|(index, row)| {
            row.hotkey.as_deref().is_some_and(|hotkey| {
                hotkey.len() == 1
                    && hotkey.chars().next().map(|c| c.to_ascii_lowercase()) == Some(needle)
                    && self.values[index].is_some()
            })
        })
    }

    /// Whether a row claims `j` and a row claims `k`.
    fn claims_both_vim_keys(&self) -> bool {
        self.claims_vim_key('j') && self.claims_vim_key('k')
    }

    /// Whether exactly one of the pair is bound — always a mistake.
    fn half_binds_vim_keys(&self) -> bool {
        self.claims_vim_key('j') != self.claims_vim_key('k')
    }

    /// Index of the row whose hotkey matches `ch`, case-insensitively.
    ///
    /// `j` and `k` are shared between the list and its rows, and the rule is
    /// symmetry: a menu may claim **both** or **neither**.
    ///
    /// Claiming only one is what went wrong before — `j` merged a tab while `k`
    /// still moved the cursor, so the pair behaved differently in the same
    /// list. A direction picker that maps all four of `h j k l` is fine, because
    /// nothing is left half-bound.
    fn by_hotkey(&self, ch: char) -> Option<usize> {
        let needle = ch.to_ascii_lowercase();
        if matches!(needle, 'j' | 'k') && !self.claims_both_vim_keys() {
            return None;
        }
        self.view.rows.iter().enumerate().find_map(|(index, row)| {
            let hotkey = row.hotkey.as_deref()?;
            let matches = hotkey.len() == 1
                && hotkey.chars().next()?.to_ascii_lowercase() == needle
                && self.values[index].is_some();
            matches.then_some(index)
        })
    }

    /// Show the menu and return the chosen value, or `None` if cancelled.
    pub fn run(&mut self, term: &mut Term) -> Result<Option<T>> {
        self.run_with(term, |_| Interrupt::Unhandled)
    }

    /// Like [`Menu::run`], but `extra` gets first refusal on every key press.
    pub fn run_with(
        &mut self,
        term: &mut Term,
        mut extra: impl FnMut(Key) -> Interrupt,
    ) -> Result<Option<T>> {
        let all_rows = std::mem::take(&mut self.view.rows);
        let mut cursor = 0usize;

        loop {
            let visible = self.visible();
            let selectable = self.selectable(&visible);
            cursor = cursor.min(selectable.len().saturating_sub(1));

            self.view.rows = visible
                .iter()
                .map(|i| {
                    if self.pinned[*i] {
                        self.render_pinned(&all_rows[*i])
                    } else {
                        all_rows[*i].clone()
                    }
                })
                .collect();

            // Number what is on screen, after filtering, so `1`–`9` always
            // match the digits the reader can see.
            if self.numbered {
                let mut n = 0usize;
                for (position, row) in self.view.rows.iter_mut().enumerate() {
                    if self.values[visible[position]].is_none() || n >= 9 {
                        continue;
                    }
                    n += 1;
                    row.hotkey = Some(n.to_string());
                }
            }
            self.view.cursor = selectable
                .get(cursor)
                .and_then(|row| visible.iter().position(|v| v == row));
            if let Some(query) = &self.filter {
                self.view.query = Some(query.clone());
                self.view.match_count = Some(selectable.len());
            }
            // Half-binding the vim pair advertises a key that does nothing;
            // caught here because only the whole menu can see both rows.
            debug_assert!(
                !self.half_binds_vim_keys(),
                "a menu may bind both `j` and `k` or neither, not one of them"
            );
            term.render(&self.view)?;

            let key = term.key()?;
            match extra(key) {
                Interrupt::Redraw => continue,
                Interrupt::Close => return Ok(None),
                Interrupt::Unhandled => {}
            }

            let step = |cursor: &mut usize, delta: isize| {
                if !selectable.is_empty() {
                    let len = selectable.len() as isize;
                    *cursor = ((*cursor as isize + delta).rem_euclid(len)) as usize;
                }
            };

            match key {
                Key::Esc | Key::Interrupt => return Ok(None),
                Key::Up => step(&mut cursor, -1),
                Key::Down => step(&mut cursor, 1),
                Key::Enter => {
                    if let Some(index) = selectable.get(cursor) {
                        return Ok(self.values[*index].clone());
                    }
                }
                Key::Backspace => {
                    if let Some(query) = self.filter.as_mut() {
                        query.pop();
                        cursor = 0;
                    }
                }
                // A click selects the row it landed on; a second click on the
                // same row confirms it, matching how menus behave elsewhere.
                Key::Click(row) => {
                    let Some(position) = visible.get(row) else { continue };
                    let Some(index) = selectable.iter().position(|s| s == position) else {
                        continue;
                    };
                    if cursor == index {
                        return Ok(self.values[*position].clone());
                    }
                    cursor = index;
                }
                Key::Scroll(delta) => step(&mut cursor, delta as isize),
                Key::Char(ch) => {
                    // Digits pick by position even while filtering: a number is
                    // almost never what someone is searching for, and losing
                    // 1..9 would break the documented Quick Move keys.
                    if let Some(index) = self.by_hotkey(ch) {
                        if ch.is_ascii_digit() || self.filter.is_none() {
                            return Ok(self.values[index].clone());
                        }
                    }
                    match self.filter.as_mut() {
                        Some(query) => {
                            query.push(ch);
                            cursor = 0;
                        }
                        None => match ch {
                            // vim-style movement alongside the arrow keys.
                            'j' => step(&mut cursor, 1),
                            'k' => step(&mut cursor, -1),
                            'q' => return Ok(None),
                            _ => {}
                        },
                    }
                }
                Key::Tab | Key::Other => {}
            }
        }
    }
}

/// Lowercased text of a row, for filtering.
fn searchable(row: &Row) -> String {
    let mut text = row.primary.to_lowercase();
    if let Some(secondary) = &row.secondary {
        text.push(' ');
        text.push_str(&secondary.to_lowercase());
    }
    if let Some(detail) = &row.detail {
        text.push(' ');
        text.push_str(&detail.to_lowercase());
    }
    text
}

/// Whether every character of `query` appears in `haystack`, in order.
///
/// Deliberately loose: `cldmb` finds `Claude · mushi-battle`, which is what a
/// picker over long agent titles needs. Both sides are already lowercase.
fn subsequence(query: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|needle| chars.any(|c| c == needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A menu with the given hotkeys on selectable rows.
    fn menu_with(hotkeys: &[&str]) -> Menu<usize> {
        let mut menu = Menu::new("test");
        for (index, key) in hotkeys.iter().enumerate() {
            menu.item(Row::item(format!("row {index}")).hotkey(*key), index);
        }
        menu
    }

    #[test]
    fn a_menu_binding_neither_vim_key_leaves_them_to_the_cursor() {
        let menu = menu_with(&["m", "s", "e", "c"]);
        assert!(!menu.claims_both_vim_keys());
        assert!(!menu.half_binds_vim_keys());
        // j and k fall through to cursor movement.
        assert_eq!(menu.by_hotkey('j'), None);
        assert_eq!(menu.by_hotkey('k'), None);
        assert_eq!(menu.by_hotkey('s'), Some(1));
    }

    #[test]
    fn a_direction_picker_binding_all_four_keeps_them() {
        let menu = menu_with(&["h", "l", "k", "j"]);
        assert!(menu.claims_both_vim_keys());
        assert!(!menu.half_binds_vim_keys());
        assert_eq!(menu.by_hotkey('k'), Some(2));
        assert_eq!(menu.by_hotkey('j'), Some(3));
    }

    #[test]
    fn binding_only_one_of_the_pair_is_reported_as_a_mistake() {
        // This is the shape that made `j` merge a tab while `k` moved the
        // cursor. The key stops dispatching, and debug builds assert.
        let menu = menu_with(&["m", "j", "e"]);
        assert!(menu.half_binds_vim_keys());
        assert_eq!(menu.by_hotkey('j'), None);

        let menu = menu_with(&["e", "k", "g"]);
        assert!(menu.half_binds_vim_keys());
        assert_eq!(menu.by_hotkey('k'), None);
    }

    #[test]
    fn numbering_follows_what_is_on_screen() {
        // A filter that keeps rows 0, 2 and 5 must number them 1, 2, 3 —
        // not 1, 3, 6 — or the digits stop matching the visible list.
        let mut menu: Menu<usize> = Menu::new("t").filterable().numbered();
        for (i, name) in ["alpha", "beta", "alfresco", "gamma", "delta", "alpenglow"]
            .iter()
            .enumerate()
        {
            menu.item(Row::item(*name), i);
        }
        menu.filter = Some("al".into());
        let visible = menu.visible();
        assert_eq!(visible, vec![0, 2, 5], "filter should keep the three al* rows");

        // Mirror the render loop's numbering step.
        let mut n = 0;
        let numbers: Vec<String> = visible
            .iter()
            .map(|_| {
                n += 1;
                n.to_string()
            })
            .collect();
        assert_eq!(numbers, ["1", "2", "3"]);
    }

    #[test]
    fn subsequence_matches_scattered_initials() {
        assert!(subsequence("cldmb", "claude · mushi-battle"));
        assert!(subsequence("mushi", "claude · mushi-battle"));
        assert!(!subsequence("zzz", "claude · mushi-battle"));
    }

    #[test]
    fn subsequence_respects_order() {
        assert!(subsequence("ab", "a-b"));
        assert!(!subsequence("ba", "a-b"));
    }

    #[test]
    fn subsequence_ignores_spaces_in_the_query() {
        assert!(subsequence("cl mb", "claude · mushi-battle"));
    }

    #[test]
    fn an_empty_query_matches_everything() {
        assert!(subsequence("", "anything"));
    }

    #[test]
    fn an_unfiltered_menu_shows_every_row() {
        let mut menu: Menu<u8> = Menu::new("t").filterable();
        menu.row(Row::header("Tab 1"));
        menu.item(Row::item("Claude"), 1);
        menu.item(Row::item("Codex"), 2);

        // Rendering moves the rows out of the view; visibility must not
        // depend on them still being there.
        let rows = std::mem::take(&mut menu.view.rows);
        assert_eq!(menu.visible(), vec![0, 1, 2]);
        menu.view.rows = rows;
    }

    #[test]
    fn filtering_keeps_only_matching_selectable_rows() {
        let mut menu: Menu<u8> = Menu::new("t").filterable();
        menu.row(Row::header("Tab 1"));
        menu.item(Row::item("Claude · mushi-battle"), 1);
        menu.item(Row::item("Codex · ComposerSketch"), 2);

        assert_eq!(menu.visible().len(), 3);
        menu.filter = Some("codex".into());
        assert_eq!(menu.visible(), vec![2]);
    }

    #[test]
    fn pinned_rows_survive_a_filter_that_matches_nothing() {
        let mut menu: Menu<u8> = Menu::new("t").filterable();
        menu.item(Row::item("Tab 1"), 1);
        menu.item_pinned(Row::item("+ New Tab {query}"), 2);

        menu.filter = Some("zzzz".into());
        assert_eq!(menu.visible(), vec![1]);
    }

    #[test]
    fn a_pinned_row_names_what_it_would_create() {
        let mut menu: Menu<u8> = Menu::new("t").filterable();
        let row = Row::item("+ New Tab {query}");
        menu.item_pinned(row.clone(), 1);

        assert_eq!(menu.render_pinned(&row).primary, "+ New Tab ...");
        menu.filter = Some("review".into());
        assert_eq!(menu.render_pinned(&row).primary, "+ New Tab \"review\"");
    }

    #[test]
    fn the_query_is_readable_by_the_caller() {
        let mut menu: Menu<u8> = Menu::new("t").filterable();
        assert_eq!(menu.query(), "");
        menu.filter = Some("review".into());
        assert_eq!(menu.query(), "review");
    }

    #[test]
    fn extra_match_text_is_searchable_without_being_shown() {
        let mut menu: Menu<u8> = Menu::new("t").filterable();
        menu.item_matching(Row::item("Codex"), 1, "AgentRecipes");
        menu.filter = Some("agentrec".into());
        assert_eq!(menu.visible(), vec![0]);
    }
}
