const std = @import("std");
const testing = std.testing;

// ---------------------------------------------------------------------------
// Termania — Keyboard input handling
//
// Translates platform key events into:
//   1. Byte sequences for the terminal PTY (keyEventToBytes)
//   2. Application-level keybinding actions (handleAppKeybinding)
//
// Escape sequence encoding follows xterm conventions used by Ghostty,
// Alacritty, and most modern terminal emulators.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Key code enumeration
// ---------------------------------------------------------------------------

pub const KeyCode = enum {
    // Letters
    a, b, c, d, e, f, g, h, i, j, k, l, m,
    n, o, p, q, r, s, t, u, v, w, x, y, z,

    // Digits
    @"0", @"1", @"2", @"3", @"4", @"5", @"6", @"7", @"8", @"9",

    // Function keys
    f1, f2, f3, f4, f5, f6, f7, f8, f9, f10, f11, f12,

    // Navigation
    up, down, left, right,
    home, end,
    page_up, page_down,
    insert, delete,

    // Whitespace / control
    enter,
    tab,
    escape,
    backspace,
    space,

    // Punctuation / symbols
    minus,
    equal,
    left_bracket,
    right_bracket,
    backslash,
    semicolon,
    apostrophe,
    grave,
    comma,
    period,
    slash,
    plus,

    /// Returns the ASCII character for a letter key, or null if not a letter.
    pub fn letter(self: KeyCode) ?u8 {
        return switch (self) {
            .a => 'a', .b => 'b', .c => 'c', .d => 'd', .e => 'e',
            .f => 'f', .g => 'g', .h => 'h', .i => 'i', .j => 'j',
            .k => 'k', .l => 'l', .m => 'm', .n => 'n', .o => 'o',
            .p => 'p', .q => 'q', .r => 'r', .s => 's', .t => 't',
            .u => 'u', .v => 'v', .w => 'w', .x => 'x', .y => 'y',
            .z => 'z',
            else => null,
        };
    }

    /// Returns the ASCII character for a digit key, or null if not a digit.
    pub fn digit(self: KeyCode) ?u8 {
        return switch (self) {
            .@"0" => '0', .@"1" => '1', .@"2" => '2', .@"3" => '3',
            .@"4" => '4', .@"5" => '5', .@"6" => '6', .@"7" => '7',
            .@"8" => '8', .@"9" => '9',
            else => null,
        };
    }

    /// Returns the printable ASCII character for this key (unmodified),
    /// or null if the key does not map to a single printable character.
    pub fn printable(self: KeyCode) ?u8 {
        if (self.letter()) |ch| return ch;
        if (self.digit()) |ch| return ch;
        return switch (self) {
            .space => ' ',
            .minus => '-',
            .equal => '=',
            .left_bracket => '[',
            .right_bracket => ']',
            .backslash => '\\',
            .semicolon => ';',
            .apostrophe => '\'',
            .grave => '`',
            .comma => ',',
            .period => '.',
            .slash => '/',
            .plus => '+',
            else => null,
        };
    }
};

// ---------------------------------------------------------------------------
// Modifier flags
// ---------------------------------------------------------------------------

pub const Modifiers = struct {
    ctrl: bool = false,
    alt: bool = false,
    shift: bool = false,
    super: bool = false,

    pub const none = Modifiers{};

    pub fn eql(a: Modifiers, b: Modifiers) bool {
        return a.ctrl == b.ctrl and a.alt == b.alt and
            a.shift == b.shift and a.super == b.super;
    }
};

// ---------------------------------------------------------------------------
// KeyEvent
// ---------------------------------------------------------------------------

pub const KeyEvent = struct {
    key: KeyCode,
    mods: Modifiers = Modifiers.none,
};

// ---------------------------------------------------------------------------
// Application-level keybinding actions
// ---------------------------------------------------------------------------

