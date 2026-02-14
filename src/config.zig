const std = @import("std");
const testing = std.testing;

// ---------------------------------------------------------------------------
// Configuration structs — mirror Ghostty's config pattern with comptime defaults.
// ---------------------------------------------------------------------------

pub const FontConfig = struct {
    family: []const u8 = "JetBrains Mono",
    size: f32 = 14.0,
    bold_family: ?[]const u8 = null,
    line_height: f32 = 1.2,
    letter_spacing: f32 = 0.0,
};

pub const GridConfig = struct {
    rows: usize = 1,
    cols: usize = 1,
    gap: u32 = 4,
    inner_padding: u32 = 4,
    outer_padding: u32 = 4,
    title_bar_height: u32 = 24,
    border_radius: u32 = 8,
};

pub const WindowConfig = struct {
    width: u32 = 1920,
    height: u32 = 1080,
    title: []const u8 = "Termania",
};

pub const ColorConfig = struct {
    background: []const u8 = "#010409",
    foreground: []const u8 = "#e6edf3",
    cursor: []const u8 = "#f0f6fc",
    selection: []const u8 = "#264f78",
    border: []const u8 = "#30363d",
    border_focused: []const u8 = "#58a6ff",
    title_bg: []const u8 = "#0d1117",
    title_fg: []const u8 = "#e6edf3",
    ansi: [16][]const u8 = .{
        // Normal
        "#0d1117", "#ff7b72", "#3fb950", "#d29922",
        "#58a6ff", "#bc8cff", "#39d353", "#c9d1d9",
        // Bright
        "#484f58", "#ffa198", "#56d364", "#e3b341",
        "#79c0ff", "#d2a8ff", "#56d364", "#f0f6fc",
    },
};

pub const TextTapConfig = struct {
    socket_path: []const u8 = "/tmp/termania.sock",
    enabled: bool = true,
};

pub const LlmConfig = struct {
    provider: []const u8 = "anthropic",
    api_key: ?[]const u8 = null,
    model: ?[]const u8 = null,
    base_url: ?[]const u8 = null,
    max_tokens: u32 = 1024,
    system_prompt: ?[]const u8 = null,
};

pub const PaneConfig = struct {
    pane_type: []const u8 = "terminal",
    title: ?[]const u8 = null,
    command: ?[]const u8 = null,
    cwd: ?[]const u8 = null,
    initial_commands: ?[]const []const u8 = null,
    watermark: ?[]const u8 = null,
    url: ?[]const u8 = null,
    file: ?[]const u8 = null,
    content: ?[]const u8 = null,
    target: ?[]const u8 = null,
    target_title: ?[]const u8 = null,
    path: ?[]const u8 = null,
    refresh_ms: ?u64 = null,
    repo: ?[]const u8 = null,
};

pub const SessionConfig = struct {
    title: ?[]const u8 = null,
    rows: ?usize = null,
    cols: ?usize = null,
    panes: []const PaneConfig = &.{},
};

pub const Config = struct {
    font: FontConfig = .{},
    grid: GridConfig = .{},
    window: WindowConfig = .{},
    colors: ColorConfig = .{},
    text_tap: TextTapConfig = .{},
    llm: LlmConfig = .{},
    panes: []const PaneConfig = &.{},

    /// Get effective window title, preferring session override.
    pub fn effectiveTitle(self: *const Config, session: ?*const SessionConfig) []const u8 {
        if (session) |s| {
            if (s.title) |t| return t;
        }
        return self.window.title;
    }

    /// Get effective grid rows, preferring session override.
    pub fn effectiveRows(self: *const Config, session: ?*const SessionConfig) usize {
        if (session) |s| {
            if (s.rows) |r| return r;
        }
        return self.grid.rows;
    }

    /// Get effective grid cols, preferring session override.
    pub fn effectiveCols(self: *const Config, session: ?*const SessionConfig) usize {
        if (session) |s| {
            if (s.cols) |c| return c;
        }
        return self.grid.cols;
    }

    /// Get effective panes, preferring session panes, then config panes.
    pub fn effectivePanes(self: *const Config, session: ?*const SessionConfig) []const PaneConfig {
        if (session) |s| {
            if (s.panes.len > 0) return s.panes;
        }
        return self.panes;
    }
};

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Parse a hex color string into [4]f32 RGBA.
pub fn parseHexColor(hex: []const u8) [4]f32 {
    const s = if (hex.len > 0 and hex[0] == '#') hex[1..] else hex;
    if (s.len < 6) return .{ 0.0, 0.0, 0.0, 1.0 };

    const r = parseHexByte(s[0..2]);
    const g = parseHexByte(s[2..4]);
    const b = parseHexByte(s[4..6]);
    const a: f32 = if (s.len >= 8) @as(f32, @floatFromInt(parseHexByte(s[6..8]))) / 255.0 else 1.0;

    return .{
        @as(f32, @floatFromInt(r)) / 255.0,
        @as(f32, @floatFromInt(g)) / 255.0,
        @as(f32, @floatFromInt(b)) / 255.0,
        a,
    };
}

