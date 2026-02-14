const std = @import("std");
const Allocator = std.mem.Allocator;
const testing = std.testing;

// ---------------------------------------------------------------------------
// Cell types
// ---------------------------------------------------------------------------

/// Color representation for a terminal cell — matches xterm-256color.
pub const CellColor = union(enum) {
    default: void,
    ansi: u8, // 0-15
    indexed: u8, // 0-255
    rgb: struct { r: u8, g: u8, b: u8 },

    pub fn eql(a: CellColor, b: CellColor) bool {
        const tag_a = std.meta.activeTag(a);
        const tag_b = std.meta.activeTag(b);
        if (tag_a != tag_b) return false;
        return switch (a) {
            .default => true,
            .ansi => |v| v == b.ansi,
            .indexed => |v| v == b.indexed,
            .rgb => |v| v.r == b.rgb.r and v.g == b.rgb.g and v.b == b.rgb.b,
        };
    }
};

/// A single terminal cell.
pub const Cell = struct {
    ch: u21 = ' ',
    fg: CellColor = .default,
    bg: CellColor = .default,
    bold: bool = false,
    italic: bool = false,
    underline: bool = false,
    inverse: bool = false,

    pub const blank = Cell{};
};

// ---------------------------------------------------------------------------
// VTE parser state machine (follows vt100.net spec, like Ghostty)
// ---------------------------------------------------------------------------

const MAX_CSI_PARAMS = 16;
const MAX_OSC_LEN = 256;

const VteState = enum {
    ground,
    escape,
    escape_intermediate,
    csi_entry,
    csi_param,
    csi_intermediate,
    osc_string,
    dcs_entry,
    dcs_passthrough,
};

pub const VteParser = struct {
    state: VteState = .ground,
    params: [MAX_CSI_PARAMS]u16 = [_]u16{0} ** MAX_CSI_PARAMS,
    param_count: usize = 0,
    intermediates: [2]u8 = [_]u8{0} ** 2,
    intermediate_count: usize = 0,
    osc_buf: [MAX_OSC_LEN]u8 = [_]u8{0} ** MAX_OSC_LEN,
    osc_len: usize = 0,

    pub fn reset(self: *VteParser) void {
        self.state = .ground;
        self.param_count = 0;
        self.intermediate_count = 0;
        self.osc_len = 0;
    }

    /// Advance the parser by one byte, calling back into the terminal.
    pub fn advance(self: *VteParser, term: *Terminal, byte: u8) void {
        switch (self.state) {
            .ground => self.stateGround(term, byte),
            .escape => self.stateEscape(term, byte),
            .escape_intermediate => self.stateEscapeIntermediate(term, byte),
            .csi_entry => self.stateCsiEntry(term, byte),
            .csi_param => self.stateCsiParam(term, byte),
            .csi_intermediate => self.stateCsiIntermediate(term, byte),
            .osc_string => self.stateOscString(term, byte),
            .dcs_entry => {
                // Simplified: skip DCS body
                if (byte == 0x1b) {
                    self.state = .escape;
                } else if (byte == 0x9c) {
                    self.state = .ground;
                }
            },
            .dcs_passthrough => {
                if (byte == 0x1b) {
                    self.state = .escape;
                } else if (byte == 0x9c) {
                    self.state = .ground;
                }
            },
        }
    }

    fn stateGround(self: *VteParser, term: *Terminal, byte: u8) void {
        if (byte == 0x1b) {
            self.state = .escape;
            self.intermediate_count = 0;
            return;
        }
        if (byte < 0x20) {
            // C0 control
            term.executeControl(byte);
            return;
        }
        // Printable — decode UTF-8
        term.printByte(byte);
    }

    fn stateEscape(self: *VteParser, term: *Terminal, byte: u8) void {
        if (byte == '[') {
            self.state = .csi_entry;
            self.param_count = 0;
            self.intermediate_count = 0;
            @memset(&self.params, 0);
            return;
        }
        if (byte == ']') {
            self.state = .osc_string;
            self.osc_len = 0;
            return;
        }
        if (byte == 'P') {
            self.state = .dcs_entry;
            return;
        }
        if (byte >= 0x20 and byte <= 0x2f) {
            self.state = .escape_intermediate;
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            return;
        }
        // ESC dispatch
        term.escDispatch(self.intermediates[0..self.intermediate_count], byte);
        self.state = .ground;
    }

    fn stateEscapeIntermediate(self: *VteParser, term: *Terminal, byte: u8) void {
        if (byte >= 0x20 and byte <= 0x2f) {
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            return;
        }
        if (byte >= 0x30) {
            term.escDispatch(self.intermediates[0..self.intermediate_count], byte);
            self.state = .ground;
        }
    }

    fn stateCsiEntry(self: *VteParser, term_ptr: *Terminal, byte: u8) void {
        if (byte >= '0' and byte <= '9') {
            self.params[0] = byte - '0';
            self.param_count = 1;
            self.state = .csi_param;
            return;
        }
        if (byte == ';') {
            self.param_count = 2;
            self.state = .csi_param;
            return;
        }
        if (byte == '?') {
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            self.state = .csi_param;
            return;
        }
        if (byte >= 0x40 and byte <= 0x7e) {
            // Dispatch immediately with no params
            self.csiDispatch(term_ptr, byte);
            return;
        }
        // Intermediate
        if (byte >= 0x20 and byte <= 0x2f) {
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            self.state = .csi_intermediate;
        }
    }

    fn stateCsiParam(self: *VteParser, term_ptr: *Terminal, byte: u8) void {
        if (byte >= '0' and byte <= '9') {
            if (self.param_count == 0) self.param_count = 1;
            const idx = self.param_count - 1;
            if (idx < MAX_CSI_PARAMS) {
                self.params[idx] = self.params[idx] *% 10 +% (byte - '0');
            }
            return;
        }
        if (byte == ';') {
            if (self.param_count < MAX_CSI_PARAMS) {
                self.param_count += 1;
            }
            return;
        }
        if (byte >= 0x40 and byte <= 0x7e) {
            self.csiDispatch(term_ptr, byte);
            return;
        }
        if (byte >= 0x20 and byte <= 0x2f) {
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            self.state = .csi_intermediate;
        }
    }

    fn stateCsiIntermediate(self: *VteParser, term_ptr: *Terminal, byte: u8) void {
        if (byte >= 0x20 and byte <= 0x2f) {
            if (self.intermediate_count < self.intermediates.len) {
                self.intermediates[self.intermediate_count] = byte;
                self.intermediate_count += 1;
            }
            return;
        }
        if (byte >= 0x40 and byte <= 0x7e) {
            self.csiDispatch(term_ptr, byte);
        }
    }

    fn stateOscString(self: *VteParser, term: *Terminal, byte: u8) void {
        if (byte == 0x07 or byte == 0x9c) {
            // BEL or ST terminates OSC
            term.oscDispatch(self.osc_buf[0..self.osc_len]);
            self.state = .ground;
            return;
        }
        if (byte == 0x1b) {
            // ESC might start ST (ESC \)
            term.oscDispatch(self.osc_buf[0..self.osc_len]);
            self.state = .escape;
            return;
        }
        if (self.osc_len < MAX_OSC_LEN) {
            self.osc_buf[self.osc_len] = byte;
            self.osc_len += 1;
        }
    }

    fn csiDispatch(self: *VteParser, term_ptr: *Terminal, action: u8) void {
        const intermediates = self.intermediates[0..self.intermediate_count];
        const pcount = if (self.param_count == 0) @as(usize, 0) else self.param_count;
        term_ptr.csiDispatch(self.params[0..pcount], intermediates, action);
        self.state = .ground;
    }
};