pub const AppAction = enum {
    new_pane,
    close_pane,
    navigate_up,
    navigate_down,
    navigate_left,
    navigate_right,
    jump_to_pane_1,
    jump_to_pane_2,
    jump_to_pane_3,
    jump_to_pane_4,
    jump_to_pane_5,
    jump_to_pane_6,
    jump_to_pane_7,
    jump_to_pane_8,
    jump_to_pane_9,
    rename_pane,
    broadcast_toggle,
    font_size_increase,
    font_size_decrease,
    command_overlay_toggle,
    help_toggle,
};

// ---------------------------------------------------------------------------
// keyEventToBytes — convert a KeyEvent into the PTY byte sequence
// ---------------------------------------------------------------------------

/// Maximum number of bytes a single key event can produce.
pub const max_key_bytes = 8;

/// Encodes a KeyEvent into the byte sequence that should be written to the
/// terminal PTY.  Returns the number of valid bytes written into `buf`.
/// If the event cannot be represented (e.g. bare modifier press), returns 0.
pub fn keyEventToBytes(event: KeyEvent, buf: *[max_key_bytes]u8) usize {
    const key = event.key;
    const mods = event.mods;

    // ----- Ctrl+letter → 0x01..0x1A -----
    if (mods.ctrl and !mods.shift and !mods.super) {
        if (key.letter()) |ch| {
            const ctrl_byte = ch - 'a' + 1; // 'a'→0x01, 'z'→0x1A
            if (mods.alt) {
                buf[0] = 0x1b; // ESC prefix for Alt
                buf[1] = ctrl_byte;
                return 2;
            }
            buf[0] = ctrl_byte;
            return 1;
        }
    }

    // ----- Special keys -----
    switch (key) {
        .enter => {
            if (mods.alt) {
                buf[0] = 0x1b;
                buf[1] = '\r';
                return 2;
            }
            buf[0] = '\r';
            return 1;
        },
        .tab => {
            if (mods.shift) {
                // Reverse tab: ESC [ Z
                buf[0] = 0x1b;
                buf[1] = '[';
                buf[2] = 'Z';
                return 3;
            }
            if (mods.alt) {
                buf[0] = 0x1b;
                buf[1] = '\t';
                return 2;
            }
            buf[0] = '\t';
            return 1;
        },
        .escape => {
            buf[0] = 0x1b;
            return 1;
        },
        .backspace => {
            if (mods.alt) {
                buf[0] = 0x1b;
                buf[1] = 0x7f;
                return 2;
            }
            if (mods.ctrl) {
                buf[0] = 0x08; // BS
                return 1;
            }
            buf[0] = 0x7f;
            return 1;
        },

        // Arrow keys: ESC [ A/B/C/D
        .up => return writeArrowOrNav(buf, 'A', mods),
        .down => return writeArrowOrNav(buf, 'B', mods),
        .right => return writeArrowOrNav(buf, 'C', mods),
        .left => return writeArrowOrNav(buf, 'D', mods),

        // Home / End
        .home => return writeTildeOrLetter(buf, 'H', null, mods),
        .end => return writeTildeOrLetter(buf, 'F', null, mods),

        // Page Up / Down, Insert, Delete — ESC [ <n> ~
        .page_up => return writeTildeOrLetter(buf, null, 5, mods),
        .page_down => return writeTildeOrLetter(buf, null, 6, mods),
        .insert => return writeTildeOrLetter(buf, null, 2, mods),
        .delete => return writeTildeOrLetter(buf, null, 3, mods),

        // Function keys
        .f1 => return writeFKey(buf, 1, mods),
        .f2 => return writeFKey(buf, 2, mods),
        .f3 => return writeFKey(buf, 3, mods),
        .f4 => return writeFKey(buf, 4, mods),
        .f5 => return writeFKey(buf, 5, mods),
        .f6 => return writeFKey(buf, 6, mods),
        .f7 => return writeFKey(buf, 7, mods),
        .f8 => return writeFKey(buf, 8, mods),
        .f9 => return writeFKey(buf, 9, mods),
        .f10 => return writeFKey(buf, 10, mods),
        .f11 => return writeFKey(buf, 11, mods),
        .f12 => return writeFKey(buf, 12, mods),

        else => {},
    }

    // ----- Regular printable characters -----
    if (key.printable()) |ch| {
        var out_char = ch;
        if (mods.shift) {
            out_char = shiftChar(ch);
        }
        if (mods.alt) {
            buf[0] = 0x1b;
            buf[1] = out_char;
            return 2;
        }
        buf[0] = out_char;
        return 1;
    }

    return 0;
}

