const std = @import("std");
const testing = std.testing;
const Allocator = std.mem.Allocator;
const config = @import("config.zig");
const LlmConfig = config.LlmConfig;

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
    replace_pane: struct {
        pane: usize,
        pane_type: []const u8 = "terminal",
        title: ?[]const u8 = null,
        command: ?[]const u8 = null,
        cwd: ?[]const u8 = null,
        url: ?[]const u8 = null,
        content: ?[]const u8 = null,
    },
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
        .replace_pane => |a| try std.fmt.allocPrint(allocator, "  replace pane {d} with {s}", .{ a.pane, a.pane_type }),
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
// LlmClient — HTTP client for LLM API integration.
// ---------------------------------------------------------------------------

/// Determines which API format to use for requests / responses.
pub const ApiProvider = enum {
    anthropic,
    openai,

    pub fn fromString(s: []const u8) ApiProvider {
        if (std.mem.eql(u8, s, "anthropic") or std.mem.eql(u8, s, "claude")) return .anthropic;
        return .openai;
    }
};

/// Status of the LlmClient.
pub const ClientStatus = enum {
    idle,
    waiting,
    err,
};

pub const LlmClient = struct {
    cfg: *const LlmConfig,
    status: ClientStatus,
    last_response: ?LlmResponse,
    pending_request: ?[]const u8,
    allocator: Allocator,
    /// Arena for response data that persists until the next request or deinit.
    response_arena: std.heap.ArenaAllocator,

    pub fn init(allocator: Allocator, llm_config: *const LlmConfig) LlmClient {
        return .{
            .cfg = llm_config,
            .status = .idle,
            .last_response = null,
            .pending_request = null,
            .allocator = allocator,
            .response_arena = std.heap.ArenaAllocator.init(allocator),
        };
    }

    pub fn deinit(self: *LlmClient) void {
        self.response_arena.deinit();
        if (self.pending_request) |p| {
            self.allocator.free(p);
            self.pending_request = null;
        }
    }

    // -----------------------------------------------------------------------
    // buildRequestBody — construct JSON payload for the LLM API.
    // -----------------------------------------------------------------------

    pub fn buildRequestBody(
        self: *LlmClient,
        allocator: Allocator,
        user_prompt: []const u8,
        system_prompt: []const u8,
    ) ![]u8 {
        const provider = ApiProvider.fromString(self.cfg.provider);
        const model = self.cfg.model orelse switch (provider) {
            .anthropic => "claude-sonnet-4-20250514",
            .openai => "gpt-4o",
        };
        const max_tokens = self.cfg.max_tokens;

        var buf = std.ArrayList(u8).init(allocator);
        errdefer buf.deinit();
        const writer = buf.writer();

        switch (provider) {
            .anthropic => {
                // Anthropic format:
                // {"model":"...","max_tokens":N,"system":"...","messages":[{"role":"user","content":"..."}]}
                try writer.writeAll("{\"model\":\"");
                try writeJsonEscaped(writer, model);
                try writer.print("\",\"max_tokens\":{d},\"system\":\"", .{max_tokens});
                try writeJsonEscaped(writer, system_prompt);
                try writer.writeAll("\",\"messages\":[{\"role\":\"user\",\"content\":\"");
                try writeJsonEscaped(writer, user_prompt);
                try writer.writeAll("\"}]}");
            },
            .openai => {
                // OpenAI format:
                // {"model":"...","max_tokens":N,"messages":[{"role":"system","content":"..."},{"role":"user","content":"..."}]}
                try writer.writeAll("{\"model\":\"");
                try writeJsonEscaped(writer, model);
                try writer.print("\",\"max_tokens\":{d},\"messages\":[", .{max_tokens});
                try writer.writeAll("{\"role\":\"system\",\"content\":\"");
                try writeJsonEscaped(writer, system_prompt);
                try writer.writeAll("\"},{\"role\":\"user\",\"content\":\"");
                try writeJsonEscaped(writer, user_prompt);
                try writer.writeAll("\"}]}");
            },
        }

        return buf.toOwnedSlice();
    }

    // -----------------------------------------------------------------------
    // parseResponseBody — extract text content from the LLM API response.
    // -----------------------------------------------------------------------

    pub fn parseResponseBody(self: *LlmClient, allocator: Allocator, body: []const u8) ![]const u8 {
        const provider = ApiProvider.fromString(self.cfg.provider);

        const parsed = std.json.parseFromSlice(std.json.Value, allocator, body, .{
            .ignore_unknown_fields = true,
        }) catch return error.InvalidResponse;
        defer parsed.deinit();

        const root = parsed.value;
        if (root != .object) return error.InvalidResponse;

        switch (provider) {
            .anthropic => {
                // Anthropic: {"content": [{"type": "text", "text": "..."}]}
                const content_array = root.object.get("content") orelse return error.InvalidResponse;
                if (content_array != .array) return error.InvalidResponse;
                if (content_array.array.items.len == 0) return error.InvalidResponse;

                const first = content_array.array.items[0];
                if (first != .object) return error.InvalidResponse;
                const text_val = first.object.get("text") orelse return error.InvalidResponse;
                if (text_val != .string) return error.InvalidResponse;
                return try allocator.dupe(u8, text_val.string);
            },
            .openai => {
                // OpenAI: {"choices": [{"message": {"content": "..."}}]}
                const choices = root.object.get("choices") orelse return error.InvalidResponse;
                if (choices != .array) return error.InvalidResponse;
                if (choices.array.items.len == 0) return error.InvalidResponse;

                const first = choices.array.items[0];
                if (first != .object) return error.InvalidResponse;
                const message = first.object.get("message") orelse return error.InvalidResponse;
                if (message != .object) return error.InvalidResponse;
                const content_val = message.object.get("content") orelse return error.InvalidResponse;
                if (content_val != .string) return error.InvalidResponse;
                return try allocator.dupe(u8, content_val.string);
            },
        }
    }

    // -----------------------------------------------------------------------
    // sendRequest — perform the HTTP request to the LLM API.
    // -----------------------------------------------------------------------

    pub fn sendRequest(self: *LlmClient, user_prompt: []const u8, panes: []const PaneContext) !void {
        const arena_alloc = self.response_arena.allocator();

        // Build the system prompt.
        const system_prompt = if (self.cfg.system_prompt) |sp|
            try arena_alloc.dupe(u8, sp)
        else
            try buildSystemPrompt(arena_alloc, panes);

        // Build the request body.
        const body = try self.buildRequestBody(arena_alloc, user_prompt, system_prompt);

        const provider = ApiProvider.fromString(self.cfg.provider);

        // Determine the URL.
        const url = self.cfg.base_url orelse switch (provider) {
            .anthropic => "https://api.anthropic.com/v1/messages",
            .openai => "https://api.openai.com/v1/chat/completions",
        };

        // Build auth header.
        const api_key = self.cfg.api_key orelse return error.MissingApiKey;

        // Set up extra headers based on provider.
        var extra_headers_buf: [3]std.http.Header = undefined;
        var n_extra: usize = 0;

        extra_headers_buf[n_extra] = .{ .name = "content-type", .value = "application/json" };
        n_extra += 1;

        var auth_value_buf: [512]u8 = undefined;
        switch (provider) {
            .anthropic => {
                extra_headers_buf[n_extra] = .{ .name = "x-api-key", .value = api_key };
                n_extra += 1;
                extra_headers_buf[n_extra] = .{ .name = "anthropic-version", .value = "2023-06-01" };
                n_extra += 1;
            },
            .openai => {
                const auth_val = std.fmt.bufPrint(&auth_value_buf, "Bearer {s}", .{api_key}) catch return error.ApiKeyTooLong;
                extra_headers_buf[n_extra] = .{ .name = "authorization", .value = auth_val };
                n_extra += 1;
            },
        }

        // Perform the HTTP request using std.http.Client.fetch.
        var client = std.http.Client{ .allocator = self.allocator };
        defer client.deinit();

        var response_body = std.ArrayList(u8).init(arena_alloc);

        const result = client.fetch(.{
            .location = .{ .url = url },
            .method = .POST,
            .payload = body,
            .extra_headers = extra_headers_buf[0..n_extra],
            .response_storage = .{ .dynamic = &response_body },
            .max_append_size = 1024 * 1024,
        }) catch |e| {
            self.status = .err;
            self.last_response = null;
            return e;
        };

        if (result.status != .ok) {
            self.status = .err;
            self.last_response = null;
            return error.HttpError;
        }

        // Parse the API response to extract the text content.
        const text_content = self.parseResponseBody(arena_alloc, response_body.items) catch {
            self.status = .err;
            self.last_response = null;
            return error.InvalidResponse;
        };

        // Parse the text content into actions.
        const llm_resp = self.parseAction(arena_alloc, text_content) catch {
            // If action parsing fails, return the raw text as a message action.
            var actions = try arena_alloc.alloc(TermaniaAction, 1);
            actions[0] = .{ .message = .{ .text = text_content } };
            self.last_response = .{
                .explanation = text_content,
                .actions = actions,
            };
            self.status = .idle;
            return;
        };

        self.last_response = llm_resp;
        self.status = .idle;
    }

    // -----------------------------------------------------------------------
    // send — queue a request for later execution via poll().
    // -----------------------------------------------------------------------

    pub fn send(self: *LlmClient, prompt: []const u8) !void {
        // Free any previously pending request.
        if (self.pending_request) |old| {
            self.allocator.free(old);
        }
        self.pending_request = try self.allocator.dupe(u8, prompt);
        self.status = .waiting;
    }

    // -----------------------------------------------------------------------
    // poll — check for a pending request and execute it synchronously.
    // -----------------------------------------------------------------------

    pub fn poll(self: *LlmClient, panes: []const PaneContext) !bool {
        if (self.status != .waiting) return false;
        const prompt = self.pending_request orelse return false;

        // Reset arena for new response data.
        _ = self.response_arena.reset(.retain_capacity);
        self.last_response = null;

        defer {
            self.allocator.free(prompt);
            self.pending_request = null;
        }

        self.sendRequest(prompt, panes) catch {
            self.status = .err;
            return false;
        };

        return true;
    }

    // -----------------------------------------------------------------------
    // parseAction — parse LLM response text into a TermaniaAction list.
    // -----------------------------------------------------------------------

    pub fn parseAction(self: *LlmClient, allocator: Allocator, text: []const u8) !LlmResponse {
        _ = self;

        const json_text = extractJson(text) orelse return error.NoJsonFound;

        const parsed = std.json.parseFromSlice(std.json.Value, allocator, json_text, .{
            .ignore_unknown_fields = true,
        }) catch return error.InvalidJson;
        defer parsed.deinit();

        const root = parsed.value;
        if (root != .object) return error.InvalidJson;

        // Extract explanation.
        const explanation = blk: {
            const val = root.object.get("explanation") orelse break :blk try allocator.dupe(u8, "");
            if (val != .string) break :blk try allocator.dupe(u8, "");
            break :blk try allocator.dupe(u8, val.string);
        };

        // Extract actions array.
        const actions_val = root.object.get("actions") orelse return error.NoActionsField;
        if (actions_val != .array) return error.InvalidActions;

        var actions = std.ArrayList(TermaniaAction).init(allocator);
        errdefer actions.deinit();

        for (actions_val.array.items) |item| {
            if (item != .object) continue;

            const action_type_val = item.object.get("type") orelse continue;
            if (action_type_val != .string) continue;
            const action_type = action_type_val.string;

            const action = try parseOneAction(allocator, action_type, item.object);
            if (action) |a| try actions.append(a);
        }

        return .{
            .explanation = explanation,
            .actions = try actions.toOwnedSlice(),
        };
    }
};

