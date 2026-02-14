const std = @import("std");
const testing = std.testing;
const Allocator = std.mem.Allocator;

/// Information about a child process.
pub const ProcessInfo = struct {
    pid: u32,
    command: []const u8,
};

/// Run a command and capture its stdout.
fn runCommand(allocator: Allocator, argv: []const []const u8) ![]u8 {
    var child = std.process.Child.init(argv, allocator);
    child.stdout_behavior = .Pipe;
    child.stderr_behavior = .Ignore;
    try child.spawn();

    const stdout = child.stdout.?.reader();
    const output = try stdout.readAllAlloc(allocator, 65536);

    _ = try child.wait();
    return output;
}

/// Get child processes of a given PID using `pgrep -P`.
pub fn getChildProcesses(allocator: Allocator, parent_pid: u32) ![]ProcessInfo {
    var pid_buf: [32]u8 = undefined;
    const pid_str = try std.fmt.bufPrint(&pid_buf, "{d}", .{parent_pid});

    const output = runCommand(allocator, &.{ "pgrep", "-P", pid_str }) catch return &.{};
    defer allocator.free(output);

    var result = std.ArrayList(ProcessInfo).init(allocator);

    var lines = std.mem.splitScalar(u8, output, '\n');
    while (lines.next()) |line| {
        const trimmed = std.mem.trim(u8, line, " \t\r");
        if (trimmed.len == 0) continue;
        const pid = std.fmt.parseInt(u32, trimmed, 10) catch continue;
        const cmd = getProcessCommand(allocator, pid) catch
            try std.fmt.allocPrint(allocator, "(pid {d})", .{pid});
        try result.append(.{ .pid = pid, .command = cmd });
    }

    return result.toOwnedSlice();
}

/// Get the command name for a PID using `ps`.
fn getProcessCommand(allocator: Allocator, pid: u32) ![]u8 {
    var pid_buf: [32]u8 = undefined;
    const pid_str = try std.fmt.bufPrint(&pid_buf, "{d}", .{pid});

    const output = try runCommand(allocator, &.{ "ps", "-p", pid_str, "-o", "comm=" });
    const trimmed = std.mem.trim(u8, output, " \t\n\r");
    if (trimmed.len == 0) {
        allocator.free(output);
        return error.NoCommand;
    }
    const result = try allocator.dupe(u8, trimmed);
    allocator.free(output);
    return result;
}

/// Build a human-readable subprocess context string for a shell PID.
pub fn buildProcessContext(allocator: Allocator, shell_pid: u32) ![]u8 {
    const children = try getChildProcesses(allocator, shell_pid);
    defer {
        for (children) |child| allocator.free(child.command);
        allocator.free(children);
    }

    if (children.len == 0) return try allocator.dupe(u8, "");

    var buf = std.ArrayList(u8).init(allocator);
    const writer = buf.writer();
    try writer.writeAll("Child processes:\n");
    for (children) |child| {
        try writer.print("  pid={d} cmd={s}\n", .{ child.pid, child.command });
    }
    return buf.toOwnedSlice();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "build process context nonexistent pid" {
    const result = try buildProcessContext(testing.allocator, 999999999);
    defer testing.allocator.free(result);
    try testing.expectEqual(@as(usize, 0), result.len);
}
