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

/// Simple TOML-like key=value config file loader.
/// Parses a subset of TOML: sections [section], key = "value", key = number.
/// Returns a Config with fields populated from the file content.
pub fn loadConfigFromString(content: []const u8) Config {
    _ = content;
    // For now return defaults — full TOML parser would be added as a build dependency
    return Config{};
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
