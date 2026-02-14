const std = @import("std");
const testing = std.testing;
const Allocator = std.mem.Allocator;
const config_mod = @import("config.zig");

/// Describes the pixel-level layout rectangle for a single pane.
pub const PaneLayout = struct {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    title_height: f32,
};

/// Manages the grid layout of terminal panes.
///
/// The grid is a jagged table: each row can have a different number of columns.
/// All rows share the same height; within a row all panes share the same width.
/// Following Ghostty's comptime-friendly design patterns for layout computation.
pub const GridManager = struct {
    /// Number of columns in each row (length = number of rows).
    row_cols: std.ArrayList(usize),

    pub fn init(allocator: Allocator, num_rows: usize, num_cols: usize) !GridManager {
        var row_cols = std.ArrayList(usize).init(allocator);
        for (0..num_rows) |_| {
            try row_cols.append(num_cols);
        }
        return .{ .row_cols = row_cols };
    }

    pub fn deinit(self: *GridManager) void {
        self.row_cols.deinit();
    }

    /// Total number of rows.
    pub fn numRows(self: *const GridManager) usize {
        return self.row_cols.items.len;
    }

    /// Add a column to the given row (insert a pane slot to the right).
    pub fn addColToRow(self: *GridManager, row: usize) void {
        if (row < self.row_cols.items.len) {
            self.row_cols.items[row] += 1;
        }
    }

    /// Remove a column from the given row. If the row becomes empty, remove it.
    /// Returns true if the row was removed entirely.
    pub fn removeColFromRow(self: *GridManager, row: usize) bool {
        if (row >= self.row_cols.items.len) return false;
        if (self.row_cols.items[row] > 1) {
            self.row_cols.items[row] -= 1;
            return false;
        }
        _ = self.row_cols.orderedRemove(row);
        return true;
    }

    /// Add a new row with one pane.
    pub fn addRow(self: *GridManager) !void {
        try self.row_cols.append(1);
    }

    /// Given a flat pane index, return (row, col_within_row).
    pub fn panePosition(self: *const GridManager, pane_idx: usize) ?struct { row: usize, col: usize } {
        var offset: usize = 0;
        for (self.row_cols.items, 0..) |cols, row| {
            if (pane_idx < offset + cols) {
                return .{ .row = row, .col = pane_idx - offset };
            }
            offset += cols;
        }
        return null;
    }

    /// Given (row, col), return the flat pane index.
    pub fn flatIndex(self: *const GridManager, row: usize, col: usize) ?usize {
        if (row >= self.row_cols.items.len) return null;
        if (col >= self.row_cols.items[row]) return null;
        var offset: usize = 0;
        for (self.row_cols.items[0..row]) |c| {
            offset += c;
        }
        return offset + col;
    }

    /// Number of columns in a given row.
    pub fn colsInRow(self: *const GridManager, row: usize) usize {
        if (row >= self.row_cols.items.len) return 0;
        return self.row_cols.items[row];
    }

    /// Get the total number of pane slots in the grid.
    pub fn totalPanes(self: *const GridManager) usize {
        var total: usize = 0;
        for (self.row_cols.items) |c| total += c;
        return total;
    }

    /// Compute pixel layout for each pane given the total window size.
    /// `scale` converts config values (logical pixels) to physical pixels.
    pub fn computeLayout(
        self: *const GridManager,
        allocator: Allocator,
        window_width: u32,
        window_height: u32,
        cfg: *const config_mod.Config,
        scale: f32,
    ) ![]PaneLayout {
        const outer: f32 = @as(f32, @floatFromInt(cfg.grid.outer_padding)) * scale;
        const gap: f32 = @as(f32, @floatFromInt(cfg.grid.gap)) * scale;
        const title_h: f32 = @as(f32, @floatFromInt(cfg.grid.title_bar_height)) * scale;

        const n_rows = @max(self.row_cols.items.len, 1);

        const total_w: f32 = @as(f32, @floatFromInt(window_width)) - 2.0 * outer;
        const total_h: f32 = @as(f32, @floatFromInt(window_height)) - 2.0 * outer;

        const pane_h = (total_h - (@as(f32, @floatFromInt(n_rows)) - 1.0) * gap) / @as(f32, @floatFromInt(n_rows));

        var layouts = std.ArrayList(PaneLayout).init(allocator);

        for (self.row_cols.items, 0..) |cols_val, row| {
            const n_cols = @max(cols_val, 1);
            const pane_w = (total_w - (@as(f32, @floatFromInt(n_cols)) - 1.0) * gap) / @as(f32, @floatFromInt(n_cols));

            for (0..cols_val) |col| {
                const x = outer + @as(f32, @floatFromInt(col)) * (pane_w + gap);
                const y = outer + @as(f32, @floatFromInt(row)) * (pane_h + gap);
                try layouts.append(.{
                    .x = x,
                    .y = y,
                    .width = pane_w,
                    .height = pane_h,
                    .title_height = title_h,
                });
            }
        }

        return layouts.toOwnedSlice();
    }
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "grid new" {
    var grid = try GridManager.init(testing.allocator, 2, 3);
    defer grid.deinit();
    try testing.expectEqual(@as(usize, 2), grid.numRows());
    try testing.expectEqual(@as(usize, 6), grid.totalPanes());
    try testing.expectEqual(@as(usize, 3), grid.colsInRow(0));
    try testing.expectEqual(@as(usize, 3), grid.colsInRow(1));
}

test "grid 1x1" {
    var grid = try GridManager.init(testing.allocator, 1, 1);
    defer grid.deinit();
    try testing.expectEqual(@as(usize, 1), grid.numRows());
    try testing.expectEqual(@as(usize, 1), grid.totalPanes());
}

test "pane position 2x2" {
    var grid = try GridManager.init(testing.allocator, 2, 2);
    defer grid.deinit();

    const p0 = grid.panePosition(0).?;
    try testing.expectEqual(@as(usize, 0), p0.row);
    try testing.expectEqual(@as(usize, 0), p0.col);

    const p1 = grid.panePosition(1).?;
    try testing.expectEqual(@as(usize, 0), p1.row);
    try testing.expectEqual(@as(usize, 1), p1.col);

    const p2 = grid.panePosition(2).?;
    try testing.expectEqual(@as(usize, 1), p2.row);
    try testing.expectEqual(@as(usize, 0), p2.col);

    const p3 = grid.panePosition(3).?;
    try testing.expectEqual(@as(usize, 1), p3.row);
    try testing.expectEqual(@as(usize, 1), p3.col);

    try testing.expect(grid.panePosition(4) == null);
}

test "flat index" {
    var grid = try GridManager.init(testing.allocator, 2, 3);
    defer grid.deinit();
    try testing.expectEqual(@as(usize, 0), grid.flatIndex(0, 0).?);
    try testing.expectEqual(@as(usize, 2), grid.flatIndex(0, 2).?);
    try testing.expectEqual(@as(usize, 3), grid.flatIndex(1, 0).?);
    try testing.expectEqual(@as(usize, 5), grid.flatIndex(1, 2).?);
}

test "flat index out of bounds" {
    var grid = try GridManager.init(testing.allocator, 2, 2);
    defer grid.deinit();
    try testing.expect(grid.flatIndex(2, 0) == null);
    try testing.expect(grid.flatIndex(0, 3) == null);
}

test "add col to row" {
    var grid = try GridManager.init(testing.allocator, 2, 2);
    defer grid.deinit();
    grid.addColToRow(0);
    try testing.expectEqual(@as(usize, 3), grid.colsInRow(0));
    try testing.expectEqual(@as(usize, 2), grid.colsInRow(1));
    try testing.expectEqual(@as(usize, 5), grid.totalPanes());
}

test "remove col from row" {
    var grid = try GridManager.init(testing.allocator, 2, 3);
    defer grid.deinit();
    const removed = grid.removeColFromRow(0);
    try testing.expect(!removed);
    try testing.expectEqual(@as(usize, 2), grid.colsInRow(0));
}

test "remove col removes row" {
    var grid = try GridManager.init(testing.allocator, 2, 1);
    defer grid.deinit();
    const removed = grid.removeColFromRow(0);
    try testing.expect(removed);
    try testing.expectEqual(@as(usize, 1), grid.numRows());
}

test "add row" {
    var grid = try GridManager.init(testing.allocator, 1, 2);
    defer grid.deinit();
    try grid.addRow();
    try testing.expectEqual(@as(usize, 2), grid.numRows());
    try testing.expectEqual(@as(usize, 1), grid.colsInRow(1));
    try testing.expectEqual(@as(usize, 3), grid.totalPanes());
}

test "cols in row out of range" {
    var grid = try GridManager.init(testing.allocator, 1, 2);
    defer grid.deinit();
    try testing.expectEqual(@as(usize, 0), grid.colsInRow(5));
}

test "jagged grid" {
    var grid = try GridManager.init(testing.allocator, 1, 2);
    defer grid.deinit();
    try grid.addRow();
    grid.addColToRow(1);
    grid.addColToRow(1);
    // Row 0: 2 panes, Row 1: 3 panes
    try testing.expectEqual(@as(usize, 5), grid.totalPanes());
    const p2 = grid.panePosition(2).?;
    try testing.expectEqual(@as(usize, 1), p2.row);
    try testing.expectEqual(@as(usize, 0), p2.col);
    const p4 = grid.panePosition(4).?;
    try testing.expectEqual(@as(usize, 1), p4.row);
    try testing.expectEqual(@as(usize, 2), p4.col);
}

test "compute layout basic" {
    var cfg = config_mod.Config{};
    var grid = try GridManager.init(testing.allocator, 1, 1);
    defer grid.deinit();
    const layouts = try grid.computeLayout(testing.allocator, 800, 600, &cfg, 1.0);
    defer testing.allocator.free(layouts);
    try testing.expectEqual(@as(usize, 1), layouts.len);
    try testing.expect(layouts[0].width > 0.0);
    try testing.expect(layouts[0].height > 0.0);
}

test "compute layout 2x2" {
    var cfg = config_mod.Config{};
    var grid = try GridManager.init(testing.allocator, 2, 2);
    defer grid.deinit();
    const layouts = try grid.computeLayout(testing.allocator, 800, 600, &cfg, 1.0);
    defer testing.allocator.free(layouts);
    try testing.expectEqual(@as(usize, 4), layouts.len);
    try testing.expect(layouts[1].x > layouts[0].x);
    try testing.expect(layouts[2].y > layouts[0].y);
}

test "compute layout all positive" {
    var cfg = config_mod.Config{};
    var grid = try GridManager.init(testing.allocator, 3, 3);
    defer grid.deinit();
    const layouts = try grid.computeLayout(testing.allocator, 1920, 1080, &cfg, 2.0);
    defer testing.allocator.free(layouts);
    for (layouts) |layout| {
        try testing.expect(layout.x >= 0.0);
        try testing.expect(layout.y >= 0.0);
        try testing.expect(layout.width > 0.0);
        try testing.expect(layout.height > 0.0);
    }
}