// ---------------------------------------------------------------------------
// Arrow / navigation helpers
// ---------------------------------------------------------------------------

/// Writes an arrow key escape sequence. Final char is one of A/B/C/D.
/// With modifiers the form is ESC [ 1 ; <mod> <final>.
fn writeArrowOrNav(buf: *[max_key_bytes]u8, final: u8, mods: Modifiers) usize {
    const mod_param = modifierParam(mods);
    if (mod_param == 0) {
        buf[0] = 0x1b;
        buf[1] = '[';
        buf[2] = final;
        return 3;
    }
    buf[0] = 0x1b;
    buf[1] = '[';
    buf[2] = '1';
    buf[3] = ';';
    buf[4] = '0' + mod_param;
    buf[5] = final;
    return 6;
}

/// Writes either ESC [ <final> (home/end style) or ESC [ <n> ~ (tilde style).
/// With modifiers: ESC [ <n> ; <mod> ~ or ESC [ 1 ; <mod> <final>.
fn writeTildeOrLetter(buf: *[max_key_bytes]u8, final_letter: ?u8, tilde_num: ?u8, mods: Modifiers) usize {
    const mod_param = modifierParam(mods);

    if (final_letter) |fl| {
        // Home/End style
        if (mod_param == 0) {
            buf[0] = 0x1b;
            buf[1] = '[';
            buf[2] = fl;
            return 3;
        }
        buf[0] = 0x1b;
        buf[1] = '[';
        buf[2] = '1';
        buf[3] = ';';
        buf[4] = '0' + mod_param;
        buf[5] = fl;
        return 6;
    }

    if (tilde_num) |tn| {
        if (mod_param == 0) {
            buf[0] = 0x1b;
            buf[1] = '[';
            buf[2] = '0' + tn;
            buf[3] = '~';
            return 4;
        }
        buf[0] = 0x1b;
        buf[1] = '[';
        buf[2] = '0' + tn;
        buf[3] = ';';
        buf[4] = '0' + mod_param;
        buf[5] = '~';
        return 6;
    }

    return 0;
}

// ---------------------------------------------------------------------------
// Function key helper
// ---------------------------------------------------------------------------

/// Encodes a function key (1-12) into the appropriate escape sequence.
///   F1-F4:  ESC O P/Q/R/S  (SS3 style, no modifiers)
///           ESC [ 1 ; <mod> P/Q/R/S  (with modifiers)
///   F5-F12: ESC [ <num> ~ where num is:
///           F5=15, F6=17, F7=18, F8=19, F9=20, F10=21, F11=23, F12=24
fn writeFKey(buf: *[max_key_bytes]u8, fnum: u8, mods: Modifiers) usize {
    const mod_param = modifierParam(mods);

    // F1-F4: SS3 encoding
    if (fnum >= 1 and fnum <= 4) {
        const final_char: u8 = switch (fnum) {
            1 => 'P',
            2 => 'Q',
            3 => 'R',
            4 => 'S',
            else => unreachable,
        };
        if (mod_param == 0) {
            buf[0] = 0x1b;
            buf[1] = 'O';
            buf[2] = final_char;
            return 3;
        }
        // With modifiers: ESC [ 1 ; <mod> P/Q/R/S
        buf[0] = 0x1b;
        buf[1] = '[';
        buf[2] = '1';
        buf[3] = ';';
        buf[4] = '0' + mod_param;
        buf[5] = final_char;
        return 6;
    }

    // F5-F12: tilde encoding ESC [ <num> ~
    // The VT number for each function key:
    const vt_num: u8 = switch (fnum) {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        else => return 0,
    };

    // Two-digit number: tens and units
    const tens: u8 = vt_num / 10;
    const units: u8 = vt_num % 10;

    if (mod_param == 0) {
        buf[0] = 0x1b;
        buf[1] = '[';
        buf[2] = '0' + tens;
        buf[3] = '0' + units;
        buf[4] = '~';
        return 5;
    }
    buf[0] = 0x1b;
    buf[1] = '[';
    buf[2] = '0' + tens;
    buf[3] = '0' + units;
    buf[4] = ';';
    buf[5] = '0' + mod_param;
    buf[6] = '~';
    return 7;
}

