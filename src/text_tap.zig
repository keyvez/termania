const std = @import("std");
const testing = std.testing;
const posix = std.posix;
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

/// A connected tap client.
pub const ClientConnection = struct {
    fd: posix.socket_t,
    subscribed: bool = false,
    read_buf: [4096]u8 = undefined,
    read_pos: usize = 0,
};

/// Text Tap server state.
pub const TextTapServer = struct {
    allocator: std.mem.Allocator,
    socket_path: []const u8,
    pane_count: usize = 0,
    running: bool = false,
    /// Listener socket file descriptor.
    listener_fd: ?posix.socket_t = null,
    /// Connected clients.
    clients: std.ArrayList(ClientConnection),
    /// Pending commands from tap clients.
    pending_commands: std.ArrayList(TapCommand),

    pub fn init(allocator: std.mem.Allocator, socket_path: []const u8) TextTapServer {
        return .{
            .allocator = allocator,
            .socket_path = socket_path,
            .pending_commands = std.ArrayList(TapCommand).init(allocator),
            .clients = std.ArrayList(ClientConnection).init(allocator),
        };
    }

    pub fn deinit(self: *TextTapServer) void {
        if (self.running) self.stop();
        self.pending_commands.deinit();
        self.clients.deinit();
    }

    pub fn setPaneCount(self: *TextTapServer, count: usize) void {
        self.pane_count = count;
    }

    /// Drain pending commands.
    pub fn drainCommands(self: *TextTapServer) []const TapCommand {
        const items = self.pending_commands.toOwnedSlice() catch return &.{};
        return items;
    }

    /// Start the server: create Unix socket, bind, listen, set non-blocking.
    pub fn start(self: *TextTapServer) !void {
        if (self.running) return;

        // Remove any stale socket file.
        removeSocketFile(self.socket_path);

        // Create Unix domain socket.
        const fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM | posix.SOCK.NONBLOCK | posix.SOCK.CLOEXEC, 0);
        errdefer posix.close(fd);

        // Bind to the socket path.
        var addr = std.net.Address.initUnix(self.socket_path) catch |err| {
            posix.close(fd);
            return err;
        };
        posix.bind(fd, &addr.any, addr.getOsSockLen()) catch |err| {
            posix.close(fd);
            return err;
        };

        // Listen with a small backlog.
        posix.listen(fd, 8) catch |err| {
            posix.close(fd);
            return err;
        };

        self.listener_fd = fd;
        self.running = true;
    }

    /// Stop the server: close all client connections, close listener, remove socket file.
    pub fn stop(self: *TextTapServer) void {
        // Close all client connections.
        for (self.clients.items) |client| {
            posix.close(client.fd);
        }
        self.clients.clearRetainingCapacity();

        // Close listener socket.
        if (self.listener_fd) |fd| {
            posix.close(fd);
            self.listener_fd = null;
        }

        // Remove socket file.
        removeSocketFile(self.socket_path);

        self.running = false;
    }

    /// Non-blocking poll: accept new connections and read from existing clients.
    /// Should be called from the main event loop each tick.
    pub fn poll(self: *TextTapServer) void {
        if (!self.running) return;

        // Accept new connections (non-blocking).
        self.acceptNewClients();

        // Read from existing clients (iterate in reverse for safe removal).
        self.readFromClients();
    }

    /// Accept all pending connections on the listener socket.
    fn acceptNewClients(self: *TextTapServer) void {
        const listener = self.listener_fd orelse return;

        while (true) {
            const result = posix.accept(listener, null, null, posix.SOCK.NONBLOCK | posix.SOCK.CLOEXEC) catch |err| {
                switch (err) {
                    error.WouldBlock => return, // No more pending connections.
                    else => return, // Unexpected error, stop accepting.
                }
            };
            self.clients.append(.{ .fd = result }) catch {
                posix.close(result);
            };
        }
    }

    /// Read available data from all connected clients and process commands.
    fn readFromClients(self: *TextTapServer) void {
        var i: usize = self.clients.items.len;
        while (i > 0) {
            i -= 1;
            const alive = self.readFromClient(i);
            if (!alive) {
                const client = self.clients.orderedRemove(i);
                posix.close(client.fd);
            }
        }
    }

    /// Read from a single client. Returns false if the client should be removed.
    fn readFromClient(self: *TextTapServer, idx: usize) bool {
        const client = &self.clients.items[idx];
        const remaining = client.read_buf[client.read_pos..];
        if (remaining.len == 0) {
            // Buffer is full with no newline — discard and reset.
            client.read_pos = 0;
            return true;
        }

        const n = posix.read(client.fd, remaining) catch |err| {
            switch (err) {
                error.WouldBlock => return true, // No data available.
                else => return false, // Error, disconnect client.
            }
        };

        if (n == 0) return false; // Client disconnected (EOF).

        client.read_pos += n;

        // Process complete lines (newline-delimited JSON).
        self.processClientBuffer(idx);

        return true;
    }

    /// Process newline-delimited messages in a client's buffer.
    fn processClientBuffer(self: *TextTapServer, idx: usize) void {
        const client = &self.clients.items[idx];
        var offset: usize = 0;

        while (offset < client.read_pos) {
            // Find the next newline.
            const slice = client.read_buf[offset..client.read_pos];
            const nl = std.mem.indexOf(u8, slice, "\n") orelse break;
            const line = slice[0..nl];

            if (line.len > 0) {
                self.handleClientMessage(idx, line);
            }

            offset += nl + 1;
        }

        // Compact: move unconsumed data to the beginning.
        if (offset > 0) {
            const leftover = client.read_pos - offset;
            if (leftover > 0) {
                std.mem.copyForwards(u8, &client.read_buf, client.read_buf[offset..client.read_pos]);
            }
            client.read_pos = leftover;
        }
    }

    /// Handle a single JSON message from a client.
    fn handleClientMessage(self: *TextTapServer, idx: usize, msg: []const u8) void {
        const trimmed = std.mem.trim(u8, msg, " \t\r");
        if (trimmed.len == 0) return;

        // Extract the "type" field.
        const msg_type = extractQuotedValueStatic(trimmed, "type") orelse return;

        if (std.mem.eql(u8, msg_type, "subscribe")) {
            self.clients.items[idx].subscribed = true;
            self.respond(idx, "{\"status\": \"subscribed\"}\n");
        } else if (std.mem.eql(u8, msg_type, "unsubscribe")) {
            self.clients.items[idx].subscribed = false;
            self.respond(idx, "{\"status\": \"unsubscribed\"}\n");
        } else if (std.mem.eql(u8, msg_type, "list_panes")) {
            var buf: [128]u8 = undefined;
            const response = std.fmt.bufPrint(&buf, "{{\"pane_count\": {d}}}\n", .{self.pane_count}) catch return;
            self.respond(idx, response);
        } else if (std.mem.eql(u8, msg_type, "read_pane")) {
            const pane = extractNumberAfter(trimmed, "pane") orelse {
                self.respond(idx, "{\"error\": \"missing pane\"}\n");
                return;
            };
            // Enqueue a read_pane as an action — the main loop will
            // gather visible text and call broadcast. For now, acknowledge.
            var buf: [128]u8 = undefined;
            const response = std.fmt.bufPrint(&buf, "{{\"status\": \"read_pane_queued\", \"pane\": {d}}}\n", .{pane}) catch return;
            self.respond(idx, response);
        } else if (std.mem.eql(u8, msg_type, "send")) {
            const pane = extractNumberAfter(trimmed, "pane") orelse return;
            const text = extractQuotedValue(self.allocator, trimmed, "text") catch return orelse return;
            self.pending_commands.append(.{
                .send = .{
                    .target = .{ .pane = pane },
                    .input = text,
                },
            }) catch {
                self.allocator.free(text);
                return;
            };
            self.respond(idx, "{\"status\": \"queued\"}\n");
        } else if (std.mem.eql(u8, msg_type, "send_all")) {
            const text = extractQuotedValue(self.allocator, trimmed, "text") catch return orelse return;
            self.pending_commands.append(.{
                .send = .{
                    .target = .{ .all = {} },
                    .input = text,
                },
            }) catch {
                self.allocator.free(text);
                return;
            };
            self.respond(idx, "{\"status\": \"queued\"}\n");
        } else if (std.mem.eql(u8, msg_type, "action")) {
            self.handleActionMessage(idx, trimmed);
        } else {
            self.respond(idx, "{\"error\": \"unknown command\"}\n");
        }
    }

    /// Handle an "action" type message by parsing the sub-action.
    fn handleActionMessage(self: *TextTapServer, idx: usize, msg: []const u8) void {
        const action_type = extractQuotedValueStatic(msg, "action") orelse {
            self.respond(idx, "{\"error\": \"missing action field\"}\n");
            return;
        };

        if (std.mem.eql(u8, action_type, "send_command")) {
            const pane = extractNumberAfter(msg, "pane") orelse return;
            const command = extractQuotedValue(self.allocator, msg, "command") catch return orelse return;
            self.pending_commands.append(.{
                .action = .{ .send_command = .{ .pane = pane, .command = command } },
            }) catch {
                self.allocator.free(command);
                return;
            };
            self.respond(idx, "{\"status\": \"queued\"}\n");
        } else if (std.mem.eql(u8, action_type, "send_to_all")) {
            const command = extractQuotedValue(self.allocator, msg, "command") catch return orelse return;
            self.pending_commands.append(.{
                .action = .{ .send_to_all = .{ .command = command } },
            }) catch {
                self.allocator.free(command);
                return;
            };
            self.respond(idx, "{\"status\": \"queued\"}\n");
        } else if (std.mem.eql(u8, action_type, "message")) {
            const text = extractQuotedValue(self.allocator, msg, "text") catch return orelse return;
            self.pending_commands.append(.{
                .action = .{ .message = .{ .text = text } },
            }) catch {
                self.allocator.free(text);
                return;
            };
            self.respond(idx, "{\"status\": \"queued\"}\n");
        } else {
            self.respond(idx, "{\"error\": \"unknown action\"}\n");
        }
    }

    /// Write a JSON response to a specific client.
    pub fn respond(self: *TextTapServer, idx: usize, data: []const u8) void {
        if (idx >= self.clients.items.len) return;
        const fd = self.clients.items[idx].fd;
        _ = posix.write(fd, data) catch {};
    }

    /// Broadcast a message to all subscribed clients.
    pub fn broadcast(self: *TextTapServer, data: []const u8) void {
        for (self.clients.items) |client| {
            if (client.subscribed) {
                _ = posix.write(client.fd, data) catch {};
            }
        }
    }

    /// Broadcast pane content to all subscribed clients as JSON.
    pub fn broadcastPaneContent(self: *TextTapServer, pane: usize, content: []const u8) void {
        const escaped = jsonEscapeString(self.allocator, content) catch return;
        defer self.allocator.free(escaped);

        // Build: {"type": "pane_output", "pane": N, "content": "..."}\n
        const msg = std.fmt.allocPrint(
            self.allocator,
            "{{\"type\": \"pane_output\", \"pane\": {d}, \"content\": {s}}}\n",
            .{ pane, escaped },
        ) catch return;
        defer self.allocator.free(msg);

        self.broadcast(msg);
    }

    /// Return whether any clients are subscribed.
    pub fn hasSubscribers(self: *TextTapServer) bool {
        for (self.clients.items) |client| {
            if (client.subscribed) return true;
        }
        return false;
    }

    /// Return the number of connected clients.
    pub fn clientCount(self: *TextTapServer) usize {
        return self.clients.items.len;
    }
};

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Remove a Unix socket file if it exists.
fn removeSocketFile(path: []const u8) void {
    std.fs.cwd().deleteFile(path) catch {};
}

