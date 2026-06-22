//! The cell grid: styled cells, a cursor, and the intrinsic screen operations
//! that VT sequences map onto. No `vte` here; [`crate::screen`] does the parsing.

// ========================================================================
// Data Structures
// ========================================================================

/// A fixed-size grid of styled cells with a cursor, a current pen style, and a
/// scrollback history ring. Scrolling up reveals previously scrolled-off rows.
#[derive(Clone, Debug)]
pub struct Grid {
    /// Intern ID of the currently active OSC 8 hyperlink; 0 = none.
    active_link: u16,
    alt_buffer: Option<Box<AltBuffer>>,
    bracketed_paste: bool,
    cells: Vec<Cell>,
    cols: usize,
    cursor: Cursor,
    focus_event: bool,
    /// Intern table for OSC 8 URLs; index 0 is always the empty string (no link).
    link_table: Vec<String>,
    max_scrollback: usize,
    mouse_button: bool,
    mouse_drag: bool,
    mouse_sgr: bool,
    rows: usize,
    saved_cursor: Option<Cursor>,
    scroll_bottom: usize,
    scroll_offset: usize,
    scroll_top: usize,
    scrollback: Vec<Vec<Cell>>,
    style: Style,
}

/// One screen cell: a character and its styling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

/// A cell's visual attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub background: Color,
    pub bold: bool,
    pub foreground: Color,
    pub italic: bool,
    /// Intern ID into [`Grid::link_table`]; 0 means no hyperlink.
    pub link: u16,
    pub underline: bool,
}

/// A foreground or background color.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Color {
    /// The terminal's default fg/bg.
    #[default]
    Default,
    /// A 256-color palette index.
    Indexed(u8),
    /// A 24-bit true color.
    Rgb(RgbColor),
}

/// A 24-bit color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Which region an erase affects, relative to the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EraseMode {
    /// From the start of the region up to and including the cursor.
    ToStart,
    /// From the cursor to the end of the region.
    ToEnd,
    /// The whole region.
    Whole,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    col: usize,
    row: usize,
    shape: CursorShape,
}

// ========================================================================
// CursorShape
// ========================================================================

impl CursorShape {
    /// Interpret a `cursor` config value (`"block"`/`"bar"`/`"underline"`).
    /// Common synonyms (`"beam"`, `"underscore"`) are accepted; unknown values
    /// fall back to `Block` so a typo never produces a missing cursor.
    pub fn from_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "bar" | "beam" | "line" => Self::Bar,
            "underline" | "underscore" => Self::Underline,
            _ => Self::Block,
        }
    }

    /// The canonical config value for this shape (round-trips through
    /// [`Self::from_value`]).
    pub fn as_value(&self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Bar => "bar",
            Self::Underline => "underline",
        }
    }
}

#[derive(Clone, Debug)]
struct AltBuffer {
    cells: Vec<Cell>,
    cursor: Cursor,
    saved_cursor: Option<Cursor>,
    style: Style,
}

// ========================================================================
// Constants
// ========================================================================

const TAB_WIDTH: usize = 8;
pub const MAX_SCROLLBACK: usize = 10_000;

const MODE_ALT_SCREEN: u16 = 1049;
const MODE_BRACKETED_PASTE: u16 = 2004;
const MODE_CURSOR: u16 = 25;
const MODE_MOUSE_BUTTON: u16 = 1000;
const MODE_MOUSE_DRAG: u16 = 1002;
const MODE_MOUSE_SGR: u16 = 1006;
const MODE_FOCUS_EVENT: u16 = 1004;
const MODE_ORIGIN: u16 = 6;

// ========================================================================
// Grid
// ========================================================================

