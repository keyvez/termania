use std::sync::{Arc, Mutex};

use crate::config::{Config, PaneConfig};
use crate::pty::Pty;

/// A single terminal cell
#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: CellColor,
    pub bg: CellColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellColor {
    Default,
    Ansi(u8),         // 0-15
    Rgb(u8, u8, u8),  // 24-bit color
    Indexed(u8),      // 256-color
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: CellColor::Default,
            bg: CellColor::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

/// Represents a single terminal instance with screen buffer
pub struct Terminal {
    /// Screen buffer: rows of cells
    cells: Vec<Vec<Cell>>,
    /// Number of columns
    cols: usize,
    /// Number of rows
    rows: usize,
    /// Cursor row
    cursor_row: usize,
    /// Cursor col
    cursor_col: usize,
    /// Current text attributes
    current_attr: Cell,
    /// Window title (set via OSC escape)
    title: String,
    /// Default title
    default_title: String,
    /// Scrollback buffer
    scrollback: Vec<Vec<Cell>>,
    /// Max scrollback lines
    max_scrollback: usize,
    /// PTY file descriptor for writing
    pty: Option<Pty>,
    /// Saved cursor position
    saved_cursor: Option<(usize, usize)>,
    /// Scroll region top
    scroll_top: usize,
    /// Scroll region bottom
    scroll_bottom: usize,
    /// Whether we have pending output
    dirty: bool,
    /// Alternate screen buffer
    alt_cells: Option<Vec<Vec<Cell>>>,
    /// Whether we're on the alternate screen
    on_alt_screen: bool,
    /// VTE parser state machine — persists across read() calls so escape
    /// sequences that span multiple reads are handled correctly
    vte_parser: vte::Parser,
    /// Commands to send once the shell is ready
    pending_initial_commands: Vec<String>,
    /// Timestamp of last PTY output, used to detect shell idle
    last_output_time: Option<std::time::Instant>,
    /// Whether initial commands have been sent
    initial_commands_sent: bool,
    /// Whether an error has been detected in recent output
    has_error: bool,
    /// Timestamp when error was last detected (for auto-clear)
    error_detected_at: Option<std::time::Instant>,
    /// Scroll offset into scrollback (0 = live view, >0 = looking at history)
    scroll_offset: usize,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, title: &str) -> Self {
        let cells = vec![vec![Cell::default(); cols]; rows];
        Self {
            cells,
            cols,
            rows,
            cursor_row: 0,
            cursor_col: 0,
            current_attr: Cell::default(),
            title: title.to_string(),
            default_title: title.to_string(),
            scrollback: Vec::new(),
            max_scrollback: 10000,
            pty: None,
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            dirty: true,
            alt_cells: None,
            on_alt_screen: false,
            vte_parser: vte::Parser::new(),
            pending_initial_commands: Vec::new(),
            last_output_time: None,
            initial_commands_sent: false,
            has_error: false,
            error_detected_at: None,
            scroll_offset: 0,
        }
    }

    /// Queue initial commands to be sent once the shell is idle
    pub fn set_initial_commands(&mut self, commands: Vec<String>) {
        self.pending_initial_commands = commands;
    }

    pub fn set_pty(&mut self, pty: Pty) {
        self.pty = Some(pty);
    }

    /// Resize the terminal buffer and notify the PTY
    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }
        if new_cols == 0 || new_rows == 0 {
            return;
        }

        // When shrinking rows, preserve content around the cursor by pushing
        // excess top lines into scrollback. This prevents the shell prompt
        // from appearing to gain extra blank lines on resize.
        if new_rows < self.rows && !self.on_alt_screen {
            // How many rows we need to discard from the top
            // Keep the cursor at the same visual position relative to the bottom,
            // but don't push more than needed.
            let cursor_bottom_distance = self.rows - 1 - self.cursor_row;
            let new_cursor_row = if cursor_bottom_distance < new_rows {
                new_rows - 1 - cursor_bottom_distance
            } else {
                0
            };
            let rows_to_push = if self.cursor_row >= new_cursor_row {
                self.cursor_row - new_cursor_row
            } else {
                0
            };

            // Push top rows into scrollback
            for i in 0..rows_to_push {
                self.scrollback.push(self.cells[i].clone());
            }
            if self.scrollback.len() > self.max_scrollback {
                let excess = self.scrollback.len() - self.max_scrollback;
                self.scrollback.drain(0..excess);
            }

            // Shift cells up
            if rows_to_push > 0 {
                self.cells.drain(0..rows_to_push);
            }
            self.cursor_row = new_cursor_row;
        } else if new_rows > self.rows {
            // Growing: cursor row stays the same, new empty rows added at bottom
        }

        // Build new buffer
        let mut new_cells = vec![vec![Cell::default(); new_cols]; new_rows];
        for row in 0..new_rows.min(self.cells.len()) {
            for col in 0..new_cols.min(self.cells[row].len()) {
                new_cells[row][col] = self.cells[row][col].clone();
            }
        }
        self.cells = new_cells;

        // Also resize alt screen buffer if present
        if let Some(ref mut alt) = self.alt_cells {
            let mut new_alt = vec![vec![Cell::default(); new_cols]; new_rows];
            for row in 0..new_rows.min(alt.len()) {
                for col in 0..new_cols.min(if alt.is_empty() { 0 } else { alt[0].len() }) {
                    new_alt[row][col] = alt[row][col].clone();
                }
            }
            *alt = new_alt;
        }

        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_row = self.cursor_row.min(new_rows - 1);
        self.cursor_col = self.cursor_col.min(new_cols - 1);
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;

        // Clamp saved cursor to new dimensions
        if let Some((ref mut row, ref mut col)) = self.saved_cursor {
            *row = (*row).min(new_rows - 1);
            *col = (*col).min(new_cols - 1);
        }

        // Notify PTY of new size
        if let Some(pty) = &self.pty {
            pty.resize(new_cols as u16, new_rows as u16);
        }

        self.dirty = true;
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
        self.dirty = true;
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub fn visible_lines(&self) -> Vec<Vec<Cell>> {
        self.cells.clone()
    }

    pub fn visible_text(&self) -> String {
        let mut text = String::new();
        for row in &self.cells {
            let line: String = row.iter().map(|c| c.ch).collect();
            text.push_str(line.trim_end());
            text.push('\n');
        }
        text
    }

    /// Scroll the view up into scrollback history by `n` lines
    pub fn scroll_view_up(&mut self, n: usize) {
        if self.on_alt_screen {
            return; // No scrollback on alt screen
        }
        let max = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + n).min(max);
        self.dirty = true;
    }

    /// Scroll the view down (toward live) by `n` lines
    pub fn scroll_view_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.dirty = true;
    }

    /// Reset scroll to live view
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// Current scroll offset (0 = live view)
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Return lines to display, mixing scrollback and visible cells based on scroll_offset.
    /// When scroll_offset > 0, we show older content from scrollback at the top.
    pub fn scrolled_lines(&self) -> Vec<Vec<Cell>> {
        if self.scroll_offset == 0 || self.on_alt_screen {
            return self.cells.clone();
        }

        let sb_len = self.scrollback.len();
        let offset = self.scroll_offset.min(sb_len);

        // We want to show `self.rows` lines total.
        // The "virtual" buffer is: scrollback ++ cells
        // Live bottom is at index (sb_len + self.rows - 1)
        // We want to show rows ending at (sb_len + self.rows - 1 - offset)
        // i.e., starting at (sb_len + self.rows - offset - self.rows) = (sb_len - offset)
        let virtual_start = sb_len.saturating_sub(offset);

        let mut result = Vec::with_capacity(self.rows);
        for i in 0..self.rows {
            let vi = virtual_start + i;
            if vi < sb_len {
                // From scrollback
                result.push(self.scrollback[vi].clone());
            } else {
                // From visible cells
                let cell_idx = vi - sb_len;
                if cell_idx < self.cells.len() {
                    result.push(self.cells[cell_idx].clone());
                } else {
                    result.push(vec![Cell::default(); self.cols]);
                }
            }
        }

        // Pad or trim columns to match current terminal width
        for row in &mut result {
            row.resize(self.cols, Cell::default());
        }

        result
    }

    /// Get the child process PID from the PTY
    pub fn child_pid(&self) -> Option<u32> {
        self.pty.as_ref().map(|p| p.child_pid())
    }

    pub fn has_error(&self) -> bool {
        self.has_error
    }

    /// Check if the PTY child process has exited
    pub fn is_exited(&self) -> bool {
        match &self.pty {
            Some(pty) => !pty.is_alive(),
            None => true,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Write data from PTY output into the terminal
    pub fn process_output(&mut self, data: &[u8]) {
        // Split self to satisfy borrow checker: we need &mut self for VteParser
        // performer callbacks, but also &mut self.vte_parser for advance().
        // Take the parser out temporarily.
        let mut statemachine = std::mem::replace(&mut self.vte_parser, vte::Parser::new());
        {
            let mut performer = VteParser { terminal: self };
            for byte in data {
                statemachine.advance(&mut performer, *byte);
            }
        }
        self.vte_parser = statemachine;
        self.dirty = true;
    }

    /// Detect error patterns in PTY output
    fn detect_errors(&mut self, data: &[u8]) {
        // Auto-clear error after 10 seconds of no new errors
        if let Some(detected_at) = self.error_detected_at {
            if detected_at.elapsed().as_secs() >= 10 {
                self.has_error = false;
                self.error_detected_at = None;
            }
        }

        if let Ok(text) = std::str::from_utf8(data) {
            let lower = text.to_lowercase();
            let error_patterns = [
                "error:", "error[", "fatal:", "panic:", "traceback",
                "exception:", "failed:", "segfault", "command not found",
                "no such file", "permission denied", "errno",
            ];
            for pattern in &error_patterns {
                if lower.contains(pattern) {
                    self.has_error = true;
                    self.error_detected_at = Some(std::time::Instant::now());
                    break;
                }
            }
        }
    }

    /// Write user input to the PTY
    pub fn write_input(&mut self, data: &[u8]) {
        // Snap to live view when user types
        if self.scroll_offset > 0 {
            self.scroll_offset = 0;
            self.dirty = true;
        }
        if let Some(pty) = &mut self.pty {
            let _ = pty.write(data);
        }
    }

    /// Read available output from the PTY
    pub fn read_output(&mut self) -> Option<Vec<u8>> {
        if let Some(pty) = &mut self.pty {
            pty.read()
        } else {
            None
        }
    }

    /// Poll for new data and process it
    pub fn poll(&mut self) {
        if let Some(data) = self.read_output() {
            self.process_output(&data);
            self.last_output_time = Some(std::time::Instant::now());
        }

        // Send initial commands after the shell has been idle for 500ms.
        // This ensures zsh/bash init (.zshrc, etc.) has finished.
        if !self.initial_commands_sent && !self.pending_initial_commands.is_empty() {
            if let Some(last) = self.last_output_time {
                if last.elapsed().as_millis() >= 500 {
                    self.initial_commands_sent = true;
                    let commands = std::mem::take(&mut self.pending_initial_commands);
                    for cmd in commands {
                        let input = format!("{}\r", cmd);
                        self.write_input(input.as_bytes());
                    }
                }
            }
        }
    }

    fn scroll_up(&mut self) {
        // Move top line into scrollback (only for primary screen)
        if !self.on_alt_screen && self.scroll_top == 0 {
            let line = self.cells[0].clone();
            self.scrollback.push(line);
            if self.scrollback.len() > self.max_scrollback {
                self.scrollback.remove(0);
            }
        }

        // Scroll the region up
        for i in self.scroll_top..self.scroll_bottom {
            self.cells[i] = self.cells[i + 1].clone();
        }
        self.cells[self.scroll_bottom] = vec![Cell::default(); self.cols];
    }

    fn scroll_down(&mut self) {
        for i in (self.scroll_top + 1..=self.scroll_bottom).rev() {
            self.cells[i] = self.cells[i - 1].clone();
        }
        self.cells[self.scroll_top] = vec![Cell::default(); self.cols];
    }

    fn new_line(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up();
        } else if self.cursor_row < self.rows - 1 {
            self.cursor_row += 1;
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.new_line();
        }
        if self.cursor_row < self.rows && self.cursor_col < self.cols {
            self.cells[self.cursor_row][self.cursor_col] = Cell {
                ch,
                fg: self.current_attr.fg,
                bg: self.current_attr.bg,
                bold: self.current_attr.bold,
                italic: self.current_attr.italic,
                underline: self.current_attr.underline,
                inverse: self.current_attr.inverse,
            };
            self.cursor_col += 1;
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            // Clear from cursor to end of screen
            0 => {
                // Clear rest of current line
                for col in self.cursor_col..self.cols {
                    self.cells[self.cursor_row][col] = Cell::default();
                }
                // Clear lines below
                for row in (self.cursor_row + 1)..self.rows {
                    self.cells[row] = vec![Cell::default(); self.cols];
                }
            }
            // Clear from start to cursor
            1 => {
                for row in 0..self.cursor_row {
                    self.cells[row] = vec![Cell::default(); self.cols];
                }
                for col in 0..=self.cursor_col.min(self.cols - 1) {
                    self.cells[self.cursor_row][col] = Cell::default();
                }
            }
            // Clear entire screen
            2 | 3 => {
                for row in 0..self.rows {
                    self.cells[row] = vec![Cell::default(); self.cols];
                }
            }
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        match mode {
            0 => {
                for col in self.cursor_col..self.cols {
                    self.cells[self.cursor_row][col] = Cell::default();
                }
            }
            1 => {
                for col in 0..=self.cursor_col.min(self.cols - 1) {
                    self.cells[self.cursor_row][col] = Cell::default();
                }
            }
            2 => {
                self.cells[self.cursor_row] = vec![Cell::default(); self.cols];
            }
            _ => {}
        }
    }

    fn enter_alt_screen(&mut self) {
        if !self.on_alt_screen {
            self.alt_cells = Some(self.cells.clone());
            self.cells = vec![vec![Cell::default(); self.cols]; self.rows];
            self.on_alt_screen = true;
        }
    }

    fn exit_alt_screen(&mut self) {
        if self.on_alt_screen {
            if let Some(mut cells) = self.alt_cells.take() {
                // Ensure restored buffer matches current dimensions (may have resized)
                if cells.len() != self.rows || cells.first().map_or(true, |r| r.len() != self.cols) {
                    let mut resized = vec![vec![Cell::default(); self.cols]; self.rows];
                    for row in 0..self.rows.min(cells.len()) {
                        for col in 0..self.cols.min(cells[row].len()) {
                            resized[row][col] = cells[row][col].clone();
                        }
                    }
                    cells = resized;
                }
                self.cells = cells;
            }
            self.on_alt_screen = false;
        }
    }
}

/// VTE parser performer that writes into our Terminal
struct VteParser<'a> {
    terminal: &'a mut Terminal,
}

