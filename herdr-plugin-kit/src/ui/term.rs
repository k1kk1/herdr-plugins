//! Minimal full-screen picker rendering.
//!
//! Pane Manager runs inside a Herdr plugin pane, which is a real terminal, so
//! the UI is a small alternate-screen list rather than anything Herdr has to
//! render on our behalf.

use std::io::{Stdout, Write};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, queue};

/// A key press reduced to what the pickers care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    /// Enter with Shift held.
    ///
    /// Only reachable when the terminal reports modified Enter — see
    /// [`Term::open`]. Where it is not, the key arrives as plain [`Key::Enter`]
    /// and any behaviour bound to it must therefore be an *alternative* to
    /// something Enter already does, never the only way to reach it.
    ShiftEnter,
    /// Enter with Alt (Option on a Mac) held.
    AltEnter,
    Up,
    Down,
    Tab,
    /// Shift+Tab. Reported by every terminal as its own code, unlike
    /// Shift+Enter, so it needs no keyboard-protocol negotiation.
    BackTab,
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
    /// The layout to draw in the preview area when this row is highlighted.
    ///
    /// Held as a tree rather than as finished lines because only the renderer
    /// knows how much room is left once the list is placed, and a picture that
    /// fills the space is worth more than one drawn to a guessed size.
    pub preview: Option<Preview>,
    /// Dimmed text pinned to the right edge of the line.
    ///
    /// For a value that repeats down the list — which tool a conversation
    /// belongs to, say. Put in front it pushes every heading right and is the
    /// first thing the eye lands on, which is backwards: the headings are what
    /// is being read, and the repeated value only has to be *findable*. A
    /// right-hand column keeps the headings flush left and still reads down.
    pub trailing: Option<String>,
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
            preview: None,
            trailing: None,
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

    pub fn preview(mut self, shape: crate::layout::Shape, marked: Vec<String>) -> Self {
        self.preview = Some(Preview::new(vec![
            Panel::new(String::new(), shape).marking(marked),
        ]));
        self
    }

    /// The same, for callers whose preview may not exist.
    pub fn preview_of(mut self, preview: Option<(crate::layout::Shape, Vec<String>)>) -> Self {
        self.preview = preview.map(|(shape, marked)| {
            Preview::new(vec![Panel::new(String::new(), shape).marking(marked)])
        });
        self
    }

    /// Several captioned diagrams, drawn side by side.
    pub fn panels(mut self, panels: Vec<Panel>) -> Self {
        self.preview = (!panels.is_empty()).then(|| Preview::new(panels));
        self
    }

    /// Name what the boxes hold, under the picture.
    pub fn legend(mut self, lines: Vec<String>) -> Self {
        if let Some(preview) = self.preview.as_mut() {
            preview.legend = lines;
        }
        self
    }

    pub fn trailing(mut self, text: impl Into<String>) -> Self {
        self.trailing = Some(text.into());
        self
    }
}

/// One entry of a view's tab strip.
///
/// A strip exists so the choice a key cycles through is *visible* rather than
/// described. Spelling the options out in a sentence — "Tab narrows to one of
/// them" — tells the reader a key exists but not where they currently are.
#[derive(Debug, Clone)]
pub struct Chip {
    pub label: String,
    pub active: bool,
}

impl Chip {
    pub fn new(label: impl Into<String>, active: bool) -> Self {
        Self {
            label: label.into(),
            active,
        }
    }
}

/// One captioned diagram inside a preview.
#[derive(Debug, Clone)]
pub struct Panel {
    pub caption: String,
    /// `None` draws the caption over an explicit "nothing left" box, which is
    /// what a tab that closes should look like.
    pub shape: Option<crate::layout::Shape>,
    /// The tab's arrangement could not be read. Drawn as an outline with a
    /// question mark rather than as a guess.
    pub unreadable: bool,
    pub marked: Vec<String>,
    /// Short identifiers written into the panes, e.g. `1`, `2`, `3`.
    pub labels: Vec<(String, String)>,
    /// Draw a second sheet behind the diagram, so the panel reads as a tab
    /// that does not exist yet rather than as the one already on screen.
    pub stacked: bool,
    /// Name written into the sheet behind, when that sheet stands for a second
    /// real tab rather than for depth.
    pub behind: Option<String>,
}