/// Extract a quoted value without allocating (returns a slice into the input).
/// Only works for simple values without escape sequences.
fn extractQuotedValueStatic(s: []const u8, key: []const u8) ?[]const u8 {
    var pattern_buf: [64]u8 = undefined;
    const pattern = std.fmt.bufPrint(&pattern_buf, "\"{s}\"", .{key}) catch return null;

    const pos = std.mem.indexOf(u8, s, pattern) orelse return null;
    const after = s[pos + pattern.len ..];

    const colon_pos = std.mem.indexOf(u8, after, ":") orelse return null;
    const value_str = std.mem.trimLeft(u8, after[colon_pos + 1 ..], " \t");
    if (value_str.len == 0 or value_str[0] != '"') return null;

    // Find closing quote (no escape handling — for simple type/action values).
    var i: usize = 1;
    while (i < value_str.len) : (i += 1) {
        if (value_str[i] == '"') {
            return value_str[1..i];
        }
    }
    return null;
}

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

test "server start and stop" {
    const path = "/tmp/test_termania_start_stop.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();
    try testing.expect(server.running);
    try testing.expect(server.listener_fd != null);

    // Socket file should exist.
    const stat = std.fs.cwd().statFile(path);
    try testing.expect(stat != error.FileNotFound);

    server.stop();
    try testing.expect(!server.running);
    try testing.expect(server.listener_fd == null);
}