// ---------------------------------------------------------------------------
// UTF-8 decoder (stateful, survives across read boundaries like Ghostty)
// ---------------------------------------------------------------------------

const Utf8Decoder = struct {
    buf: [4]u8 = undefined,
    len: u3 = 0,
    expected: u3 = 0,

    fn feed(self: *Utf8Decoder, byte: u8) ?u21 {
        if (self.expected == 0) {
            // Start of new character
            if (byte < 0x80) return @as(u21, byte);
            if (byte >= 0xc0 and byte < 0xe0) {
                self.expected = 2;
            } else if (byte >= 0xe0 and byte < 0xf0) {
                self.expected = 3;
            } else if (byte >= 0xf0 and byte < 0xf8) {
                self.expected = 4;
            } else {
                return '?'; // invalid start byte
            }
            self.buf[0] = byte;
            self.len = 1;
            return null;
        }

        // Continuation byte
        if ((byte & 0xc0) != 0x80) {
            // Invalid continuation — reset and try this byte as new start
            self.expected = 0;
            self.len = 0;
            return self.feed(byte);
        }

        self.buf[self.len] = byte;
        self.len += 1;

        if (self.len < self.expected) return null;

        // Decode
        const result: ?u21 = switch (self.expected) {
            2 => @as(u21, self.buf[0] & 0x1f) << 6 | @as(u21, self.buf[1] & 0x3f),
            3 => @as(u21, self.buf[0] & 0x0f) << 12 |
                @as(u21, self.buf[1] & 0x3f) << 6 |
                @as(u21, self.buf[2] & 0x3f),
            4 => @as(u21, self.buf[0] & 0x07) << 18 |
                @as(u21, self.buf[1] & 0x3f) << 12 |
                @as(u21, self.buf[2] & 0x3f) << 6 |
                @as(u21, self.buf[3] & 0x3f),
            else => null,
        };

        self.expected = 0;
        self.len = 0;
        return result orelse '?';
    }
};

// ---------------------------------------------------------------------------
// Semantic zones (OSC 133 — shell integration / FinalTerm protocol)
// ---------------------------------------------------------------------------

pub const SemanticZoneType = enum {
    prompt,
    input,
    output,
};

pub const SemanticZone = struct {
    zone_type: SemanticZoneType,
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
    exit_code: ?i32, // only for output zones
};

// ---------------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------------

