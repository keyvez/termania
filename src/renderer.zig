const std = @import("std");
const testing = std.testing;
const terminal = @import("terminal.zig");
const config_mod = @import("config.zig");
const grid_mod = @import("grid.zig");
const plugin_mod = @import("plugin.zig");

// ---------------------------------------------------------------------------
// Renderer — following Ghostty's comptime backend pattern.
//
// Ghostty uses comptime to select between Metal (macOS) and OpenGL (Linux).
// Here we define the renderer interface and provide a software/null backend
// for headless operation and testing. GPU backends (OpenGL, Vulkan, Metal)
// can be plugged in at comptime.
// ---------------------------------------------------------------------------

/// Backend selection — chosen at comptime like Ghostty's renderer backends.
pub const Backend = enum {
    /// Null/headless backend for testing and CI.
    null_backend,
    /// OpenGL 3.3+ backend (Linux, like Ghostty).
    opengl,
};

/// Compile-time selected backend (can be overridden via build options).
pub const active_backend: Backend = .null_backend;

/// Data needed to render a single pane.
pub const PaneRenderData = struct {
    title: []const u8,
    plugin_data: plugin_mod.PanePluginRenderData,
    is_focused: bool = false,
    watermark: ?[]const u8 = null,
    has_error: bool = false,
    is_selected: bool = false,
    broadcast_mode: bool = false,
    text_selection: ?struct { start_row: usize, start_col: usize, end_row: usize, end_col: usize } = null,
};

/// Data for the command overlay.
pub const OverlayRenderData = struct {
    text: []const u8 = "",
    target_label: []const u8 = "",
    mode_label: []const u8 = "",
    is_thinking: bool = false,
    response_lines: []const []const u8 = &.{},
};

/// Vertex for textured quad rendering (text glyphs).
pub const Vertex = extern struct {
    position: [2]f32,
    tex_coords: [2]f32,
    color: [4]f32,
};

/// Vertex for solid-color rectangles.
pub const RectVertex = extern struct {
    position: [2]f32,
    color: [4]f32,
};

/// Vertex for SDF rounded rectangles.
pub const RoundedRectVertex = extern struct {
    position: [2]f32,
    color: [4]f32,
    local_pos: [2]f32,
    half_size: [2]f32,
    radius: f32,
    border_thickness: f32,
};

/// Glyph cache key.
pub const GlyphKey = struct {
    ch: u21,
    bold: bool,
    italic: bool,
    size_cp: u32,
};

/// Glyph atlas entry.
pub const GlyphInfo = struct {
    tex_x: f32 = 0,
    tex_y: f32 = 0,
    tex_w: f32 = 0,
    tex_h: f32 = 0,
    width: f32 = 0,
    height: f32 = 0,
    offset_x: f32 = 0,
    offset_y: f32 = 0,
};

