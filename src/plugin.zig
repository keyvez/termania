const std = @import("std");
const terminal = @import("terminal.zig");
const config_mod = @import("config.zig");
const testing = std.testing;

/// Types of pane plugins — following Ghostty's comptime enum pattern.
pub const PaneType = enum {
    terminal_pane,
    webview,
    notes,
    screen_capture,
    file_browser,
    process_monitor,
    log_viewer,
    markdown_preview,
    system_info,
    git_status,

    pub fn toString(self: PaneType) []const u8 {
        return switch (self) {
            .terminal_pane => "terminal",
            .webview => "webview",
            .notes => "notes",
            .screen_capture => "screen_capture",
            .file_browser => "file_browser",
            .process_monitor => "process_monitor",
            .log_viewer => "log_viewer",
            .markdown_preview => "markdown_preview",
            .system_info => "system_info",
            .git_status => "git_status",
        };
    }

    pub fn fromString(s: []const u8) ?PaneType {
        const map = .{
            .{ "terminal", PaneType.terminal_pane },
            .{ "webview", PaneType.webview },
            .{ "notes", PaneType.notes },
            .{ "screen_capture", PaneType.screen_capture },
            .{ "file_browser", PaneType.file_browser },
            .{ "process_monitor", PaneType.process_monitor },
            .{ "log_viewer", PaneType.log_viewer },
            .{ "markdown_preview", PaneType.markdown_preview },
            .{ "system_info", PaneType.system_info },
            .{ "git_status", PaneType.git_status },
        };
        inline for (map) |entry| {
            if (std.mem.eql(u8, s, entry[0])) return entry[1];
        }
        return null;
    }
};

/// Data returned by a plugin for rendering.
/// Tagged union — Ghostty uses similar comptime tagged unions for render data.
pub const PanePluginRenderData = union(enum) {
    /// Terminal cell grid — rendered via GPU cell pipeline.
    terminal_data: struct {
        lines: []const terminal.Cell,
        rows: usize,
        cols: usize,
        cursor_row: usize,
        cursor_col: usize,
        watermark: ?[]const u8,
    },
    /// A native view is managed by the plugin (the renderer draws border/title only).
    native_view: struct {
        view_id: u64,
    },
    /// Plugin provides custom cell data for rendering.
    custom_cells: struct {
        lines: []const terminal.Cell,
        rows: usize,
        cols: usize,
    },
    /// GPU texture placeholder for future use.
    gpu_texture: struct {},
};

/// Vtable for pane plugins — Ghostty-style comptime interface.
/// Each plugin provides a pointer to this struct with function pointers.
pub const PanePluginVTable = struct {
    paneType: *const fn (ctx: *anyopaque) PaneType,
    title: *const fn (ctx: *anyopaque) []const u8,
    setTitle: *const fn (ctx: *anyopaque, new_title: []const u8) void,
    init_fn: *const fn (ctx: *anyopaque) void,
    shutdown: *const fn (ctx: *anyopaque) void,
    handleKey: *const fn (ctx: *anyopaque, text: []const u8) void,
    resize_fn: *const fn (ctx: *anyopaque, width_px: f32, height_px: f32, cell_w: f32, cell_h: f32) void,
    renderData: *const fn (ctx: *anyopaque) PanePluginRenderData,
    visibleText: *const fn (ctx: *anyopaque, buf: []u8) usize,
    writeInput: *const fn (ctx: *anyopaque, data: []const u8) void,
    poll: *const fn (ctx: *anyopaque) bool,
    hasError: *const fn (ctx: *anyopaque) bool,
    isDirty: *const fn (ctx: *anyopaque) bool,
    clearDirty: *const fn (ctx: *anyopaque) void,
    scrollUp: *const fn (ctx: *anyopaque, lines: usize) void,
    scrollDown: *const fn (ctx: *anyopaque, lines: usize) void,
    isExited: *const fn (ctx: *anyopaque) bool,
    isNativeView: *const fn (ctx: *anyopaque) bool,
    childPid: *const fn (ctx: *anyopaque) ?u32,
    deinit_fn: *const fn (ctx: *anyopaque) void,
};

