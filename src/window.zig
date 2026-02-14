const std = @import("std");
const testing = std.testing;
const input = @import("input.zig");

// ---------------------------------------------------------------------------
// Termania -- X11 windowing and event loop
//
// Provides the X11Window struct which:
//   - Opens an X11 display connection and creates a window
//   - Creates a GLX context for future OpenGL rendering
//   - Translates X11 key/button/motion events into Termania's
//     input.KeyEvent structs
//   - Drives a ~60 fps event loop with a user-supplied tick callback
//
// Architecture follows Ghostty's per-surface model where each window owns
// its display connection and GL context. The event loop uses non-blocking
// XPending + XNextEvent polling combined with nanosleep to hit the target
// frame rate without busy-waiting.
// ---------------------------------------------------------------------------

const x11 = @cImport({
    @cInclude("X11/Xlib.h");
    @cInclude("X11/Xutil.h");
    @cInclude("X11/keysym.h");
    @cInclude("GL/glx.h");
});

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Target frame rate for the main event loop.
const target_fps: u64 = 60;

/// Nanoseconds per frame at the target frame rate.
const ns_per_frame: u64 = 1_000_000_000 / target_fps;

/// Event mask requesting the events we care about.
const event_mask: c_long = x11.KeyPressMask |
    x11.KeyReleaseMask |
    x11.ButtonPressMask |
    x11.ButtonReleaseMask |
    x11.PointerMotionMask |
    x11.ExposureMask |
    x11.StructureNotifyMask |
    x11.FocusChangeMask;

// ---------------------------------------------------------------------------
// X11Window
// ---------------------------------------------------------------------------

