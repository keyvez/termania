const std = @import("std");
const posix = std.posix;
const testing = std.testing;
const linux = std.os.linux;

// We use extern declarations for PTY functions not in Zig's std.
extern "c" fn grantpt(fd: c_int) c_int;
extern "c" fn unlockpt(fd: c_int) c_int;
extern "c" fn ptsname(fd: c_int) ?[*:0]const u8;
extern "c" fn setsid() c_int;
extern "c" fn fork() c_int;
extern "c" fn dup2(oldfd: c_int, newfd: c_int) c_int;
extern "c" fn execvp(file: [*:0]const u8, argv: [*:null]const ?[*:0]const u8) c_int;
extern "c" fn chdir(path: [*:0]const u8) c_int;
extern "c" fn putenv(string: [*:0]u8) c_int;
extern "c" fn getenv(name: [*:0]const u8) ?[*:0]const u8;
extern "c" fn waitpid(pid: c_int, status: *c_int, options: c_int) c_int;

const TIOCSWINSZ = 0x5414;
const TIOCSCTTY = 0x540E;
const WNOHANG = 1;

const winsize = extern struct {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
};

/// Manages a pseudo-terminal (PTY) for a child process.
/// Following Ghostty's pattern of per-surface PTY management.
pub const Pty = struct {
    master_fd: posix.fd_t,
    child_pid: c_int,

    /// Spawn a new PTY with the given dimensions, running a shell.
    pub fn spawn(
        cols: u16,
        rows: u16,
        shell: ?[]const u8,
        cwd: ?[]const u8,
    ) !Pty {
        // Open /dev/ptmx directly (equivalent to posix_openpt)
        const master_fd = posix.open(
            "/dev/ptmx",
            .{ .ACCMODE = .RDWR, .NOCTTY = true },
            0,
        ) catch return error.OpenPtyFailed;

        if (grantpt(master_fd) != 0) {
            posix.close(master_fd);
            return error.GrantPtyFailed;
        }
        if (unlockpt(master_fd) != 0) {
            posix.close(master_fd);
            return error.UnlockPtyFailed;
        }

        // Get slave name
        const slave_name = ptsname(master_fd);
        if (slave_name == null) {
            posix.close(master_fd);
            return error.PtsnameFailed;
        }

        // Set window size
        var ws = winsize{
            .ws_row = rows,
            .ws_col = cols,
            .ws_xpixel = 0,
            .ws_ypixel = 0,
        };
        _ = linux.ioctl(master_fd, TIOCSWINSZ, @intFromPtr(&ws));

        // Fork
        const pid = fork();
        if (pid < 0) {
            posix.close(master_fd);
            return error.ForkFailed;
        }

        if (pid == 0) {
            // ---- Child process ----
            _ = setsid();

            // Open slave PTY
            const slave_fd_result = posix.open(
                std.mem.span(slave_name.?),
                .{ .ACCMODE = .RDWR },
                0,
            ) catch {
                posix.exit(1);
            };

            // Set as controlling terminal
            _ = linux.ioctl(slave_fd_result, TIOCSCTTY, 0);
            _ = linux.ioctl(slave_fd_result, TIOCSWINSZ, @intFromPtr(&ws));

            // Redirect stdio
            _ = dup2(@intCast(slave_fd_result), 0);
            _ = dup2(@intCast(slave_fd_result), 1);
            _ = dup2(@intCast(slave_fd_result), 2);
            if (slave_fd_result > 2) posix.close(slave_fd_result);
            posix.close(master_fd);

            // Change directory
            if (cwd) |dir| {
                var dir_buf: [4096]u8 = undefined;
                const len = @min(dir.len, dir_buf.len - 1);
                @memcpy(dir_buf[0..len], dir[0..len]);
                dir_buf[len] = 0;
                _ = chdir(@ptrCast(dir_buf[0..len :0]));
            }

            // Set TERM environment variable
            _ = putenv(@constCast(@as([*:0]u8, @ptrCast(@constCast("TERM=xterm-256color")))));
            _ = putenv(@constCast(@as([*:0]u8, @ptrCast(@constCast("COLORTERM=truecolor")))));

            // Determine shell
            var shell_buf: [256]u8 = undefined;
            var shell_path: [*:0]const u8 = "/bin/bash";

            if (shell) |s| {
                const slen = @min(s.len, shell_buf.len - 1);
                @memcpy(shell_buf[0..slen], s[0..slen]);
                shell_buf[slen] = 0;
                shell_path = @ptrCast(shell_buf[0..slen :0]);
            } else {
                const env_shell = getenv("SHELL");
                if (env_shell) |es| shell_path = es;
            }

            const args = [_:null]?[*:0]const u8{shell_path};
            _ = execvp(shell_path, &args);
            posix.exit(1);
        }

        // ---- Parent process ----
        // Set master to non-blocking (O_NONBLOCK = 0x800 on Linux x86_64)
        const O_NONBLOCK: u32 = 0x800;
        const current_flags = linux.fcntl(master_fd, linux.F.GETFL, @as(u32, 0));
        _ = linux.fcntl(master_fd, linux.F.SETFL, current_flags | O_NONBLOCK);

        return .{
            .master_fd = master_fd,
            .child_pid = pid,
        };
    }

    /// Write data to the PTY master (sends to child's stdin).
    pub fn write(self: *const Pty, data: []const u8) !usize {
        const result = linux.write(self.master_fd, data.ptr, data.len);
        const signed: isize = @bitCast(result);
        if (signed < 0) return error.WriteFailed;
        return @intCast(result);
    }

    /// Read data from the PTY master (receives child's stdout).
    /// Returns the number of bytes read, or null if nothing available.
    pub fn read(self: *const Pty, buf: []u8) ?usize {
        const result = linux.read(self.master_fd, buf.ptr, buf.len);
        const signed: isize = @bitCast(result);
        if (signed <= 0) return null;
        return @intCast(result);
    }

    /// Check if the child process is still alive.
    pub fn isAlive(self: *const Pty) bool {
        var status: c_int = 0;
        const result = waitpid(self.child_pid, &status, WNOHANG);
        return result == 0;
    }

    /// Get the child process PID.
    pub fn childPid(self: *const Pty) u32 {
        return @intCast(self.child_pid);
    }

    /// Resize the PTY.
    pub fn doResize(self: *const Pty, cols_new: u16, rows_new: u16) void {
        var ws = winsize{
            .ws_row = rows_new,
            .ws_col = cols_new,
            .ws_xpixel = 0,
            .ws_ypixel = 0,
        };
        _ = linux.ioctl(self.master_fd, TIOCSWINSZ, @intFromPtr(&ws));
    }

    /// Close the PTY master fd.
    pub fn close(self: *Pty) void {
        posix.close(self.master_fd);
        self.master_fd = -1;
    }
};

test "pty types compile" {
    // Basic compile-time check — actually spawning a PTY requires a real system
    _ = Pty{
        .master_fd = -1,
        .child_pid = -1,
    };
}