// ---------------------------------------------------------------------------
// Helper: escape a string for JSON output.
// ---------------------------------------------------------------------------

fn writeJsonEscaped(writer: anytype, s: []const u8) !void {
    for (s) |ch| {
        switch (ch) {
            '"' => try writer.writeAll("\\\""),
            '\\' => try writer.writeAll("\\\\"),
            '\n' => try writer.writeAll("\\n"),
            '\r' => try writer.writeAll("\\r"),
            '\t' => try writer.writeAll("\\t"),
            else => {
                if (ch < 0x20) {
                    try writer.print("\\u{x:0>4}", .{ch});
                } else {
                    try writer.writeByte(ch);
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: parse a single action from a JSON object.
// ---------------------------------------------------------------------------

fn getStr(obj: std.json.ObjectMap, key: []const u8) ?[]const u8 {
    const val = obj.get(key) orelse return null;
    if (val != .string) return null;
    return val.string;
}

fn getUsize(obj: std.json.ObjectMap, key: []const u8) ?usize {
    const val = obj.get(key) orelse return null;
    if (val == .integer) {
        if (val.integer < 0) return null;
        return @intCast(val.integer);
    }
    return null;
}

fn dupeStr(allocator: Allocator, s: ?[]const u8) !?[]const u8 {
    if (s) |v| return try allocator.dupe(u8, v);
    return null;
}

fn parseOneAction(allocator: Allocator, action_type: []const u8, obj: std.json.ObjectMap) !?TermaniaAction {
    if (std.mem.eql(u8, action_type, "send_command")) {
        const pane = getUsize(obj, "pane") orelse return null;
        const command = getStr(obj, "command") orelse return null;
        return .{ .send_command = .{
            .pane = pane,
            .command = try allocator.dupe(u8, command),
        } };
    }

    if (std.mem.eql(u8, action_type, "send_to_all")) {
        const command = getStr(obj, "command") orelse return null;
        return .{ .send_to_all = .{
            .command = try allocator.dupe(u8, command),
        } };
    }

    if (std.mem.eql(u8, action_type, "set_title")) {
        const pane = getUsize(obj, "pane") orelse return null;
        const title = getStr(obj, "title") orelse return null;
        return .{ .set_title = .{
            .pane = pane,
            .title = try allocator.dupe(u8, title),
        } };
    }

    if (std.mem.eql(u8, action_type, "set_watermark")) {
        const pane = getUsize(obj, "pane") orelse return null;
        const watermark = getStr(obj, "watermark") orelse return null;
        return .{ .set_watermark = .{
            .pane = pane,
            .watermark = try allocator.dupe(u8, watermark),
        } };
    }

    if (std.mem.eql(u8, action_type, "clear_watermark")) {
        const pane = getUsize(obj, "pane") orelse return null;
        return .{ .clear_watermark = .{ .pane = pane } };
    }

    if (std.mem.eql(u8, action_type, "navigate")) {
        const pane = getUsize(obj, "pane") orelse return null;
        const url = getStr(obj, "url") orelse return null;
        return .{ .navigate = .{
            .pane = pane,
            .url = try allocator.dupe(u8, url),
        } };
    }

    if (std.mem.eql(u8, action_type, "set_content")) {
        const pane = getUsize(obj, "pane") orelse return null;
        const content = getStr(obj, "content") orelse return null;
        return .{ .set_content = .{
            .pane = pane,
            .content = try allocator.dupe(u8, content),
        } };
    }

    if (std.mem.eql(u8, action_type, "spawn_pane")) {
        return .{ .spawn_pane = .{
            .pane_type = try allocator.dupe(u8, getStr(obj, "pane_type") orelse "terminal"),
            .title = try dupeStr(allocator, getStr(obj, "title")),
            .command = try dupeStr(allocator, getStr(obj, "command")),
            .cwd = try dupeStr(allocator, getStr(obj, "cwd")),
            .url = try dupeStr(allocator, getStr(obj, "url")),
            .content = try dupeStr(allocator, getStr(obj, "content")),
            .watermark = try dupeStr(allocator, getStr(obj, "watermark")),
            .row = getUsize(obj, "row"),
        } };
    }

    if (std.mem.eql(u8, action_type, "close_pane")) {
        const pane = getUsize(obj, "pane") orelse return null;
        return .{ .close_pane = .{ .pane = pane } };
    }

    if (std.mem.eql(u8, action_type, "replace_pane")) {
        const pane = getUsize(obj, "pane") orelse return null;
        return .{ .replace_pane = .{
            .pane = pane,
            .pane_type = try allocator.dupe(u8, getStr(obj, "pane_type") orelse "terminal"),
            .title = try dupeStr(allocator, getStr(obj, "title")),
            .command = try dupeStr(allocator, getStr(obj, "command")),
            .cwd = try dupeStr(allocator, getStr(obj, "cwd")),
            .url = try dupeStr(allocator, getStr(obj, "url")),
            .content = try dupeStr(allocator, getStr(obj, "content")),
        } };
    }

    if (std.mem.eql(u8, action_type, "swap_panes")) {
        const a = getUsize(obj, "a") orelse return null;
        const b = getUsize(obj, "b") orelse return null;
        return .{ .swap_panes = .{ .a = a, .b = b } };
    }

    if (std.mem.eql(u8, action_type, "focus_pane")) {
        const pane = getUsize(obj, "pane") orelse return null;
        return .{ .focus_pane = .{ .pane = pane } };
    }

    if (std.mem.eql(u8, action_type, "message")) {
        const text = getStr(obj, "text") orelse return null;
        return .{ .message = .{ .text = try allocator.dupe(u8, text) } };
    }

    return null;
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

// ---------------------------------------------------------------------------
// LlmClient tests
// ---------------------------------------------------------------------------

test "api provider from string" {
    try testing.expectEqual(ApiProvider.anthropic, ApiProvider.fromString("anthropic"));
    try testing.expectEqual(ApiProvider.anthropic, ApiProvider.fromString("claude"));
    try testing.expectEqual(ApiProvider.openai, ApiProvider.fromString("openai"));
    try testing.expectEqual(ApiProvider.openai, ApiProvider.fromString("ollama"));
    try testing.expectEqual(ApiProvider.openai, ApiProvider.fromString("anything-else"));
}

test "llm client init and deinit" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    try testing.expectEqual(ClientStatus.idle, client.status);
    try testing.expect(client.last_response == null);
    try testing.expect(client.pending_request == null);
}

test "llm client send queues request" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    try client.send("hello world");
    try testing.expectEqual(ClientStatus.waiting, client.status);
    try testing.expect(client.pending_request != null);
    try testing.expectEqualSlices(u8, "hello world", client.pending_request.?);
}

test "llm client send replaces pending request" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    try client.send("first");
    try client.send("second");
    try testing.expectEqualSlices(u8, "second", client.pending_request.?);
}

test "build request body anthropic" {
    const cfg = LlmConfig{ .provider = "anthropic", .model = "claude-test", .max_tokens = 512 };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const body = try client.buildRequestBody(testing.allocator, "What is 2+2?", "You are helpful.");
    defer testing.allocator.free(body);

    // Verify it's valid JSON.
    const parsed = try std.json.parseFromSlice(std.json.Value, testing.allocator, body, .{
        .ignore_unknown_fields = true,
    });
    defer parsed.deinit();

    const root = parsed.value;
    try testing.expect(root == .object);

    // Check model.
    const model_val = root.object.get("model").?;
    try testing.expectEqualSlices(u8, "claude-test", model_val.string);

    // Check max_tokens.
    const max_val = root.object.get("max_tokens").?;
    try testing.expectEqual(@as(i64, 512), max_val.integer);

    // Check system prompt is a top-level field (Anthropic format).
    const sys_val = root.object.get("system").?;
    try testing.expectEqualSlices(u8, "You are helpful.", sys_val.string);

    // Check messages array.
    const msgs = root.object.get("messages").?;
    try testing.expect(msgs == .array);
    try testing.expectEqual(@as(usize, 1), msgs.array.items.len);
    const msg0 = msgs.array.items[0];
    try testing.expectEqualSlices(u8, "user", msg0.object.get("role").?.string);
    try testing.expectEqualSlices(u8, "What is 2+2?", msg0.object.get("content").?.string);
}

test "build request body openai" {
    const cfg = LlmConfig{ .provider = "openai", .model = "gpt-4o-test", .max_tokens = 256 };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const body = try client.buildRequestBody(testing.allocator, "Hello", "System prompt here.");
    defer testing.allocator.free(body);

    const parsed = try std.json.parseFromSlice(std.json.Value, testing.allocator, body, .{
        .ignore_unknown_fields = true,
    });
    defer parsed.deinit();

    const root = parsed.value;
    try testing.expect(root == .object);

    // OpenAI format: system prompt is in messages, not a top-level field.
    try testing.expect(root.object.get("system") == null);

    const msgs = root.object.get("messages").?;
    try testing.expect(msgs == .array);
    try testing.expectEqual(@as(usize, 2), msgs.array.items.len);

    // First message is the system role.
    const sys_msg = msgs.array.items[0];
    try testing.expectEqualSlices(u8, "system", sys_msg.object.get("role").?.string);
    try testing.expectEqualSlices(u8, "System prompt here.", sys_msg.object.get("content").?.string);

    // Second message is the user role.
    const user_msg = msgs.array.items[1];
    try testing.expectEqualSlices(u8, "user", user_msg.object.get("role").?.string);
    try testing.expectEqualSlices(u8, "Hello", user_msg.object.get("content").?.string);
}

test "build request body escapes special chars" {
    const cfg = LlmConfig{ .provider = "anthropic", .model = "test" };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const body = try client.buildRequestBody(testing.allocator, "line1\nline2\t\"quoted\"", "sys");
    defer testing.allocator.free(body);

    // Should be valid JSON despite special characters in user prompt.
    const parsed = try std.json.parseFromSlice(std.json.Value, testing.allocator, body, .{
        .ignore_unknown_fields = true,
    });
    defer parsed.deinit();

    const msgs = parsed.value.object.get("messages").?;
    const content = msgs.array.items[0].object.get("content").?.string;
    try testing.expectEqualSlices(u8, "line1\nline2\t\"quoted\"", content);
}

test "parse response body anthropic" {
    const cfg = LlmConfig{ .provider = "anthropic" };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const body =
        \\{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"Hello from Claude!"}],"model":"claude","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}
    ;

    const text = try client.parseResponseBody(testing.allocator, body);
    defer testing.allocator.free(text);
    try testing.expectEqualSlices(u8, "Hello from Claude!", text);
}

test "parse response body openai" {
    const cfg = LlmConfig{ .provider = "openai" };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const body =
        \\{"id":"chatcmpl-1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"Hello from GPT!"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}
    ;

    const text = try client.parseResponseBody(testing.allocator, body);
    defer testing.allocator.free(text);
    try testing.expectEqualSlices(u8, "Hello from GPT!", text);
}

test "parse response body invalid json" {
    const cfg = LlmConfig{ .provider = "anthropic" };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const result = client.parseResponseBody(testing.allocator, "not json at all");
    try testing.expectError(error.InvalidResponse, result);
}

test "parse response body missing content" {
    const cfg = LlmConfig{ .provider = "anthropic" };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const result = client.parseResponseBody(testing.allocator, "{\"id\":\"msg_1\"}");
    try testing.expectError(error.InvalidResponse, result);
}

test "parse action send command" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "List files", "actions": [{"type": "send_command", "pane": 0, "command": "ls -la"}]}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);

    try testing.expectEqualSlices(u8, "List files", resp.explanation);
    try testing.expectEqual(@as(usize, 1), resp.actions.len);

    switch (resp.actions[0]) {
        .send_command => |a| {
            try testing.expectEqual(@as(usize, 0), a.pane);
            try testing.expectEqualSlices(u8, "ls -la", a.command);
        },
        else => return error.TestUnexpectedResult,
    }
}