impl<'a> vte::Perform for VteParser<'a> {
    fn print(&mut self, ch: char) {
        self.terminal.put_char(ch);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            // BEL
            0x07 => {}
            // BS
            0x08 => {
                if self.terminal.cursor_col > 0 {
                    self.terminal.cursor_col -= 1;
                }
            }
            // HT (tab)
            0x09 => {
                let next_tab = (self.terminal.cursor_col / 8 + 1) * 8;
                self.terminal.cursor_col = next_tab.min(self.terminal.cols - 1);
            }
            // LF, VT, FF
            0x0A | 0x0B | 0x0C => {
                self.terminal.new_line();
            }
            // CR
            0x0D => {
                self.terminal.cursor_col = 0;
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() >= 2 {
            match params[0] {
                // Set window title
                b"0" | b"2" => {
                    if let Ok(title) = std::str::from_utf8(params[1]) {
                        self.terminal.set_title(title.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let params: Vec<u16> = params.iter().map(|p| p[0]).collect();
        let p0 = params.first().copied().unwrap_or(0);
        let p1 = params.get(1).copied().unwrap_or(0);

        match action {
            // CUU - Cursor Up
            'A' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_row = self.terminal.cursor_row.saturating_sub(n);
            }
            // CUD - Cursor Down
            'B' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_row =
                    (self.terminal.cursor_row + n).min(self.terminal.rows - 1);
            }
            // CUF - Cursor Forward
            'C' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_col =
                    (self.terminal.cursor_col + n).min(self.terminal.cols - 1);
            }
            // CUB - Cursor Back
            'D' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_col = self.terminal.cursor_col.saturating_sub(n);
            }
            // CUP / HVP - Cursor Position
            'H' | 'f' => {
                let row = if p0 == 0 { 1 } else { p0 as usize };
                let col = if p1 == 0 { 1 } else { p1 as usize };
                self.terminal.cursor_row = (row - 1).min(self.terminal.rows - 1);
                self.terminal.cursor_col = (col - 1).min(self.terminal.cols - 1);
            }
            // ED - Erase in Display
            'J' => {
                self.terminal.erase_in_display(p0);
            }
            // EL - Erase in Line
            'K' => {
                self.terminal.erase_in_line(p0);
            }
            // IL - Insert Lines
            'L' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                for _ in 0..n {
                    self.terminal.scroll_down();
                }
            }
            // DL - Delete Lines
            'M' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                for _ in 0..n {
                    self.terminal.scroll_up();
                }
            }
            // DCH - Delete Characters
            'P' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                let row = self.terminal.cursor_row;
                let col = self.terminal.cursor_col;
                for i in col..self.terminal.cols {
                    if i + n < self.terminal.cols {
                        self.terminal.cells[row][i] = self.terminal.cells[row][i + n].clone();
                    } else {
                        self.terminal.cells[row][i] = Cell::default();
                    }
                }
            }
            // SU - Scroll Up
            'S' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                for _ in 0..n {
                    self.terminal.scroll_up();
                }
            }
            // SD - Scroll Down
            'T' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                for _ in 0..n {
                    self.terminal.scroll_down();
                }
            }
            // ICH - Insert Characters
            '@' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                let row = self.terminal.cursor_row;
                let col = self.terminal.cursor_col;
                for i in (col..self.terminal.cols).rev() {
                    if i >= col + n {
                        self.terminal.cells[row][i] = self.terminal.cells[row][i - n].clone();
                    }
                }
                for i in col..(col + n).min(self.terminal.cols) {
                    self.terminal.cells[row][i] = Cell::default();
                }
            }
            // SGR - Select Graphic Rendition
            'm' => {
                self.handle_sgr(&params);
            }
            // DECSTBM - Set Scrolling Region
            'r' => {
                let top = if p0 == 0 { 1 } else { p0 as usize };
                let bottom = if p1 == 0 {
                    self.terminal.rows
                } else {
                    p1 as usize
                };
                self.terminal.scroll_top = (top - 1).min(self.terminal.rows - 1);
                self.terminal.scroll_bottom = (bottom - 1).min(self.terminal.rows - 1);
                self.terminal.cursor_row = 0;
                self.terminal.cursor_col = 0;
            }
            // DECSC/DECRC and SM/RM with ? prefix handled via intermediates
            'h' => {
                // Handle DEC private modes
                if _intermediates.contains(&b'?') {
                    for &p in &params {
                        match p {
                            // Alt screen buffer (with save/restore cursor)
                            1049 => {
                                self.terminal.saved_cursor = Some((
                                    self.terminal.cursor_row,
                                    self.terminal.cursor_col,
                                ));
                                self.terminal.enter_alt_screen();
                            }
                            1047 | 47 => {
                                self.terminal.enter_alt_screen();
                            }
                            // Show cursor
                            25 => {}
                            // Application cursor keys
                            1 => {}
                            _ => {}
                        }
                    }
                }
            }
            'l' => {
                if _intermediates.contains(&b'?') {
                    for &p in &params {
                        match p {
                            1049 => {
                                self.terminal.exit_alt_screen();
                                if let Some((row, col)) = self.terminal.saved_cursor {
                                    self.terminal.cursor_row = row.min(self.terminal.rows.saturating_sub(1));
                                    self.terminal.cursor_col = col.min(self.terminal.cols.saturating_sub(1));
                                }
                            }
                            1047 | 47 => {
                                self.terminal.exit_alt_screen();
                            }
                            25 => {}
                            1 => {}
                            _ => {}
                        }
                    }
                }
            }
            // Save cursor position
            's' => {
                self.terminal.saved_cursor = Some((
                    self.terminal.cursor_row,
                    self.terminal.cursor_col,
                ));
            }
            // Restore cursor position
            'u' => {
                if let Some((row, col)) = self.terminal.saved_cursor {
                    self.terminal.cursor_row = row.min(self.terminal.rows.saturating_sub(1));
                    self.terminal.cursor_col = col.min(self.terminal.cols.saturating_sub(1));
                }
            }
            // CHA - Cursor Horizontal Absolute
            'G' => {
                let col = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_col = (col - 1).min(self.terminal.cols - 1);
            }
            // VPA - Vertical Position Absolute
            'd' => {
                let row = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_row = (row - 1).min(self.terminal.rows - 1);
            }
            // ECH - Erase Characters
            'X' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                let row = self.terminal.cursor_row;
                for i in self.terminal.cursor_col..(self.terminal.cursor_col + n).min(self.terminal.cols) {
                    self.terminal.cells[row][i] = Cell::default();
                }
            }
            // CNL - Cursor Next Line
            'E' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_row =
                    (self.terminal.cursor_row + n).min(self.terminal.rows - 1);
                self.terminal.cursor_col = 0;
            }
            // CPL - Cursor Previous Line
            'F' => {
                let n = if p0 == 0 { 1 } else { p0 as usize };
                self.terminal.cursor_row = self.terminal.cursor_row.saturating_sub(n);
                self.terminal.cursor_col = 0;
            }
            // DSR - Device Status Report
            'n' => {
                if p0 == 6 {
                    // Report cursor position
                    let response = format!(
                        "\x1b[{};{}R",
                        self.terminal.cursor_row + 1,
                        self.terminal.cursor_col + 1
                    );
                    self.terminal.write_input(response.as_bytes());
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // RI - Reverse Index
            b'M' => {
                if self.terminal.cursor_row == self.terminal.scroll_top {
                    self.terminal.scroll_down();
                } else if self.terminal.cursor_row > 0 {
                    self.terminal.cursor_row -= 1;
                }
            }
            // DECSC - Save cursor
            b'7' => {
                self.terminal.saved_cursor = Some((
                    self.terminal.cursor_row,
                    self.terminal.cursor_col,
                ));
            }
            // DECRC - Restore cursor
            b'8' => {
                if let Some((row, col)) = self.terminal.saved_cursor {
                    self.terminal.cursor_row = row.min(self.terminal.rows.saturating_sub(1));
                    self.terminal.cursor_col = col.min(self.terminal.cols.saturating_sub(1));
                }
            }
            // RIS - Full reset
            b'c' => {
                self.terminal.cells = vec![vec![Cell::default(); self.terminal.cols]; self.terminal.rows];
                self.terminal.cursor_row = 0;
                self.terminal.cursor_col = 0;
                self.terminal.current_attr = Cell::default();
                self.terminal.scroll_top = 0;
                self.terminal.scroll_bottom = self.terminal.rows - 1;
            }
            // IND - Index (move down)
            b'D' => {
                self.terminal.new_line();
            }
            // NEL - Next line
            b'E' => {
                self.terminal.cursor_col = 0;
                self.terminal.new_line();
            }
            _ => {}
        }
    }
}