/// A type-erased pane plugin. Holds a context pointer and a vtable.
pub const PanePlugin = struct {
    ctx: *anyopaque,
    vtable: *const PanePluginVTable,

    pub fn paneType(self: PanePlugin) PaneType {
        return self.vtable.paneType(self.ctx);
    }

    pub fn title(self: PanePlugin) []const u8 {
        return self.vtable.title(self.ctx);
    }

    pub fn setTitle(self: PanePlugin, new_title: []const u8) void {
        self.vtable.setTitle(self.ctx, new_title);
    }

    pub fn doInit(self: PanePlugin) void {
        self.vtable.init_fn(self.ctx);
    }

    pub fn shutdown(self: PanePlugin) void {
        self.vtable.shutdown(self.ctx);
    }

    pub fn handleKey(self: PanePlugin, text: []const u8) void {
        self.vtable.handleKey(self.ctx, text);
    }

    pub fn doResize(self: PanePlugin, width_px: f32, height_px: f32, cell_w: f32, cell_h: f32) void {
        self.vtable.resize_fn(self.ctx, width_px, height_px, cell_w, cell_h);
    }

    pub fn renderData(self: PanePlugin) PanePluginRenderData {
        return self.vtable.renderData(self.ctx);
    }

    pub fn visibleText(self: PanePlugin, buf: []u8) usize {
        return self.vtable.visibleText(self.ctx, buf);
    }

    pub fn writeInput(self: PanePlugin, data: []const u8) void {
        self.vtable.writeInput(self.ctx, data);
    }

    pub fn poll(self: PanePlugin) bool {
        return self.vtable.poll(self.ctx);
    }

    pub fn hasError(self: PanePlugin) bool {
        return self.vtable.hasError(self.ctx);
    }

    pub fn isDirty(self: PanePlugin) bool {
        return self.vtable.isDirty(self.ctx);
    }

    pub fn clearDirty(self: PanePlugin) void {
        self.vtable.clearDirty(self.ctx);
    }

    pub fn scrollUp(self: PanePlugin, lines: usize) void {
        self.vtable.scrollUp(self.ctx, lines);
    }

    pub fn scrollDown(self: PanePlugin, lines: usize) void {
        self.vtable.scrollDown(self.ctx, lines);
    }

    pub fn isExited(self: PanePlugin) bool {
        return self.vtable.isExited(self.ctx);
    }

    pub fn isNativeView(self: PanePlugin) bool {
        return self.vtable.isNativeView(self.ctx);
    }

    pub fn childPid(self: PanePlugin) ?u32 {
        return self.vtable.childPid(self.ctx);
    }

    pub fn deinit(self: PanePlugin) void {
        self.vtable.deinit_fn(self.ctx);
    }
};

// ---------------------------------------------------------------------------
// Terminal Plugin — wraps Terminal + PTY as a PanePlugin
// ---------------------------------------------------------------------------