// ---------------------------------------------------------------------------
// Modifier parameter encoding (xterm style)
// ---------------------------------------------------------------------------

/// Returns the xterm modifier parameter value, or 0 for no modifiers.
///   Shift        = 2
///   Alt          = 3
///   Alt+Shift    = 4
///   Ctrl         = 5
///   Ctrl+Shift   = 6
///   Ctrl+Alt     = 7
///   Ctrl+Alt+Shift = 8
fn modifierParam(mods: Modifiers) u8 {
    if (!mods.ctrl and !mods.alt and !mods.shift) return 0;

    var val: u8 = 1;
    if (mods.shift) val += 1;
    if (mods.alt) val += 2;
    if (mods.ctrl) val += 4;
    return val;
}

// ---------------------------------------------------------------------------
// Shift character mapping
// ---------------------------------------------------------------------------

/// Map a character to its shifted equivalent (US keyboard layout).
fn shiftChar(ch: u8) u8 {
    return switch (ch) {
        'a'...'z' => ch - 32, // uppercase
        '0' => ')',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        '`' => '~',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '+' => '+', // plus stays plus when shifted
        else => ch,
    };
}

// ---------------------------------------------------------------------------
// handleAppKeybinding — check for application-level shortcuts
// ---------------------------------------------------------------------------

/// Checks whether the given key event matches an application-level
/// keybinding.  Returns the corresponding AppAction if consumed, or null
/// if the key should be forwarded to the terminal.
pub fn handleAppKeybinding(event: KeyEvent) ?AppAction {
    const mods = event.mods;
    const key = event.key;

    // All application bindings require Ctrl+Shift (without super).
    if (!mods.ctrl or !mods.shift or mods.super) return null;

    // Ctrl+Shift+<key>
    return switch (key) {
        .n => .new_pane,
        .w => .close_pane,
        .up => .navigate_up,
        .down => .navigate_down,
        .left => .navigate_left,
        .right => .navigate_right,
        .@"1" => .jump_to_pane_1,
        .@"2" => .jump_to_pane_2,
        .@"3" => .jump_to_pane_3,
        .@"4" => .jump_to_pane_4,
        .@"5" => .jump_to_pane_5,
        .@"6" => .jump_to_pane_6,
        .@"7" => .jump_to_pane_7,
        .@"8" => .jump_to_pane_8,
        .@"9" => .jump_to_pane_9,
        .r => .rename_pane,
        .b => .broadcast_toggle,
        .plus, .equal => .font_size_increase,
        .minus => .font_size_decrease,
        .enter => .command_overlay_toggle,
        .slash => .help_toggle,
        else => null,
    };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "regular letter produces ASCII" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .a }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 'a'), buf[0]);
}

test "shifted letter produces uppercase" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .a, .mods = .{ .shift = true } }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 'A'), buf[0]);
}

test "digit key" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .@"5" }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, '5'), buf[0]);
}

test "shifted digit produces symbol" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .@"1", .mods = .{ .shift = true } }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, '!'), buf[0]);
}

test "ctrl+a produces 0x01" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .a, .mods = .{ .ctrl = true } }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 0x01), buf[0]);
}

test "ctrl+c produces 0x03" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .c, .mods = .{ .ctrl = true } }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 0x03), buf[0]);
}

test "ctrl+z produces 0x1A" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .z, .mods = .{ .ctrl = true } }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 0x1A), buf[0]);
}

test "alt+a produces ESC a" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .a, .mods = .{ .alt = true } }, &buf);
    try testing.expectEqual(@as(usize, 2), n);
    try testing.expectEqual(@as(u8, 0x1b), buf[0]);
    try testing.expectEqual(@as(u8, 'a'), buf[1]);
}