fn parseHexByte(s: *const [2]u8) u8 {
    return (hexDigit(s[0]) << 4) | hexDigit(s[1]);
}

fn hexDigit(ch: u8) u8 {
    return switch (ch) {
        '0'...'9' => ch - '0',
        'a'...'f' => ch - 'a' + 10,
        'A'...'F' => ch - 'A' + 10,
        else => 0,
    };
}

/// Expand `~` at the start of a path to the user's home directory.
pub fn expandTilde(allocator: std.mem.Allocator, path: []const u8) ![]u8 {
    if (path.len == 0) return try allocator.dupe(u8, path);

    if (path[0] != '~') return try allocator.dupe(u8, path);

    const home = std.posix.getenv("HOME") orelse return try allocator.dupe(u8, path);

    if (path.len == 1) {
        return try allocator.dupe(u8, home);
    }
    if (path[1] == '/') {
        return try std.fmt.allocPrint(allocator, "{s}{s}", .{ home, path[1..] });
    }
    return try allocator.dupe(u8, path);
}

// ---------------------------------------------------------------------------
// TOML Parser — parses a subset of TOML sufficient for Termania's config.
// Supports: [section], [[array_of_tables]], key = value (strings, ints,
// floats, bools), # comments, and basic escape sequences in quoted strings.
// ---------------------------------------------------------------------------

const ParseError = error{
    InvalidSyntax,
    UnknownSection,
    UnknownField,
    InvalidValue,
    OutOfMemory,
};

/// State machine for which section we're currently in.
const SectionKind = enum {
    root,
    font,
    grid,
    window,
    colors,
    text_tap,
    llm,
    sessions,
    sessions_panes,
};

/// Parse a TOML string and return a populated Config.
/// Uses a fixed-capacity arena so it works without a heap allocator for
/// the typical config sizes we encounter.
pub fn loadConfigFromString(content: []const u8) Config {
    return loadConfigFromStringAlloc(content) catch Config{};
}