impl<'a> VteParser<'a> {
    fn handle_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.terminal.current_attr = Cell::default();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.terminal.current_attr = Cell::default(),
                1 => self.terminal.current_attr.bold = true,
                3 => self.terminal.current_attr.italic = true,
                4 => self.terminal.current_attr.underline = true,
                7 => self.terminal.current_attr.inverse = true,
                22 => self.terminal.current_attr.bold = false,
                23 => self.terminal.current_attr.italic = false,
                24 => self.terminal.current_attr.underline = false,
                27 => self.terminal.current_attr.inverse = false,
                // Foreground colors
                30..=37 => {
                    self.terminal.current_attr.fg = CellColor::Ansi((params[i] - 30) as u8);
                }
                38 => {
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            5 => {
                                // 256-color
                                if i + 2 < params.len() {
                                    self.terminal.current_attr.fg =
                                        CellColor::Indexed(params[i + 2] as u8);
                                    i += 2;
                                }
                            }
                            2 => {
                                // 24-bit RGB
                                if i + 4 < params.len() {
                                    self.terminal.current_attr.fg = CellColor::Rgb(
                                        params[i + 2] as u8,
                                        params[i + 3] as u8,
                                        params[i + 4] as u8,
                                    );
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                39 => self.terminal.current_attr.fg = CellColor::Default,
                // Background colors
                40..=47 => {
                    self.terminal.current_attr.bg = CellColor::Ansi((params[i] - 40) as u8);
                }
                48 => {
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            5 => {
                                if i + 2 < params.len() {
                                    self.terminal.current_attr.bg =
                                        CellColor::Indexed(params[i + 2] as u8);
                                    i += 2;
                                }
                            }
                            2 => {
                                if i + 4 < params.len() {
                                    self.terminal.current_attr.bg = CellColor::Rgb(
                                        params[i + 2] as u8,
                                        params[i + 3] as u8,
                                        params[i + 4] as u8,
                                    );
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                49 => self.terminal.current_attr.bg = CellColor::Default,
                // Bright foreground
                90..=97 => {
                    self.terminal.current_attr.fg = CellColor::Ansi((params[i] - 90 + 8) as u8);
                }
                // Bright background
                100..=107 => {
                    self.terminal.current_attr.bg = CellColor::Ansi((params[i] - 100 + 8) as u8);
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Manages multiple terminal panes
pub struct TerminalManager {
    terminals: Vec<Arc<Mutex<Terminal>>>,
}

impl TerminalManager {
    pub fn new(count: usize, panes: &[PaneConfig], _config: &Config) -> Self {
        let mut terminals = Vec::new();
        for i in 0..count {
            let title = if i < panes.len() {
                panes[i]
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Pane {}", i + 1))
            } else {
                format!("Pane {}", i + 1)
            };

            let cols = 80;
            let rows = 24;
            let mut term = Terminal::new(cols, rows, &title);

            // Spawn PTY
            let shell = if i < panes.len() {
                panes[i].command.clone()
            } else {
                None
            };
            let cwd = if i < panes.len() {
                panes[i].cwd.as_deref().map(crate::config::expand_tilde)
            } else {
                None
            };

            // Queue initial commands (will be sent after shell produces first output)
            if let Some(cmds) = panes.get(i).and_then(|p| p.initial_commands.as_ref()) {
                term.set_initial_commands(cmds.clone());
            }

            match Pty::spawn(cols as u16, rows as u16, shell.as_deref(), cwd.as_deref()) {
                Ok(pty) => {
                    term.set_pty(pty);
                }
                Err(e) => {
                    log::error!("Failed to spawn PTY for pane {}: {}", i, e);
                }
            }

            terminals.push(Arc::new(Mutex::new(term)));
        }

        Self { terminals }
    }

    pub fn pane_count(&self) -> usize {
        self.terminals.len()
    }

    pub fn get_terminal(&self, index: usize) -> Arc<Mutex<Terminal>> {
        self.terminals[index].clone()
    }

    pub fn poll_all(&mut self) {
        for term in &self.terminals {
            let mut t = term.lock().unwrap();
            t.poll();
        }
    }

    pub fn any_dirty(&self) -> bool {
        self.terminals.iter().any(|t| t.lock().unwrap().is_dirty())
    }

    pub fn write_to_pane(&self, index: usize, data: &[u8]) {
        if index < self.terminals.len() {
            let mut t = self.terminals[index].lock().unwrap();
            t.write_input(data);
        }
    }

    pub fn get_visible_text(&self, index: usize) -> Option<String> {
        if index < self.terminals.len() {
            let t = self.terminals[index].lock().unwrap();
            if t.is_dirty() {
                Some(t.visible_text())
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn spawn_pane(&mut self, _config: &Config) {
        let i = self.terminals.len();
        let title = format!("Pane {}", i + 1);
        let mut term = Terminal::new(80, 24, &title);
        match Pty::spawn(80, 24, None, None) {
            Ok(pty) => term.set_pty(pty),
            Err(e) => log::error!("Failed to spawn PTY: {}", e),
        }
        self.terminals.push(Arc::new(Mutex::new(term)));
    }

    pub fn resize_pane(&self, index: usize, cols: usize, rows: usize) {
        if index < self.terminals.len() {
            let mut t = self.terminals[index].lock().unwrap();
            t.resize(cols, rows);
        }
    }

    pub fn close_pane(&mut self, index: usize) {
        if index < self.terminals.len() && self.terminals.len() > 1 {
            self.terminals.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_terminal(cols: usize, rows: usize) -> Terminal {
        Terminal::new(cols, rows, "Test")
    }

    #[test]
    fn test_terminal_creation() {
        let t = make_terminal(80, 24);
        assert_eq!(t.cursor_position(), (0, 0));
        assert_eq!(t.title(), "Test");
        // Without a PTY, is_exited() returns true (no child process)
        assert!(t.is_exited());
    }

    #[test]
    fn test_put_char() {
        let mut t = make_terminal(80, 24);
        t.put_char('A');
        assert_eq!(t.cursor_position(), (0, 1));
        let text = t.visible_text();
        assert!(text.starts_with('A'));
    }

    #[test]
    fn test_cursor_movement_csi() {
        let mut t = make_terminal(80, 24);
        // Move cursor to row 5, col 10 using CUP (CSI H)
        t.process_output(b"\x1b[5;10H");
        assert_eq!(t.cursor_position(), (4, 9)); // 0-indexed
    }

    #[test]
    fn test_cursor_up() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\x1b[10;1H"); // move to row 10
        t.process_output(b"\x1b[3A"); // up 3
        assert_eq!(t.cursor_position().0, 6); // 9 - 3 = 6
    }

    #[test]
    fn test_cursor_down() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\x1b[3B"); // down 3
        assert_eq!(t.cursor_position().0, 3);
    }

    #[test]
    fn test_cursor_forward() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\x1b[5C"); // forward 5
        assert_eq!(t.cursor_position().1, 5);
    }

    #[test]
    fn test_cursor_back() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\x1b[10C"); // forward 10
        t.process_output(b"\x1b[3D"); // back 3
        assert_eq!(t.cursor_position().1, 7);
    }

    #[test]
    fn test_sgr_colors() {
        let mut t = make_terminal(80, 24);
        // Set red foreground, write char
        t.process_output(b"\x1b[31mX\x1b[0m");
        let lines = t.visible_lines();
        match lines[0][0].fg {
            CellColor::Ansi(1) => {} // red
            other => panic!("Expected Ansi(1), got {:?}", other),
        }
    }

    #[test]
    fn test_sgr_rgb() {
        let mut t = make_terminal(80, 24);
        // Set 24-bit foreground
        t.process_output(b"\x1b[38;2;255;128;0mR");
        let lines = t.visible_lines();
        match lines[0][0].fg {
            CellColor::Rgb(255, 128, 0) => {}
            other => panic!("Expected Rgb(255,128,0), got {:?}", other),
        }
    }

    #[test]
    fn test_sgr_256color() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\x1b[38;5;42mC");
        let lines = t.visible_lines();
        match lines[0][0].fg {
            CellColor::Indexed(42) => {}
            other => panic!("Expected Indexed(42), got {:?}", other),
        }
    }

    #[test]
    fn test_sgr_bold_italic_underline() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\x1b[1;3;4mB");
        let lines = t.visible_lines();
        assert!(lines[0][0].bold);
        assert!(lines[0][0].italic);
        assert!(lines[0][0].underline);
    }

    #[test]
    fn test_erase_in_display_clear_all() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"Hello World");
        t.process_output(b"\x1b[2J"); // clear all
        let text = t.visible_text();
        assert!(text.trim().is_empty());
    }

    #[test]
    fn test_erase_in_line() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"Hello World");
        t.process_output(b"\x1b[1;6H"); // move to col 6
        t.process_output(b"\x1b[K"); // erase from cursor to end
        let text = t.visible_text();
        assert!(text.starts_with("Hello"));
        assert!(!text.contains("World"));
    }

    #[test]
    fn test_newline_scroll() {
        let mut t = make_terminal(80, 5);
        for i in 0..10 {
            t.process_output(format!("Line {}\n", i).as_bytes());
        }
        // Should have some scrollback
        assert!(!t.scrollback.is_empty());
    }

    #[test]
    fn test_alt_screen() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"Primary content");
        // Enter alt screen
        t.process_output(b"\x1b[?1049h");
        assert!(t.on_alt_screen);
        let text = t.visible_text();
        assert!(!text.contains("Primary"));
        // Exit alt screen
        t.process_output(b"\x1b[?1049l");
        assert!(!t.on_alt_screen);
        let text = t.visible_text();
        assert!(text.contains("Primary"));
    }

    #[test]
    fn test_resize() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"Hello");
        t.resize(40, 12);
        assert_eq!(t.cols, 40);
        assert_eq!(t.rows, 12);
        let text = t.visible_text();
        assert!(text.contains("Hello"));
    }

    #[test]
    fn test_resize_no_change() {
        let mut t = make_terminal(80, 24);
        t.clear_dirty();
        t.resize(80, 24);
        assert!(!t.is_dirty()); // Should not mark dirty
    }

    #[test]
    fn test_scrollback() {
        let mut t = make_terminal(80, 5);
        for i in 0..20 {
            t.process_output(format!("Line {}\r\n", i).as_bytes());
        }
        assert!(t.scrollback.len() > 0);
        t.scroll_view_up(3);
        assert_eq!(t.scroll_offset(), 3);
        t.scroll_view_down(1);
        assert_eq!(t.scroll_offset(), 2);
        t.scroll_to_bottom();
        assert_eq!(t.scroll_offset(), 0);
    }

    #[test]
    fn test_visible_text() {
        let mut t = make_terminal(10, 3);
        t.process_output(b"abc");
        let text = t.visible_text();
        assert!(text.starts_with("abc"));
    }

    #[test]
    fn test_scrolled_lines_live() {
        let mut t = make_terminal(10, 3);
        t.process_output(b"ABC");
        let lines = t.scrolled_lines();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0][0].ch, 'A');
    }

    #[test]
    fn test_set_title() {
        let mut t = make_terminal(80, 24);
        t.set_title("New Title".to_string());
        assert_eq!(t.title(), "New Title");
    }

    #[test]
    fn test_osc_title() {
        let mut t = make_terminal(80, 24);
        // OSC 0 ; title BEL
        t.process_output(b"\x1b]0;My Terminal\x07");
        assert_eq!(t.title(), "My Terminal");
    }

    #[test]
    fn test_tab_stop() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\t");
        assert_eq!(t.cursor_position().1, 8);
    }

    #[test]
    fn test_carriage_return() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"Hello\rWorld");
        let text = t.visible_text();
        assert!(text.starts_with("World"));
    }

    #[test]
    fn test_backspace() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"ABC\x08D"); // BS after C, then D
        let text = t.visible_text();
        assert!(text.starts_with("ABD"));
    }

    #[test]
    fn test_save_restore_cursor() {
        let mut t = make_terminal(80, 24);
        t.process_output(b"\x1b[5;10H"); // move to (4,9)
        t.process_output(b"\x1b7");       // save cursor (ESC 7)
        t.process_output(b"\x1b[1;1H");   // move to (0,0)
        t.process_output(b"\x1b8");       // restore cursor (ESC 8)
        assert_eq!(t.cursor_position(), (4, 9));
    }

    #[test]
    fn test_dirty_flag() {
        let mut t = make_terminal(80, 24);
        assert!(t.is_dirty()); // dirty after creation
        t.clear_dirty();
        assert!(!t.is_dirty());
        t.process_output(b"X");
        assert!(t.is_dirty());
    }

    #[test]
    fn test_child_pid_none_without_pty() {
        let t = make_terminal(80, 24);
        assert!(t.child_pid().is_none());
    }
}