test "server double start is no-op" {
    const path = "/tmp/test_termania_double.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();
    const fd1 = server.listener_fd.?;
    try server.start(); // Should be a no-op.
    try testing.expectEqual(fd1, server.listener_fd.?);

    server.stop();
}

test "server poll with no clients" {
    const path = "/tmp/test_termania_poll_empty.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();
    // Polling with no clients should not crash.
    server.poll();
    try testing.expectEqual(@as(usize, 0), server.clientCount());
    server.stop();
}

test "server accept client and read subscribe" {
    const path = "/tmp/test_termania_accept.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();

    // Connect a client.
    var addr = try std.net.Address.initUnix(path);
    const client_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(client_fd);
    try posix.connect(client_fd, &addr.any, addr.getOsSockLen());

    // Poll to accept.
    server.poll();
    try testing.expectEqual(@as(usize, 1), server.clientCount());

    // Send a subscribe command.
    _ = try posix.write(client_fd, "{\"type\": \"subscribe\"}\n");

    // Give a tiny moment for data to arrive, then poll.
    std.time.sleep(1_000_000); // 1ms
    server.poll();

    // Client should be subscribed.
    try testing.expect(server.clients.items[0].subscribed);
    try testing.expect(server.hasSubscribers());

    server.stop();
}

test "server list_panes command" {
    const path = "/tmp/test_termania_list.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();
    server.setPaneCount(3);

    try server.start();

    var addr = try std.net.Address.initUnix(path);
    const client_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(client_fd);
    try posix.connect(client_fd, &addr.any, addr.getOsSockLen());

    server.poll(); // Accept.

    _ = try posix.write(client_fd, "{\"type\": \"list_panes\"}\n");
    std.time.sleep(1_000_000);
    server.poll(); // Read + respond.

    // Read the response.
    var buf: [256]u8 = undefined;
    std.time.sleep(1_000_000);
    const n = posix.read(client_fd, &buf) catch 0;
    if (n > 0) {
        const response = buf[0..n];
        try testing.expect(std.mem.indexOf(u8, response, "\"pane_count\": 3") != null);
    }

    server.stop();
}