pub const TerminalPlugin = struct {
    allocator: std.mem.Allocator,
    term: *terminal.Terminal,
    pty_fd: ?@import("pty.zig").Pty = null,
    read_buf: [65536]u8 = undefined,
    pending_initial_commands: std.ArrayList([]const u8),
    initial_commands_sent: bool = false,
    last_output_time: ?i64 = null,

    pub fn create(
        allocator: std.mem.Allocator,
        index: usize,
        pane_config: ?*const config_mod.PaneConfig,
    ) !*TerminalPlugin {
        const title_str = if (pane_config) |pc| pc.title orelse "Pane" else "Pane";
        var title_buf: [64]u8 = undefined;
        const title = if (pane_config != null and pane_config.?.title != null)
            title_str
        else
            std.fmt.bufPrint(&title_buf, "Pane {d}", .{index + 1}) catch "Pane";

        const term = try terminal.Terminal.init(allocator, 80, 24, title);

        const self = try allocator.create(TerminalPlugin);
        self.* = .{
            .allocator = allocator,
            .term = term,
            .pending_initial_commands = std.ArrayList([]const u8).init(allocator),
        };

        // Queue initial commands
        if (pane_config) |pc| {
            if (pc.initial_commands) |cmds| {
                for (cmds) |cmd| {
                    try self.pending_initial_commands.append(cmd);
                }
            }
        }

        // Spawn PTY
        const shell = if (pane_config) |pc| pc.command else null;
        const cwd_val = if (pane_config) |pc| pc.cwd else null;

        self.pty_fd = @import("pty.zig").Pty.spawn(80, 24, shell, cwd_val) catch null;

        return self;
    }

    pub fn destroy(self: *TerminalPlugin) void {
        self.term.deinit();
        self.pending_initial_commands.deinit();
        if (self.pty_fd) |*p| p.close();
        self.allocator.destroy(self);
    }

    fn pollImpl(self: *TerminalPlugin) bool {
        const was_dirty = self.term.isDirty();

        // Read from PTY
        if (self.pty_fd) |pty| {
            if (pty.read(&self.read_buf)) |n| {
                self.term.processOutput(self.read_buf[0..n]);
                self.term.detectErrors(self.read_buf[0..n]);
                self.last_output_time = std.time.timestamp();
            }
        }

        // Send initial commands after 500ms idle
        if (!self.initial_commands_sent and self.pending_initial_commands.items.len > 0) {
            if (self.last_output_time) |last| {
                const now = std.time.timestamp();
                if (now - last >= 1) { // ~1 second idle
                    self.initial_commands_sent = true;
                    if (self.pty_fd) |pty| {
                        for (self.pending_initial_commands.items) |cmd| {
                            _ = pty.write(cmd) catch {};
                            _ = pty.write("\r") catch {};
                        }
                    }
                }
            }
        }

        return self.term.isDirty() and !was_dirty;
    }

    // VTable implementation functions
    fn vtPaneType(ctx: *anyopaque) PaneType {
        _ = ptrCast(ctx);
        return .terminal_pane;
    }

    fn vtTitle(ctx: *anyopaque) []const u8 {
        const self = ptrCast(ctx);
        return self.term.getTitle();
    }

    fn vtSetTitle(ctx: *anyopaque, new_title: []const u8) void {
        const self = ptrCast(ctx);
        self.term.setTitle(new_title);
    }

    fn vtInit(ctx: *anyopaque) void {
        _ = ptrCast(ctx);
    }

    fn vtShutdown(ctx: *anyopaque) void {
        _ = ptrCast(ctx);
    }

    fn vtHandleKey(ctx: *anyopaque, text: []const u8) void {
        _ = ptrCast(ctx);
        _ = text;
    }

    fn vtResize(ctx: *anyopaque, width_px: f32, height_px: f32, cell_w: f32, cell_h: f32) void {
        const self = ptrCast(ctx);
        const new_cols: usize = @intFromFloat(@max(@divFloor(width_px, cell_w), 1.0));
        const new_rows: usize = @intFromFloat(@max(@divFloor(height_px, cell_h), 1.0));
        self.term.resize(new_cols, new_rows);
        if (self.pty_fd) |pty| {
            pty.doResize(@intCast(new_cols), @intCast(new_rows));
        }
    }

    fn vtRenderData(ctx: *anyopaque) PanePluginRenderData {
        const self = ptrCast(ctx);
        const is_scrolled = self.term.getScrollOffset() > 0;
        return .{ .terminal_data = .{
            .lines = self.term.cells,
            .rows = self.term.rows,
            .cols = self.term.cols,
            .cursor_row = if (is_scrolled) std.math.maxInt(usize) else self.term.cursor_row,
            .cursor_col = if (is_scrolled) std.math.maxInt(usize) else self.term.cursor_col,
            .watermark = null,
        } };
    }

    fn vtVisibleText(ctx: *anyopaque, buf: []u8) usize {
        const self = ptrCast(ctx);
        return self.term.visibleText(buf);
    }

    fn vtWriteInput(ctx: *anyopaque, data: []const u8) void {
        const self = ptrCast(ctx);
        // Snap to live view
        if (self.term.scroll_offset > 0) {
            self.term.scroll_offset = 0;
            self.term.dirty = true;
        }
        if (self.pty_fd) |pty| {
            _ = pty.write(data) catch {};
        }
    }

    fn vtPoll(ctx: *anyopaque) bool {
        const self = ptrCast(ctx);
        return self.pollImpl();
    }

    fn vtHasError(ctx: *anyopaque) bool {
        const self = ptrCast(ctx);
        return self.term.hasError();
    }

    fn vtIsDirty(ctx: *anyopaque) bool {
        const self = ptrCast(ctx);
        return self.term.isDirty();
    }

    fn vtClearDirty(ctx: *anyopaque) void {
        const self = ptrCast(ctx);
        self.term.clearDirty();
    }

    fn vtScrollUp(ctx: *anyopaque, lines: usize) void {
        const self = ptrCast(ctx);
        self.term.scrollViewUp(lines);
    }

    fn vtScrollDown(ctx: *anyopaque, lines: usize) void {
        const self = ptrCast(ctx);
        self.term.scrollViewDown(lines);
    }

    fn vtIsExited(ctx: *anyopaque) bool {
        const self = ptrCast(ctx);
        if (self.pty_fd) |pty| return !pty.isAlive();
        return true;
    }

    fn vtIsNativeView(_: *anyopaque) bool {
        return false;
    }

    fn vtChildPid(ctx: *anyopaque) ?u32 {
        const self = ptrCast(ctx);
        if (self.pty_fd) |pty| return pty.childPid();
        return null;
    }

    fn vtDeinit(ctx: *anyopaque) void {
        const self = ptrCast(ctx);
        self.destroy();
    }

    fn ptrCast(ctx: *anyopaque) *TerminalPlugin {
        return @ptrCast(@alignCast(ctx));
    }

    pub const vtable = PanePluginVTable{
        .paneType = vtPaneType,
        .title = vtTitle,
        .setTitle = vtSetTitle,
        .init_fn = vtInit,
        .shutdown = vtShutdown,
        .handleKey = vtHandleKey,
        .resize_fn = vtResize,
        .renderData = vtRenderData,
        .visibleText = vtVisibleText,
        .writeInput = vtWriteInput,
        .poll = vtPoll,
        .hasError = vtHasError,
        .isDirty = vtIsDirty,
        .clearDirty = vtClearDirty,
        .scrollUp = vtScrollUp,
        .scrollDown = vtScrollDown,
        .isExited = vtIsExited,
        .isNativeView = vtIsNativeView,
        .childPid = vtChildPid,
        .deinit_fn = vtDeinit,
    };

    /// Convert to a type-erased PanePlugin.
    pub fn plugin(self: *TerminalPlugin) PanePlugin {
        return .{
            .ctx = @ptrCast(self),
            .vtable = &vtable,
        };
    }
};

