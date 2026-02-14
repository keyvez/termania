const std = @import("std");
const termania = @import("termania");

const config_mod = termania.config;
const grid_mod = termania.grid;
const plugin_mod = termania.plugin;
const renderer_mod = termania.renderer;
const text_tap_mod = termania.text_tap;
const llm_mod = termania.llm;

// ---------------------------------------------------------------------------
// Termania — Multi-pane terminal emulator (Zig rewrite)
//
// Architecture follows Ghostty's patterns:
//   - Comptime backend selection for rendering
//   - Per-surface threading model for I/O
//   - Tagged unions for actions and render data
//   - Vtable-based plugin interfaces
// ---------------------------------------------------------------------------

const OverlayMode = enum {
    llm,
    raw_command,
    token_input,
};

const CommandOverlay = struct {
    input: std.ArrayList(u8),
    mode: OverlayMode = .llm,
    llm_thinking: bool = false,

    fn init(allocator: std.mem.Allocator) CommandOverlay {
        return .{ .input = std.ArrayList(u8).init(allocator) };
    }

    fn deinit(self: *CommandOverlay) void {
        self.input.deinit();
    }
};

/// Main application state.
const App = struct {
    allocator: std.mem.Allocator,
    config: config_mod.Config,
    grid: grid_mod.GridManager,
    renderer: renderer_mod.Renderer,
    plugins: std.ArrayList(plugin_mod.PanePlugin),
    text_tap: text_tap_mod.TextTapServer,
    overlay: ?CommandOverlay = null,

    focused_pane: usize = 0,
    broadcast_mode: bool = false,
    running: bool = true,

    fn init(allocator: std.mem.Allocator) !App {
        const cfg = config_mod.loadConfig();
        const session: ?*const config_mod.SessionConfig = null;

        const num_rows = cfg.effectiveRows(session);
        const num_cols = cfg.effectiveCols(session);
        var grd = try grid_mod.GridManager.init(allocator, num_rows, num_cols);

        const total_panes = grd.totalPanes();
        var plugins = std.ArrayList(plugin_mod.PanePlugin).init(allocator);

        const pane_cfgs = cfg.effectivePanes(session);

        for (0..total_panes) |i| {
            const pc: ?*const config_mod.PaneConfig = if (i < pane_cfgs.len) &pane_cfgs[i] else null;
            const p = try plugin_mod.createPlugin(allocator, i, pc);
            try plugins.append(p);
        }

        const rend = renderer_mod.Renderer.init(allocator, &cfg);

        var tap = text_tap_mod.TextTapServer.init(allocator, cfg.text_tap.socket_path);
        tap.setPaneCount(total_panes);
        if (cfg.text_tap.enabled) tap.start();

        return .{
            .allocator = allocator,
            .config = cfg,
            .grid = grd,
            .renderer = rend,
            .plugins = plugins,
            .text_tap = tap,
        };
    }

    fn deinit(self: *App) void {
        for (self.plugins.items) |p| p.deinit();
        self.plugins.deinit();
        self.grid.deinit();
        self.renderer.deinit();
        self.text_tap.stop();
        self.text_tap.deinit();
        if (self.overlay) |*o| o.deinit();
    }

    /// Main event loop tick — poll panes and render.
    fn tick(self: *App) void {
        // Poll all plugins
        for (self.plugins.items) |p| {
            _ = p.poll();
        }

        // Process text tap commands
        const cmds = self.text_tap.drainCommands();
        defer self.allocator.free(cmds);
        for (cmds) |cmd| {
            self.executeTapCommand(cmd);
        }

        // Render frame (null backend is a no-op)
        self.renderer.render(&.{}, &.{}, null);
    }

    fn executeTapCommand(self: *App, cmd: text_tap_mod.TapCommand) void {
        switch (cmd) {
            .send => |s| {
                switch (s.target) {
                    .pane => |idx| {
                        if (idx < self.plugins.items.len) {
                            self.plugins.items[idx].writeInput(s.input);
                        }
                    },
                    .all => {
                        for (self.plugins.items) |p| {
                            p.writeInput(s.input);
                        }
                    },
                }
            },
            .action => |a| self.executeAction(a),
        }
    }

    fn executeAction(self: *App, action: llm_mod.TermaniaAction) void {
        switch (action) {
            .send_command => |a| {
                if (a.pane < self.plugins.items.len) {
                    self.plugins.items[a.pane].writeInput(a.command);
                    self.plugins.items[a.pane].writeInput("\r");
                }
            },
            .send_to_all => |a| {
                for (self.plugins.items) |p| {
                    p.writeInput(a.command);
                    p.writeInput("\r");
                }
            },
            .set_title => |a| {
                if (a.pane < self.plugins.items.len) {
                    self.plugins.items[a.pane].setTitle(a.title);
                }
            },
            .focus_pane => |a| {
                if (a.pane < self.plugins.items.len) {
                    self.focused_pane = a.pane;
                }
            },
            .close_pane => |a| {
                if (a.pane < self.plugins.items.len and self.plugins.items.len > 1) {
                    self.plugins.items[a.pane].deinit();
                    _ = self.plugins.orderedRemove(a.pane);
                    if (self.focused_pane >= self.plugins.items.len) {
                        self.focused_pane = self.plugins.items.len - 1;
                    }
                }
            },
            .message => {},
            else => {},
        }
    }
};

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const stdout = std.io.getStdOut().writer();

    try stdout.print(
        \\
        \\  ████████╗███████╗██████╗ ███╗   ███╗ █████╗ ███╗   ██╗██╗ █████╗
        \\  ╚══██╔══╝██╔════╝██╔══██╗████╗ ████║██╔══██╗████╗  ██║██║██╔══██╗
        \\     ██║   █████╗  ██████╔╝██╔████╔██║███████║██╔██╗ ██║██║███████║
        \\     ██║   ██╔══╝  ██╔══██╗██║╚██╔╝██║██╔══██║██║╚██╗██║██║██╔══██║
        \\     ██║   ███████╗██║  ██║██║ ╚═╝ ██║██║  ██║██║ ╚████║██║██║  ██║
        \\     ╚═╝   ╚══════╝╚═╝  ╚═╝╚═╝     ╚═╝╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝╚═╝  ╚═╝
        \\
        \\  GPU-accelerated multi-pane terminal emulator
        \\  Written in Zig — inspired by Ghostty
        \\  Version 0.2.0
        \\
    , .{});

    // In a full implementation, this would create a window (via X11/Wayland/GTK)
    // and enter the event loop. For now, verify the core systems initialize.
    var app = try App.init(allocator);
    defer app.deinit();

    try stdout.print("Initialized {d} pane(s) in a {d}x{d} grid.\n", .{
        app.plugins.items.len,
        app.grid.numRows(),
        app.config.grid.cols,
    });
    try stdout.print("Renderer backend: {s}\n", .{@tagName(renderer_mod.active_backend)});
    try stdout.print("Cell dimensions: {d:.1}x{d:.1}px\n", .{
        app.renderer.getCellWidth(),
        app.renderer.getCellHeight(),
    });

    try stdout.writeAll("\nTermania core initialized successfully.\n");
    try stdout.writeAll("Windowing requires a display server (X11/Wayland).\n");
}