pub const Terminal = struct {
    allocator: Allocator,

    /// Screen buffer: flat array of cells, row-major.
    cells: []Cell,
    cols: usize,
    rows: usize,

    cursor_row: usize = 0,
    cursor_col: usize = 0,

    /// Current SGR attributes applied to new characters.
    current_attr: Cell = Cell.blank,

    title: []u8,
    default_title: []u8,

    /// Scrollback buffer (list of rows, each row is a []Cell).
    scrollback: std.ArrayList([]Cell),
    max_scrollback: usize = 10_000,

    saved_cursor: ?struct { row: usize, col: usize } = null,
    scroll_top: usize = 0,
    scroll_bottom: usize = 0,

    dirty: bool = true,

    /// Alternate screen buffer (for vim, less, etc.).
    alt_cells: ?[]Cell = null,
    on_alt_screen: bool = false,

    /// VTE parser state — persists across read boundaries.
    parser: VteParser = .{},
    /// UTF-8 decoder state — persists across read boundaries.
    utf8: Utf8Decoder = .{},

    /// Scroll view offset (0 = live view, >0 = looking at history).
    scroll_offset: usize = 0,

    /// Error detection.
    has_error: bool = false,
    error_timestamp: ?i64 = null,

    /// Semantic zone tracking (OSC 133 — shell integration).
    semantic_zones: std.ArrayList(SemanticZone),
    current_zone_type: ?SemanticZoneType = null,
    last_exit_code: ?i32 = null,

    /// Initialize a new terminal with the given dimensions.
    pub fn init(allocator: Allocator, cols: usize, rows: usize, title_str: []const u8) !*Terminal {
        const self = try allocator.create(Terminal);
        const cells = try allocator.alloc(Cell, cols * rows);
        @memset(cells, Cell.blank);

        const title = try allocator.dupe(u8, title_str);
        const default_title = try allocator.dupe(u8, title_str);

        self.* = .{
            .allocator = allocator,
            .cells = cells,
            .cols = cols,
            .rows = rows,
            .title = title,
            .default_title = default_title,
            .scrollback = std.ArrayList([]Cell).init(allocator),
            .scroll_bottom = rows -| 1,
            .semantic_zones = std.ArrayList(SemanticZone).init(allocator),
        };
        return self;
    }

    pub fn deinit(self: *Terminal) void {
        self.allocator.free(self.cells);
        self.allocator.free(self.title);
        self.allocator.free(self.default_title);
        for (self.scrollback.items) |row| {
            self.allocator.free(row);
        }
        self.scrollback.deinit();
        self.semantic_zones.deinit();
        if (self.alt_cells) |ac| self.allocator.free(ac);
        self.allocator.destroy(self);
    }

    // -- Accessors --

    pub fn getTitle(self: *const Terminal) []const u8 {
        return self.title;
    }

    pub fn setTitle(self: *Terminal, new_title: []const u8) void {
        self.allocator.free(self.title);
        self.title = self.allocator.dupe(u8, new_title) catch return;
        self.dirty = true;
    }

    pub fn cursorPosition(self: *const Terminal) struct { row: usize, col: usize } {
        return .{ .row = self.cursor_row, .col = self.cursor_col };
    }

    pub fn isDirty(self: *const Terminal) bool {
        return self.dirty;
    }

    pub fn clearDirty(self: *Terminal) void {
        self.dirty = false;
    }

    pub fn hasError(self: *const Terminal) bool {
        return self.has_error;
    }

    pub fn getScrollOffset(self: *const Terminal) usize {
        return self.scroll_offset;
    }

    /// Get a cell at the given position.
    pub fn getCell(self: *const Terminal, row: usize, col: usize) Cell {
        if (row >= self.rows or col >= self.cols) return Cell.blank;
        return self.cells[row * self.cols + col];
    }

    fn setCell(self: *Terminal, row: usize, col: usize, cell: Cell) void {
        if (row < self.rows and col < self.cols) {
            self.cells[row * self.cols + col] = cell;
        }
    }

    // -- Visible text extraction --

    pub fn visibleText(self: *const Terminal, buf: []u8) usize {
        var pos: usize = 0;
        for (0..self.rows) |row| {
            // Find last non-space column
            var last_col: usize = 0;
            for (0..self.cols) |col| {
                if (self.getCell(row, col).ch != ' ') last_col = col + 1;
            }
            for (0..last_col) |col| {
                const ch = self.getCell(row, col).ch;
                // Encode as UTF-8
                var tmp: [4]u8 = undefined;
                const len = std.unicode.utf8Encode(ch, &tmp) catch 1;
                if (pos + len < buf.len) {
                    @memcpy(buf[pos..][0..len], tmp[0..len]);
                    pos += len;
                }
            }
            if (pos < buf.len) {
                buf[pos] = '\n';
                pos += 1;
            }
        }
        return pos;
    }

    // -- Scrollback view --

    pub fn scrollViewUp(self: *Terminal, n: usize) void {
        if (self.on_alt_screen) return;
        const max = self.scrollback.items.len;
        self.scroll_offset = @min(self.scroll_offset + n, max);
        self.dirty = true;
    }

    pub fn scrollViewDown(self: *Terminal, n: usize) void {
        self.scroll_offset -|= n;
        self.dirty = true;
    }

    pub fn scrollToBottom(self: *Terminal) void {
        self.scroll_offset = 0;
        self.dirty = true;
    }

    /// Copy visible (possibly scrolled) lines into the provided buffer.
    /// Returns the number of rows written.
    pub fn scrolledLines(self: *const Terminal, out: []Cell) usize {
        if (self.scroll_offset == 0 or self.on_alt_screen) {
            const n = @min(out.len, self.cells.len);
            @memcpy(out[0..n], self.cells[0..n]);
            return self.rows;
        }
        const sb_len = self.scrollback.items.len;
        const offset = @min(self.scroll_offset, sb_len);
        const virtual_start = sb_len -| offset;

        var rows_written: usize = 0;
        for (0..self.rows) |i| {
            const vi = virtual_start + i;
            const out_start = i * self.cols;
            if (out_start + self.cols > out.len) break;
            if (vi < sb_len) {
                const sb_row = self.scrollback.items[vi];
                const copy_len = @min(sb_row.len, self.cols);
                @memcpy(out[out_start..][0..copy_len], sb_row[0..copy_len]);
                // Pad remainder with blanks
                @memset(out[out_start + copy_len .. out_start + self.cols], Cell.blank);
            } else {
                const cell_idx = vi - sb_len;
                if (cell_idx < self.rows) {
                    const src_start = cell_idx * self.cols;
                    @memcpy(out[out_start..][0..self.cols], self.cells[src_start..][0..self.cols]);
                } else {
                    @memset(out[out_start..][0..self.cols], Cell.blank);
                }
            }
            rows_written += 1;
        }
        return rows_written;
    }

    // -- Resize --

    pub fn resize(self: *Terminal, new_cols: usize, new_rows: usize) void {
        if (new_cols == self.cols and new_rows == self.rows) return;
        if (new_cols == 0 or new_rows == 0) return;

        // Allocate new buffer
        const new_cells = self.allocator.alloc(Cell, new_cols * new_rows) catch return;
        @memset(new_cells, Cell.blank);

        // Copy content
        const copy_rows = @min(new_rows, self.rows);
        const copy_cols = @min(new_cols, self.cols);
        for (0..copy_rows) |row| {
            for (0..copy_cols) |col| {
                new_cells[row * new_cols + col] = self.cells[row * self.cols + col];
            }
        }

        self.allocator.free(self.cells);
        self.cells = new_cells;
        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_row = @min(self.cursor_row, new_rows -| 1);
        self.cursor_col = @min(self.cursor_col, new_cols -| 1);
        self.scroll_top = 0;
        self.scroll_bottom = new_rows -| 1;

        if (self.saved_cursor) |*sc| {
            sc.row = @min(sc.row, new_rows -| 1);
            sc.col = @min(sc.col, new_cols -| 1);
        }

        self.dirty = true;
    }

    // -- Process output from PTY --

    pub fn processOutput(self: *Terminal, data: []const u8) void {
        for (data) |byte| {
            self.parser.advance(self, byte);
        }
        self.dirty = true;
    }

    // -- Control character execution --

    fn executeControl(self: *Terminal, byte: u8) void {
        switch (byte) {
            0x07 => {}, // BEL
            0x08 => { // BS
                self.cursor_col -|= 1;
            },
            0x09 => { // HT (tab)
                const next_tab = (self.cursor_col / 8 + 1) * 8;
                self.cursor_col = @min(next_tab, self.cols -| 1);
            },
            0x0A, 0x0B, 0x0C => { // LF, VT, FF
                self.newLine();
            },
            0x0D => { // CR
                self.cursor_col = 0;
            },
            else => {},
        }
    }

    // -- Print a byte (UTF-8 aware) --

    fn printByte(self: *Terminal, byte: u8) void {
        if (self.utf8.feed(byte)) |codepoint| {
            self.putChar(codepoint);
        }
    }

    fn putChar(self: *Terminal, ch: u21) void {
        if (self.cursor_col >= self.cols) {
            self.cursor_col = 0;
            self.newLine();
        }
        if (self.cursor_row < self.rows and self.cursor_col < self.cols) {
            self.setCell(self.cursor_row, self.cursor_col, .{
                .ch = ch,
                .fg = self.current_attr.fg,
                .bg = self.current_attr.bg,
                .bold = self.current_attr.bold,
                .italic = self.current_attr.italic,
                .underline = self.current_attr.underline,
                .inverse = self.current_attr.inverse,
            });
            self.cursor_col += 1;
        }
    }

    // -- Scroll operations --

    fn scrollUp(self: *Terminal) void {
        // Move top line of scroll region into scrollback (primary screen only)
        if (!self.on_alt_screen and self.scroll_top == 0) {
            const row = self.allocator.alloc(Cell, self.cols) catch return;
            @memcpy(row, self.cells[0..self.cols]);
            self.scrollback.append(row) catch {
                self.allocator.free(row);
                return;
            };
            // Trim scrollback if too large
            while (self.scrollback.items.len > self.max_scrollback) {
                const old = self.scrollback.orderedRemove(0);
                self.allocator.free(old);
            }
        }

        // Shift rows up within scroll region
        var i = self.scroll_top;
        while (i < self.scroll_bottom) : (i += 1) {
            const dst_start = i * self.cols;
            const src_start = (i + 1) * self.cols;
            @memcpy(self.cells[dst_start..][0..self.cols], self.cells[src_start..][0..self.cols]);
        }
        // Clear bottom row
        @memset(self.cells[self.scroll_bottom * self.cols ..][0..self.cols], Cell.blank);
    }

    fn scrollDown(self: *Terminal) void {
        var i = self.scroll_bottom;
        while (i > self.scroll_top) : (i -= 1) {
            const dst_start = i * self.cols;
            const src_start = (i - 1) * self.cols;
            @memcpy(self.cells[dst_start..][0..self.cols], self.cells[src_start..][0..self.cols]);
        }
        @memset(self.cells[self.scroll_top * self.cols ..][0..self.cols], Cell.blank);
    }

    fn newLine(self: *Terminal) void {
        if (self.cursor_row == self.scroll_bottom) {
            self.scrollUp();
        } else if (self.cursor_row < self.rows - 1) {
            self.cursor_row += 1;
        }
    }

    // -- Erase operations --

    fn eraseInDisplay(self: *Terminal, mode: u16) void {
        switch (mode) {
            0 => {
                // Clear from cursor to end of screen
                for (self.cursor_col..self.cols) |col| {
                    self.setCell(self.cursor_row, col, Cell.blank);
                }
                var row = self.cursor_row + 1;
                while (row < self.rows) : (row += 1) {
                    @memset(self.cells[row * self.cols ..][0..self.cols], Cell.blank);
                }
            },
            1 => {
                // Clear from start to cursor
                var row: usize = 0;
                while (row < self.cursor_row) : (row += 1) {
                    @memset(self.cells[row * self.cols ..][0..self.cols], Cell.blank);
                }
                for (0..@min(self.cursor_col + 1, self.cols)) |col| {
                    self.setCell(self.cursor_row, col, Cell.blank);
                }
            },
            2, 3 => {
                // Clear entire screen
                @memset(self.cells, Cell.blank);
            },
            else => {},
        }
    }

    fn eraseInLine(self: *Terminal, mode: u16) void {
        switch (mode) {
            0 => {
                for (self.cursor_col..self.cols) |col| {
                    self.setCell(self.cursor_row, col, Cell.blank);
                }
            },
            1 => {
                for (0..@min(self.cursor_col + 1, self.cols)) |col| {
                    self.setCell(self.cursor_row, col, Cell.blank);
                }
            },
            2 => {
                @memset(self.cells[self.cursor_row * self.cols ..][0..self.cols], Cell.blank);
            },
            else => {},
        }
    }

    // -- Alt screen --

    fn enterAltScreen(self: *Terminal) void {
        if (self.on_alt_screen) return;
        const alt = self.allocator.alloc(Cell, self.cols * self.rows) catch return;
        @memcpy(alt, self.cells);
        self.alt_cells = alt;
        @memset(self.cells, Cell.blank);
        self.on_alt_screen = true;
    }

    fn exitAltScreen(self: *Terminal) void {
        if (!self.on_alt_screen) return;
        if (self.alt_cells) |ac| {
            const copy_len = @min(ac.len, self.cells.len);
            @memcpy(self.cells[0..copy_len], ac[0..copy_len]);
            self.allocator.free(ac);
            self.alt_cells = null;
        }
        self.on_alt_screen = false;
    }

    // -- ESC dispatch --

    fn escDispatch(self: *Terminal, intermediates: []const u8, byte: u8) void {
        _ = intermediates;
        switch (byte) {
            'M' => { // RI - Reverse Index
                if (self.cursor_row == self.scroll_top) {
                    self.scrollDown();
                } else if (self.cursor_row > 0) {
                    self.cursor_row -= 1;
                }
            },
            '7' => { // DECSC - Save cursor
                self.saved_cursor = .{ .row = self.cursor_row, .col = self.cursor_col };
            },
            '8' => { // DECRC - Restore cursor
                if (self.saved_cursor) |sc| {
                    self.cursor_row = @min(sc.row, self.rows -| 1);
                    self.cursor_col = @min(sc.col, self.cols -| 1);
                }
            },
            'c' => { // RIS - Full reset
                @memset(self.cells, Cell.blank);
                self.cursor_row = 0;
                self.cursor_col = 0;
                self.current_attr = Cell.blank;
                self.scroll_top = 0;
                self.scroll_bottom = self.rows -| 1;
            },
            'D' => { // IND - Index
                self.newLine();
            },
            'E' => { // NEL - Next line
                self.cursor_col = 0;
                self.newLine();
            },
            else => {},
        }
    }

    // -- OSC dispatch --

    fn oscDispatch(self: *Terminal, data: []const u8) void {
        // OSC format: "<code>;<payload>"
        // Find the semicolon separator
        var sep: usize = 0;
        while (sep < data.len and data[sep] != ';') : (sep += 1) {}
        if (sep >= data.len) return;

        const code = data[0..sep];
        const payload = data[sep + 1 ..];

        // OSC 0 or 2: set window title
        if ((code.len == 1 and (code[0] == '0' or code[0] == '2'))) {
            self.setTitle(payload);
        }
    }

    // -- CSI dispatch --

    fn csiDispatch(self: *Terminal, params: []const u16, intermediates: []const u8, action: u8) void {
        const p0 = if (params.len > 0) params[0] else 0;
        const p1 = if (params.len > 1) params[1] else 0;

        switch (action) {
            'A' => { // CUU - Cursor Up
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_row -|= n;
            },
            'B' => { // CUD - Cursor Down
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_row = @min(self.cursor_row + n, self.rows -| 1);
            },
            'C' => { // CUF - Cursor Forward
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_col = @min(self.cursor_col + n, self.cols -| 1);
            },
            'D' => { // CUB - Cursor Back
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_col -|= n;
            },
            'H', 'f' => { // CUP / HVP - Cursor Position
                const row = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                const col = if (p1 == 0) @as(usize, 1) else @as(usize, p1);
                self.cursor_row = @min(row -| 1, self.rows -| 1);
                self.cursor_col = @min(col -| 1, self.cols -| 1);
            },
            'J' => self.eraseInDisplay(p0),
            'K' => self.eraseInLine(p0),
            'L' => { // IL - Insert Lines
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                for (0..n) |_| self.scrollDown();
            },
            'M' => { // DL - Delete Lines
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                for (0..n) |_| self.scrollUp();
            },
            'P' => { // DCH - Delete Characters
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                const row = self.cursor_row;
                var i = self.cursor_col;
                while (i < self.cols) : (i += 1) {
                    if (i + n < self.cols) {
                        self.setCell(row, i, self.getCell(row, i + n));
                    } else {
                        self.setCell(row, i, Cell.blank);
                    }
                }
            },
            'S' => { // SU - Scroll Up
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                for (0..n) |_| self.scrollUp();
            },
            'T' => { // SD - Scroll Down
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                for (0..n) |_| self.scrollDown();
            },
            '@' => { // ICH - Insert Characters
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                const row = self.cursor_row;
                var i = self.cols;
                while (i > self.cursor_col) {
                    i -= 1;
                    if (i >= self.cursor_col + n) {
                        self.setCell(row, i, self.getCell(row, i - n));
                    }
                }
                for (self.cursor_col..@min(self.cursor_col + n, self.cols)) |c| {
                    self.setCell(row, c, Cell.blank);
                }
            },
            'm' => self.handleSgr(params),
            'r' => { // DECSTBM - Set Scrolling Region
                const top = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                const bottom = if (p1 == 0) self.rows else @as(usize, p1);
                self.scroll_top = @min(top -| 1, self.rows -| 1);
                self.scroll_bottom = @min(bottom -| 1, self.rows -| 1);
                self.cursor_row = 0;
                self.cursor_col = 0;
            },
            'h' => { // SM - Set Mode
                if (intermediates.len > 0 and intermediates[0] == '?') {
                    for (params) |p| {
                        switch (p) {
                            1049 => {
                                self.saved_cursor = .{ .row = self.cursor_row, .col = self.cursor_col };
                                self.enterAltScreen();
                            },
                            1047, 47 => self.enterAltScreen(),
                            else => {},
                        }
                    }
                }
            },
            'l' => { // RM - Reset Mode
                if (intermediates.len > 0 and intermediates[0] == '?') {
                    for (params) |p| {
                        switch (p) {
                            1049 => {
                                self.exitAltScreen();
                                if (self.saved_cursor) |sc| {
                                    self.cursor_row = @min(sc.row, self.rows -| 1);
                                    self.cursor_col = @min(sc.col, self.cols -| 1);
                                }
                            },
                            1047, 47 => self.exitAltScreen(),
                            else => {},
                        }
                    }
                }
            },
            's' => { // Save cursor position
                self.saved_cursor = .{ .row = self.cursor_row, .col = self.cursor_col };
            },
            'u' => { // Restore cursor position
                if (self.saved_cursor) |sc| {
                    self.cursor_row = @min(sc.row, self.rows -| 1);
                    self.cursor_col = @min(sc.col, self.cols -| 1);
                }
            },
            'G' => { // CHA - Cursor Horizontal Absolute
                const col = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_col = @min(col -| 1, self.cols -| 1);
            },
            'd' => { // VPA - Vertical Position Absolute
                const row = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_row = @min(row -| 1, self.rows -| 1);
            },
            'X' => { // ECH - Erase Characters
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                for (self.cursor_col..@min(self.cursor_col + n, self.cols)) |c| {
                    self.setCell(self.cursor_row, c, Cell.blank);
                }
            },
            'E' => { // CNL - Cursor Next Line
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_row = @min(self.cursor_row + n, self.rows -| 1);
                self.cursor_col = 0;
            },
            'F' => { // CPL - Cursor Previous Line
                const n = if (p0 == 0) @as(usize, 1) else @as(usize, p0);
                self.cursor_row -|= n;
                self.cursor_col = 0;
            },
            'n' => { // DSR - Device Status Report
                // Response requires PTY access — handled at a higher level.
            },
            else => {},
        }
    }

    // -- SGR (Select Graphic Rendition) --

    fn handleSgr(self: *Terminal, params: []const u16) void {
        if (params.len == 0) {
            self.current_attr = Cell.blank;
            return;
        }
        var i: usize = 0;
        while (i < params.len) {
            switch (params[i]) {
                0 => self.current_attr = Cell.blank,
                1 => self.current_attr.bold = true,
                3 => self.current_attr.italic = true,
                4 => self.current_attr.underline = true,
                7 => self.current_attr.inverse = true,
                22 => self.current_attr.bold = false,
                23 => self.current_attr.italic = false,
                24 => self.current_attr.underline = false,
                27 => self.current_attr.inverse = false,
                30...37 => {
                    self.current_attr.fg = .{ .ansi = @intCast(params[i] - 30) };
                },
                38 => {
                    if (i + 1 < params.len) {
                        switch (params[i + 1]) {
                            5 => {
                                if (i + 2 < params.len) {
                                    self.current_attr.fg = .{ .indexed = @intCast(params[i + 2]) };
                                    i += 2;
                                }
                            },
                            2 => {
                                if (i + 4 < params.len) {
                                    self.current_attr.fg = .{ .rgb = .{
                                        .r = @intCast(params[i + 2]),
                                        .g = @intCast(params[i + 3]),
                                        .b = @intCast(params[i + 4]),
                                    } };
                                    i += 4;
                                }
                            },
                            else => {},
                        }
                    }
                },
                39 => self.current_attr.fg = .default,
                40...47 => {
                    self.current_attr.bg = .{ .ansi = @intCast(params[i] - 40) };
                },
                48 => {
                    if (i + 1 < params.len) {
                        switch (params[i + 1]) {
                            5 => {
                                if (i + 2 < params.len) {
                                    self.current_attr.bg = .{ .indexed = @intCast(params[i + 2]) };
                                    i += 2;
                                }
                            },
                            2 => {
                                if (i + 4 < params.len) {
                                    self.current_attr.bg = .{ .rgb = .{
                                        .r = @intCast(params[i + 2]),
                                        .g = @intCast(params[i + 3]),
                                        .b = @intCast(params[i + 4]),
                                    } };
                                    i += 4;
                                }
                            },
                            else => {},
                        }
                    }
                },
                49 => self.current_attr.bg = .default,
                90...97 => {
                    self.current_attr.fg = .{ .ansi = @intCast(params[i] - 90 + 8) };
                },
                100...107 => {
                    self.current_attr.bg = .{ .ansi = @intCast(params[i] - 100 + 8) };
                },
                else => {},
            }
            i += 1;
        }
    }

    // -- Error detection --

    pub fn detectErrors(self: *Terminal, data: []const u8) void {
        // Auto-clear after 10 seconds
        if (self.error_timestamp) |ts| {
            const now = std.time.timestamp();
            if (now - ts >= 10) {
                self.has_error = false;
                self.error_timestamp = null;
            }
        }

        const error_patterns = [_][]const u8{
            "error:", "error[", "fatal:", "panic:", "traceback",
            "exception:", "failed:", "segfault", "command not found",
            "no such file", "permission denied", "errno",
        };

        const lower_buf_size = 4096;
        var lower_buf: [lower_buf_size]u8 = undefined;
        const check_len = @min(data.len, lower_buf_size);
        for (data[0..check_len], 0..) |c, idx| {
            lower_buf[idx] = if (c >= 'A' and c <= 'Z') c + 32 else c;
        }
        const lower = lower_buf[0..check_len];

        for (error_patterns) |pattern| {
            if (std.mem.indexOf(u8, lower, pattern) != null) {
                self.has_error = true;
                self.error_timestamp = std.time.timestamp();
                break;
            }
        }
    }
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn makeTerminal(cols: usize, rows: usize) !*Terminal {
    return Terminal.init(testing.allocator, cols, rows, "Test");
}

test "terminal creation" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    const pos = t.cursorPosition();
    try testing.expectEqual(@as(usize, 0), pos.row);
    try testing.expectEqual(@as(usize, 0), pos.col);
    try testing.expectEqualSlices(u8, "Test", t.getTitle());
}