// ---------------------------------------------------------------------------
// Stub plugins (file_browser, process_monitor, system_info, etc.)
// These follow the same vtable pattern. In a full build they'd have
// real implementations; here they provide the architecture scaffold.
// ---------------------------------------------------------------------------

/// Generic stub plugin for non-terminal pane types.
pub const StubPlugin = struct {
    allocator: std.mem.Allocator,
    pane_type_val: PaneType,
    title_buf: [128]u8 = undefined,
    title_len: usize = 0,

    pub fn create(allocator: std.mem.Allocator, ptype: PaneType, title_str: []const u8) !*StubPlugin {
        const self = try allocator.create(StubPlugin);
        self.* = .{
            .allocator = allocator,
            .pane_type_val = ptype,
        };
        const len = @min(title_str.len, self.title_buf.len);
        @memcpy(self.title_buf[0..len], title_str[0..len]);
        self.title_len = len;
        return self;
    }

    pub fn destroy(self: *StubPlugin) void {
        self.allocator.destroy(self);
    }

    fn ptrCast(ctx: *anyopaque) *StubPlugin {
        return @ptrCast(@alignCast(ctx));
    }

    fn vtPaneType(ctx: *anyopaque) PaneType {
        return ptrCast(ctx).pane_type_val;
    }
    fn vtTitle(ctx: *anyopaque) []const u8 {
        const self = ptrCast(ctx);
        return self.title_buf[0..self.title_len];
    }
    fn vtSetTitle(ctx: *anyopaque, new_title: []const u8) void {
        const self = ptrCast(ctx);
        const len = @min(new_title.len, self.title_buf.len);
        @memcpy(self.title_buf[0..len], new_title[0..len]);
        self.title_len = len;
    }
    fn vtNoop(_: *anyopaque) void {}
    fn vtHandleKey(_: *anyopaque, _: []const u8) void {}
    fn vtResize(_: *anyopaque, _: f32, _: f32, _: f32, _: f32) void {}
    fn vtRenderData(_: *anyopaque) PanePluginRenderData {
        return .{ .custom_cells = .{ .lines = &.{}, .rows = 0, .cols = 0 } };
    }
    fn vtVisibleText(_: *anyopaque, _: []u8) usize {
        return 0;
    }
    fn vtWriteInput(_: *anyopaque, _: []const u8) void {}
    fn vtPollFalse(_: *anyopaque) bool {
        return false;
    }
    fn vtBoolFalse(_: *anyopaque) bool {
        return false;
    }
    fn vtScrollNoop(_: *anyopaque, _: usize) void {}
    fn vtChildPidNull(_: *anyopaque) ?u32 {
        return null;
    }
    fn vtDeinit(ctx: *anyopaque) void {
        ptrCast(ctx).destroy();
    }

    pub const vtable = PanePluginVTable{
        .paneType = vtPaneType,
        .title = vtTitle,
        .setTitle = vtSetTitle,
        .init_fn = vtNoop,
        .shutdown = vtNoop,
        .handleKey = vtHandleKey,
        .resize_fn = vtResize,
        .renderData = vtRenderData,
        .visibleText = vtVisibleText,
        .writeInput = vtWriteInput,
        .poll = vtPollFalse,
        .hasError = vtBoolFalse,
        .isDirty = vtBoolFalse,
        .clearDirty = vtNoop,
        .scrollUp = vtScrollNoop,
        .scrollDown = vtScrollNoop,
        .isExited = vtBoolFalse,
        .isNativeView = vtBoolFalse,
        .childPid = vtChildPidNull,
        .deinit_fn = vtDeinit,
    };

    pub fn plugin(self: *StubPlugin) PanePlugin {
        return .{
            .ctx = @ptrCast(self),
            .vtable = &vtable,
        };
    }
};

