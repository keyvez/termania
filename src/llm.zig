const std = @import("std");
const testing = std.testing;
const Allocator = std.mem.Allocator;

// ---------------------------------------------------------------------------
// TermaniaAction — actions the LLM (or text tap API) can perform.
// Uses Zig tagged unions (like Ghostty's Action enum patterns).
// ---------------------------------------------------------------------------

pub const TermaniaAction = union(enum) {
    send_command: struct { pane: usize, command: []const u8 },
    send_to_all: struct { command: []const u8 },
    set_title: struct { pane: usize, title: []const u8 },
    set_watermark: struct { pane: usize, watermark: []const u8 },
    clear_watermark: struct { pane: usize },
    navigate: struct { pane: usize, url: []const u8 },
    set_content: struct { pane: usize, content: []const u8 },
    spawn_pane: struct {
        pane_type: []const u8 = "terminal",
        title: ?[]const u8 = null,
        command: ?[]const u8 = null,
        cwd: ?[]const u8 = null,
        url: ?[]const u8 = null,
        content: ?[]const u8 = null,
        watermark: ?[]const u8 = null,
        row: ?usize = null,
    },
    close_pane: struct { pane: usize },
    swap_panes: struct { a: usize, b: usize },
    focus_pane: struct { pane: usize },
    message: struct { text: []const u8 },
};

/// Information about a pane (for LLM context).
pub const PaneContext = struct {
    index: usize,
    pane_type: []const u8,
    title: []const u8,
    visible_text: []const u8,
    subprocess_info: ?[]const u8 = null,
};

/// A parsed LLM response.
pub const LlmResponse = struct {
    explanation: []const u8,
    actions: []TermaniaAction,
};

/// Status of an in-flight LLM request.
pub const LlmStatus = union(enum) {
    thinking: void,
    complete: LlmResponse,
    failed: []const u8,
};

/// Format a TermaniaAction for display in the command overlay.
pub fn formatActionForDisplay(allocator: Allocator, action: TermaniaAction) ![]u8 {
    return switch (action) {
        .send_command => |a| try std.fmt.allocPrint(allocator, "  [pane {d}] $ {s}", .{ a.pane, a.command }),
        .send_to_all => |a| try std.fmt.allocPrint(allocator, "  [all] $ {s}", .{a.command}),
        .set_title => |a| try std.fmt.allocPrint(allocator, "  [pane {d}] title = \"{s}\"", .{ a.pane, a.title }),
        .set_watermark => |a| try std.fmt.allocPrint(allocator, "  [pane {d}] watermark = \"{s}\"", .{ a.pane, a.watermark }),
        .clear_watermark => |a| try std.fmt.allocPrint(allocator, "  [pane {d}] clear watermark", .{a.pane}),
        .navigate => |a| try std.fmt.allocPrint(allocator, "  [pane {d}] navigate -> {s}", .{ a.pane, a.url }),
        .set_content => |a| try std.fmt.allocPrint(allocator, "  [pane {d}] set content", .{a.pane}),
        .spawn_pane => |a| try std.fmt.allocPrint(allocator, "  spawn {s}", .{a.pane_type}),
        .close_pane => |a| try std.fmt.allocPrint(allocator, "  close pane {d}", .{a.pane}),
        .swap_panes => |a| try std.fmt.allocPrint(allocator, "  swap pane {d} <-> pane {d}", .{ a.a, a.b }),
        .focus_pane => |a| try std.fmt.allocPrint(allocator, "  focus pane {d}", .{a.pane}),
        .message => |a| try std.fmt.allocPrint(allocator, "  {s}", .{a.text}),
    };
}

/// Extract JSON from text, handling markdown code fences.
pub fn extractJson(text: []const u8) ?[]const u8 {
    const trimmed = std.mem.trim(u8, text, " \t\n\r");
    if (trimmed.len == 0) return null;

    // Direct JSON object
    if (trimmed[0] == '{') return trimmed;

    // Strip ```json ... ```
    if (std.mem.indexOf(u8, trimmed, "```json")) |start| {
        const after = trimmed[start + 7 ..];
        if (std.mem.indexOf(u8, after, "```")) |end| {
            return std.mem.trim(u8, after[0..end], " \t\n\r");
        }
    }

    // Strip ``` ... ```
    if (std.mem.indexOf(u8, trimmed, "```")) |start| {
        const after = trimmed[start + 3 ..];
        if (std.mem.indexOf(u8, after, "\n")) |nl| {
            const inner = after[nl + 1 ..];
            if (std.mem.indexOf(u8, inner, "```")) |end| {
                const candidate = std.mem.trim(u8, inner[0..end], " \t\n\r");
                if (candidate.len > 0 and candidate[0] == '{') return candidate;
            }
        }
    }

    // Find first { to last }
    if (std.mem.indexOf(u8, trimmed, "{")) |start| {
        if (std.mem.lastIndexOf(u8, trimmed, "}")) |end| {
            if (end > start) return trimmed[start .. end + 1];
        }
    }

    return null;
}

