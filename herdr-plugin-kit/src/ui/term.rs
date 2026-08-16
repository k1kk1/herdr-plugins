//! Minimal full-screen picker rendering.
//!
//! Pane Manager runs inside a Herdr plugin pane, which is a real terminal, so
//! the UI is a small alternate-screen list rather than anything Herdr has to
//! render on our behalf.

use std::io::{Stdout, Write};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, queue};

/// A key press reduced to what the pickers care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Up,
    Down,
    Tab,
    Backspace,
    Esc,
    /// Left click on the rendered row at this index.
    Click(usize),
    /// Wheel movement: -1 up, 1 down.
    Scroll(i8),
    /// Ctrl+C / Ctrl+D, treated as Cancel everywhere.
    Interrupt,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Header,
    Item,
    Separator,
    Note,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub kind: RowKind,
    /// Hotkey shown in the left gutter, e.g. `1`, `n`, `m`.
    pub hotkey: Option<String>,
    /// Agent state glyph (spec §13).
    pub glyph: Option<char>,
    pub glyph_color: Color,
    pub primary: String,
    /// Dimmed trailing detail on the same line.
    pub secondary: Option<String>,
    /// Dimmed line underneath, e.g. the terminal title.
    pub detail: Option<String>,
}

impl Row {
    pub fn item(primary: impl Into<String>) -> Self {
        Self {
            kind: RowKind::Item,
            hotkey: None,
            glyph: None,
            glyph_color: Color::Reset,
            primary: primary.into(),
            secondary: None,
            detail: None,
        }
    }

    pub fn header(text: impl Into<String>) -> Self {
        Self {
            kind: RowKind::Header,
            ..Self::item(text)
        }
    }

    pub fn note(text: impl Into<String>) -> Self {
        Self {
            kind: RowKind::Note,
            ..Self::item(text)
        }
    }

    pub fn separator() -> Self {
        Self {
            kind: RowKind::Separator,
            ..Self::item("")
        }
    }

    /// Give the row a single-key shortcut.
    ///
    /// Give the row a single-key shortcut.
    ///
    /// `j` and `k` may only be used if the menu binds both of them (see
    /// `Menu::claims_both_vim_keys`); binding one and not the other is checked
    /// at menu level, where both rows are visible.
    pub fn hotkey(mut self, key: impl Into<String>) -> Self {
        self.hotkey = Some(key.into());
        self
    }

    pub fn glyph(mut self, glyph: char, color: Color) -> Self {
        self.glyph = Some(glyph);
        self.glyph_color = color;
        self
    }

    pub fn secondary(mut self, text: impl Into<String>) -> Self {
        self.secondary = Some(text.into());
        self
    }

    pub fn detail(mut self, text: Option<String>) -> Self {
        self.detail = text;
        self
    }
}

#[derive(Debug, Clone)]
pub struct View {
    pub title: String,
    pub subtitle: Option<String>,
    pub rows: Vec<Row>,
    pub footer: Option<String>,
    /// Index into `rows` currently under the arrow-key cursor.
    pub cursor: Option<usize>,
    pub accent: Color,
    /// Current filter text, shown as a prompt line. `None` hides the prompt.
    pub query: Option<String>,
    /// How many entries survive the filter, shown beside the prompt.
    pub match_count: Option<usize>,
}

impl View {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            rows: Vec::new(),
            footer: None,
            cursor: None,
            accent: Color::Cyan,
            query: None,
            match_count: None,
        }
    }

    pub fn subtitle(mut self, text: impl Into<String>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    pub fn rows(mut self, rows: Vec<Row>) -> Self {
        self.rows = rows;
        self
    }

    pub fn footer(mut self, text: impl Into<String>) -> Self {
        self.footer = Some(text.into());
        self
    }

    pub fn accent(mut self, color: Color) -> Self {
        self.accent = color;
        self
    }
}

/// Alternate-screen terminal in raw mode; restores itself on drop.
pub struct Term {
    out: Stdout,
    active: bool,
    /// First screen line of each rendered row, paired with that row's index in
    /// the full list, so a mouse click can be turned back into the row the user
    /// aimed at even when the list is scrolled.
    row_lines: Vec<(u16, usize)>,
    /// Index of the first row drawn. Kept between renders so the list only
    /// scrolls when the cursor would otherwise leave the screen, rather than
    /// sliding under the reader on every keypress.
    scroll: usize,
}