impl Panel {
    pub fn new(caption: impl Into<String>, shape: crate::layout::Shape) -> Self {
        Self {
            caption: caption.into(),
            shape: Some(shape),
            marked: Vec::new(),
            labels: Vec::new(),
            stacked: false,
            behind: None,
            unreadable: false,
        }
    }

    /// A panel for a tab whose arrangement Herdr would not report.
    ///
    /// Drawing a single box instead — the old fallback — said "one pane" about
    /// a tab that may have five. A picture that is merely unavailable is much
    /// better than one that is wrong.
    pub fn unreadable(caption: impl Into<String>) -> Self {
        Self {
            unreadable: true,
            ..Self::gone(caption)
        }
    }

    /// A panel for something that will not exist afterwards.
    pub fn gone(caption: impl Into<String>) -> Self {
        Self {
            caption: caption.into(),
            shape: None,
            marked: Vec::new(),
            labels: Vec::new(),
            stacked: false,
            behind: None,
            unreadable: false,
        }
    }

    pub fn marking(mut self, marked: Vec<String>) -> Self {
        self.marked = marked;
        self
    }

    /// Draw the diagram as the front of a stack of two sheets.
    ///
    /// A new tab and the current one otherwise look identical, and captions
    /// cannot carry the difference: a tab created from a project directory is
    /// named after it, so both sides of an Extract read the same word. The
    /// second outline says "another tab" in a way a name cannot.
    pub fn stacked(mut self) -> Self {
        self.stacked = true;
        self
    }

    /// Stack, and name the sheet behind — for an operation that really does
    /// make two tabs.
    pub fn behind(mut self, caption: impl Into<String>) -> Self {
        self.stacked = true;
        self.behind = Some(caption.into());
        self
    }

    /// Put compact identifiers inside panes in the diagram.
    pub fn labeling(mut self, labels: Vec<(String, String)>) -> Self {
        self.labels = labels;
        self
    }
}

/// What an operation would leave behind, drawn.
///
/// Several panels rather than one because most of these operations move a pane
/// *between* tabs: a single picture can only show where it came from or where
/// it lands, and the useful thing is the pair.
#[derive(Debug, Clone)]
pub struct Preview {
    pub panels: Vec<Panel>,
    /// Lines printed under the boxes, naming what is in them.
    ///
    /// A box can hold `p5` and nothing more: a conversation title is long,
    /// often CJK, and would push the walls out of true. The names go under the
    /// picture instead, where they have a full line each and can be truncated
    /// by display width rather than by character count.
    pub legend: Vec<String>,
}

