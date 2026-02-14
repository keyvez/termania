// Termania — GPU-accelerated multi-pane terminal emulator
// Zig rewrite following Ghostty architectural patterns.

pub const terminal = @import("terminal.zig");
pub const pty = @import("pty.zig");
pub const grid = @import("grid.zig");
pub const config = @import("config.zig");
pub const plugin = @import("plugin.zig");
pub const renderer = @import("renderer.zig");
pub const llm = @import("llm.zig");
pub const text_tap = @import("text_tap.zig");
pub const process_info = @import("process_info.zig");
pub const input = @import("input.zig");

test {
    @import("std").testing.refAllDecls(@This());
}