test "server send command queued" {
    const path = "/tmp/test_termania_send.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();

    var addr = try std.net.Address.initUnix(path);
    const client_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(client_fd);
    try posix.connect(client_fd, &addr.any, addr.getOsSockLen());

    server.poll(); // Accept.

    _ = try posix.write(client_fd, "{\"type\": \"send\", \"pane\": 0, \"text\": \"ls -la\"}\n");
    std.time.sleep(1_000_000);
    server.poll(); // Read + process.

    const cmds = server.drainCommands();
    defer {
        for (cmds) |cmd| {
            switch (cmd) {
                .send => |send_cmd| testing.allocator.free(send_cmd.input),
                else => {},
            }
        }
        testing.allocator.free(cmds);
    }
    try testing.expectEqual(@as(usize, 1), cmds.len);

    switch (cmds[0]) {
        .send => |send_cmd| {
            try testing.expectEqualSlices(u8, "ls -la", send_cmd.input);
            switch (send_cmd.target) {
                .pane => |p| try testing.expectEqual(@as(usize, 0), p),
                else => return error.TestUnexpectedResult,
            }
        },
        else => return error.TestUnexpectedResult,
    }

    server.stop();
}

test "server unsubscribe" {
    const path = "/tmp/test_termania_unsub.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();

    var addr = try std.net.Address.initUnix(path);
    const client_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(client_fd);
    try posix.connect(client_fd, &addr.any, addr.getOsSockLen());

    server.poll();

    // Subscribe.
    _ = try posix.write(client_fd, "{\"type\": \"subscribe\"}\n");
    std.time.sleep(1_000_000);
    server.poll();
    try testing.expect(server.clients.items[0].subscribed);

    // Drain the subscribe response.
    var drain_buf: [256]u8 = undefined;
    _ = posix.read(client_fd, &drain_buf) catch 0;

    // Unsubscribe.
    _ = try posix.write(client_fd, "{\"type\": \"unsubscribe\"}\n");
    std.time.sleep(1_000_000);
    server.poll();
    try testing.expect(!server.clients.items[0].subscribed);
    try testing.expect(!server.hasSubscribers());

    server.stop();
}