test "alt+ctrl+a produces ESC 0x01" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .a, .mods = .{ .ctrl = true, .alt = true } }, &buf);
    try testing.expectEqual(@as(usize, 2), n);
    try testing.expectEqual(@as(u8, 0x1b), buf[0]);
    try testing.expectEqual(@as(u8, 0x01), buf[1]);
}

test "enter produces CR" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .enter }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, '\r'), buf[0]);
}

test "alt+enter produces ESC CR" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .enter, .mods = .{ .alt = true } }, &buf);
    try testing.expectEqual(@as(usize, 2), n);
    try testing.expectEqual(@as(u8, 0x1b), buf[0]);
    try testing.expectEqual(@as(u8, '\r'), buf[1]);
}

test "tab produces HT" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .tab }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, '\t'), buf[0]);
}

test "shift+tab produces ESC [ Z" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .tab, .mods = .{ .shift = true } }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1b[Z", buf[0..3]);
}

test "escape produces ESC" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .escape }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 0x1b), buf[0]);
}

test "backspace produces DEL" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .backspace }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 0x7f), buf[0]);
}

test "alt+backspace produces ESC DEL" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .backspace, .mods = .{ .alt = true } }, &buf);
    try testing.expectEqual(@as(usize, 2), n);
    try testing.expectEqual(@as(u8, 0x1b), buf[0]);
    try testing.expectEqual(@as(u8, 0x7f), buf[1]);
}

test "ctrl+backspace produces BS" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .backspace, .mods = .{ .ctrl = true } }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, 0x08), buf[0]);
}

test "arrow up produces ESC [ A" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .up }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1b[A", buf[0..3]);
}

test "arrow down produces ESC [ B" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .down }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1b[B", buf[0..3]);
}

test "arrow right produces ESC [ C" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .right }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1b[C", buf[0..3]);
}

test "arrow left produces ESC [ D" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .left }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1b[D", buf[0..3]);
}

test "shift+up produces ESC [ 1 ; 2 A" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .up, .mods = .{ .shift = true } }, &buf);
    try testing.expectEqual(@as(usize, 6), n);
    try testing.expectEqualSlices(u8, "\x1b[1;2A", buf[0..6]);
}

test "ctrl+right produces ESC [ 1 ; 5 C" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .right, .mods = .{ .ctrl = true } }, &buf);
    try testing.expectEqual(@as(usize, 6), n);
    try testing.expectEqualSlices(u8, "\x1b[1;5C", buf[0..6]);
}

test "home produces ESC [ H" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .home }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1b[H", buf[0..3]);
}

test "end produces ESC [ F" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .end }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1b[F", buf[0..3]);
}

test "page up produces ESC [ 5 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .page_up }, &buf);
    try testing.expectEqual(@as(usize, 4), n);
    try testing.expectEqualSlices(u8, "\x1b[5~", buf[0..4]);
}

test "page down produces ESC [ 6 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .page_down }, &buf);
    try testing.expectEqual(@as(usize, 4), n);
    try testing.expectEqualSlices(u8, "\x1b[6~", buf[0..4]);
}

test "insert produces ESC [ 2 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .insert }, &buf);
    try testing.expectEqual(@as(usize, 4), n);
    try testing.expectEqualSlices(u8, "\x1b[2~", buf[0..4]);
}

test "delete produces ESC [ 3 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .delete }, &buf);
    try testing.expectEqual(@as(usize, 4), n);
    try testing.expectEqualSlices(u8, "\x1b[3~", buf[0..4]);
}

test "F1 produces ESC O P" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f1 }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1bOP", buf[0..3]);
}

test "F2 produces ESC O Q" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f2 }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1bOQ", buf[0..3]);
}

test "F3 produces ESC O R" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f3 }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1bOR", buf[0..3]);
}

test "F4 produces ESC O S" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f4 }, &buf);
    try testing.expectEqual(@as(usize, 3), n);
    try testing.expectEqualSlices(u8, "\x1bOS", buf[0..3]);
}

test "F5 produces ESC [ 1 5 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f5 }, &buf);
    try testing.expectEqual(@as(usize, 5), n);
    try testing.expectEqualSlices(u8, "\x1b[15~", buf[0..5]);
}