impl Grid {
    /// A blank grid of `cols` x `rows` cells with the cursor at the origin.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            active_link: 0,
            alt_buffer: None,
            bracketed_paste: false,
            cells: vec![Cell::default(); cols * rows],
            cols,
            cursor: Cursor::default(),
            focus_event: false,
            // Index 0 is the sentinel "no link" entry so id 0 always means none.
            link_table: vec![String::new()],
            max_scrollback: MAX_SCROLLBACK,
            mouse_button: false,
            mouse_drag: false,
            mouse_sgr: false,
            rows,
            saved_cursor: None,
            scroll_bottom: rows.saturating_sub(1),
            scroll_offset: 0,
            scroll_top: 0,
            scrollback: Vec::new(),
            style: Style::default(),
        }
    }

    /// Set the maximum number of scrollback rows retained. Must be called before
    /// any output is produced; existing scrollback is not retroactively trimmed.
    pub fn with_max_scrollback(mut self, max: usize) -> Self {
        self.max_scrollback = max.max(1);
        self
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The cursor's (row, col).
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor.row, self.cursor.col)
    }

    /// The cell at (row, col), or `None` if out of bounds.
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.cells.get(self.index(row, col)?)
    }

    /// The grid as text, one line per row (trailing blanks trimmed). For tests
    /// and debugging; the GPU renderer reads cells directly.
    pub fn to_text(&self) -> String {
        let mut lines = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            let mut line = String::with_capacity(self.cols);
            for col in 0..self.cols {
                line.push(self.cells[row * self.cols + col].ch);
            }
            lines.push(line.trim_end().to_string());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// The current pen style, applied to printed cells.
    pub fn style(&self) -> Style {
        self.style
    }

    pub fn set_style(&mut self, style: Style) {
        self.style = style;
    }

    /// Open or close an OSC 8 hyperlink. `None` or an empty string clears the
    /// active link; any other value is interned and stamped into future cells.
    pub fn set_active_link(&mut self, url: Option<&str>) {
        self.active_link = match url {
            None | Some("") => 0,
            Some(u) => self.intern_link(u),
        };
    }

    /// Intern `url` into the link table, returning its ID (>0). Reuses an
    /// existing slot when the same URL has been seen before.
    fn intern_link(&mut self, url: &str) -> u16 {
        if let Some(i) = self.link_table.iter().position(|s| s == url) {
            return i as u16;
        }
        let id = self.link_table.len() as u16;
        self.link_table.push(url.to_string());
        id
    }

    /// Resolve a link ID to its URL. Returns `None` for id 0 (no link).
    pub fn link_url(&self, id: u16) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.link_table.get(id as usize).map(String::as_str)
    }

    /// The hyperlink URL of the visible cell at (row, col), if any.
    pub fn cell_link(&self, row: usize, col: usize) -> Option<&str> {
        let cell = self.visible_cell(row, col)?;
        self.link_url(cell.style.link)
    }

    /// Return the link ID (non-zero) for the given URL, or 0 if it has never
    /// been interned. Used to resolve a URL string back to its rendering ID so
    /// the renderer can highlight all cells belonging to the hovered link.
    pub fn find_link_id(&self, url: &str) -> u16 {
        self.link_table
            .iter()
            .position(|s| s == url)
            .map(|i| i as u16)
            .unwrap_or(0)
    }

    /// Scan the live cell buffer for plain-text `http://` / `https://` patterns
    /// and stamp matching cells with auto-detected link IDs. Cells that already
    /// carry an OSC 8 link are left untouched. Only the live (non-scrollback)
    /// rows are scanned; scrollback is read-only.
    pub fn detect_urls(&mut self) {
        // Phase 1: collect (cell_start_idx, span_len, url_string) triples by
        // reading self.cells without taking any long-lived borrows.
        let mut spans: Vec<(usize, usize, String)> = Vec::new();

        for row in 0..self.rows {
            let row_start = row * self.cols;
            let mut col = 0;
            while col < self.cols {
                let prefix_len = url_prefix_len(&self.cells, row_start, col, self.cols);
                if prefix_len == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                let mut end = col + prefix_len;
                while end < self.cols {
                    let ch = self.cells[row_start + end].ch;
                    if is_url_stop(ch) {
                        break;
                    }
                    end += 1;
                }
                if end > start + prefix_len {
                    let url: String =
                        (start..end).map(|c| self.cells[row_start + c].ch).collect();
                    spans.push((row_start + start, end - start, url));
                }
                col = end;
            }
        }

        // Phase 2: intern collected URLs and stamp cells (link_table borrow ends
        // before each cells access).
        for (cell_start, len, url) in spans {
            let link_id = self.intern_link(&url);
            for i in 0..len {
                let idx = cell_start + i;
                if self.cells[idx].style.link == 0 {
                    self.cells[idx].style.link = link_id;
                }
            }
        }
    }

    /// Print a character at the cursor and advance, wrapping and scrolling as
    /// needed.
    pub fn print(&mut self, ch: char) {
        if self.cursor.col >= self.cols {
            self.cursor.col = 0;
            self.line_feed();
        }
        if let Some(index) = self.index(self.cursor.row, self.cursor.col) {
            self.cells[index] = Cell {
                ch,
                style: Style { link: self.active_link, ..self.style },
            };
        }
        self.cursor.col += 1;
    }

    /// Move to the next row. If inside the scroll region at the bottom margin,
    /// scroll the region up. If outside the scroll region or not at the bottom
    /// margin, just move down.
    pub fn line_feed(&mut self) {
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row + 1 < self.rows {
            self.cursor.row += 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
    }

    pub fn backspace(&mut self) {
        self.cursor.col = self.cursor.col.saturating_sub(1);
    }

    /// Advance the cursor to the next 8-column tab stop.
    pub fn tab(&mut self) {
        let next = (self.cursor.col / TAB_WIDTH + 1) * TAB_WIDTH;
        self.cursor.col = next.min(self.cols.saturating_sub(1));
    }

    /// Move the cursor to (row, col), clamped to the grid.
    pub fn move_to(&mut self, row: usize, col: usize) {
        self.cursor.row = row.min(self.rows.saturating_sub(1));
        self.cursor.col = col.min(self.cols.saturating_sub(1));
    }

    pub fn move_up(&mut self, n: usize) {
        self.cursor.row = self.cursor.row.saturating_sub(n);
    }

    pub fn move_down(&mut self, n: usize) {
        self.cursor.row = (self.cursor.row + n).min(self.rows.saturating_sub(1));
    }

    pub fn move_left(&mut self, n: usize) {
        self.cursor.col = self.cursor.col.saturating_sub(n);
    }

    pub fn move_right(&mut self, n: usize) {
        self.cursor.col = (self.cursor.col + n).min(self.cols.saturating_sub(1));
    }

    /// Erase part of the cursor's line.
    pub fn erase_in_line(&mut self, mode: EraseMode) {
        let (start, end) = self.line_range(mode);
        for col in start..end {
            if let Some(index) = self.index(self.cursor.row, col) {
                self.cells[index] = Cell::default();
            }
        }
    }

    /// Erase part of the display.
    pub fn erase_in_display(&mut self, mode: EraseMode) {
        self.erase_in_line(mode);
        let (first, last) = match mode {
            EraseMode::ToEnd => (self.cursor.row + 1, self.rows),
            EraseMode::ToStart => (0, self.cursor.row),
            EraseMode::Whole => (0, self.rows),
        };
        for row in first..last {
            for col in 0..self.cols {
                if let Some(index) = self.index(row, col) {
                    self.cells[index] = Cell::default();
                }
            }
        }
    }

    /// Scroll the scroll region up by `n` rows. The top row of the region is
    /// saved to the scrollback buffer (only if the region starts at row 0);
    /// blank rows scroll in at the bottom of the region.
    pub fn scroll_up(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let shift = n.min(bottom.saturating_sub(top) + 1);
        if top == 0 && self.alt_buffer.is_none() {
            for row in 0..shift {
                let start = row * self.cols;
                let end = start + self.cols;
                let scrolled: Vec<Cell> = self.cells[start..end].to_vec();
                self.scrollback.push(scrolled);
            }
            if self.scrollback.len() > self.max_scrollback {
                let excess = self.scrollback.len() - self.max_scrollback;
                self.scrollback.drain(0..excess);
            }
        }
        let region_len = bottom + 1 - top;
        if shift < region_len {
            for row in top..=bottom - shift {
                let src = (row + shift) * self.cols;
                let dst = row * self.cols;
                let end = src + self.cols;
                self.cells.copy_within(src..end, dst);
            }
        }
        for row in (bottom + 1 - shift)..=bottom {
            let start = row * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = Cell::default();
            }
        }
        self.scroll_offset = 0;
    }

    /// Scroll the scroll region down by `n` rows. Blank rows scroll in at the
    /// top of the region; the bottom rows are discarded.
    pub fn scroll_down(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let shift = n.min(bottom.saturating_sub(top) + 1);
        for row in (top + shift..=bottom).rev() {
            let src = (row - shift) * self.cols;
            let dst = row * self.cols;
            let end = src + self.cols;
            self.cells.copy_within(src..end, dst);
        }
        for row in top..top + shift {
            let start = row * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = Cell::default();
            }
        }
    }

    /// How many rows of scrollback history are available.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// The current scroll offset (0 = no scroll, at the live view).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Scroll up in history by `n` rows, clamped to the available scrollback.
    pub fn scroll_up_history(&mut self, n: usize) {
        let max = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }

    /// Scroll down in history by `n` rows, clamped to 0.
    pub fn scroll_down_history(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Set the scroll offset directly, clamped to the available scrollback.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.scrollback.len());
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor);
    }

    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.cursor = saved;
        }
    }

    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.scroll_top = top.min(self.rows.saturating_sub(1));
        self.scroll_bottom = bottom.min(self.rows.saturating_sub(1));
        if self.scroll_top > self.scroll_bottom {
            std::mem::swap(&mut self.scroll_top, &mut self.scroll_bottom);
        }
        self.cursor.row = 0;
        self.cursor.col = 0;
    }

    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }

    /// Insert `n` blank lines at the cursor row, shifting existing lines down
    /// within the scroll region. Lines that fall below the bottom margin are lost.
    pub fn insert_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        let shift = n.min(bottom - row + 1);
        for r in (row + shift..=bottom).rev() {
            let src = (r - shift) * self.cols;
            let dst = r * self.cols;
            let end = src + self.cols;
            self.cells.copy_within(src..end, dst);
        }
        for r in row..row + shift {
            let start = r * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = Cell::default();
            }
        }
    }

    /// Delete `n` lines at the cursor row, shifting lines up from below within
    /// the scroll region. Blank lines appear at the bottom margin.
    pub fn delete_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        let shift = n.min(bottom - row + 1);
        for r in row..=bottom - shift {
            let src = (r + shift) * self.cols;
            let dst = r * self.cols;
            let end = src + self.cols;
            self.cells.copy_within(src..end, dst);
        }
        for r in (bottom + 1 - shift)..=bottom {
            let start = r * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = Cell::default();
            }
        }
    }

    /// Insert `n` blank characters at the cursor position, shifting characters
    /// to the right. Characters past the end of the row are lost.
    pub fn insert_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let shift = n.min(self.cols.saturating_sub(col));
        let row_start = row * self.cols;
        let src_start = row_start + col;
        let src_end = row_start + self.cols - shift;
        if src_start < src_end {
            self.cells
                .copy_within(src_start..src_end, src_start + shift);
        }
        for i in src_start..src_start + shift {
            if i < self.cells.len() {
                self.cells[i] = Cell::default();
            }
        }
    }

    /// Delete `n` characters at the cursor position, shifting characters from the
    /// right. Blank characters appear at the end of the row.
    pub fn delete_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let shift = n.min(self.cols.saturating_sub(col));
        let row_start = row * self.cols;
        let dst = row_start + col;
        let src = dst + shift;
        let row_end = row_start + self.cols;
        if src < row_end {
            self.cells.copy_within(src..row_end, dst);
        }
        let clear_start = row_end.saturating_sub(shift);
        for i in clear_start..row_end {
            self.cells[i] = Cell::default();
        }
    }

    /// Switch to the alternate screen buffer, saving the current state.
    pub fn enter_alt_screen(&mut self) {
        if self.alt_buffer.is_some() {
            return;
        }
        self.alt_buffer = Some(Box::new(AltBuffer {
            cells: std::mem::take(&mut self.cells),
            cursor: self.cursor,
            saved_cursor: self.saved_cursor,
            style: self.style,
        }));
        self.cells = vec![Cell::default(); self.cols * self.rows];
        self.cursor = Cursor::default();
        self.saved_cursor = None;
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.active_link = 0;
    }

    /// Switch back to the primary screen buffer, restoring the saved state.
    pub fn leave_alt_screen(&mut self) {
        let Some(alt) = self.alt_buffer.take() else {
            return;
        };
        self.cells = alt.cells;
        self.cursor = alt.cursor;
        self.saved_cursor = alt.saved_cursor;
        self.style = alt.style;
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.active_link = 0;
    }

    pub fn is_alt_screen(&self) -> bool {
        self.alt_buffer.is_some()
    }

    /// Whether bracketed paste mode (CSI ?2004h) is active.
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// Whether any mouse tracking mode is active (button or drag).
    pub fn mouse_tracking(&self) -> bool {
        self.mouse_button || self.mouse_drag
    }

    /// Whether drag tracking (CSI ?1002h) specifically is active.
    pub fn mouse_drag_tracking(&self) -> bool {
        self.mouse_drag
    }

    /// Whether SGR extended mouse mode (CSI ?1006h) is active.
    pub fn mouse_sgr(&self) -> bool {
        self.mouse_sgr
    }

    /// Whether focus event mode (CSI ?1004h) is active.
    pub fn focus_event(&self) -> bool {
        self.focus_event
    }

    /// Current cursor shape set by DECSCUSR.
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor.shape
    }

    /// Top of the scroll region (0-based row).
    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    /// Bottom of the scroll region (0-based row, inclusive).
    pub fn scroll_bottom(&self) -> usize {
        self.scroll_bottom
    }

    /// Set cursor shape (DECSCUSR).
    pub fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.cursor.shape = shape;
    }

    /// Handle DECSET/DECRST for a single mode number. Called from screen.rs
    /// which parses the CSI ? sequences.
    pub fn set_private_mode(&mut self, mode: u16, set: bool) {
        match mode {
            MODE_ALT_SCREEN => {
                if set {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            MODE_BRACKETED_PASTE => self.bracketed_paste = set,
            MODE_CURSOR | MODE_ORIGIN => {}
            MODE_FOCUS_EVENT => self.focus_event = set,
            MODE_MOUSE_BUTTON => self.mouse_button = set,
            MODE_MOUSE_DRAG => self.mouse_drag = set,
            MODE_MOUSE_SGR => self.mouse_sgr = set,
            _ => {}
        }
    }

    /// The effective cell at (row, col), accounting for scroll offset.
    /// When scrolled back, row 0 is the oldest visible scrollback row.
    pub fn visible_cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        if self.scroll_offset > 0 {
            let scrolled_rows = self.scrollback.len() - self.scroll_offset;
            if row < self.scroll_offset.min(self.rows) {
                let sb_index = scrolled_rows + row;
                if sb_index < self.scrollback.len() {
                    return self.scrollback[sb_index].get(col);
                }
                return None;
            }
            let live_row = row - self.scroll_offset.min(self.rows);
            return self.cells.get(live_row * self.cols + col);
        }
        self.cells.get(row * self.cols + col)
    }

    /// The column of the last non-blank cell in visible `row`, or 0 for a blank
    /// row. Lets Normal-mode navigation stop at a line's real end instead of
    /// running into the trailing blank padding of prompts and outputs.
    pub fn visible_line_end(&self, row: usize) -> usize {
        let mut end = 0;
        for col in 0..self.cols {
            if let Some(cell) = self.visible_cell(row, col) {
                if cell.ch != '\0' && !cell.ch.is_whitespace() {
                    end = col;
                }
            }
        }
        end
    }

    /// The last visible row that holds any printed character, or 0 when the
    /// screen is blank. The vertical analog of [`Grid::visible_line_end`]: lets
    /// Normal-mode navigation stop at the real bottom of content instead of
    /// descending into the blank padding below the prompt.
    pub fn last_content_row(&self) -> usize {
        (0..self.rows)
            .rev()
            .find(|&row| self.row_has_content(row))
            .unwrap_or(0)
    }

    /// Whether visible `row` holds any non-blank cell.
    fn row_has_content(&self, row: usize) -> bool {
        (0..self.cols).any(|col| {
            self.visible_cell(row, col)
                .is_some_and(|cell| cell.ch != '\0' && !cell.ch.is_whitespace())
        })
    }

    /// Resize the grid, preserving the top-left overlap and clamping the cursor.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let mut next = vec![Cell::default(); cols * rows];
        for row in 0..rows.min(self.rows) {
            for col in 0..cols.min(self.cols) {
                next[row * cols + col] = self.cells[row * self.cols + col];
            }
        }
        self.cells = next;
        self.cols = cols;
        self.rows = rows;
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
    }

    fn line_range(&self, mode: EraseMode) -> (usize, usize) {
        match mode {
            EraseMode::ToEnd => (self.cursor.col, self.cols),
            EraseMode::ToStart => (0, self.cursor.col + 1),
            EraseMode::Whole => (0, self.cols),
        }
    }

    fn index(&self, row: usize, col: usize) -> Option<usize> {
        if row < self.rows && col < self.cols {
            Some(row * self.cols + col)
        } else {
            None
        }
    }
}