impl Preview {
    pub fn new(panels: Vec<Panel>) -> Self {
        Self {
            panels,
            legend: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct View {
    pub title: String,
    pub subtitle: Option<String>,
    /// Tab strip drawn under the subtitle. Empty hides the line.
    pub tabs: Vec<Chip>,
    pub rows: Vec<Row>,
    pub footer: Option<String>,
    /// Index into `rows` currently under the arrow-key cursor.
    pub cursor: Option<usize>,
    pub accent: Color,
    /// Fixed block drawn just above the footer.
    ///
    /// Kept out of the list on purpose: a picture that appears under whichever
    /// row is highlighted pushes every row below it down, so the list moves
    /// under the reader as they arrow through it. Reserving the space instead
    /// costs a few lines and keeps every row exactly where it was.
    pub preview: Option<Preview>,
    /// Whether to keep room for a preview even on rows that have none, so the
    /// list cannot reflow as the cursor moves.
    pub reserve_preview: bool,
    /// Shown in the preview area for rows with nothing to draw.
    pub no_preview: String,
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
            tabs: Vec::new(),
            rows: Vec::new(),
            footer: None,
            cursor: None,
            accent: Color::Cyan,
            preview: None,
            reserve_preview: false,
            no_preview: String::new(),
            query: None,
            match_count: None,
        }
    }

    pub fn tabs(mut self, tabs: Vec<Chip>) -> Self {
        self.tabs = tabs;
        self
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
    /// The terminal answered yes to the Kitty keyboard protocol, so modified
    /// Enter presses are distinguishable.
    enhanced: bool,
    /// First screen line of each rendered row, paired with that row's index in
    /// the full list, so a mouse click can be turned back into the row the user
    /// aimed at even when the list is scrolled.
    row_lines: Vec<(u16, usize)>,
    /// Index of the first row drawn. Kept between renders so the list only
    /// scrolls when the cursor would otherwise leave the screen, rather than
    /// sliding under the reader on every keypress.
    scroll: usize,
}

/// Whether the terminal can report Shift+Enter, decided without a round trip
/// where that is possible.
///
/// `supports_keyboard_enhancement` writes a query and waits for an answer, and
/// crossterm gives it two full seconds before giving up. That is the whole of
/// this plugin's startup time: measured inside a Herdr pane, the first frame
/// took 2014ms while every piece of real work — panes, tabs, layouts, the
/// preview — came to about 6ms.
///
/// Herdr renders the pane itself and does speak the protocol, so inside Herdr
/// the answer is known in advance and the query is pure cost. `HERDR_KEYBOARD`
/// is the escape hatch if that ever stops being true, and anywhere else the
/// question is still asked the slow way.
fn supports_enhancement() -> bool {
    match std::env::var("HERDR_KEYBOARD").ok().as_deref() {
        Some("enhanced") => return true,
        Some("plain") => return false,
        _ => {}
    }
    if std::env::var_os("HERDR_ENV").is_some() {
        return true;
    }
    matches!(terminal::supports_keyboard_enhancement(), Ok(true))
}

impl Term {
    pub fn open() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        queue!(out, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;

        // Ask for the Kitty keyboard protocol, without which Shift+Enter is
        // indistinguishable from Enter: a plain terminal sends the same CR for
        // both. Ghostty, kitty and WezTerm answer yes; Terminal.app does not,
        // and there the Shift+Enter bindings simply fall back to Enter.
        let enhanced = supports_enhancement();
        if enhanced {
            queue!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }

        out.flush()?;
        Ok(Self {
            out,
            active: true,
            enhanced,
            row_lines: Vec::new(),
            scroll: 0,
        })
    }

    /// Whether the terminal can tell Shift+Enter from Enter.
    ///
    /// Callers use this to word their own footer honestly rather than to
    /// advertise a key that will not arrive.
    pub fn distinguishes_modified_enter(&self) -> bool {
        self.enhanced
    }

    pub fn close(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        if self.enhanced {
            let _ = queue!(self.out, PopKeyboardEnhancementFlags);
        }
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
        if !view.tabs.is_empty() {
            // Drawn piece by piece rather than as one string: the active chip
            // is the only thing on this line that should catch the eye, and
            // that needs its own colours.
            let mut column = 0usize;
            queue!(self.out, cursor::MoveTo(0, line))?;
            for chip in &view.tabs {
                let text = format!(" {} ", chip.label);
                if column + text.chars().count() > width {
                    break;
                }
                column += text.chars().count();
                if chip.active {
                    queue!(
                        self.out,
                        SetAttribute(Attribute::Reverse),
                        SetForegroundColor(view.accent),
                        Print(text),
                        SetAttribute(Attribute::Reset),
                        ResetColor
                    )?;
                } else {
                    queue!(
                        self.out,
                        SetForegroundColor(Color::DarkGrey),
                        Print(text),
                        ResetColor
                    )?;
                }
            }
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
            .map(|row| usize::from(row.detail.is_some()) + 1)
            .collect();
        let total: usize = heights.iter().sum();
        // Everything above the footer and the preview belongs to the list.
        // The preview keeps a floor so a long list cannot squeeze it away; any
        // room the list does not use goes to the picture, which is why the
        // diagram is drawn at render time rather than built in advance.
        let reserved = if view.reserve_preview {
            PREVIEW_MIN + 1
        } else {
            0
        };
        let available = (height.saturating_sub(line).saturating_sub(1) as usize)
            .saturating_sub(reserved);
        let overflowing = total > available;
        // When the list overflows, one line goes to the "more above / below"
        // marker so the reader knows the rest exists.
        let budget = if overflowing {
            available.saturating_sub(1)
        } else {
            available
        };

        self.scroll = scroll_for(self.scroll, view.cursor, &heights, budget);
        let align = secondary_column(&view.rows, width);

        let mut used = 0usize;
        for (index, row) in view.rows.iter().enumerate().skip(self.scroll) {
            if used + heights[index] > budget {
                break;
            }
            used += heights[index];
            self.row_lines.push((line, index));
            let selected = view.cursor == Some(index);
            self.render_row(row, selected, width, &mut line, height, view.accent, align)?;
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

        if view.reserve_preview {
            // Everything between the list and the footer.
            let top = line + 1;
            let room = height.saturating_sub(1).saturating_sub(top) as usize;
            let lines = match (&view.preview, room >= 5) {
                (Some(preview), true) => preview_lines(preview, room, width as usize),
                _ => vec![view.no_preview.clone()],
            };
            for (offset, text) in lines.iter().enumerate() {
                let row = top + offset as u16;
                if row >= height.saturating_sub(1) {
                    break;
                }
                queue!(self.out, cursor::MoveTo(0, row), Print("  "))?;
                // A label written inside a filled pane is still part of that
                // pane, so it takes the fill's colour. Colouring only the fill
                // character left the name as a dark hole in the middle of a
                // bright rectangle — it read as a gap rather than as a name.
                //
                // Walls close the run: a label in the *unfilled* pane next
                // door must stay dim, or every pane would look selected.
                let mut in_fill = false;
                for ch in truncate(text, width.saturating_sub(2)).chars() {
                    if is_wall(ch) {
                        in_fill = false;
                    } else if ch == crate::layout::HIGHLIGHT {
                        in_fill = true;
                    }
                    let colour = if in_fill {
                        view.accent
                    } else {
                        Color::DarkGrey
                    };
                    queue!(self.out, SetForegroundColor(colour), Print(ch))?;
                }
                queue!(self.out, ResetColor)?;
            }
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
        align: usize,
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
                    // Pad to the column shared by every row on screen, so the
                    // descriptions read down as one block instead of stepping
                    // in and out with the length of each name.
                    let pad = align.saturating_sub(columns(&text));
                    text.push_str(&" ".repeat(pad));
                    text.push_str(&format!("  {secondary}"));
                }
                if selected {
                    queue!(self.out, SetAttribute(Attribute::Bold))?;
                }

                // Everything already printed on this line: the cursor column,
                // the hotkey field, and the glyph if there is one.
                let used = 2 + 3 + usize::from(row.glyph.is_some()) * 2;
                let available = width.saturating_sub(used + 1);
                let reserved = row
                    .trailing
                    .as_ref()
                    .map_or(0, |trailing| columns(trailing) + 2);

                let body = truncate(&text, available.saturating_sub(reserved));
                queue!(self.out, Print(&body), SetAttribute(Attribute::Reset))?;

                if let Some(trailing) = &row.trailing {
                    let pad = available.saturating_sub(columns(&body) + columns(trailing));
                    queue!(
                        self.out,
                        Print(" ".repeat(pad)),
                        SetForegroundColor(Color::DarkGrey),
                        Print(trailing),
                        ResetColor
                    )?;
                }
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
                        KeyCode::Enter if modifiers.contains(KeyModifiers::ALT) => Key::AltEnter,
                        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => Key::ShiftEnter,
                        KeyCode::Enter => Key::Enter,
                        KeyCode::Up => Key::Up,
                        KeyCode::Down => Key::Down,
                        KeyCode::Tab => Key::Tab,
                        KeyCode::BackTab => Key::BackTab,
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

/// The column every row's secondary text starts at.
///
/// Rows are drawn one at a time, so without this the descriptions begin
/// wherever each name happens to end and the column staggers down the screen.
///
/// Alignment is abandoned when the longest name takes more than a third of the
/// width: in a filtered list of conversations the names run to whatever length
/// they run to, and padding every row out to the longest would open a gutter
/// wider than the text on either side of it.
fn secondary_column(rows: &[Row], width: usize) -> usize {
    let widest = rows
        .iter()
        .filter(|row| row.kind == RowKind::Item && row.secondary.is_some())
        .map(|row| columns(&row.primary))
        .max()
        .unwrap_or(0);
    if widest > width / 3 {
        0
    } else {
        widest
    }
}

/// Box-drawing characters that bound a pane in a diagram.
fn is_wall(ch: char) -> bool {
    matches!(
        ch,
        '│' | '─' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼'
            | '╎' | '╌'
    )
}

/// Drawn between panels: the operation reads left to right.
const ARROW: &str = "  →  ";

/// Lay captioned diagrams out side by side.
///
/// Captions sit on their own line above the boxes rather than beside them, so
/// a long caption cannot push the diagrams out of alignment with each other.
/// Lay a preview out in the rows it has been given.
///
/// Separated from drawing so it can be checked: the invariant is that the
/// result never uses more than `room` lines and never runs wider than the
/// pane, whatever the panels hold.
#[doc(hidden)]
pub fn preview_lines_for_test(preview: &Preview, room: usize, width: usize) -> Vec<String> {
    preview_lines(preview, room, width)
}

fn preview_lines(preview: &Preview, room: usize, width: usize) -> Vec<String> {
    // Everything that shares the space is taken off the top, and the boxes get
    // what is left. Drawing first and hoping it fits is how the sheet behind
    // and the last line of the legend ended up below the bottom of the pane.
    //
    // In the very shortest pane there is no row to spare for the sheet behind,
    // and a box drawn one row shorter would not be a box: the stack is the
    // part that gives way. The captions cost nothing either way — they are
    // written into the frames themselves.
    let stacked = preview.panels.iter().any(|panel| panel.stacked) && room > SMALLEST_BOX;
    let reserved = usize::from(stacked);

    // The boxes are never drawn smaller than `SMALLEST_BOX` rows: the drawing
    // code clamps there, so budgeting for less does not shrink the picture, it
    // just draws part of it off the screen. The legend takes what is spare,
    // and is dropped entirely when nothing is.
    let spare = room.saturating_sub(reserved).saturating_sub(SMALLEST_BOX);
    let legend: Vec<String> = preview.legend.iter().take(spare).cloned().collect();
    let room = room.saturating_sub(reserved).saturating_sub(legend.len());

    let cells_h = (room.saturating_sub(1) / 2).max(SMALLEST_CELLS);
    let rows = 2 * cells_h + 1;
    // Shaped like a screen rather than stretched to the pane. A terminal cell
    // is about twice as tall as it is wide, so 16:9 on screen is roughly 3.5
    // columns per line — running to the full width instead gives a letterbox
    // nothing on a real monitor looks like.
    let count = preview.panels.len().max(1);
    let arrows = count.saturating_sub(1) * ARROW.chars().count();
    let budget = width.saturating_sub(4).saturating_sub(arrows);
    let across = (rows * 32 / 9).min(budget / count);
    let cells_w = across.saturating_sub(1) / 2;

    let panels: Vec<Panel> = preview
        .panels
        .iter()
        .map(|panel| Panel {
            stacked: panel.stacked && stacked,
            ..panel.clone()
        })
        .collect();
    let mut lines = draw_panels(&panels, cells_w, cells_h);
    lines.extend(legend);
    lines
}

fn draw_panels(panels: &[Panel], cells_w: usize, cells_h: usize) -> Vec<String> {
    let drawn: Vec<Vec<String>> = panels
        .iter()
        .map(|panel| match &panel.shape {
            // Too many panes for the space: below this the rasteriser drops
            // some of them, and a diagram missing three of ten boxes still
            // looks like a complete diagram. Say the number instead of drawing
            // a tab that is not the reader's tab.
            Some(shape) if !shape.renders_in(cells_w, cells_h) => centre(
                blank_box(cells_w, cells_h),
                &format!("{} panes", shape.pane_ids().len()),
            ),
            Some(shape) => {
                let marked: Vec<&str> = panel.marked.iter().map(String::as_str).collect();
                let labels: Vec<(&str, &str)> = panel
                    .labels
                    .iter()
                    .map(|(pane, label)| (pane.as_str(), label.as_str()))
                    .collect();
                shape.diagram_marking_labeled(cells_w, cells_h, &marked, &labels)
            }
            None if panel.unreadable => centre(blank_box(cells_w, cells_h), "?"),
            None => empty_box(cells_w, cells_h),
        })
        .zip(panels)
        .map(|(lines, panel)| {
            let lines = title(lines, &panel.caption);
            if panel.stacked {
                stack(lines, panel.behind.as_deref())
            } else {
                lines
            }
        })
        .collect();

    let widths: Vec<usize> = drawn
        .iter()
        .map(|lines| lines.iter().map(|l| columns(l)).max().unwrap_or(0))
        .collect();

    let mut out = Vec::new();
    let height = drawn.iter().map(Vec::len).max().unwrap_or(0);
    // The arrow belongs on the middle line, where the eye is.
    let middle = height / 2;
    for row in 0..height {
        let joiner = if row == middle {
            ARROW.to_string()
        } else {
            " ".repeat(ARROW.chars().count())
        };
        let cells: Vec<String> = drawn
            .iter()
            .zip(&widths)
            .map(|(lines, width)| pad(lines.get(row).map(String::as_str).unwrap_or(""), *width))
            .collect();
        out.push(cells.join(&joiner));
    }
    out
}

/// Put a second sheet behind a diagram, offset up and to the left.
///
/// Only the back sheet's top edge and left wall are ever visible; the rest is
/// covered by the front one. That is what makes it read as depth rather than
/// as two diagrams — a whole second outline beside the first would just be
/// another tab, which is the opposite of what this says.
fn stack(front: Vec<String>, behind: Option<&str>) -> Vec<String> {
    let Some(width) = front.iter().map(|line| columns(line)).max() else {
        return front;
    };
    if front.len() < 3 || width < 3 {
        return front;
    }
    let mut out = Vec::with_capacity(front.len() + 1);
    let back = format!("\u{250c}{}\u{2510}", "\u{2500}".repeat(width - 2));
    out.push(match behind {
        Some(name) => write_title(&back, name),
        None => back,
    });
    let last = front.len() - 1;
    for (row, line) in front.into_iter().enumerate() {
        // The back sheet's bottom-left corner sits one row above the front's,
        // which is the only place its bottom edge is not hidden.
        let edge = if row + 1 == last { '\u{2514}' } else { '\u{2502}' };
        let edge = if row == last { ' ' } else { edge };
        out.push(format!("{edge}{line}"));
    }
    out
}

/// Write a panel's name into its own top border.
///
/// A caption on a line of its own reads as a heading over a picture; written
/// into the frame it reads as the tab's own name, which is what it is — and it
/// is the only way two stacked sheets can each say what they are, since there
/// is one line above them and two names to put there.
fn title(mut lines: Vec<String>, caption: &str) -> Vec<String> {
    if caption.is_empty() {
        return lines;
    }
    if let Some(top) = lines.first_mut() {
        *top = write_title(top, caption);
    }
    lines
}

/// Overwrite a border's dashes with `  name  `, two cells in from the corner.
///
/// Only dashes are consumed, and only while they last: a name longer than the
/// frame is cut rather than pushing the corner out of place, because every
/// line of the diagram has to keep the same width or the walls stop lining up.
fn write_title(border: &str, name: &str) -> String {
    const INDENT: usize = 3;
    let cells: Vec<char> = border.chars().collect();
    let room = cells.len().saturating_sub(INDENT + 2);
    if room < 3 {
        return border.to_string();
    }
    let text = truncate(name, room);
    let mut out = String::new();
    let mut column = 0usize;
    let mut written = 0usize;
    let mut taken = 0usize;
    for ch in cells {
        if column >= INDENT && written < columns(&text) {
            // One dash per column the name occupies, so a wide character eats
            // two of them and the border keeps its length.
            if taken == 0 {
                out.push_str(&text);
            }
            taken += 1;
            written += 1;
            column += 1;
            continue;
        }
        out.push(ch);
        column += 1;
    }
    out
}

/// Write one short string in the middle of a drawn box.
fn centre(mut lines: Vec<String>, text: &str) -> Vec<String> {
    let row = lines.len() / 2;
    let Some(line) = lines.get_mut(row) else {
        return lines;
    };
    let width = columns(line);
    let start = width.saturating_sub(columns(text)) / 2;
    let mut out = String::new();
    for (column, ch) in line.chars().enumerate() {
        if column == start {
            out.push_str(text);
        } else if column > start && column < start + text.chars().count() {
            continue;
        } else {
            out.push(ch);
        }
    }
    *line = out;
    lines
}

/// A dashed outline, for a tab that will not be there afterwards.
fn empty_box(cells_w: usize, cells_h: usize) -> Vec<String> {
    outline(cells_w, cells_h, '╌', '╎')
}

/// A solid outline with nothing in it, for a tab that stays but cannot be
/// drawn — too many panes for the space, or a layout Herdr would not report.
///
/// Solid rather than dashed on purpose: dashes are this preview's word for
/// "gone", and a tab the reader still has must not wear it.
fn blank_box(cells_w: usize, cells_h: usize) -> Vec<String> {
    outline(cells_w, cells_h, '─', '│')
}

fn outline(cells_w: usize, cells_h: usize, horizontal: char, vertical: char) -> Vec<String> {
    let width = 2 * cells_w.max(2) + 1;
    let height = 2 * cells_h.max(2) + 1;
    let bar = horizontal.to_string().repeat(width.saturating_sub(2));
    let mut out = vec![format!("┌{bar}┐")];
    for _ in 1..height.saturating_sub(1) {
        out.push(format!(
            "{vertical}{}{vertical}",
            " ".repeat(width.saturating_sub(2))
        ));
    }
    out.push(format!("└{bar}┘"));
    out
}

fn pad(text: &str, width: usize) -> String {
    let used = columns(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(used)))
}

/// Smallest preview worth drawing; the list may not take these lines.
const PREVIEW_MIN: usize = 6;

/// The fewest cells a diagram is ever drawn at, and the rows that costs.
///
/// `Shape::diagram` clamps to two cells, so asking for one still produces five
/// lines. Budgeting for fewer does not shrink the picture — it just draws part
/// of it past the bottom of the pane.
const SMALLEST_CELLS: usize = 2;
const SMALLEST_BOX: usize = 2 * SMALLEST_CELLS + 1;

/// Display width, counting CJK as two columns.
fn columns(text: &str) -> usize {
    text.chars().map(char_width).sum()
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

use crate::layout::char_width;

#[cfg(test)]
mod tests {
    use super::*;

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
    fn descriptions_line_up_on_the_longest_name() {
        let rows = vec![
            Row::item("Swap").secondary("a"),
            Row::item("Merge Tab").secondary("b"),
            Row::header("ignored, and much much longer than any item"),
            Row::item("no description here at all, also long"),
        ];
        // Only Item rows that actually have a description are measured.
        assert_eq!(secondary_column(&rows, 80), columns("Merge Tab"));
    }

    #[test]
    fn alignment_gives_up_rather_than_open_a_gutter() {
        let rows = vec![Row::item("a name far too long to align against").secondary("x")];
        assert_eq!(secondary_column(&rows, 40), 0);
        // Same rows, more room: worth aligning again.
        assert!(secondary_column(&rows, 200) > 0);
    }

    #[test]
    fn a_trailing_value_is_reserved_room_before_the_body_is_cut() {
        // The body must never be allowed to eat the right-hand column, or the
        // tool name would vanish on exactly the rows with the longest titles.
        let available = 40usize;
        let trailing = "(Claude)";
        let reserved = columns(trailing) + 2;
        let body = truncate(
            "セッション一覧を表示して選択すると開くプラグインを作る",
            available - reserved,
        );
        assert!(columns(&body) + reserved <= available);
        let pad = available - columns(&body) - columns(trailing);
        assert!(pad >= 2, "at least a gap must remain, got {pad}");
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

#[cfg(test)]
mod stack_tests {
    use super::*;

    #[test]
    fn a_stacked_panel_shows_a_second_sheet_behind_the_first() {
        let shape = crate::layout::Shape::pane("p1");
        let front = shape.diagram_marking_labeled(8, 3, &["p1"], &[("p1", "p1")]);
        let width = front.iter().map(|l| columns(l)).max().unwrap();
        let stacked = stack(front.clone(), None);

        // One row taller and one column wider: the sheet behind peeks out at
        // the top and down the left.
        assert_eq!(stacked.len(), front.len() + 1);
        assert_eq!(columns(&stacked[0]), width);
        assert!(stacked[0].starts_with('\u{250c}') && stacked[0].ends_with('\u{2510}'));
        // Every original line is still there, shifted right by the back wall.
        for (row, line) in front.iter().enumerate() {
            assert!(stacked[row + 1].ends_with(line), "row {row}");
        }
        // The back sheet closes one row above the front's bottom edge.
        assert!(stacked[stacked.len() - 2].starts_with('\u{2514}'));
        assert!(stacked[stacked.len() - 1].starts_with(' '));
    }

    #[test]
    fn a_diagram_too_small_to_stack_is_left_alone() {
        let front = vec!["ab".to_string(), "cd".to_string()];
        assert_eq!(stack(front.clone(), None), front);
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    fn boxed() -> Vec<String> {
        crate::layout::Shape::pane("p1").diagram_marking_labeled(9, 3, &[], &[])
    }

    #[test]
    fn a_name_is_written_into_the_top_border() {
        let lines = title(boxed(), "herdr-plugins");
        assert!(lines[0].starts_with("\u{250c}\u{2500}\u{2500}herdr-plugins"));
        assert!(lines[0].ends_with('\u{2510}'));
        // Every line keeps its width, or the walls stop lining up.
        let width = columns(&lines[0]);
        assert!(lines.iter().all(|line| columns(line) == width));
    }

    #[test]
    fn a_name_too_long_for_the_frame_is_cut_rather_than_widening_it() {
        let plain = boxed();
        let lines = title(plain.clone(), "a-very-long-project-name-indeed");
        assert_eq!(columns(&lines[0]), columns(&plain[0]));
        assert!(lines[0].ends_with('\u{2510}'));
    }

    #[test]
    fn two_stacked_sheets_each_carry_their_own_name() {
        // One line above the boxes, two names to put there: the frames are the
        // only place both can be said.
        let lines = stack(title(boxed(), "dotfiles"), Some("herdr-plugins"));
        assert!(lines[0].contains("herdr-plugins"));
        assert!(lines[1].contains("dotfiles"));
        assert!(!lines[0].contains("dotfiles"));
    }

    #[test]
    fn an_unnamed_sheet_behind_is_plain_depth() {
        let lines = stack(title(boxed(), "dotfiles"), None);
        assert!(lines[0].chars().all(|ch| "\u{250c}\u{2500}\u{2510}".contains(ch)));
    }
}

#[cfg(test)]
mod crowding_tests {
    use super::*;
    use crate::layout::{Shape, Side};

    fn many(panes: usize) -> Shape {
        let mut shape = Shape::pane("p0");
        for n in 1..panes {
            shape.split(
                &format!("p{}", n - 1),
                &format!("p{n}"),
                if n % 2 == 0 { Side::Down } else { Side::Right },
            );
        }
        shape
    }

    #[test]
    fn a_tab_with_more_panes_than_the_space_holds_says_so() {
        let panel = Panel::new("busy", many(10));
        let lines = draw_panels(&[panel], 12, 3);
        assert!(lines.iter().any(|line| line.contains("10 panes")));
        // Still a box, still the tab's name in its border.
        assert!(lines[0].contains("busy"));
    }

    #[test]
    fn a_crowded_preview_keeps_every_line_the_same_width() {
        for cells_h in 2..8 {
            let panel = Panel::new("busy", many(10)).labeling(
                (0..10)
                    .map(|n| (format!("p{n}"), format!("p{n}")))
                    .collect(),
            );
            let lines = draw_panels(&[panel], 12, cells_h);
            let width = columns(&lines[0]);
            for line in &lines {
                assert_eq!(columns(line), width, "cells_h {cells_h}: {line}");
            }
        }
    }

    #[test]
    fn a_preview_never_uses_more_rows_than_it_was_given() {
        // The sheet behind and the last line of the legend used to be drawn
        // past the bottom of the pane, because the space was divided up after
        // the picture had already been sized.
        for panes in [1usize, 2, 4, 10] {
            for legend in 0..6 {
                for stacked in [false, true] {
                    for room in 5..24 {
                        let mut panel = Panel::new("tab", many(panes));
                        if stacked {
                            panel = panel.behind("other");
                        }
                        let mut preview = Preview::new(vec![panel]);
                        preview.legend =
                            (0..legend).map(|n| format!("p{n}: something")).collect();
                        let lines = preview_lines(&preview, room, 80);
                        assert!(
                            lines.len() <= room,
                            "{panes} panes, legend {legend}, stacked {stacked}, room {room}: {} lines",
                            lines.len()
                        );
                        for line in &lines {
                            assert!(columns(line) <= 80, "too wide: {line}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_pane_too_short_for_a_legend_drops_it_rather_than_the_picture() {
        let mut preview = Preview::new(vec![Panel::new("tab", many(2))]);
        preview.legend = (0..4).map(|n| format!("p{n}: something")).collect();
        // Five rows is exactly one box and nothing else.
        assert_eq!(preview_lines(&preview, 5, 80).len(), 5);
        // Room for the box and two names.
        assert_eq!(preview_lines(&preview, 7, 80).len(), 7);
    }

    #[test]
    fn a_tab_that_does_fit_is_drawn_normally() {
        let panel = Panel::new("calm", many(3));
        let lines = draw_panels(&[panel], 12, 6);
        assert!(!lines.iter().any(|line| line.contains("panes")));
    }
}