test "F6 produces ESC [ 1 7 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f6 }, &buf);
    try testing.expectEqual(@as(usize, 5), n);
    try testing.expectEqualSlices(u8, "\x1b[17~", buf[0..5]);
}

test "F12 produces ESC [ 2 4 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f12 }, &buf);
    try testing.expectEqual(@as(usize, 5), n);
    try testing.expectEqualSlices(u8, "\x1b[24~", buf[0..5]);
}

test "shift+F1 produces ESC [ 1 ; 2 P" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f1, .mods = .{ .shift = true } }, &buf);
    try testing.expectEqual(@as(usize, 6), n);
    try testing.expectEqualSlices(u8, "\x1b[1;2P", buf[0..6]);
}

test "ctrl+F5 produces ESC [ 1 5 ; 5 ~" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .f5, .mods = .{ .ctrl = true } }, &buf);
    try testing.expectEqual(@as(usize, 7), n);
    try testing.expectEqualSlices(u8, "\x1b[15;5~", buf[0..7]);
}

test "space produces 0x20" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .space }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, ' '), buf[0]);
}

test "punctuation keys" {
    var buf: [max_key_bytes]u8 = undefined;

    var n = keyEventToBytes(.{ .key = .minus }, &buf);
    try testing.expectEqual(@as(u8, '-'), buf[0]);
    try testing.expectEqual(@as(usize, 1), n);

    n = keyEventToBytes(.{ .key = .slash }, &buf);
    try testing.expectEqual(@as(u8, '/'), buf[0]);
    try testing.expectEqual(@as(usize, 1), n);

    n = keyEventToBytes(.{ .key = .period }, &buf);
    try testing.expectEqual(@as(u8, '.'), buf[0]);
    try testing.expectEqual(@as(usize, 1), n);
}

test "shift punctuation" {
    var buf: [max_key_bytes]u8 = undefined;
    const n = keyEventToBytes(.{ .key = .minus, .mods = .{ .shift = true } }, &buf);
    try testing.expectEqual(@as(usize, 1), n);
    try testing.expectEqual(@as(u8, '_'), buf[0]);
}

// ----- App keybinding tests -----