test "put_char" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("A");
    const pos = t.cursorPosition();
    try testing.expectEqual(@as(usize, 0), pos.row);
    try testing.expectEqual(@as(usize, 1), pos.col);
    try testing.expectEqual(@as(u21, 'A'), t.getCell(0, 0).ch);
}

test "cursor movement CUP" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[5;10H");
    const pos = t.cursorPosition();
    try testing.expectEqual(@as(usize, 4), pos.row);
    try testing.expectEqual(@as(usize, 9), pos.col);
}

test "cursor up" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[10;1H");
    t.processOutput("\x1b[3A");
    try testing.expectEqual(@as(usize, 6), t.cursorPosition().row);
}

test "cursor down" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[3B");
    try testing.expectEqual(@as(usize, 3), t.cursorPosition().row);
}

test "cursor forward" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[5C");
    try testing.expectEqual(@as(usize, 5), t.cursorPosition().col);
}

test "cursor back" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[10C");
    t.processOutput("\x1b[3D");
    try testing.expectEqual(@as(usize, 7), t.cursorPosition().col);
}

test "SGR colors - red foreground" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[31mX\x1b[0m");
    try testing.expect(CellColor.eql(t.getCell(0, 0).fg, .{ .ansi = 1 }));
}

test "SGR 24-bit RGB" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[38;2;255;128;0mR");
    try testing.expect(CellColor.eql(t.getCell(0, 0).fg, .{ .rgb = .{ .r = 255, .g = 128, .b = 0 } }));
}

test "SGR 256 color" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[38;5;42mC");
    try testing.expect(CellColor.eql(t.getCell(0, 0).fg, .{ .indexed = 42 }));
}

test "SGR bold italic underline" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[1;3;4mB");
    const cell = t.getCell(0, 0);
    try testing.expect(cell.bold);
    try testing.expect(cell.italic);
    try testing.expect(cell.underline);
}

test "erase in display clear all" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("Hello World");
    t.processOutput("\x1b[2J");
    try testing.expectEqual(@as(u21, ' '), t.getCell(0, 0).ch);
}