/// Truncate visible text to the last N lines.
pub fn truncateVisibleText(text: []const u8, max_lines: usize) []const u8 {
    var line_count: usize = 0;
    for (text) |ch| {
        if (ch == '\n') line_count += 1;
    }
    if (line_count <= max_lines) return text;

    // Find the start of the last max_lines lines
    var skip = line_count - max_lines;
    var pos: usize = 0;
    while (pos < text.len and skip > 0) : (pos += 1) {
        if (text[pos] == '\n') skip -= 1;
    }
    return text[pos..];
}

/// Build the system prompt for LLM requests.
pub fn buildSystemPrompt(allocator: Allocator, panes: []const PaneContext) ![]u8 {
    var buf = std.ArrayList(u8).init(allocator);
    const writer = buf.writer();

    try writer.writeAll(
        "You are an AI assistant integrated into Termania, a multi-pane terminal emulator. " ++
            "You have deep programmatic control over the entire application.\n\nCurrent panes:\n",
    );

    for (panes) |pane| {
        try writer.print("\n--- Pane {d} [{s}] (\"{s}\") ---\n", .{ pane.index, pane.pane_type, pane.title });
        if (pane.subprocess_info) |info| {
            if (info.len > 0) try writer.print("{s}\n", .{info});
        }
        const truncated = truncateVisibleText(pane.visible_text, 50);
        try writer.print("Last visible output:\n{s}\n", .{truncated});
    }

    try writer.writeAll(
        "\n\nRespond with JSON in this exact format:\n" ++
            "```json\n{\n  \"explanation\": \"Brief description\",\n  \"actions\": [\n" ++
            "    {\"type\": \"send_command\", \"pane\": 0, \"command\": \"ls -la\"}\n" ++
            "  ]\n}\n```\n\nAvailable action types: send_command, send_to_all, " ++
            "set_title, set_watermark, clear_watermark, navigate, set_content, " ++
            "spawn_pane, close_pane, swap_panes, focus_pane, message.\n" ++
            "Return ONLY the JSON, no other text.",
    );

    return buf.toOwnedSlice();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "extract json direct" {
    const input = "{\"explanation\": \"test\", \"actions\": []}";
    const result = extractJson(input);
    try testing.expect(result != null);
    try testing.expect(result.?[0] == '{');
}

test "extract json markdown fenced" {
    const input = "Here:\n```json\n{\"explanation\": \"test\", \"actions\": []}\n```\n";
    const result = extractJson(input);
    try testing.expect(result != null);
    try testing.expect(std.mem.indexOf(u8, result.?, "explanation") != null);
}

test "extract json generic fence" {
    const input = "```\n{\"explanation\": \"hi\", \"actions\": []}\n```";
    const result = extractJson(input);
    try testing.expect(result != null);
}

test "extract json embedded" {
    const input = "Sure: {\"explanation\": \"ok\", \"actions\": []} done.";
    const result = extractJson(input);
    try testing.expect(result != null);
}

test "extract json no json" {
    const input = "This is plain text with no JSON";
    try testing.expect(extractJson(input) == null);
}

test "truncate visible text short" {
    const text = "line1\nline2\nline3";
    try testing.expectEqualSlices(u8, text, truncateVisibleText(text, 10));
}

test "truncate visible text long" {
    const text = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj";
    const result = truncateVisibleText(text, 3);
    // Should contain the last 3 lines
    try testing.expect(std.mem.indexOf(u8, result, "j") != null);
}

test "format action send command" {
    const action = TermaniaAction{ .send_command = .{ .pane = 0, .command = "ls -la" } };
    const display = try formatActionForDisplay(testing.allocator, action);
    defer testing.allocator.free(display);
    try testing.expect(std.mem.indexOf(u8, display, "[pane 0]") != null);
    try testing.expect(std.mem.indexOf(u8, display, "ls -la") != null);
}

test "format action message" {
    const action = TermaniaAction{ .message = .{ .text = "Hello world" } };
    const display = try formatActionForDisplay(testing.allocator, action);
    defer testing.allocator.free(display);
    try testing.expect(std.mem.indexOf(u8, display, "Hello world") != null);
}

test "build system prompt" {
    const panes = [_]PaneContext{
        .{
            .index = 0,
            .pane_type = "terminal",
            .title = "Shell",
            .visible_text = "$ hello\n",
        },
    };
    const prompt = try buildSystemPrompt(testing.allocator, &panes);
    defer testing.allocator.free(prompt);
    try testing.expect(std.mem.indexOf(u8, prompt, "Pane 0") != null);
    try testing.expect(std.mem.indexOf(u8, prompt, "[terminal]") != null);
    try testing.expect(std.mem.indexOf(u8, prompt, "Shell") != null);
    try testing.expect(std.mem.indexOf(u8, prompt, "send_command") != null);
}