pub const X11Window = struct {
    allocator: std.mem.Allocator,

    // X11 handles
    display: *x11.Display,
    screen: c_int,
    window: x11.Window,
    wm_delete_window: x11.Atom,

    // GLX handles
    glx_context: x11.GLXContext,
    visual_info: *x11.XVisualInfo,

    // Window state
    width: u32,
    height: u32,
    focused: bool = true,
    should_close: bool = false,
    needs_redraw: bool = true,

    // ---------------------------------------------------------------------------
    // Lifecycle
    // ---------------------------------------------------------------------------

    /// Open an X11 display, create a window with a GLX context, and map it.
    ///
    /// The caller must call `deinit` when the window is no longer needed.
    pub fn init(allocator: std.mem.Allocator, width: u32, height: u32, title: []const u8) !X11Window {
        // Open connection to the X server.
        const display = x11.XOpenDisplay(null) orelse return error.DisplayOpenFailed;
        errdefer _ = x11.XCloseDisplay(display);

        const screen = x11.XDefaultScreen(display);
        const root = x11.XRootWindow(display, screen);

        // Choose a GLX visual (RGBA, double-buffered, with depth).
        var attribs = [_]c_int{
            x11.GLX_RGBA,
            x11.GLX_DOUBLEBUFFER,
            x11.GLX_DEPTH_SIZE, 24,
            x11.GLX_RED_SIZE,   8,
            x11.GLX_GREEN_SIZE, 8,
            x11.GLX_BLUE_SIZE,  8,
            x11.GLX_ALPHA_SIZE, 8,
            x11.None,
        };
        const vi = x11.glXChooseVisual(display, screen, &attribs) orelse
            return error.NoSuitableVisual;
        errdefer _ = x11.XFree(vi);

        // Create a colormap for the chosen visual.
        const colormap = x11.XCreateColormap(display, root, vi.*.visual, x11.AllocNone);

        // Set window attributes.
        var swa: x11.XSetWindowAttributes = std.mem.zeroes(x11.XSetWindowAttributes);
        swa.colormap = colormap;
        swa.event_mask = event_mask;

        // Create the window.
        const win = x11.XCreateWindow(
            display,
            root,
            0,
            0,
            width,
            height,
            0, // border width
            vi.*.depth,
            x11.InputOutput,
            vi.*.visual,
            x11.CWColormap | x11.CWEventMask,
            &swa,
        );
        errdefer _ = x11.XDestroyWindow(display, win);

        // Set the window title. XStoreName expects a null-terminated C string.
        // The title slice from config is comptime-known and null-terminated in
        // practice, but to be safe we copy it.
        var title_buf: [256]u8 = undefined;
        const copy_len = @min(title.len, title_buf.len - 1);
        @memcpy(title_buf[0..copy_len], title[0..copy_len]);
        title_buf[copy_len] = 0;
        _ = x11.XStoreName(display, win, &title_buf);

        // Register interest in the WM_DELETE_WINDOW protocol so we can
        // intercept the close button rather than having the server kill us.
        var wm_delete = x11.XInternAtom(display, "WM_DELETE_WINDOW", x11.False);
        _ = x11.XSetWMProtocols(display, win, &wm_delete, 1);

        // Create a GLX rendering context.
        const glx_ctx = x11.glXCreateContext(display, vi, null, x11.True) orelse
            return error.GlxContextCreationFailed;
        errdefer x11.glXDestroyContext(display, glx_ctx);

        // Make the context current so the caller can issue GL commands
        // immediately after init.
        _ = x11.glXMakeCurrent(display, win, glx_ctx);

        // Map (show) the window.
        _ = x11.XMapWindow(display, win);
        _ = x11.XFlush(display);

        return X11Window{
            .allocator = allocator,
            .display = display,
            .screen = screen,
            .window = win,
            .wm_delete_window = wm_delete,
            .glx_context = glx_ctx,
            .visual_info = vi,
            .width = width,
            .height = height,
        };
    }

    /// Tear down the window, GLX context, and display connection.
    pub fn deinit(self: *X11Window) void {
        x11.glXDestroyContext(self.display, self.glx_context);
        _ = x11.XDestroyWindow(self.display, self.window);
        _ = x11.XFree(self.visual_info);
        _ = x11.XCloseDisplay(self.display);
    }

    // ---------------------------------------------------------------------------
    // Event loop
    // ---------------------------------------------------------------------------

    /// Run the main event loop.
    ///
    /// Each iteration:
    ///   1. Drain all pending X11 events (non-blocking via XPending).
    ///   2. Invoke the user-provided `tick` callback. If it returns `false`,
    ///      the loop exits.
    ///   3. Swap the GL back-buffer.
    ///   4. Sleep for the remainder of the frame to target ~60 fps.
    ///
    /// The tick callback receives a pointer to this window so it can query
    /// state (dimensions, focus, etc.) and issue GL draw calls.
    pub fn run(self: *X11Window, tick: *const fn (*X11Window) bool) void {
        while (!self.should_close) {
            const frame_start = std.time.nanoTimestamp();

            // --- Drain pending X11 events ---------------------------------
            while (x11.XPending(self.display) > 0) {
                var ev: x11.XEvent = undefined;
                _ = x11.XNextEvent(self.display, &ev);
                self.handleEvent(&ev);
            }

            // --- User tick -------------------------------------------------
            if (!tick(self)) break;

            // --- Swap buffers ----------------------------------------------
            x11.glXSwapBuffers(self.display, self.window);

            // --- Frame pacing (sleep until next frame) ---------------------
            const elapsed: u64 = @intCast(std.time.nanoTimestamp() - frame_start);
            if (elapsed < ns_per_frame) {
                std.time.sleep(ns_per_frame - elapsed);
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Swap buffers (public, for callers that drive the loop externally)
    // ---------------------------------------------------------------------------

    /// Swap the front and back GL buffers. Call this at the end of each
    /// frame if you are driving the event loop yourself instead of using `run`.
    pub fn swapBuffers(self: *X11Window) void {
        x11.glXSwapBuffers(self.display, self.window);
    }

    // ---------------------------------------------------------------------------
    // Internal event handling
    // ---------------------------------------------------------------------------

    fn handleEvent(self: *X11Window, ev: *x11.XEvent) void {
        switch (ev.type) {
            x11.Expose => {
                self.needs_redraw = true;
            },

            x11.ConfigureNotify => {
                const ce = &ev.xconfigure;
                const new_w: u32 = @intCast(ce.width);
                const new_h: u32 = @intCast(ce.height);
                if (new_w != self.width or new_h != self.height) {
                    self.width = new_w;
                    self.height = new_h;
                    self.needs_redraw = true;
                }
            },

            x11.ClientMessage => {
                // Check for WM_DELETE_WINDOW (close button).
                const cm = &ev.xclient;
                if (@as(x11.Atom, @intCast(cm.data.l[0])) == self.wm_delete_window) {
                    self.should_close = true;
                }
            },

            x11.FocusIn => {
                self.focused = true;
            },

            x11.FocusOut => {
                self.focused = false;
            },

            // Key press / release are translated and could be forwarded to
            // the terminal PTY or matched against app keybindings. For now
            // we store the last key event so the tick callback can retrieve
            // it. A more complete implementation would use an event queue.
            x11.KeyPress, x11.KeyRelease => {
                // Key translation is handled by translateKeyEvent (below),
                // and can be called by the tick callback via pollKeyEvent.
                // We simply mark that a redraw may be needed.
                self.needs_redraw = true;
            },

            // Button and motion events -- stored for future mouse support.
            x11.ButtonPress, x11.ButtonRelease, x11.MotionNotify => {},

            else => {},
        }
    }

    // ---------------------------------------------------------------------------
    // Public: poll for key events (non-blocking)
    // ---------------------------------------------------------------------------

    /// Drain the next key press event from the X11 queue and translate it
    /// to a Termania `KeyEvent`.  Returns `null` if no key event is pending.
    ///
    /// Also writes the text input bytes (from XLookupString) into `text_buf`
    /// and returns the count via `text_len`.
    pub fn pollKeyEvent(self: *X11Window, text_buf: *[32]u8, text_len: *usize) ?input.KeyEvent {
        while (x11.XPending(self.display) > 0) {
            var ev: x11.XEvent = undefined;
            _ = x11.XNextEvent(self.display, &ev);

            // Handle non-key events internally.
            if (ev.type != x11.KeyPress) {
                self.handleEvent(&ev);
                continue;
            }

            // Translate the key event.
            const xkey = &ev.xkey;
            var lookup_buf: [32]u8 = undefined;
            var keysym: x11.KeySym = x11.NoSymbol;
            const count = x11.XLookupString(xkey, &lookup_buf, lookup_buf.len, &keysym, null);

            // Copy text bytes.
            const copy_len: usize = @intCast(@min(count, 32));
            @memcpy(text_buf[0..copy_len], lookup_buf[0..copy_len]);
            text_len.* = copy_len;

            return translateKeyEvent(keysym, xkey.state);
        }

        text_len.* = 0;
        return null;
    }

    // ---------------------------------------------------------------------------
    // Accessors
    // ---------------------------------------------------------------------------

    pub fn getWidth(self: *const X11Window) u32 {
        return self.width;
    }

    pub fn getHeight(self: *const X11Window) u32 {
        return self.height;
    }

    pub fn isFocused(self: *const X11Window) bool {
        return self.focused;
    }

    pub fn shouldClose(self: *const X11Window) bool {
        return self.should_close;
    }

    pub fn requestClose(self: *X11Window) void {
        self.should_close = true;
    }

    pub fn needsRedraw(self: *const X11Window) bool {
        return self.needs_redraw;
    }

    pub fn clearRedrawFlag(self: *X11Window) void {
        self.needs_redraw = false;
    }
};

// ---------------------------------------------------------------------------
// Key translation: X11 KeySym + modifier state --> input.KeyEvent
// ---------------------------------------------------------------------------

/// Translate an X11 KeySym and modifier bitmask into the Termania
/// `input.KeyEvent` used by the rest of the application.
///
/// Returns `null` for KeySyms that do not map to any KeyCode (e.g. bare
/// modifier keys, multimedia keys).
pub fn translateKeyEvent(keysym: x11.KeySym, state: c_uint) ?input.KeyEvent {
    const keycode = keysymToKeyCode(keysym) orelse return null;
    const mods = extractModifiers(state);
    return input.KeyEvent{ .key = keycode, .mods = mods };
}

/// Extract Termania modifier flags from the X11 modifier bitmask.
pub fn extractModifiers(state: c_uint) input.Modifiers {
    return .{
        .shift = (state & x11.ShiftMask) != 0,
        .ctrl = (state & x11.ControlMask) != 0,
        .alt = (state & x11.Mod1Mask) != 0,
        .super = (state & x11.Mod4Mask) != 0,
    };
}

/// Map an X11 KeySym to the Termania `input.KeyCode` enum.
///
/// This covers the ASCII printable range, navigation keys, and function keys.
/// Returns `null` for unmapped keysyms.
pub fn keysymToKeyCode(keysym: x11.KeySym) ?input.KeyCode {
    return switch (keysym) {
        // --- Letters (lowercase and uppercase map to the same KeyCode) ---
        x11.XK_a, x11.XK_A => .a,
        x11.XK_b, x11.XK_B => .b,
        x11.XK_c, x11.XK_C => .c,
        x11.XK_d, x11.XK_D => .d,
        x11.XK_e, x11.XK_E => .e,
        x11.XK_f, x11.XK_F => .f,
        x11.XK_g, x11.XK_G => .g,
        x11.XK_h, x11.XK_H => .h,
        x11.XK_i, x11.XK_I => .i,
        x11.XK_j, x11.XK_J => .j,
        x11.XK_k, x11.XK_K => .k,
        x11.XK_l, x11.XK_L => .l,
        x11.XK_m, x11.XK_M => .m,
        x11.XK_n, x11.XK_N => .n,
        x11.XK_o, x11.XK_O => .o,
        x11.XK_p, x11.XK_P => .p,
        x11.XK_q, x11.XK_Q => .q,
        x11.XK_r, x11.XK_R => .r,
        x11.XK_s, x11.XK_S => .s,
        x11.XK_t, x11.XK_T => .t,
        x11.XK_u, x11.XK_U => .u,
        x11.XK_v, x11.XK_V => .v,
        x11.XK_w, x11.XK_W => .w,
        x11.XK_x, x11.XK_X => .x,
        x11.XK_y, x11.XK_Y => .y,
        x11.XK_z, x11.XK_Z => .z,

        // --- Digits ---
        x11.XK_0 => .@"0",
        x11.XK_1 => .@"1",
        x11.XK_2 => .@"2",
        x11.XK_3 => .@"3",
        x11.XK_4 => .@"4",
        x11.XK_5 => .@"5",
        x11.XK_6 => .@"6",
        x11.XK_7 => .@"7",
        x11.XK_8 => .@"8",
        x11.XK_9 => .@"9",

        // --- Function keys ---
        x11.XK_F1 => .f1,
        x11.XK_F2 => .f2,
        x11.XK_F3 => .f3,
        x11.XK_F4 => .f4,
        x11.XK_F5 => .f5,
        x11.XK_F6 => .f6,
        x11.XK_F7 => .f7,
        x11.XK_F8 => .f8,
        x11.XK_F9 => .f9,
        x11.XK_F10 => .f10,
        x11.XK_F11 => .f11,
        x11.XK_F12 => .f12,

        // --- Navigation ---
        x11.XK_Up => .up,
        x11.XK_Down => .down,
        x11.XK_Left => .left,
        x11.XK_Right => .right,
        x11.XK_Home => .home,
        x11.XK_End => .end,
        x11.XK_Page_Up => .page_up,
        x11.XK_Page_Down => .page_down,
        x11.XK_Insert => .insert,
        x11.XK_Delete => .delete,

        // --- Whitespace / control ---
        x11.XK_Return, x11.XK_KP_Enter => .enter,
        x11.XK_Tab, x11.XK_ISO_Left_Tab => .tab,
        x11.XK_Escape => .escape,
        x11.XK_BackSpace => .backspace,
        x11.XK_space => .space,

        // --- Punctuation / symbols ---
        x11.XK_minus, x11.XK_underscore => .minus,
        x11.XK_equal, x11.XK_plus => .equal,
        x11.XK_bracketleft, x11.XK_braceleft => .left_bracket,
        x11.XK_bracketright, x11.XK_braceright => .right_bracket,
        x11.XK_backslash, x11.XK_bar => .backslash,
        x11.XK_semicolon, x11.XK_colon => .semicolon,
        x11.XK_apostrophe, x11.XK_quotedbl => .apostrophe,
        x11.XK_grave, x11.XK_asciitilde => .grave,
        x11.XK_comma, x11.XK_less => .comma,
        x11.XK_period, x11.XK_greater => .period,
        x11.XK_slash, x11.XK_question => .slash,

        else => null,
    };
}

// ---------------------------------------------------------------------------
// Tests
//
// These tests verify key mapping and modifier extraction without requiring
// an actual X11 display connection.
// ---------------------------------------------------------------------------

test "keysymToKeyCode maps lowercase letters" {
    try testing.expectEqual(input.KeyCode.a, keysymToKeyCode(x11.XK_a).?);
    try testing.expectEqual(input.KeyCode.z, keysymToKeyCode(x11.XK_z).?);
    try testing.expectEqual(input.KeyCode.m, keysymToKeyCode(x11.XK_m).?);
}

test "keysymToKeyCode maps uppercase letters to same KeyCode" {
    try testing.expectEqual(input.KeyCode.a, keysymToKeyCode(x11.XK_A).?);
    try testing.expectEqual(input.KeyCode.z, keysymToKeyCode(x11.XK_Z).?);
}

test "keysymToKeyCode maps digits" {
    try testing.expectEqual(input.KeyCode.@"0", keysymToKeyCode(x11.XK_0).?);
    try testing.expectEqual(input.KeyCode.@"5", keysymToKeyCode(x11.XK_5).?);
    try testing.expectEqual(input.KeyCode.@"9", keysymToKeyCode(x11.XK_9).?);
}

test "keysymToKeyCode maps function keys" {
    try testing.expectEqual(input.KeyCode.f1, keysymToKeyCode(x11.XK_F1).?);
    try testing.expectEqual(input.KeyCode.f12, keysymToKeyCode(x11.XK_F12).?);
}

test "keysymToKeyCode maps navigation keys" {
    try testing.expectEqual(input.KeyCode.up, keysymToKeyCode(x11.XK_Up).?);
    try testing.expectEqual(input.KeyCode.down, keysymToKeyCode(x11.XK_Down).?);
    try testing.expectEqual(input.KeyCode.left, keysymToKeyCode(x11.XK_Left).?);
    try testing.expectEqual(input.KeyCode.right, keysymToKeyCode(x11.XK_Right).?);
    try testing.expectEqual(input.KeyCode.home, keysymToKeyCode(x11.XK_Home).?);
    try testing.expectEqual(input.KeyCode.end, keysymToKeyCode(x11.XK_End).?);
    try testing.expectEqual(input.KeyCode.page_up, keysymToKeyCode(x11.XK_Page_Up).?);
    try testing.expectEqual(input.KeyCode.page_down, keysymToKeyCode(x11.XK_Page_Down).?);
    try testing.expectEqual(input.KeyCode.insert, keysymToKeyCode(x11.XK_Insert).?);
    try testing.expectEqual(input.KeyCode.delete, keysymToKeyCode(x11.XK_Delete).?);
}

test "keysymToKeyCode maps whitespace and control keys" {
    try testing.expectEqual(input.KeyCode.enter, keysymToKeyCode(x11.XK_Return).?);
    try testing.expectEqual(input.KeyCode.tab, keysymToKeyCode(x11.XK_Tab).?);
    try testing.expectEqual(input.KeyCode.escape, keysymToKeyCode(x11.XK_Escape).?);
    try testing.expectEqual(input.KeyCode.backspace, keysymToKeyCode(x11.XK_BackSpace).?);
    try testing.expectEqual(input.KeyCode.space, keysymToKeyCode(x11.XK_space).?);
}

test "keysymToKeyCode maps punctuation" {
    try testing.expectEqual(input.KeyCode.minus, keysymToKeyCode(x11.XK_minus).?);
    try testing.expectEqual(input.KeyCode.equal, keysymToKeyCode(x11.XK_equal).?);
    try testing.expectEqual(input.KeyCode.left_bracket, keysymToKeyCode(x11.XK_bracketleft).?);
    try testing.expectEqual(input.KeyCode.right_bracket, keysymToKeyCode(x11.XK_bracketright).?);
    try testing.expectEqual(input.KeyCode.backslash, keysymToKeyCode(x11.XK_backslash).?);
    try testing.expectEqual(input.KeyCode.semicolon, keysymToKeyCode(x11.XK_semicolon).?);
    try testing.expectEqual(input.KeyCode.apostrophe, keysymToKeyCode(x11.XK_apostrophe).?);
    try testing.expectEqual(input.KeyCode.grave, keysymToKeyCode(x11.XK_grave).?);
    try testing.expectEqual(input.KeyCode.comma, keysymToKeyCode(x11.XK_comma).?);
    try testing.expectEqual(input.KeyCode.period, keysymToKeyCode(x11.XK_period).?);
    try testing.expectEqual(input.KeyCode.slash, keysymToKeyCode(x11.XK_slash).?);
}

test "keysymToKeyCode maps shifted punctuation to base key" {
    // Shifted symbols should map to the same KeyCode as the base key
    // because the Modifiers struct carries the shift flag separately.
    try testing.expectEqual(input.KeyCode.minus, keysymToKeyCode(x11.XK_underscore).?);
    try testing.expectEqual(input.KeyCode.equal, keysymToKeyCode(x11.XK_plus).?);
    try testing.expectEqual(input.KeyCode.left_bracket, keysymToKeyCode(x11.XK_braceleft).?);
    try testing.expectEqual(input.KeyCode.right_bracket, keysymToKeyCode(x11.XK_braceright).?);
    try testing.expectEqual(input.KeyCode.backslash, keysymToKeyCode(x11.XK_bar).?);
    try testing.expectEqual(input.KeyCode.semicolon, keysymToKeyCode(x11.XK_colon).?);
    try testing.expectEqual(input.KeyCode.apostrophe, keysymToKeyCode(x11.XK_quotedbl).?);
    try testing.expectEqual(input.KeyCode.grave, keysymToKeyCode(x11.XK_asciitilde).?);
    try testing.expectEqual(input.KeyCode.comma, keysymToKeyCode(x11.XK_less).?);
    try testing.expectEqual(input.KeyCode.period, keysymToKeyCode(x11.XK_greater).?);
    try testing.expectEqual(input.KeyCode.slash, keysymToKeyCode(x11.XK_question).?);
}

test "keysymToKeyCode returns null for unknown keysym" {
    try testing.expect(keysymToKeyCode(0xdeadbeef) == null);
    // Bare modifier keys should not produce a KeyCode.
    try testing.expect(keysymToKeyCode(x11.XK_Shift_L) == null);
    try testing.expect(keysymToKeyCode(x11.XK_Control_L) == null);
    try testing.expect(keysymToKeyCode(x11.XK_Alt_L) == null);
}

test "extractModifiers no modifiers" {
    const mods = extractModifiers(0);
    try testing.expect(!mods.shift);
    try testing.expect(!mods.ctrl);
    try testing.expect(!mods.alt);
    try testing.expect(!mods.super);
}

test "extractModifiers shift only" {
    const mods = extractModifiers(x11.ShiftMask);
    try testing.expect(mods.shift);
    try testing.expect(!mods.ctrl);
    try testing.expect(!mods.alt);
    try testing.expect(!mods.super);
}

test "extractModifiers ctrl only" {
    const mods = extractModifiers(x11.ControlMask);
    try testing.expect(!mods.shift);
    try testing.expect(mods.ctrl);
    try testing.expect(!mods.alt);
    try testing.expect(!mods.super);
}

test "extractModifiers alt only" {
    const mods = extractModifiers(x11.Mod1Mask);
    try testing.expect(!mods.shift);
    try testing.expect(!mods.ctrl);
    try testing.expect(mods.alt);
    try testing.expect(!mods.super);
}

test "extractModifiers super only" {
    const mods = extractModifiers(x11.Mod4Mask);
    try testing.expect(!mods.shift);
    try testing.expect(!mods.ctrl);
    try testing.expect(!mods.alt);
    try testing.expect(mods.super);
}

test "extractModifiers ctrl+shift+alt" {
    const mods = extractModifiers(x11.ControlMask | x11.ShiftMask | x11.Mod1Mask);
    try testing.expect(mods.shift);
    try testing.expect(mods.ctrl);
    try testing.expect(mods.alt);
    try testing.expect(!mods.super);
}

test "translateKeyEvent produces correct KeyEvent" {
    const ev = translateKeyEvent(x11.XK_a, x11.ControlMask).?;
    try testing.expectEqual(input.KeyCode.a, ev.key);
    try testing.expect(ev.mods.ctrl);
    try testing.expect(!ev.mods.shift);
    try testing.expect(!ev.mods.alt);
    try testing.expect(!ev.mods.super);
}

test "translateKeyEvent returns null for unknown keysym" {
    try testing.expect(translateKeyEvent(x11.XK_Shift_L, 0) == null);
}

test "translateKeyEvent ctrl+shift+n matches app keybinding" {
    const ev = translateKeyEvent(x11.XK_n, x11.ControlMask | x11.ShiftMask).?;
    // Should produce the same event that triggers new_pane
    try testing.expectEqual(input.KeyCode.n, ev.key);
    try testing.expect(ev.mods.ctrl);
    try testing.expect(ev.mods.shift);

    // Verify it actually triggers the app keybinding
    const action = input.handleAppKeybinding(ev);
    try testing.expectEqual(input.AppAction.new_pane, action.?);
}

test "translateKeyEvent enter key" {
    const ev = translateKeyEvent(x11.XK_Return, 0).?;
    try testing.expectEqual(input.KeyCode.enter, ev.key);
    try testing.expect(ev.mods.eql(input.Modifiers.none));
}

test "translateKeyEvent F5 with ctrl" {
    const ev = translateKeyEvent(x11.XK_F5, x11.ControlMask).?;
    try testing.expectEqual(input.KeyCode.f5, ev.key);
    try testing.expect(ev.mods.ctrl);
}

test "keysymToKeyCode maps KP_Enter to enter" {
    try testing.expectEqual(input.KeyCode.enter, keysymToKeyCode(x11.XK_KP_Enter).?);
}

test "keysymToKeyCode maps ISO_Left_Tab to tab" {
    try testing.expectEqual(input.KeyCode.tab, keysymToKeyCode(x11.XK_ISO_Left_Tab).?);
}