test "erase in line" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("Hello World");
    t.processOutput("\x1b[1;6H");
    t.processOutput("\x1b[K");
    try testing.expectEqual(@as(u21, 'H'), t.getCell(0, 0).ch);
    try testing.expectEqual(@as(u21, ' '), t.getCell(0, 5).ch);
}

test "newline scroll" {
    const t = try makeTerminal(80, 5);
    defer t.deinit();
    for (0..10) |i| {
        var buf: [32]u8 = undefined;
        const line = std.fmt.bufPrint(&buf, "Line {}\n", .{i}) catch break;
        t.processOutput(line);
    }
    try testing.expect(t.scrollback.items.len > 0);
}

test "alt screen" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("Primary");
    try testing.expectEqual(@as(u21, 'P'), t.getCell(0, 0).ch);
    t.processOutput("\x1b[?1049h"); // enter alt screen
    try testing.expect(t.on_alt_screen);
    try testing.expectEqual(@as(u21, ' '), t.getCell(0, 0).ch);
    t.processOutput("\x1b[?1049l"); // exit alt screen
    try testing.expect(!t.on_alt_screen);
    try testing.expectEqual(@as(u21, 'P'), t.getCell(0, 0).ch);
}

test "resize" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("Hello");
    t.resize(40, 12);
    try testing.expectEqual(@as(usize, 40), t.cols);
    try testing.expectEqual(@as(usize, 12), t.rows);
    try testing.expectEqual(@as(u21, 'H'), t.getCell(0, 0).ch);
}