test "parse action multiple actions" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "Do stuff", "actions": [
        \\  {"type": "send_command", "pane": 0, "command": "pwd"},
        \\  {"type": "set_title", "pane": 1, "title": "New Title"},
        \\  {"type": "message", "text": "Done!"}
        \\]}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);

    try testing.expectEqual(@as(usize, 3), resp.actions.len);

    switch (resp.actions[0]) {
        .send_command => |a| try testing.expectEqualSlices(u8, "pwd", a.command),
        else => return error.TestUnexpectedResult,
    }
    switch (resp.actions[1]) {
        .set_title => |a| {
            try testing.expectEqual(@as(usize, 1), a.pane);
            try testing.expectEqualSlices(u8, "New Title", a.title);
        },
        else => return error.TestUnexpectedResult,
    }
    switch (resp.actions[2]) {
        .message => |a| try testing.expectEqualSlices(u8, "Done!", a.text),
        else => return error.TestUnexpectedResult,
    }
}

test "parse action with markdown fencing" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\Here's the result:
        \\```json
        \\{"explanation": "test", "actions": [{"type": "focus_pane", "pane": 2}]}
        \\```
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);

    try testing.expectEqual(@as(usize, 1), resp.actions.len);
    switch (resp.actions[0]) {
        .focus_pane => |a| try testing.expectEqual(@as(usize, 2), a.pane),
        else => return error.TestUnexpectedResult,
    }
}