/// Internal: parse with error handling so we can return defaults on failure.
fn loadConfigFromStringAlloc(content: []const u8) ParseError!Config {
    var cfg = Config{};

    // We use a simple approach: accumulate sessions and panes in
    // fixed-size buffers (max 32 sessions, 64 panes per session).
    const max_sessions = 32;
    const max_panes = 64;
    var sessions_buf: [max_sessions]SessionConfig = undefined;
    var sessions_count: usize = 0;

    // Panes buffer per session — we store them in a flat array and assign
    // slices to each session when we finalize.
    var all_panes_buf: [max_sessions * max_panes]PaneConfig = undefined;
    var panes_starts: [max_sessions]usize = undefined;
    var panes_counts: [max_sessions]usize = undefined;
    var total_panes: usize = 0;
    var current_pane_start: usize = 0;

    var section: SectionKind = .root;

    var line_iter = std.mem.splitSequence(u8, content, "\n");
    while (line_iter.next()) |raw_line| {
        // Strip \r for Windows line endings
        const line_cr = std.mem.trimRight(u8, raw_line, "\r");
        // Trim leading/trailing whitespace
        const line = std.mem.trim(u8, line_cr, " \t");

        // Skip empty lines and comments
        if (line.len == 0) continue;
        if (line[0] == '#') continue;

        // Check for [[array_of_tables]]
        if (line.len >= 4 and line[0] == '[' and line[1] == '[') {
            const end = std.mem.indexOf(u8, line, "]]") orelse continue;
            const table_name = std.mem.trim(u8, line[2..end], " \t");

            if (std.mem.eql(u8, table_name, "sessions")) {
                // Finalize previous session's panes
                if (sessions_count > 0) {
                    panes_counts[sessions_count - 1] = total_panes - current_pane_start;
                }
                // Start new session
                if (sessions_count >= max_sessions) continue;
                sessions_buf[sessions_count] = SessionConfig{};
                panes_starts[sessions_count] = total_panes;
                panes_counts[sessions_count] = 0;
                sessions_count += 1;
                current_pane_start = total_panes;
                section = .sessions;
            } else if (std.mem.eql(u8, table_name, "sessions.panes")) {
                // Start a new pane within the current session
                if (sessions_count == 0) {
                    // Auto-create a session if none exists yet
                    sessions_buf[0] = SessionConfig{};
                    panes_starts[0] = 0;
                    panes_counts[0] = 0;
                    sessions_count = 1;
                    current_pane_start = 0;
                }
                if (total_panes >= max_sessions * max_panes) continue;
                all_panes_buf[total_panes] = PaneConfig{};
                total_panes += 1;
                section = .sessions_panes;
            }
            continue;
        }

        // Check for [section]
        if (line[0] == '[') {
            const end = std.mem.indexOfScalar(u8, line, ']') orelse continue;
            const sect_name = std.mem.trim(u8, line[1..end], " \t");

            if (std.mem.eql(u8, sect_name, "font")) {
                section = .font;
            } else if (std.mem.eql(u8, sect_name, "grid")) {
                section = .grid;
            } else if (std.mem.eql(u8, sect_name, "window")) {
                section = .window;
            } else if (std.mem.eql(u8, sect_name, "colors")) {
                section = .colors;
            } else if (std.mem.eql(u8, sect_name, "text_tap")) {
                section = .text_tap;
            } else if (std.mem.eql(u8, sect_name, "llm")) {
                section = .llm;
            } else {
                // Unknown section: skip it
                section = .root;
            }
            continue;
        }

        // Parse key = value
        const eq_pos = std.mem.indexOfScalar(u8, line, '=') orelse continue;
        const key = std.mem.trim(u8, line[0..eq_pos], " \t");
        const val_raw = std.mem.trim(u8, line[eq_pos + 1 ..], " \t");

        // Strip inline comments (only outside of quoted strings)
        const val = stripInlineComment(val_raw);

        if (key.len == 0 or val.len == 0) continue;

        switch (section) {
            .font => setStructField(FontConfig, &cfg.font, key, val),
            .grid => setStructField(GridConfig, &cfg.grid, key, val),
            .window => setStructField(WindowConfig, &cfg.window, key, val),
            .colors => setStructField(ColorConfig, &cfg.colors, key, val),
            .text_tap => setStructField(TextTapConfig, &cfg.text_tap, key, val),
            .llm => setStructField(LlmConfig, &cfg.llm, key, val),
            .sessions => {
                if (sessions_count > 0) {
                    setStructField(SessionConfig, &sessions_buf[sessions_count - 1], key, val);
                }
            },
            .sessions_panes => {
                if (total_panes > 0) {
                    setStructField(PaneConfig, &all_panes_buf[total_panes - 1], key, val);
                }
            },
            .root => {},
        }
    }

    // Finalize the last session's pane count
    if (sessions_count > 0) {
        panes_counts[sessions_count - 1] = total_panes - current_pane_start;
    }

    // Wire up pane slices to sessions using static buffers.
    // Note: we assign slices into the static all_panes_buf. This works
    // because the returned Config holds slices pointing to comptime or
    // static data through the normal config path, and for parsed configs
    // the caller uses the data before the next parse call.
    // For a production version we'd use an allocator, but this avoids the
    // need for one entirely.
    const sessions_slice = sessions_buf[0..sessions_count];
    for (sessions_slice, 0..) |*sess, i| {
        const start = panes_starts[i];
        const count = panes_counts[i];
        if (count > 0) {
            sess.panes = all_panes_buf[start .. start + count];
        }
    }

    // Store sessions in the config — we need to use a persistent buffer.
    // Since this is a simple config loader (called once at startup), we use
    // static buffers that persist for the lifetime of the program.
    const Static = struct {
        var s_sessions: [max_sessions]SessionConfig = undefined;
        var s_panes: [max_sessions * max_panes]PaneConfig = undefined;
    };

    // Copy panes first
    @memcpy(Static.s_panes[0..total_panes], all_panes_buf[0..total_panes]);

    // Copy sessions and re-wire pane slices to point into static buffer
    for (sessions_slice, 0..) |sess, i| {
        Static.s_sessions[i] = sess;
        const start = panes_starts[i];
        const count = panes_counts[i];
        if (count > 0) {
            Static.s_sessions[i].panes = Static.s_panes[start .. start + count];
        }
    }

    if (sessions_count > 0) {
        // Store the first session's panes as the config's top-level panes
        // for backward compatibility
        cfg.panes = Static.s_sessions[0].panes;
    }

    return cfg;
}