// ========================================================================
// URL detection helpers
// ========================================================================

/// Number of characters in the URL scheme+authority prefix starting at `col`,
/// or 0 if the cell sequence does not begin `https://` or `http://`.
fn url_prefix_len(cells: &[Cell], row_start: usize, col: usize, cols: usize) -> usize {
    let matches = |pat: &[u8]| -> bool {
        pat.iter().enumerate().all(|(i, &b)| {
            cells.get(row_start + col + i).map_or(false, |c| c.ch as u8 == b && c.ch.is_ascii())
        })
    };
    if col + 8 <= cols && matches(b"https://") {
        8
    } else if col + 7 <= cols && matches(b"http://") {
        7
    } else {
        0
    }
}

/// Returns true for characters that terminate a URL in plain terminal text.
fn is_url_stop(ch: char) -> bool {
    ch == '\0'
        || ch == ' '
        || ch == '\t'
        || ch == '"'
        || ch == '\''
        || ch == '<'
        || ch == '>'
        || (ch as u32) < 0x20
}

// ========================================================================
// Cell
// ========================================================================

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_advances_cursor_and_wraps() {
        let mut grid = Grid::new(3, 2);
        for ch in "abcd".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(0, 2).map(|c| c.ch), Some('c'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('d'));
        assert_eq!(grid.cursor(), (1, 1));
    }

    #[test]
    fn test_line_feed_at_bottom_scrolls() {
        let mut grid = Grid::new(2, 2);
        grid.print('a');
        grid.carriage_return();
        grid.line_feed();
        grid.print('b');
        grid.line_feed(); // at bottom row -> scrolls
        assert_eq!(grid.to_text(), "b");
        assert_eq!(grid.cursor().0, 1);
    }

    #[test]
    fn test_move_to_clamps_to_bounds() {
        let mut grid = Grid::new(4, 3);
        grid.move_to(99, 99);
        assert_eq!(grid.cursor(), (2, 3));
    }

    #[test]
    fn test_erase_in_line_to_end_clears_from_cursor() {
        let mut grid = Grid::new(5, 1);
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 2);
        grid.erase_in_line(EraseMode::ToEnd);
        assert_eq!(grid.to_text(), "he");
    }

    #[test]
    fn test_tab_advances_to_next_stop() {
        let mut grid = Grid::new(20, 1);
        grid.print('x');
        grid.tab();
        assert_eq!(grid.cursor(), (0, 8));
    }

    #[test]
    fn test_resize_preserves_top_left() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.resize(2, 2);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some('b'));
        assert_eq!(grid.cols(), 2);
    }

    #[test]
    fn test_backspace_moves_cursor_left() {
        let mut grid = Grid::new(5, 1);
        grid.print('a');
        grid.print('b');
        grid.backspace();
        assert_eq!(grid.cursor(), (0, 1));
        grid.print('X');
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some('X'));
    }

    #[test]
    fn test_carriage_return_resets_to_col_0() {
        let mut grid = Grid::new(5, 1);
        grid.print('a');
        grid.print('b');
        grid.carriage_return();
        assert_eq!(grid.cursor(), (0, 0));
    }

    #[test]
    fn test_scroll_up_shifts_content() {
        let mut grid = Grid::new(2, 3);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('c'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('e'));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_erase_in_display_to_end_clears_from_cursor() {
        let mut grid = Grid::new(4, 2);
        for ch in "abcdefgh".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 2);
        grid.erase_in_display(EraseMode::ToEnd);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(0, 2).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_erase_in_display_to_start_clears_to_cursor() {
        let mut grid = Grid::new(4, 2);
        for ch in "abcdefgh".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 1);
        grid.erase_in_display(EraseMode::ToStart);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 1).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 2).map(|c| c.ch), Some('g'));
    }

    #[test]
    fn test_scroll_up_saves_to_scrollback() {
        let mut grid = Grid::new(3, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1);
        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.scrollback[0][0].ch, 'a');
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('d'));
    }

    #[test]
    fn test_scroll_history_navigates_scrollback() {
        let mut grid = Grid::new(3, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1);
        assert_eq!(grid.scrollback_len(), 1);
        grid.scroll_up_history(1);
        assert_eq!(grid.scroll_offset(), 1);
        grid.scroll_down_history(1);
        assert_eq!(grid.scroll_offset(), 0);
    }

    #[test]
    fn test_scroll_offset_clamps_to_zero() {
        let mut grid = Grid::new(3, 2);
        grid.scroll_down_history(10);
        assert_eq!(grid.scroll_offset(), 0);
    }

    #[test]
    fn test_save_restore_cursor() {
        let mut grid = Grid::new(5, 3);
        grid.move_to(2, 4);
        grid.save_cursor();
        grid.move_to(0, 0);
        assert_eq!(grid.cursor(), (0, 0));
        grid.restore_cursor();
        assert_eq!(grid.cursor(), (2, 4));
    }

    #[test]
    fn test_scroll_region() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdefghijkl".chars() {
            grid.print(ch);
        }
        grid.set_scroll_region(1, 2);
        grid.move_to(2, 0);
        grid.line_feed();
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('g'));
    }

    #[test]
    fn test_reset_scroll_region() {
        let mut grid = Grid::new(3, 4);
        grid.set_scroll_region(1, 2);
        grid.reset_scroll_region();
        assert_eq!(grid.scroll_top, 0);
        assert_eq!(grid.scroll_bottom, 3);
    }

    #[test]
    fn test_insert_lines() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdefghijkl".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 0);
        grid.insert_lines(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some('d'));
        assert_eq!(grid.cell(3, 0).map(|c| c.ch), Some('g'));
    }

    #[test]
    fn test_delete_lines() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdefghijkl".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 0);
        grid.delete_lines(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('g'));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some('j'));
        assert_eq!(grid.cell(3, 0).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_insert_chars() {
        let mut grid = Grid::new(5, 1);
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 1);
        grid.insert_chars(2);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(0, 2).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(0, 3).map(|c| c.ch), Some('e'));
        assert_eq!(grid.cell(0, 4).map(|c| c.ch), Some('l'));
    }

    #[test]
    fn test_delete_chars() {
        let mut grid = Grid::new(5, 1);
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 1);
        grid.delete_chars(2);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some('l'));
        assert_eq!(grid.cell(0, 4).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_scroll_down_inserts_blank_at_top() {
        let mut grid = Grid::new(2, 3);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_down(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some('c'));
    }

    #[test]
    fn test_alt_screen_switches_and_restores() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.enter_alt_screen();
        assert!(grid.is_alt_screen());
        assert_eq!(grid.to_text(), "");
        for ch in "xyz".chars() {
            grid.print(ch);
        }
        grid.leave_alt_screen();
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_set_private_mode_alt_screen() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.set_private_mode(MODE_ALT_SCREEN, true);
        assert!(grid.is_alt_screen());
        grid.set_private_mode(MODE_ALT_SCREEN, false);
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_bracketed_paste_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.bracketed_paste());
        grid.set_private_mode(MODE_BRACKETED_PASTE, true);
        assert!(grid.bracketed_paste());
        grid.set_private_mode(MODE_BRACKETED_PASTE, false);
        assert!(!grid.bracketed_paste());
    }

    #[test]
    fn test_mouse_modes() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());

        grid.set_private_mode(MODE_MOUSE_BUTTON, true);
        assert!(grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());

        grid.set_private_mode(MODE_MOUSE_DRAG, true);
        assert!(grid.mouse_tracking());
        assert!(grid.mouse_drag_tracking());

        grid.set_private_mode(MODE_MOUSE_SGR, true);
        assert!(grid.mouse_sgr());

        grid.set_private_mode(MODE_MOUSE_BUTTON, false);
        grid.set_private_mode(MODE_MOUSE_DRAG, false);
        grid.set_private_mode(MODE_MOUSE_SGR, false);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());
    }

    #[test]
    fn test_visible_line_end_ignores_trailing_blanks() {
        let mut grid = Grid::new(10, 2);
        for ch in "hi".chars() {
            grid.print(ch);
        }
        // "hi" then blank padding: end is the 'i' at col 1, not the grid width.
        assert_eq!(grid.visible_line_end(0), 1);
        // Trailing spaces are blank padding too.
        grid.print(' ');
        grid.print(' ');
        assert_eq!(grid.visible_line_end(0), 1);
        // A row with no printed content reports column 0.
        assert_eq!(grid.visible_line_end(1), 0);
    }

    #[test]
    fn test_last_content_row_ignores_trailing_blank_rows() {
        let mut grid = Grid::new(10, 4);
        // Two rows of content, then blank padding rows below.
        for ch in "first".chars() {
            grid.print(ch);
        }
        grid.line_feed();
        grid.carriage_return();
        for ch in "second".chars() {
            grid.print(ch);
        }
        // Content ends at row 1; rows 2 and 3 are blank padding.
        assert_eq!(grid.last_content_row(), 1);
    }

    #[test]
    fn test_last_content_row_zero_when_blank() {
        let grid = Grid::new(10, 4);
        assert_eq!(grid.last_content_row(), 0);
    }

    #[test]
    fn test_last_content_row_counts_single_char_at_column_zero() {
        let mut grid = Grid::new(10, 3);
        grid.line_feed();
        grid.print('x');
        // A lone 'x' at column 0 of row 1 still counts as content.
        assert_eq!(grid.last_content_row(), 1);
    }

    #[test]
    fn test_cursor_shape_default_and_set() {
        let mut grid = Grid::new(3, 2);
        assert_eq!(grid.cursor_shape(), CursorShape::Block);

        grid.set_cursor_shape(CursorShape::Underline);
        assert_eq!(grid.cursor_shape(), CursorShape::Underline);

        grid.set_cursor_shape(CursorShape::Bar);
        assert_eq!(grid.cursor_shape(), CursorShape::Bar);

        grid.set_cursor_shape(CursorShape::Block);
        assert_eq!(grid.cursor_shape(), CursorShape::Block);
    }

    #[test]
    fn test_focus_event_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.focus_event());
        grid.set_private_mode(MODE_FOCUS_EVENT, true);
        assert!(grid.focus_event());
        grid.set_private_mode(MODE_FOCUS_EVENT, false);
        assert!(!grid.focus_event());
    }

    #[test]
    fn test_scroll_region_accessors() {
        let mut grid = Grid::new(10, 5);
        assert_eq!(grid.scroll_top(), 0);
        assert_eq!(grid.scroll_bottom(), 4);
        grid.set_scroll_region(1, 3);
        assert_eq!(grid.scroll_top(), 1);
        assert_eq!(grid.scroll_bottom(), 3);
        grid.reset_scroll_region();
        assert_eq!(grid.scroll_top(), 0);
        assert_eq!(grid.scroll_bottom(), 4);
    }

    #[test]
    fn test_intern_link_deduplicates_same_url() {
        let mut grid = Grid::new(5, 1);
        let id1 = grid.intern_link("https://a.com");
        let id2 = grid.intern_link("https://a.com");
        assert!(id1 > 0);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_intern_link_different_urls_get_different_ids() {
        let mut grid = Grid::new(5, 1);
        let id1 = grid.intern_link("https://a.com");
        let id2 = grid.intern_link("https://b.com");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_link_url_zero_returns_none() {
        let grid = Grid::new(5, 1);
        assert_eq!(grid.link_url(0), None);
    }

    #[test]
    fn test_set_active_link_stamps_cells() {
        let mut grid = Grid::new(5, 1);
        grid.set_active_link(Some("https://example.com"));
        grid.print('h');
        grid.print('i');
        grid.set_active_link(None);
        grid.print('!');

        let id = grid.cell(0, 0).unwrap().style.link;
        assert!(id > 0);
        assert_eq!(grid.link_url(id), Some("https://example.com"));
        assert_eq!(grid.cell(0, 1).unwrap().style.link, id);
        assert_eq!(grid.cell(0, 2).unwrap().style.link, 0);
    }

    #[test]
    fn test_cell_link_returns_url_for_linked_cell() {
        let mut grid = Grid::new(5, 1);
        grid.set_active_link(Some("https://x.io"));
        grid.print('x');
        assert_eq!(grid.cell_link(0, 0), Some("https://x.io"));
    }

    #[test]
    fn test_cell_link_returns_none_for_unlinked_cell() {
        let mut grid = Grid::new(5, 1);
        grid.print('x');
        assert_eq!(grid.cell_link(0, 0), None);
    }

    #[test]
    fn test_detect_urls_stamps_https_link() {
        let mut grid = Grid::new(40, 1);
        for ch in "visit https://example.com/page here".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        // The 'h' of 'https' starts the link; every char until space gets the ID.
        let link_id = grid.cells[6].style.link;
        assert!(link_id > 0, "https:// cell should have a link ID");
        let url = grid.link_url(link_id).unwrap();
        assert_eq!(url, "https://example.com/page");
        // Cells before and after the URL have no link.
        assert_eq!(grid.cells[0].style.link, 0);
        assert_eq!(grid.cells[30].style.link, 0);
    }

    #[test]
    fn test_detect_urls_http_scheme() {
        let mut grid = Grid::new(30, 1);
        for ch in "http://foo.io end".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        let link_id = grid.cells[0].style.link;
        assert!(link_id > 0);
        assert_eq!(grid.link_url(link_id), Some("http://foo.io"));
        // Space terminates the URL; "end" has no link.
        assert_eq!(grid.cells[14].style.link, 0);
    }

    #[test]
    fn test_detect_urls_does_not_override_osc8_link() {
        let mut grid = Grid::new(40, 1);
        grid.set_active_link(Some("https://osc8.io"));
        for ch in "https://osc8.io".chars() {
            grid.print(ch);
        }
        grid.set_active_link(None);
        // Manually store the osc8 link id before calling detect_urls.
        let osc8_id = grid.cells[0].style.link;
        assert!(osc8_id > 0);
        grid.detect_urls();
        // detect_urls should not replace the existing osc8 link.
        assert_eq!(grid.cells[0].style.link, osc8_id);
    }

    #[test]
    fn test_detect_urls_plain_text_no_urls_unchanged() {
        let mut grid = Grid::new(20, 1);
        for ch in "no links here today".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        for i in 0..19 {
            assert_eq!(grid.cells[i].style.link, 0);
        }
    }
}