test "server broadcast to subscribed clients" {
    const path = "/tmp/test_termania_bcast.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();

    var addr = try std.net.Address.initUnix(path);

    // Connect client 1 (subscribed).
    const c1_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(c1_fd);
    try posix.connect(c1_fd, &addr.any, addr.getOsSockLen());

    // Connect client 2 (not subscribed).
    const c2_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(c2_fd);
    try posix.connect(c2_fd, &addr.any, addr.getOsSockLen());

    server.poll(); // Accept both.
    try testing.expectEqual(@as(usize, 2), server.clientCount());

    // Subscribe client 1.
    _ = try posix.write(c1_fd, "{\"type\": \"subscribe\"}\n");
    std.time.sleep(1_000_000);
    server.poll();

    // Drain subscribe ack.
    var drain_buf: [256]u8 = undefined;
    _ = posix.read(c1_fd, &drain_buf) catch 0;

    // Broadcast.
    server.broadcast("{\"test\": \"data\"}\n");

    std.time.sleep(1_000_000);

    // Client 1 should have received it.
    var buf1: [256]u8 = undefined;
    const n1 = posix.read(c1_fd, &buf1) catch 0;
    try testing.expect(n1 > 0);
    try testing.expect(std.mem.indexOf(u8, buf1[0..n1], "\"test\"") != null);

    // Client 2 (not subscribed) should have nothing.
    // Set c2 to non-blocking to check.
    const flags = posix.fcntl(c2_fd, posix.F.GETFL, 0) catch 0;
    _ = posix.fcntl(c2_fd, posix.F.SETFL, flags | @as(usize, @bitCast(@as(isize, posix.SOCK.NONBLOCK)))) catch {};
    var buf2: [256]u8 = undefined;
    const n2 = posix.read(c2_fd, &buf2) catch |err| blk: {
        if (err == error.WouldBlock) break :blk @as(usize, 0);
        break :blk @as(usize, 0);
    };
    try testing.expectEqual(@as(usize, 0), n2);

    server.stop();
}

test "server client disconnect detection" {
    const path = "/tmp/test_termania_disc.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();

    var addr = try std.net.Address.initUnix(path);
    const client_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    try posix.connect(client_fd, &addr.any, addr.getOsSockLen());

    server.poll();
    try testing.expectEqual(@as(usize, 1), server.clientCount());

    // Close the client.
    posix.close(client_fd);
    std.time.sleep(1_000_000);

    // Poll should detect disconnect and remove.
    server.poll();
    try testing.expectEqual(@as(usize, 0), server.clientCount());

    server.stop();
}

test "server send_all command queued" {
    const path = "/tmp/test_termania_sendall.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();

    var addr = try std.net.Address.initUnix(path);
    const client_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(client_fd);
    try posix.connect(client_fd, &addr.any, addr.getOsSockLen());

    server.poll();

    _ = try posix.write(client_fd, "{\"type\": \"send_all\", \"text\": \"hello\"}\n");
    std.time.sleep(1_000_000);
    server.poll();

    const cmds = server.drainCommands();
    defer {
        for (cmds) |cmd| {
            switch (cmd) {
                .send => |send_cmd| testing.allocator.free(send_cmd.input),
                else => {},
            }
        }
        testing.allocator.free(cmds);
    }
    try testing.expectEqual(@as(usize, 1), cmds.len);

    switch (cmds[0]) {
        .send => |send_cmd| {
            try testing.expectEqualSlices(u8, "hello", send_cmd.input);
            switch (send_cmd.target) {
                .all => {},
                else => return error.TestUnexpectedResult,
            }
        },
        else => return error.TestUnexpectedResult,
    }

    server.stop();
}

test "extract quoted value static" {
    const result = extractQuotedValueStatic("{\"type\": \"subscribe\"}", "type");
    try testing.expect(result != null);
    try testing.expectEqualSlices(u8, "subscribe", result.?);
}

test "extract quoted value static missing" {
    const result = extractQuotedValueStatic("{\"type\": \"subscribe\"}", "action");
    try testing.expect(result == null);
}

test "broadcastPaneContent formats correctly" {
    const path = "/tmp/test_termania_bpc.sock";
    var server = TextTapServer.init(testing.allocator, path);
    defer server.deinit();

    try server.start();

    var addr = try std.net.Address.initUnix(path);
    const client_fd = try posix.socket(posix.AF.UNIX, posix.SOCK.STREAM, 0);
    defer posix.close(client_fd);
    try posix.connect(client_fd, &addr.any, addr.getOsSockLen());

    server.poll();

    // Subscribe.
    _ = try posix.write(client_fd, "{\"type\": \"subscribe\"}\n");
    std.time.sleep(1_000_000);
    server.poll();

    // Drain subscribe response.
    var drain_buf: [256]u8 = undefined;
    _ = posix.read(client_fd, &drain_buf) catch 0;

    // Broadcast pane content.
    server.broadcastPaneContent(0, "hello world");

    std.time.sleep(1_000_000);

    var buf: [512]u8 = undefined;
    const n = posix.read(client_fd, &buf) catch 0;
    if (n > 0) {
        const response = buf[0..n];
        try testing.expect(std.mem.indexOf(u8, response, "pane_output") != null);
        try testing.expect(std.mem.indexOf(u8, response, "hello world") != null);
    }

    server.stop();
}