/// The renderer state.
pub const Renderer = struct {
    allocator: std.mem.Allocator,

    width: u32,
    height: u32,
    scale_factor: f32 = 1.0,

    // Font metrics
    font_size: f32 = 14.0,
    physical_font_size: f32 = 14.0,
    line_height: f32 = 1.2,
    cell_width: f32 = 8.4,
    cell_height: f32 = 16.8,

    // Config colors (parsed to RGBA floats)
    bg_color: [4]f32 = .{ 0.004, 0.016, 0.035, 1.0 },
    fg_color: [4]f32 = .{ 0.902, 0.929, 0.953, 1.0 },
    cursor_color: [4]f32 = .{ 0.941, 0.965, 0.988, 1.0 },
    border_color: [4]f32 = .{ 0.188, 0.212, 0.239, 1.0 },
    border_focused_color: [4]f32 = .{ 0.345, 0.651, 1.0, 1.0 },
    title_bg_color: [4]f32 = .{ 0.051, 0.067, 0.090, 1.0 },
    title_fg_color: [4]f32 = .{ 0.902, 0.929, 0.953, 1.0 },
    selection_color: [4]f32 = .{ 0.149, 0.310, 0.471, 1.0 },
    ansi_colors: [16][4]f32 = undefined,

    /// Frame counter for metrics.
    frame_count: u64 = 0,

    pub fn init(allocator: std.mem.Allocator, cfg: *const config_mod.Config) Renderer {
        var self = Renderer{
            .allocator = allocator,
            .width = cfg.window.width,
            .height = cfg.window.height,
        };

        // Parse config colors
        self.bg_color = config_mod.parseHexColor(cfg.colors.background);
        self.fg_color = config_mod.parseHexColor(cfg.colors.foreground);
        self.cursor_color = config_mod.parseHexColor(cfg.colors.cursor);
        self.border_color = config_mod.parseHexColor(cfg.colors.border);
        self.border_focused_color = config_mod.parseHexColor(cfg.colors.border_focused);
        self.title_bg_color = config_mod.parseHexColor(cfg.colors.title_bg);
        self.title_fg_color = config_mod.parseHexColor(cfg.colors.title_fg);
        self.selection_color = config_mod.parseHexColor(cfg.colors.selection);

        for (cfg.colors.ansi, 0..) |hex, i| {
            self.ansi_colors[i] = config_mod.parseHexColor(hex);
        }

        // Compute cell dimensions
        self.font_size = cfg.font.size;
        self.physical_font_size = cfg.font.size * self.scale_factor;
        self.line_height = cfg.font.line_height;
        self.cell_width = self.physical_font_size * 0.6;
        self.cell_height = self.physical_font_size * self.line_height;

        return self;
    }

    pub fn deinit(self: *Renderer) void {
        _ = self;
    }

    /// Resolve a CellColor to an RGBA float array.
    pub fn resolveColor(self: *const Renderer, color: terminal.CellColor, is_fg: bool) [4]f32 {
        return switch (color) {
            .default => if (is_fg) self.fg_color else self.bg_color,
            .ansi => |idx| if (idx < 16) self.ansi_colors[idx] else self.fg_color,
            .indexed => |idx| resolveIndexedColor(idx),
            .rgb => |v| .{
                @as(f32, @floatFromInt(v.r)) / 255.0,
                @as(f32, @floatFromInt(v.g)) / 255.0,
                @as(f32, @floatFromInt(v.b)) / 255.0,
                1.0,
            },
        };
    }

    /// Render a frame (null backend — no-op).
    pub fn render(
        self: *Renderer,
        panes: []const PaneRenderData,
        layouts: []const grid_mod.PaneLayout,
        overlay: ?*const OverlayRenderData,
    ) void {
        _ = panes;
        _ = layouts;
        _ = overlay;
        self.frame_count += 1;
    }

    /// Resize the rendering surface.
    pub fn handleResize(self: *Renderer, width: u32, height: u32) void {
        self.width = width;
        self.height = height;
    }

    pub fn getCellWidth(self: *const Renderer) f32 {
        return self.cell_width;
    }

    pub fn getCellHeight(self: *const Renderer) f32 {
        return self.cell_height;
    }
};

/// Resolve a 256-color indexed color to RGB.
fn resolveIndexedColor(idx: u8) [4]f32 {
    if (idx < 16) {
        // Standard colors — would use ansi palette, fallback to basic
        return .{ 0.5, 0.5, 0.5, 1.0 };
    }
    if (idx < 232) {
        // 6x6x6 color cube (indices 16-231)
        const n = idx - 16;
        const b_val: u8 = n % 6;
        const g_val: u8 = (n / 6) % 6;
        const r_val: u8 = (n / 36) % 6;
        return .{
            if (r_val == 0) 0.0 else (@as(f32, @floatFromInt(r_val)) * 40.0 + 55.0) / 255.0,
            if (g_val == 0) 0.0 else (@as(f32, @floatFromInt(g_val)) * 40.0 + 55.0) / 255.0,
            if (b_val == 0) 0.0 else (@as(f32, @floatFromInt(b_val)) * 40.0 + 55.0) / 255.0,
            1.0,
        };
    }
    // Grayscale (indices 232-255)
    const level = @as(f32, @floatFromInt(@as(u16, idx - 232) * 10 + 8)) / 255.0;
    return .{ level, level, level, 1.0 };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "renderer init" {
    const cfg = config_mod.Config{};
    var r = Renderer.init(testing.allocator, &cfg);
    defer r.deinit();
    try testing.expect(r.width == 1920);
    try testing.expect(r.height == 1080);
    try testing.expect(r.cell_width > 0);
    try testing.expect(r.cell_height > 0);
}

test "resolve color default" {
    const cfg = config_mod.Config{};
    const r = Renderer.init(testing.allocator, &cfg);
    const fg = r.resolveColor(.default, true);
    const bg = r.resolveColor(.default, false);
    try testing.expect(fg[0] != bg[0] or fg[1] != bg[1] or fg[2] != bg[2]);
}

test "resolve indexed color cube" {
    const c = resolveIndexedColor(196); // bright red
    try testing.expect(c[0] > 0.8);
    try testing.expect(c[1] < 0.1);
}

test "resolve indexed grayscale" {
    const c = resolveIndexedColor(255); // white-ish
    try testing.expect(c[0] > 0.8);
    try testing.expectEqual(c[0], c[1]);
    try testing.expectEqual(c[1], c[2]);
}