test "parse action spawn pane" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "spawn", "actions": [{"type": "spawn_pane", "pane_type": "browser", "title": "Docs", "url": "https://example.com"}]}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);

    try testing.expectEqual(@as(usize, 1), resp.actions.len);
    switch (resp.actions[0]) {
        .spawn_pane => |a| {
            try testing.expectEqualSlices(u8, "browser", a.pane_type);
            try testing.expectEqualSlices(u8, "Docs", a.title.?);
            try testing.expectEqualSlices(u8, "https://example.com", a.url.?);
        },
        else => return error.TestUnexpectedResult,
    }
}

test "parse action replace pane" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "replace", "actions": [{"type": "replace_pane", "pane": 1, "pane_type": "browser", "url": "https://example.com"}]}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);

    try testing.expectEqual(@as(usize, 1), resp.actions.len);
    switch (resp.actions[0]) {
        .replace_pane => |a| {
            try testing.expectEqual(@as(usize, 1), a.pane);
            try testing.expectEqualSlices(u8, "browser", a.pane_type);
            try testing.expectEqualSlices(u8, "https://example.com", a.url.?);
        },
        else => return error.TestUnexpectedResult,
    }
}

test "parse action no json returns error" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const result = client.parseAction(testing.allocator, "No JSON here at all");
    try testing.expectError(error.NoJsonFound, result);
}

