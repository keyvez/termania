const std = @import("std");
const testing = std.testing;
const llm = @import("llm.zig");

// ---------------------------------------------------------------------------
// Text Tap Server — Unix socket API for external tool integration.
//
// Protocol: Newline-delimited JSON over a Unix domain socket.
// Follows the same architecture as the Rust version.
// ---------------------------------------------------------------------------

/// Command from a tap client.
pub const TapCommand = union(enum) {
    /// Legacy: send raw input to a target.
    send: struct {
        target: TapTarget,
        input: []const u8,
    },
    /// Full TermaniaAction.
    action: llm.TermaniaAction,
};

pub const TapTarget = union(enum) {
    pane: usize,
    all: void,
};

/// Text Tap server state.
pub const TextTapServer = struct {
    allocator: std.mem.Allocator,
    socket_path: []const u8,
    pane_count: usize = 0,
    running: bool = false,
    /// Pending commands from tap clients.
    pending_commands: std.ArrayList(TapCommand),

    pub fn init(allocator: std.mem.Allocator, socket_path: []const u8) TextTapServer {
        return .{
            .allocator = allocator,
            .socket_path = socket_path,
            .pending_commands = std.ArrayList(TapCommand).init(allocator),
        };
    }

    pub fn deinit(self: *TextTapServer) void {
        self.pending_commands.deinit();
    }

    pub fn setPaneCount(self: *TextTapServer, count: usize) void {
        self.pane_count = count;
    }

    /// Drain pending commands.
    pub fn drainCommands(self: *TextTapServer) []const TapCommand {
        const items = self.pending_commands.toOwnedSlice() catch return &.{};
        return items;
    }

    /// Start the server (spawns a listener thread).
    pub fn start(self: *TextTapServer) void {
        if (self.running) return;
        self.running = true;
        // In a full implementation, this would spawn a std.Thread
        // listening on the Unix socket. For now, the architecture is in place.
    }

    /// Stop the server.
    pub fn stop(self: *TextTapServer) void {
        self.running = false;
    }
};

// ---------------------------------------------------------------------------
// JSON utility functions (shared with text tap protocol)
// ---------------------------------------------------------------------------

/// JSON string escaping.
pub fn jsonEscapeString(allocator: std.mem.Allocator, s: []const u8) ![]u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.append('"');
    for (s) |ch| {
        switch (ch) {
            '"' => try buf.appendSlice("\\\""),
            '\\' => try buf.appendSlice("\\\\"),
            '\n' => try buf.appendSlice("\\n"),
            '\r' => try buf.appendSlice("\\r"),
            '\t' => try buf.appendSlice("\\t"),
            else => {
                if (ch < 0x20) {
                    var hex_buf: [6]u8 = undefined;
                    const hex = std.fmt.bufPrint(&hex_buf, "\\u{x:0>4}", .{ch}) catch "\\u0000";
                    try buf.appendSlice(hex);
                } else {
                    try buf.append(ch);
                }
            },
        }
    }
    try buf.append('"');
    return buf.toOwnedSlice();
}

/// Extract a number value after a given key in JSON-like text.
pub fn extractNumberAfter(s: []const u8, key: []const u8) ?usize {
    // Build search pattern: "key"
    var pattern_buf: [64]u8 = undefined;
    const pattern = std.fmt.bufPrint(&pattern_buf, "\"{s}\"", .{key}) catch return null;

    const pos = std.mem.indexOf(u8, s, pattern) orelse return null;
    const after = s[pos + pattern.len ..];

    // Find colon
    const colon_pos = std.mem.indexOf(u8, after, ":") orelse return null;
    const value_str = std.mem.trimLeft(u8, after[colon_pos + 1 ..], " \t");

    // Parse digits
    var end: usize = 0;
    while (end < value_str.len and value_str[end] >= '0' and value_str[end] <= '9') : (end += 1) {}
    if (end == 0) return null;
    return std.fmt.parseInt(usize, value_str[0..end], 10) catch null;
}