impl Term {
    pub fn open() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        queue!(out, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;
        out.flush()?;
        Ok(Self {
            out,
            active: true,
            row_lines: Vec::new(),
            scroll: 0,
        })
    }

    pub fn close(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let _ = queue!(
            self.out,
            cursor::Show,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.out.flush();
        let _ = terminal::disable_raw_mode();
    }

    pub fn render(&mut self, view: &View) -> Result<()> {
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let width = width.max(20) as usize;

        queue!(self.out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

        let mut line = 0u16;
        let put = |out: &mut Stdout, text: String, line: &mut u16| -> Result<()> {
            if *line >= height {
                return Ok(());
            }
            queue!(out, cursor::MoveTo(0, *line), Print(text))?;
            *line += 1;
            Ok(())
        };

        queue!(
            self.out,
            cursor::MoveTo(0, line),
            SetForegroundColor(view.accent),
            SetAttribute(Attribute::Bold),
            Print(truncate(&view.title, width)),
            SetAttribute(Attribute::Reset),
            ResetColor
        )?;
        line += 1;

        if let Some(subtitle) = &view.subtitle {
            queue!(
                self.out,
                cursor::MoveTo(0, line),
                SetForegroundColor(Color::DarkGrey),
                Print(truncate(subtitle, width)),
                ResetColor
            )?;
            line += 1;
        }
        if let Some(query) = &view.query {
            let count = view
                .match_count
                .map(|n| format!("   {n}"))
                .unwrap_or_default();
            queue!(
                self.out,
                cursor::MoveTo(0, line),
                SetForegroundColor(view.accent),
                Print("> "),
                ResetColor,
                Print(truncate(query, width.saturating_sub(8))),
                // Block cursor, so an empty query still looks like an input.
                SetForegroundColor(view.accent),
                Print("▏"),
                SetForegroundColor(Color::DarkGrey),
                Print(count),
                ResetColor
            )?;
            line += 1;
        }
        put(&mut self.out, String::new(), &mut line)?;

        self.row_lines.clear();

        // A row is one line, or two when it carries a detail line, so the
        // window has to be measured in lines rather than in rows.
        let heights: Vec<usize> = view
            .rows
            .iter()
            .map(|row| if row.detail.is_some() { 2 } else { 1 })
            .collect();
        let total: usize = heights.iter().sum();
        // Everything above the footer is available to the list.
        let available = height.saturating_sub(line).saturating_sub(1) as usize;
        let overflowing = total > available;
        // When the list overflows, one line goes to the "more above / below"
        // marker so the reader knows the rest exists.
        let budget = if overflowing {
            available.saturating_sub(1)
        } else {
            available
        };

        self.scroll = scroll_for(self.scroll, view.cursor, &heights, budget);

        let mut used = 0usize;
        for (index, row) in view.rows.iter().enumerate().skip(self.scroll) {
            if used + heights[index] > budget {
                break;
            }
            used += heights[index];
            self.row_lines.push((line, index));
            let selected = view.cursor == Some(index);
            self.render_row(row, selected, width, &mut line, height, view.accent)?;
        }

        if overflowing {
            let shown = self.row_lines.len();
            let above = self.scroll;
            let below = view.rows.len().saturating_sub(self.scroll + shown);
            let marker = match (above, below) {
                (0, 0) => String::new(),
                (0, below) => format!("  ↓ {below} more"),
                (above, 0) => format!("  ↑ {above} more"),
                (above, below) => format!("  ↑ {above}   ↓ {below}"),
            };
            queue!(self.out, SetForegroundColor(Color::DarkGrey))?;
            put(&mut self.out, marker, &mut line)?;
            queue!(self.out, ResetColor)?;
        }

        if let Some(footer) = &view.footer {
            let target = height.saturating_sub(1);
            if target > line {
                queue!(
                    self.out,
                    cursor::MoveTo(0, target),
                    SetForegroundColor(Color::DarkGrey),
                    Print(truncate(footer, width)),
                    ResetColor
                )?;
            }
        }

        self.out.flush()?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &mut self,
        row: &Row,
        selected: bool,
        width: usize,
        line: &mut u16,
        height: u16,
        accent: Color,
    ) -> Result<()> {
        if *line >= height {
            return Ok(());
        }
        queue!(self.out, cursor::MoveTo(0, *line))?;

        match row.kind {
            RowKind::Separator => {
                queue!(
                    self.out,
                    SetForegroundColor(Color::DarkGrey),
                    Print("  ".to_string() + &"─".repeat(width.saturating_sub(4).min(40))),
                    ResetColor
                )?;
            }
            RowKind::Header => {
                queue!(
                    self.out,
                    SetForegroundColor(Color::DarkGrey),
                    SetAttribute(Attribute::Bold),
                    Print(truncate(&row.primary, width)),
                    SetAttribute(Attribute::Reset),
                    ResetColor
                )?;
            }
            RowKind::Note => {
                queue!(
                    self.out,
                    SetForegroundColor(Color::DarkGrey),
                    Print(truncate(&row.primary, width)),
                    ResetColor
                )?;
            }
            RowKind::Item => {
                queue!(
                    self.out,
                    SetForegroundColor(if selected { accent } else { Color::Reset }),
                    Print(if selected { "▸ " } else { "  " }),
                    ResetColor
                )?;
                if let Some(hotkey) = &row.hotkey {
                    queue!(
                        self.out,
                        SetForegroundColor(accent),
                        Print(format!("{hotkey:<2}")),
                        ResetColor,
                        Print(" ")
                    )?;
                } else {
                    queue!(self.out, Print("   "))?;
                }
                if let Some(glyph) = row.glyph {
                    queue!(
                        self.out,
                        SetForegroundColor(row.glyph_color),
                        Print(format!("{glyph} ")),
                        ResetColor
                    )?;
                }
                let mut text = row.primary.clone();
                if let Some(secondary) = &row.secondary {
                    text.push_str(&format!("  {secondary}"));
                }
                if selected {
                    queue!(self.out, SetAttribute(Attribute::Bold))?;
                }
                queue!(
                    self.out,
                    Print(truncate(&text, width.saturating_sub(8))),
                    SetAttribute(Attribute::Reset)
                )?;
            }
        }

        *line += 1;

        if let Some(detail) = &row.detail {
            if *line < height {
                queue!(
                    self.out,
                    cursor::MoveTo(0, *line),
                    SetForegroundColor(Color::DarkGrey),
                    Print(format!("       {}", truncate(detail, width.saturating_sub(9)))),
                    ResetColor
                )?;
                *line += 1;
            }
        }
        Ok(())
    }

    /// Block until a key is pressed.
    pub fn key(&mut self) -> Result<Key> {
        loop {
            match event::read()? {
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind,
                    ..
                }) => {
                    // Key *release* events are delivered by some terminals; only
                    // act on presses so a single tap is not counted twice.
                    if kind == KeyEventKind::Release {
                        continue;
                    }
                    if modifiers.contains(KeyModifiers::CONTROL) {
                        if let KeyCode::Char('c' | 'd') = code {
                            return Ok(Key::Interrupt);
                        }
                    }
                    return Ok(match code {
                        KeyCode::Char(c) => Key::Char(c),
                        KeyCode::Enter => Key::Enter,
                        KeyCode::Up => Key::Up,
                        KeyCode::Down => Key::Down,
                        KeyCode::Tab => Key::Tab,
                        KeyCode::Backspace => Key::Backspace,
                        KeyCode::Esc => Key::Esc,
                        _ => Key::Other,
                    });
                }
                Event::Mouse(mouse) => {
                    if let Some(key) = self.mouse(mouse) {
                        return Ok(key);
                    }
                }
                Event::Resize(_, _) => return Ok(Key::Other),
                _ => continue,
            }
        }
    }
}