test "parse action unknown type is skipped" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "test", "actions": [{"type": "unknown_action", "foo": "bar"}, {"type": "message", "text": "ok"}]}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);
    // Unknown action is skipped, message is kept.
    try testing.expectEqual(@as(usize, 1), resp.actions.len);
    switch (resp.actions[0]) {
        .message => |a| try testing.expectEqualSlices(u8, "ok", a.text),
        else => return error.TestUnexpectedResult,
    }
}

test "parse action swap panes" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "swap", "actions": [{"type": "swap_panes", "a": 0, "b": 2}]}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);
    try testing.expectEqual(@as(usize, 1), resp.actions.len);
    switch (resp.actions[0]) {
        .swap_panes => |a| {
            try testing.expectEqual(@as(usize, 0), a.a);
            try testing.expectEqual(@as(usize, 2), a.b);
        },
        else => return error.TestUnexpectedResult,
    }
}

test "parse action close pane" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "close", "actions": [{"type": "close_pane", "pane": 3}]}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);
    try testing.expectEqual(@as(usize, 1), resp.actions.len);
    switch (resp.actions[0]) {
        .close_pane => |a| try testing.expectEqual(@as(usize, 3), a.pane),
        else => return error.TestUnexpectedResult,
    }
}

test "format action replace pane" {
    const action = TermaniaAction{ .replace_pane = .{ .pane = 1, .pane_type = "browser" } };
    const display = try formatActionForDisplay(testing.allocator, action);
    defer testing.allocator.free(display);
    try testing.expect(std.mem.indexOf(u8, display, "replace pane 1") != null);
    try testing.expect(std.mem.indexOf(u8, display, "browser") != null);
}