/// Create a PanePlugin from a PaneConfig.
pub fn createPlugin(allocator: std.mem.Allocator, index: usize, pane_config: ?*const config_mod.PaneConfig) !PanePlugin {
    const ptype_str = if (pane_config) |pc| pc.pane_type else "terminal";
    const ptype = PaneType.fromString(ptype_str) orelse .terminal_pane;

    switch (ptype) {
        .terminal_pane => {
            const tp = try TerminalPlugin.create(allocator, index, pane_config);
            return tp.plugin();
        },
        else => {
            const title = if (pane_config) |pc| pc.title orelse ptype.toString() else ptype.toString();
            const stub = try StubPlugin.create(allocator, ptype, title);
            return stub.plugin();
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "pane type to string" {
    try testing.expectEqualSlices(u8, "terminal", PaneType.terminal_pane.toString());
    try testing.expectEqualSlices(u8, "webview", PaneType.webview.toString());
    try testing.expectEqualSlices(u8, "git_status", PaneType.git_status.toString());
}

test "pane type from string" {
    try testing.expectEqual(PaneType.terminal_pane, PaneType.fromString("terminal").?);
    try testing.expectEqual(PaneType.webview, PaneType.fromString("webview").?);
    try testing.expect(PaneType.fromString("unknown") == null);
}

test "stub plugin vtable" {
    const stub = try StubPlugin.create(testing.allocator, .file_browser, "Files");
    defer stub.destroy();
    const p = stub.plugin();
    try testing.expectEqual(PaneType.file_browser, p.paneType());
    try testing.expectEqualSlices(u8, "Files", p.title());
    try testing.expect(!p.isExited());
    try testing.expect(!p.isDirty());
}