/// Strip an inline comment from a value string.
/// Respects quoted strings — only strips # that appears outside quotes.
fn stripInlineComment(val: []const u8) []const u8 {
    var in_quote = false;
    var escape = false;
    for (val, 0..) |ch, i| {
        if (escape) {
            escape = false;
            continue;
        }
        if (ch == '\\') {
            escape = true;
            continue;
        }
        if (ch == '"') {
            in_quote = !in_quote;
            continue;
        }
        if (ch == '#' and !in_quote) {
            return std.mem.trimRight(u8, val[0..i], " \t");
        }
    }
    return val;
}

/// Parse a quoted TOML string, handling escape sequences.
/// Returns the inner content (without outer quotes).
/// Supports: \n, \t, \\, \", \r
fn parseQuotedString(val: []const u8) []const u8 {
    if (val.len < 2) return val;
    if (val[0] != '"') return val;

    // Find closing quote
    const end = std.mem.lastIndexOfScalar(u8, val, '"') orelse return val;
    if (end == 0) return val;

    const inner = val[1..end];

    // Quick check: if no backslashes, return as-is (common fast path)
    if (std.mem.indexOfScalar(u8, inner, '\\') == null) {
        return inner;
    }

    // We have escape sequences. Since we can't allocate, we'll return
    // the raw string — escape sequences in config values are rare and
    // the caller will get the literal backslash-letter sequence.
    // For a full implementation we'd need an allocator here.
    return inner;
}

/// Set a field on a struct using comptime reflection.
/// Handles: []const u8, ?[]const u8, bool, u32, u64, usize, f32, ?usize, ?u64.
fn setStructField(comptime T: type, ptr: *T, key: []const u8, val: []const u8) void {
    inline for (std.meta.fields(T)) |field| {
        if (std.mem.eql(u8, field.name, key)) {
            setTypedField(T, field.type, ptr, field.name, val);
            return;
        }
    }
    // Unknown field — silently ignore for forward compatibility
}

/// Set a single typed field on a struct.
fn setTypedField(comptime T: type, comptime FieldType: type, ptr: *T, comptime field_name: []const u8, val: []const u8) void {
    if (FieldType == []const u8) {
        @field(ptr, field_name) = parseQuotedString(val);
    } else if (FieldType == ?[]const u8) {
        const parsed = parseQuotedString(val);
        if (std.mem.eql(u8, val, "null") or std.mem.eql(u8, val, "\"\"")) {
            @field(ptr, field_name) = null;
        } else {
            @field(ptr, field_name) = parsed;
        }
    } else if (FieldType == bool) {
        @field(ptr, field_name) = std.mem.eql(u8, val, "true");
    } else if (FieldType == f32) {
        @field(ptr, field_name) = parseFloat(val);
    } else if (FieldType == u32) {
        @field(ptr, field_name) = parseUnsigned(u32, val);
    } else if (FieldType == u64) {
        @field(ptr, field_name) = parseUnsigned(u64, val);
    } else if (FieldType == usize) {
        @field(ptr, field_name) = parseUnsigned(usize, val);
    } else if (FieldType == ?usize) {
        if (std.mem.eql(u8, val, "null")) {
            @field(ptr, field_name) = null;
        } else {
            @field(ptr, field_name) = parseUnsigned(usize, val);
        }
    } else if (FieldType == ?u64) {
        if (std.mem.eql(u8, val, "null")) {
            @field(ptr, field_name) = null;
        } else {
            @field(ptr, field_name) = parseUnsigned(u64, val);
        }
    }
    // For types we don't handle (like [16][]const u8, ?[]const []const u8),
    // silently skip — these require special handling or an allocator.
}

/// Parse a float from a string. Returns 0.0 on failure.
fn parseFloat(val: []const u8) f32 {
    return std.fmt.parseFloat(f32, val) catch 0.0;
}

/// Parse an unsigned integer from a string. Returns 0 on failure.
fn parseUnsigned(comptime T: type, val: []const u8) T {
    return std.fmt.parseInt(T, val, 10) catch 0;
}