test "resize no change" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.clearDirty();
    t.resize(80, 24);
    try testing.expect(!t.isDirty());
}

test "scrollback" {
    const t = try makeTerminal(80, 5);
    defer t.deinit();
    for (0..20) |i| {
        var buf: [32]u8 = undefined;
        const line = std.fmt.bufPrint(&buf, "Line {}\r\n", .{i}) catch break;
        t.processOutput(line);
    }
    try testing.expect(t.scrollback.items.len > 0);
    t.scrollViewUp(3);
    try testing.expectEqual(@as(usize, 3), t.getScrollOffset());
    t.scrollViewDown(1);
    try testing.expectEqual(@as(usize, 2), t.getScrollOffset());
    t.scrollToBottom();
    try testing.expectEqual(@as(usize, 0), t.getScrollOffset());
}

test "set title" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.setTitle("New Title");
    try testing.expectEqualSlices(u8, "New Title", t.getTitle());
}

test "OSC title" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b]0;My Terminal\x07");
    try testing.expectEqualSlices(u8, "My Terminal", t.getTitle());
}

test "tab stop" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\t");
    try testing.expectEqual(@as(usize, 8), t.cursorPosition().col);
}

test "carriage return" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("Hello\rWorld");
    try testing.expectEqual(@as(u21, 'W'), t.getCell(0, 0).ch);
}

test "backspace" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("ABC\x08D");
    try testing.expectEqual(@as(u21, 'A'), t.getCell(0, 0).ch);
    try testing.expectEqual(@as(u21, 'B'), t.getCell(0, 1).ch);
    try testing.expectEqual(@as(u21, 'D'), t.getCell(0, 2).ch);
}

test "save restore cursor" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    t.processOutput("\x1b[5;10H");
    t.processOutput("\x1b7"); // save
    t.processOutput("\x1b[1;1H");
    t.processOutput("\x1b8"); // restore
    const pos = t.cursorPosition();
    try testing.expectEqual(@as(usize, 4), pos.row);
    try testing.expectEqual(@as(usize, 9), pos.col);
}

test "dirty flag" {
    const t = try makeTerminal(80, 24);
    defer t.deinit();
    try testing.expect(t.isDirty());
    t.clearDirty();
    try testing.expect(!t.isDirty());
    t.processOutput("X");
    try testing.expect(t.isDirty());
}