test "ctrl+shift+n triggers new_pane" {
    const action = handleAppKeybinding(.{ .key = .n, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.new_pane, action.?);
}

test "ctrl+shift+w triggers close_pane" {
    const action = handleAppKeybinding(.{ .key = .w, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.close_pane, action.?);
}

test "ctrl+shift+arrow triggers navigation" {
    try testing.expectEqual(
        AppAction.navigate_up,
        handleAppKeybinding(.{ .key = .up, .mods = .{ .ctrl = true, .shift = true } }).?,
    );
    try testing.expectEqual(
        AppAction.navigate_down,
        handleAppKeybinding(.{ .key = .down, .mods = .{ .ctrl = true, .shift = true } }).?,
    );
    try testing.expectEqual(
        AppAction.navigate_left,
        handleAppKeybinding(.{ .key = .left, .mods = .{ .ctrl = true, .shift = true } }).?,
    );
    try testing.expectEqual(
        AppAction.navigate_right,
        handleAppKeybinding(.{ .key = .right, .mods = .{ .ctrl = true, .shift = true } }).?,
    );
}

test "ctrl+shift+1-9 triggers jump_to_pane" {
    try testing.expectEqual(
        AppAction.jump_to_pane_1,
        handleAppKeybinding(.{ .key = .@"1", .mods = .{ .ctrl = true, .shift = true } }).?,
    );
    try testing.expectEqual(
        AppAction.jump_to_pane_5,
        handleAppKeybinding(.{ .key = .@"5", .mods = .{ .ctrl = true, .shift = true } }).?,
    );
    try testing.expectEqual(
        AppAction.jump_to_pane_9,
        handleAppKeybinding(.{ .key = .@"9", .mods = .{ .ctrl = true, .shift = true } }).?,
    );
}

test "ctrl+shift+r triggers rename_pane" {
    const action = handleAppKeybinding(.{ .key = .r, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.rename_pane, action.?);
}

test "ctrl+shift+b triggers broadcast_toggle" {
    const action = handleAppKeybinding(.{ .key = .b, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.broadcast_toggle, action.?);
}

test "ctrl+shift+plus triggers font_size_increase" {
    const action = handleAppKeybinding(.{ .key = .plus, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.font_size_increase, action.?);
}

test "ctrl+shift+equal triggers font_size_increase" {
    const action = handleAppKeybinding(.{ .key = .equal, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.font_size_increase, action.?);
}

test "ctrl+shift+minus triggers font_size_decrease" {
    const action = handleAppKeybinding(.{ .key = .minus, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.font_size_decrease, action.?);
}

test "ctrl+shift+enter triggers command_overlay_toggle" {
    const action = handleAppKeybinding(.{ .key = .enter, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.command_overlay_toggle, action.?);
}

test "ctrl+shift+slash triggers help_toggle" {
    const action = handleAppKeybinding(.{ .key = .slash, .mods = .{ .ctrl = true, .shift = true } });
    try testing.expectEqual(AppAction.help_toggle, action.?);
}

test "plain letter is not an app binding" {
    const action = handleAppKeybinding(.{ .key = .a });
    try testing.expect(action == null);
}

test "ctrl without shift is not an app binding" {
    const action = handleAppKeybinding(.{ .key = .n, .mods = .{ .ctrl = true } });
    try testing.expect(action == null);
}

test "shift without ctrl is not an app binding" {
    const action = handleAppKeybinding(.{ .key = .n, .mods = .{ .shift = true } });
    try testing.expect(action == null);
}

test "ctrl+shift+super is not an app binding" {
    const action = handleAppKeybinding(.{
        .key = .n,
        .mods = .{ .ctrl = true, .shift = true, .super = true },
    });
    try testing.expect(action == null);
}

test "modifier param encoding" {
    try testing.expectEqual(@as(u8, 0), modifierParam(.{}));
    try testing.expectEqual(@as(u8, 2), modifierParam(.{ .shift = true }));
    try testing.expectEqual(@as(u8, 3), modifierParam(.{ .alt = true }));
    try testing.expectEqual(@as(u8, 4), modifierParam(.{ .shift = true, .alt = true }));
    try testing.expectEqual(@as(u8, 5), modifierParam(.{ .ctrl = true }));
    try testing.expectEqual(@as(u8, 6), modifierParam(.{ .ctrl = true, .shift = true }));
    try testing.expectEqual(@as(u8, 7), modifierParam(.{ .ctrl = true, .alt = true }));
    try testing.expectEqual(@as(u8, 8), modifierParam(.{ .ctrl = true, .alt = true, .shift = true }));
}

test "KeyCode.letter" {
    try testing.expectEqual(@as(?u8, 'a'), KeyCode.a.letter());
    try testing.expectEqual(@as(?u8, 'z'), KeyCode.z.letter());
    try testing.expect(KeyCode.@"0".letter() == null);
    try testing.expect(KeyCode.up.letter() == null);
}

test "KeyCode.digit" {
    try testing.expectEqual(@as(?u8, '0'), KeyCode.@"0".digit());
    try testing.expectEqual(@as(?u8, '9'), KeyCode.@"9".digit());
    try testing.expect(KeyCode.a.digit() == null);
}

test "KeyCode.printable" {
    try testing.expectEqual(@as(?u8, 'a'), KeyCode.a.printable());
    try testing.expectEqual(@as(?u8, '5'), KeyCode.@"5".printable());
    try testing.expectEqual(@as(?u8, ' '), KeyCode.space.printable());
    try testing.expectEqual(@as(?u8, '-'), KeyCode.minus.printable());
    try testing.expect(KeyCode.up.printable() == null);
    try testing.expect(KeyCode.f1.printable() == null);
}

test "Modifiers.eql" {
    try testing.expect(Modifiers.none.eql(.{}));
    try testing.expect((Modifiers{ .ctrl = true }).eql(.{ .ctrl = true }));
    try testing.expect(!(Modifiers{ .ctrl = true }).eql(.{ .alt = true }));
}