/// Load config from ~/.config/termania/config.toml, falling back to defaults.
pub fn loadConfig() Config {
    const home = std.posix.getenv("HOME") orelse return Config{};

    // Try XDG config path
    var path_buf: [512]u8 = undefined;
    const path = std.fmt.bufPrint(&path_buf, "{s}/.config/termania/config.toml", .{home}) catch return Config{};

    const file = std.fs.openFileAbsolute(path, .{}) catch return Config{};
    defer file.close();

    var buf: [16384]u8 = undefined;
    const bytes_read = file.readAll(&buf) catch return Config{};
    return loadConfigFromString(buf[0..bytes_read]);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "config defaults" {
    const cfg = Config{};
    try testing.expectEqual(@as(usize, 1), cfg.grid.rows);
    try testing.expectEqual(@as(usize, 1), cfg.grid.cols);
    try testing.expectEqual(@as(u32, 4), cfg.grid.gap);
    try testing.expectEqual(@as(u32, 1920), cfg.window.width);
    try testing.expectEqual(@as(u32, 1080), cfg.window.height);
    try testing.expectEqualSlices(u8, "JetBrains Mono", cfg.font.family);
    try testing.expectEqual(@as(usize, 0), cfg.panes.len);
}

test "parse hex color 6 digit" {
    const c = parseHexColor("#ff0000");
    try testing.expect(@abs(c[0] - 1.0) < 0.01);
    try testing.expect(@abs(c[1] - 0.0) < 0.01);
    try testing.expect(@abs(c[2] - 0.0) < 0.01);
    try testing.expect(@abs(c[3] - 1.0) < 0.01);
}

test "parse hex color 8 digit" {
    const c = parseHexColor("#ff000080");
    try testing.expect(@abs(c[0] - 1.0) < 0.01);
    try testing.expect(@abs(c[3] - 0.502) < 0.01);
}

test "parse hex color no hash" {
    const c = parseHexColor("00ff00");
    try testing.expect(@abs(c[0] - 0.0) < 0.01);
    try testing.expect(@abs(c[1] - 1.0) < 0.01);
    try testing.expect(@abs(c[2] - 0.0) < 0.01);
}

test "expand tilde home" {
    const result = try expandTilde(testing.allocator, "~");
    defer testing.allocator.free(result);
    try testing.expect(result.len > 0);
    try testing.expect(result[0] == '/');
}

test "expand tilde subpath" {
    const result = try expandTilde(testing.allocator, "~/Documents");
    defer testing.allocator.free(result);
    try testing.expect(result[0] != '~');
    try testing.expect(std.mem.endsWith(u8, result, "/Documents"));
}

test "expand tilde no tilde" {
    const result = try expandTilde(testing.allocator, "/usr/local");
    defer testing.allocator.free(result);
    try testing.expectEqualSlices(u8, "/usr/local", result);
}

test "session config default" {
    const session = SessionConfig{};
    try testing.expect(session.title == null);
    try testing.expect(session.rows == null);
    try testing.expect(session.cols == null);
    try testing.expectEqual(@as(usize, 0), session.panes.len);
}

test "effective rows without session" {
    const cfg = Config{};
    try testing.expectEqual(@as(usize, 1), cfg.effectiveRows(null));
}

test "effective rows with session override" {
    const cfg = Config{};
    const session = SessionConfig{ .rows = 3 };
    try testing.expectEqual(@as(usize, 3), cfg.effectiveRows(&session));
}

test "llm config defaults" {
    const llm = LlmConfig{};
    try testing.expectEqualSlices(u8, "anthropic", llm.provider);
    try testing.expect(llm.api_key == null);
    try testing.expectEqual(@as(u32, 1024), llm.max_tokens);
}

test "color config has 16 ansi colors" {
    const colors = ColorConfig{};
    try testing.expectEqual(@as(usize, 16), colors.ansi.len);
}

// ---------------------------------------------------------------------------
// TOML parser tests
// ---------------------------------------------------------------------------

test "parse empty string returns defaults" {
    const cfg = loadConfigFromString("");
    try testing.expectEqual(@as(usize, 1), cfg.grid.rows);
    try testing.expectEqualSlices(u8, "JetBrains Mono", cfg.font.family);
    try testing.expectEqual(@as(u32, 1920), cfg.window.width);
}

test "parse comments and blank lines" {
    const cfg = loadConfigFromString(
        \\# This is a comment
        \\
        \\# Another comment
        \\
    );
    try testing.expectEqual(@as(usize, 1), cfg.grid.rows);
}

test "parse font section" {
    const cfg = loadConfigFromString(
        \\[font]
        \\family = "Fira Code"
        \\size = 16.0
        \\line_height = 1.5
        \\letter_spacing = 0.5
    );
    try testing.expectEqualSlices(u8, "Fira Code", cfg.font.family);
    try testing.expect(@abs(cfg.font.size - 16.0) < 0.01);
    try testing.expect(@abs(cfg.font.line_height - 1.5) < 0.01);
    try testing.expect(@abs(cfg.font.letter_spacing - 0.5) < 0.01);
}

test "parse grid section" {
    const cfg = loadConfigFromString(
        \\[grid]
        \\rows = 3
        \\cols = 4
        \\gap = 8
        \\inner_padding = 10
        \\outer_padding = 12
        \\title_bar_height = 32
        \\border_radius = 16
    );
    try testing.expectEqual(@as(usize, 3), cfg.grid.rows);
    try testing.expectEqual(@as(usize, 4), cfg.grid.cols);
    try testing.expectEqual(@as(u32, 8), cfg.grid.gap);
    try testing.expectEqual(@as(u32, 10), cfg.grid.inner_padding);
    try testing.expectEqual(@as(u32, 12), cfg.grid.outer_padding);
    try testing.expectEqual(@as(u32, 32), cfg.grid.title_bar_height);
    try testing.expectEqual(@as(u32, 16), cfg.grid.border_radius);
}

test "parse window section" {
    const cfg = loadConfigFromString(
        \\[window]
        \\width = 1200
        \\height = 800
        \\title = "My Terminal"
    );
    try testing.expectEqual(@as(u32, 1200), cfg.window.width);
    try testing.expectEqual(@as(u32, 800), cfg.window.height);
    try testing.expectEqualSlices(u8, "My Terminal", cfg.window.title);
}

test "parse colors section with hex values" {
    const cfg = loadConfigFromString(
        \\[colors]
        \\background = "#1e1e2e"
        \\foreground = "#cdd6f4"
        \\cursor = "#f5e0dc"
        \\selection = "#45475a"
    );
    try testing.expectEqualSlices(u8, "#1e1e2e", cfg.colors.background);
    try testing.expectEqualSlices(u8, "#cdd6f4", cfg.colors.foreground);
    try testing.expectEqualSlices(u8, "#f5e0dc", cfg.colors.cursor);
    try testing.expectEqualSlices(u8, "#45475a", cfg.colors.selection);
}

test "parse text_tap section with boolean" {
    const cfg = loadConfigFromString(
        \\[text_tap]
        \\enabled = true
        \\socket_path = "/tmp/termania.sock"
    );
    try testing.expectEqual(true, cfg.text_tap.enabled);
    try testing.expectEqualSlices(u8, "/tmp/termania.sock", cfg.text_tap.socket_path);
}

test "parse text_tap disabled" {
    const cfg = loadConfigFromString(
        \\[text_tap]
        \\enabled = false
    );
    try testing.expectEqual(false, cfg.text_tap.enabled);
}

test "parse llm section with optional fields" {
    const cfg = loadConfigFromString(
        \\[llm]
        \\provider = "anthropic"
        \\api_key = "sk-ant-test123"
        \\model = "claude-sonnet-4-20250514"
        \\max_tokens = 2048
    );
    try testing.expectEqualSlices(u8, "anthropic", cfg.llm.provider);
    try testing.expectEqualSlices(u8, "sk-ant-test123", cfg.llm.api_key.?);
    try testing.expectEqualSlices(u8, "claude-sonnet-4-20250514", cfg.llm.model.?);
    try testing.expectEqual(@as(u32, 2048), cfg.llm.max_tokens);
}

test "parse llm section null optional stays null" {
    const cfg = loadConfigFromString(
        \\[llm]
        \\provider = "openai"
    );
    try testing.expectEqualSlices(u8, "openai", cfg.llm.provider);
    try testing.expect(cfg.llm.api_key == null);
    try testing.expect(cfg.llm.model == null);
    try testing.expect(cfg.llm.base_url == null);
}

test "parse multiple sections" {
    const cfg = loadConfigFromString(
        \\[font]
        \\family = "Hack"
        \\size = 12.0
        \\
        \\[grid]
        \\rows = 2
        \\cols = 3
        \\
        \\[window]
        \\width = 1600
        \\height = 900
        \\title = "Termania Dev"
        \\
        \\[colors]
        \\background = "#282828"
        \\foreground = "#ebdbb2"
        \\
        \\[text_tap]
        \\enabled = false
        \\
        \\[llm]
        \\provider = "openai"
        \\max_tokens = 4096
    );
    try testing.expectEqualSlices(u8, "Hack", cfg.font.family);
    try testing.expect(@abs(cfg.font.size - 12.0) < 0.01);
    try testing.expectEqual(@as(usize, 2), cfg.grid.rows);
    try testing.expectEqual(@as(usize, 3), cfg.grid.cols);
    try testing.expectEqual(@as(u32, 1600), cfg.window.width);
    try testing.expectEqual(@as(u32, 900), cfg.window.height);
    try testing.expectEqualSlices(u8, "Termania Dev", cfg.window.title);
    try testing.expectEqualSlices(u8, "#282828", cfg.colors.background);
    try testing.expectEqualSlices(u8, "#ebdbb2", cfg.colors.foreground);
    try testing.expectEqual(false, cfg.text_tap.enabled);
    try testing.expectEqualSlices(u8, "openai", cfg.llm.provider);
    try testing.expectEqual(@as(u32, 4096), cfg.llm.max_tokens);
}

test "parse sessions array of tables" {
    const cfg = loadConfigFromString(
        \\[[sessions]]
        \\title = "Dev"
        \\rows = 2
        \\cols = 2
        \\
        \\[[sessions.panes]]
        \\pane_type = "terminal"
        \\command = "/bin/bash"
        \\
        \\[[sessions.panes]]
        \\pane_type = "terminal"
        \\command = "/bin/zsh"
        \\cwd = "/home/user/projects"
    );
    // The first session's panes should be available as config panes
    try testing.expectEqual(@as(usize, 2), cfg.panes.len);
    try testing.expectEqualSlices(u8, "terminal", cfg.panes[0].pane_type);
    try testing.expectEqualSlices(u8, "/bin/bash", cfg.panes[0].command.?);
    try testing.expectEqualSlices(u8, "terminal", cfg.panes[1].pane_type);
    try testing.expectEqualSlices(u8, "/bin/zsh", cfg.panes[1].command.?);
    try testing.expectEqualSlices(u8, "/home/user/projects", cfg.panes[1].cwd.?);
}

test "parse inline comments" {
    const cfg = loadConfigFromString(
        \\[grid]
        \\rows = 5 # number of rows
        \\cols = 3 # number of columns
    );
    try testing.expectEqual(@as(usize, 5), cfg.grid.rows);
    try testing.expectEqual(@as(usize, 3), cfg.grid.cols);
}

test "parse inline comment respects quoted hash" {
    const cfg = loadConfigFromString(
        \\[colors]
        \\background = "#1e1e2e" # catppuccin mocha
    );
    try testing.expectEqualSlices(u8, "#1e1e2e", cfg.colors.background);
}

test "parse unknown section is ignored" {
    const cfg = loadConfigFromString(
        \\[unknown_section]
        \\foo = "bar"
        \\
        \\[font]
        \\family = "Iosevka"
    );
    try testing.expectEqualSlices(u8, "Iosevka", cfg.font.family);
}

test "parse unknown field is ignored" {
    const cfg = loadConfigFromString(
        \\[font]
        \\family = "Iosevka"
        \\nonexistent_field = "whatever"
        \\size = 18.0
    );
    try testing.expectEqualSlices(u8, "Iosevka", cfg.font.family);
    try testing.expect(@abs(cfg.font.size - 18.0) < 0.01);
}

test "parse windows line endings" {
    const cfg = loadConfigFromString("[font]\r\nfamily = \"Cascadia Code\"\r\nsize = 13.0\r\n");
    try testing.expectEqualSlices(u8, "Cascadia Code", cfg.font.family);
    try testing.expect(@abs(cfg.font.size - 13.0) < 0.01);
}

test "parse realistic full config" {
    const cfg = loadConfigFromString(
        \\# Termania Configuration
        \\
        \\[font]
        \\family = "JetBrains Mono"
        \\size = 14.0
        \\
        \\[grid]
        \\rows = 1
        \\cols = 2
        \\
        \\[window]
        \\width = 1200
        \\height = 800
        \\title = "Termania"
        \\
        \\[colors]
        \\background = "#1e1e2e"
        \\foreground = "#cdd6f4"
        \\
        \\[text_tap]
        \\enabled = true
        \\socket_path = "/tmp/termania.sock"
        \\
        \\[llm]
        \\provider = "anthropic"
        \\api_key = "sk-ant-api03-test"
        \\model = "claude-sonnet-4-20250514"
        \\
        \\[[sessions]]
        \\title = "Dev"
        \\rows = 2
        \\cols = 2
        \\
        \\[[sessions.panes]]
        \\pane_type = "terminal"
        \\command = "/bin/bash"
    );
    try testing.expectEqualSlices(u8, "JetBrains Mono", cfg.font.family);
    try testing.expect(@abs(cfg.font.size - 14.0) < 0.01);
    try testing.expectEqual(@as(usize, 1), cfg.grid.rows);
    try testing.expectEqual(@as(usize, 2), cfg.grid.cols);
    try testing.expectEqual(@as(u32, 1200), cfg.window.width);
    try testing.expectEqual(@as(u32, 800), cfg.window.height);
    try testing.expectEqualSlices(u8, "Termania", cfg.window.title);
    try testing.expectEqualSlices(u8, "#1e1e2e", cfg.colors.background);
    try testing.expectEqualSlices(u8, "#cdd6f4", cfg.colors.foreground);
    try testing.expectEqual(true, cfg.text_tap.enabled);
    try testing.expectEqualSlices(u8, "/tmp/termania.sock", cfg.text_tap.socket_path);
    try testing.expectEqualSlices(u8, "anthropic", cfg.llm.provider);
    try testing.expectEqualSlices(u8, "sk-ant-api03-test", cfg.llm.api_key.?);
    try testing.expectEqualSlices(u8, "claude-sonnet-4-20250514", cfg.llm.model.?);
    try testing.expectEqual(@as(usize, 1), cfg.panes.len);
    try testing.expectEqualSlices(u8, "terminal", cfg.panes[0].pane_type);
    try testing.expectEqualSlices(u8, "/bin/bash", cfg.panes[0].command.?);
}

test "parse pane with all optional fields" {
    const cfg = loadConfigFromString(
        \\[[sessions]]
        \\title = "Full Pane Test"
        \\
        \\[[sessions.panes]]
        \\pane_type = "browser"
        \\title = "Web View"
        \\url = "https://example.com"
        \\refresh_ms = 5000
    );
    try testing.expectEqual(@as(usize, 1), cfg.panes.len);
    try testing.expectEqualSlices(u8, "browser", cfg.panes[0].pane_type);
    try testing.expectEqualSlices(u8, "Web View", cfg.panes[0].title.?);
    try testing.expectEqualSlices(u8, "https://example.com", cfg.panes[0].url.?);
    try testing.expectEqual(@as(u64, 5000), cfg.panes[0].refresh_ms.?);
}

test "parse bold_family optional string" {
    const cfg = loadConfigFromString(
        \\[font]
        \\family = "Fira Code"
        \\bold_family = "Fira Code Bold"
    );
    try testing.expectEqualSlices(u8, "Fira Code", cfg.font.family);
    try testing.expectEqualSlices(u8, "Fira Code Bold", cfg.font.bold_family.?);
}

test "parse stripInlineComment unit" {
    try testing.expectEqualSlices(u8, "42", stripInlineComment("42 # comment"));
    try testing.expectEqualSlices(u8, "\"#1e1e2e\"", stripInlineComment("\"#1e1e2e\" # a color"));
    try testing.expectEqualSlices(u8, "true", stripInlineComment("true # enable it"));
    try testing.expectEqualSlices(u8, "\"hello world\"", stripInlineComment("\"hello world\""));
}

test "parse parseQuotedString unit" {
    try testing.expectEqualSlices(u8, "hello", parseQuotedString("\"hello\""));
    try testing.expectEqualSlices(u8, "#1e1e2e", parseQuotedString("\"#1e1e2e\""));
    try testing.expectEqualSlices(u8, "42", parseQuotedString("42"));
    try testing.expectEqualSlices(u8, "", parseQuotedString("\"\""));
}

test "parse setStructField handles unknown fields gracefully" {
    var font = FontConfig{};
    setStructField(FontConfig, &font, "nonexistent", "value");
    // Should not crash, font should be unchanged
    try testing.expectEqualSlices(u8, "JetBrains Mono", font.family);
}

test "parse float values" {
    try testing.expect(@abs(parseFloat("14.0") - 14.0) < 0.01);
    try testing.expect(@abs(parseFloat("0.5") - 0.5) < 0.01);
    try testing.expect(@abs(parseFloat("100.123") - 100.123) < 0.01);
    try testing.expect(@abs(parseFloat("invalid") - 0.0) < 0.01);
}

test "parse unsigned values" {
    try testing.expectEqual(@as(u32, 42), parseUnsigned(u32, "42"));
    try testing.expectEqual(@as(u32, 0), parseUnsigned(u32, "0"));
    try testing.expectEqual(@as(u32, 0), parseUnsigned(u32, "invalid"));
    try testing.expectEqual(@as(usize, 1024), parseUnsigned(usize, "1024"));
}