test "poll with no pending request returns false" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const panes = [_]PaneContext{};
    const result = try client.poll(&panes);
    try testing.expect(!result);
}

test "build request body default model anthropic" {
    const cfg = LlmConfig{ .provider = "anthropic" };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const body = try client.buildRequestBody(testing.allocator, "test", "sys");
    defer testing.allocator.free(body);

    const parsed = try std.json.parseFromSlice(std.json.Value, testing.allocator, body, .{
        .ignore_unknown_fields = true,
    });
    defer parsed.deinit();

    const model = parsed.value.object.get("model").?.string;
    try testing.expect(std.mem.indexOf(u8, model, "claude") != null);
}

test "build request body default model openai" {
    const cfg = LlmConfig{ .provider = "openai" };
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const body = try client.buildRequestBody(testing.allocator, "test", "sys");
    defer testing.allocator.free(body);

    const parsed = try std.json.parseFromSlice(std.json.Value, testing.allocator, body, .{
        .ignore_unknown_fields = true,
    });
    defer parsed.deinit();

    const model = parsed.value.object.get("model").?.string;
    try testing.expect(std.mem.indexOf(u8, model, "gpt") != null);
}

test "parse action empty actions array" {
    const cfg = LlmConfig{};
    var client = LlmClient.init(testing.allocator, &cfg);
    defer client.deinit();

    const input =
        \\{"explanation": "nothing to do", "actions": []}
    ;
    const arena = client.response_arena.allocator();
    const resp = try client.parseAction(arena, input);
    try testing.expectEqualSlices(u8, "nothing to do", resp.explanation);
    try testing.expectEqual(@as(usize, 0), resp.actions.len);
}

test "write json escaped" {
    var buf = std.ArrayList(u8).init(testing.allocator);
    defer buf.deinit();
    try writeJsonEscaped(buf.writer(), "hello \"world\"\nnewline\\slash");
    const result = buf.items;
    try testing.expect(std.mem.indexOf(u8, result, "\\\"") != null);
    try testing.expect(std.mem.indexOf(u8, result, "\\n") != null);
    try testing.expect(std.mem.indexOf(u8, result, "\\\\") != null);
}