impl Term {
    /// Translate a mouse event into a picker key, if it means anything here.
    fn mouse(&self, mouse: MouseEvent) -> Option<Key> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // The last row whose first line is at or above the click owns
                // it, which correctly attributes clicks on a row's detail line.
                // The stored index is the row's place in the whole list, not on
                // screen, so this stays right when the list is scrolled.
                let position = self
                    .row_lines
                    .iter()
                    .rposition(|(start, _)| *start <= mouse.row)?;
                Some(Key::Click(self.row_lines[position].1))
            }
            MouseEventKind::ScrollUp => Some(Key::Scroll(-1)),
            MouseEventKind::ScrollDown => Some(Key::Scroll(1)),
            _ => None,
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.close();
    }
}

/// Truncate to `width` display columns, counting CJK characters as two.
/// The ellipsis itself occupies a column, so it replaces trailing characters
/// rather than being appended past `width`.
/// Where the visible window should start.
///
/// Keeps the previous position unless the cursor has moved out of view, so the
/// list stays still while the reader is looking at it and only slides when it
/// has to. `heights` is each row's height in lines; `budget` is how many lines
/// the window has.
fn scroll_for(current: usize, cursor: Option<usize>, heights: &[usize], budget: usize) -> usize {
    let total: usize = heights.iter().sum();
    if total <= budget || heights.is_empty() {
        return 0;
    }

    // The last start that still fills the window, so scrolling never leaves a
    // gap at the bottom.
    let mut max_start = heights.len().saturating_sub(1);
    let mut tail = 0usize;
    for (index, height) in heights.iter().enumerate().rev() {
        tail += height;
        if tail > budget {
            break;
        }
        max_start = index;
    }

    let mut scroll = current.min(max_start);

    if let Some(cursor) = cursor {
        let cursor = cursor.min(heights.len().saturating_sub(1));
        if cursor < scroll {
            // Moved off the top: show it there.
            scroll = cursor;
        } else {
            // Moved off the bottom: pull the window down just far enough.
            let mut used = 0usize;
            let mut earliest = cursor;
            for index in (0..=cursor).rev() {
                used += heights[index];
                if used > budget {
                    break;
                }
                earliest = index;
            }
            if scroll < earliest {
                scroll = earliest;
            }
        }
    }

    scroll.min(max_start)
}