/// Extract a quoted string value after a given key in JSON-like text.
pub fn extractQuotedValue(allocator: std.mem.Allocator, s: []const u8, key: []const u8) !?[]u8 {
    var pattern_buf: [64]u8 = undefined;
    const pattern = std.fmt.bufPrint(&pattern_buf, "\"{s}\"", .{key}) catch return null;

    const pos = std.mem.indexOf(u8, s, pattern) orelse return null;
    const after = s[pos + pattern.len ..];

    const colon_pos = std.mem.indexOf(u8, after, ":") orelse return null;
    const value_str = std.mem.trimLeft(u8, after[colon_pos + 1 ..], " \t");
    if (value_str.len == 0 or value_str[0] != '"') return null;

    var result = std.ArrayList(u8).init(allocator);
    var i: usize = 1;
    while (i < value_str.len) : (i += 1) {
        if (value_str[i] == '\\' and i + 1 < value_str.len) {
            i += 1;
            switch (value_str[i]) {
                'n' => try result.append('\n'),
                'r' => try result.append('\r'),
                't' => try result.append('\t'),
                '"' => try result.append('"'),
                '\\' => try result.append('\\'),
                else => {
                    try result.append('\\');
                    try result.append(value_str[i]);
                },
            }
        } else if (value_str[i] == '"') {
            const owned = result.toOwnedSlice() catch {
                result.deinit();
                return null;
            };
            return owned;
        } else {
            try result.append(value_str[i]);
        }
    }

    result.deinit();
    return null;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "json escape simple" {
    const result = try jsonEscapeString(testing.allocator, "hello");
    defer testing.allocator.free(result);
    try testing.expectEqualSlices(u8, "\"hello\"", result);
}

test "json escape quotes" {
    const result = try jsonEscapeString(testing.allocator, "say \"hi\"");
    defer testing.allocator.free(result);
    try testing.expectEqualSlices(u8, "\"say \\\"hi\\\"\"", result);
}

test "json escape newlines" {
    const result = try jsonEscapeString(testing.allocator, "a\nb");
    defer testing.allocator.free(result);
    try testing.expectEqualSlices(u8, "\"a\\nb\"", result);
}

test "json escape backslash" {
    const result = try jsonEscapeString(testing.allocator, "a\\b");
    defer testing.allocator.free(result);
    try testing.expectEqualSlices(u8, "\"a\\\\b\"", result);
}

test "json escape tab" {
    const result = try jsonEscapeString(testing.allocator, "a\tb");
    defer testing.allocator.free(result);
    try testing.expectEqualSlices(u8, "\"a\\tb\"", result);
}

test "extract number after" {
    try testing.expectEqual(@as(?usize, 3), extractNumberAfter("{\"subscribe\": 3}", "subscribe"));
    try testing.expectEqual(@as(?usize, 0), extractNumberAfter("{\"subscribe\": 0}", "subscribe"));
    try testing.expectEqual(@as(?usize, 42), extractNumberAfter("{\"send\": 42, \"input\": \"x\"}", "send"));
}

test "extract number after missing" {
    try testing.expect(extractNumberAfter("{\"list\": true}", "subscribe") == null);
}

test "extract quoted value" {
    const result = try extractQuotedValue(testing.allocator, "{\"send\": 0, \"input\": \"hello world\"}", "input");
    defer if (result) |r| testing.allocator.free(r);
    try testing.expect(result != null);
    try testing.expectEqualSlices(u8, "hello world", result.?);
}

test "extract quoted value with escapes" {
    const result = try extractQuotedValue(testing.allocator, "{\"input\": \"line1\\nline2\"}", "input");
    defer if (result) |r| testing.allocator.free(r);
    try testing.expect(result != null);
    try testing.expectEqualSlices(u8, "line1\nline2", result.?);
}

test "extract quoted value missing" {
    const result = try extractQuotedValue(testing.allocator, "{\"send\": 0}", "input");
    try testing.expect(result == null);
}

test "text tap server creation" {
    var server = TextTapServer.init(testing.allocator, "/tmp/test_termania.sock");
    defer server.deinit();
    const cmds = server.drainCommands();
    defer testing.allocator.free(cmds);
    try testing.expectEqual(@as(usize, 0), cmds.len);
}

test "set pane count" {
    var server = TextTapServer.init(testing.allocator, "/tmp/test_termania2.sock");
    defer server.deinit();
    server.setPaneCount(5);
    try testing.expectEqual(@as(usize, 5), server.pane_count);
}