fn truncate(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = char_width(ch);
        if used + w > width {
            // Make room for the ellipsis by dropping what already fits.
            while used + 1 > width {
                match out.pop() {
                    Some(dropped) => used -= char_width(dropped),
                    None => return String::new(),
                }
            }
            out.push('…');
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Rough East-Asian-width check; good enough to stop wide agent titles from
/// wrapping and corrupting the picker layout.
fn char_width(ch: char) -> usize {
    let c = ch as u32;
    let wide = (0x1100..=0x115F).contains(&c)
        || (0x2E80..=0xA4CF).contains(&c)
        || (0xAC00..=0xD7A3).contains(&c)
        || (0xF900..=0xFAFF).contains(&c)
        || (0xFE30..=0xFE6F).contains(&c)
        || (0xFF00..=0xFF60).contains(&c)
        || (0xFFE0..=0xFFE6).contains(&c)
        || (0x1F300..=0x1FAFF).contains(&c);
    if wide {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display width, counting CJK as two columns.
    fn columns(text: &str) -> usize {
        text.chars().map(char_width).sum()
    }

    /// `n` single-line rows.
    fn flat(n: usize) -> Vec<usize> {
        vec![1; n]
    }

    /// The rows visible in a window of `budget` lines starting at `scroll`.
    fn window(scroll: usize, heights: &[usize], budget: usize) -> Vec<usize> {
        let mut used = 0;
        let mut shown = Vec::new();
        for (index, height) in heights.iter().enumerate().skip(scroll) {
            if used + height > budget {
                break;
            }
            used += height;
            shown.push(index);
        }
        shown
    }

    #[test]
    fn a_list_that_fits_never_scrolls() {
        assert_eq!(scroll_for(0, Some(9), &flat(10), 20), 0);
        // Even a stale offset is dropped once everything fits again, which is
        // what happens as soon as a filter narrows the list.
        assert_eq!(scroll_for(7, Some(0), &flat(10), 20), 0);
    }

    #[test]
    fn the_cursor_is_always_inside_the_window() {
        let heights = flat(40);
        for cursor in 0..40 {
            let scroll = scroll_for(0, Some(cursor), &heights, 10);
            assert!(
                window(scroll, &heights, 10).contains(&cursor),
                "cursor {cursor} fell outside the window at scroll {scroll}"
            );
        }
    }

    #[test]
    fn the_window_holds_still_while_the_cursor_is_visible() {
        let heights = flat(40);
        // Starting at 10 with a 10-line window, rows 10..19 are on screen.
        assert_eq!(scroll_for(10, Some(15), &heights, 10), 10);
        assert_eq!(scroll_for(10, Some(19), &heights, 10), 10);
        // One past the bottom edge moves it by exactly one row.
        assert_eq!(scroll_for(10, Some(20), &heights, 10), 11);
        // One past the top edge likewise.
        assert_eq!(scroll_for(10, Some(9), &heights, 10), 9);
    }

    #[test]
    fn scrolling_to_the_end_leaves_no_gap() {
        let heights = flat(40);
        let scroll = scroll_for(0, Some(39), &heights, 10);
        assert_eq!(window(scroll, &heights, 10).len(), 10);
        // And it cannot be pushed past that, however stale the offset is.
        assert_eq!(scroll_for(999, Some(39), &heights, 10), scroll);
    }

    #[test]
    fn two_line_rows_are_measured_in_lines_not_rows() {
        // Every row carries a detail line, so only half as many fit.
        let heights = vec![2; 10];
        let scroll = scroll_for(0, Some(9), &heights, 10);
        let shown = window(scroll, &heights, 10);
        assert_eq!(shown.len(), 5);
        assert!(shown.contains(&9));
    }

    #[test]
    fn a_mixed_list_still_keeps_the_cursor_visible() {
        let heights = vec![1, 2, 1, 2, 1, 2, 1, 2, 1, 2];
        for cursor in 0..heights.len() {
            let scroll = scroll_for(0, Some(cursor), &heights, 5);
            assert!(
                window(scroll, &heights, 5).contains(&cursor),
                "cursor {cursor} fell outside the window at scroll {scroll}"
            );
        }
    }

    /// The real Command Palette shape: 33 actions, 3 plugin headings and 2
    /// separators in a 70%-height popup on a 54-row terminal.
    #[test]
    fn every_palette_entry_can_be_reached_by_arrowing_down() {
        let rows = 33 + 3 + 2;
        let heights = flat(rows);
        // popup 37 lines − title − subtitle − blank − footer.
        let available = 37 - 4;
        let budget = available - 1; // one line for the more-above/below marker
        assert!(rows > available, "this case is only interesting when it overflows");

        // Walk the cursor down the whole list, carrying the offset along the
        // way exactly as successive renders would.
        let mut scroll = 0;
        for cursor in 0..rows {
            scroll = scroll_for(scroll, Some(cursor), &heights, budget);
            assert!(
                window(scroll, &heights, budget).contains(&cursor),
                "entry {cursor} of {rows} was not on screen"
            );
        }
        // And the very last entry sits at the bottom of a full window.
        assert_eq!(window(scroll, &heights, budget).len(), budget);
        assert_eq!(*window(scroll, &heights, budget).last().unwrap(), rows - 1);
    }

    #[test]
    fn an_empty_list_scrolls_nowhere() {
        assert_eq!(scroll_for(3, None, &[], 10), 0);
        assert_eq!(scroll_for(3, Some(0), &[], 10), 0);
    }

    #[test]
    fn truncate_counts_cjk_as_two_columns() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdef", 3), "ab…");
        // Five CJK characters occupy ten columns; four fit in a width of five
        // once the ellipsis claims one.
        assert_eq!(truncate("日本語です", 5), "日本…");
    }

    #[test]
    fn truncate_is_a_noop_when_it_fits_exactly() {
        assert_eq!(truncate("abcd", 4), "abcd");
        assert_eq!(truncate("日本", 4), "日本");
    }

    #[test]
    fn truncated_output_never_exceeds_the_requested_width() {
        for text in ["abcdef", "日本語です", "a日b本c", "…"] {
            for width in 0..12 {
                assert!(
                    columns(&truncate(text, width)) <= width,
                    "{text:?} at width {width} overflowed"
                );
            }
        }
    }
}

