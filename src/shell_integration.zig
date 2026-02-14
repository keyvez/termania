// Shell integration scripts for Termania terminal emulator.
//
// These scripts emit OSC 133 (FinalTerm) semantic prompt sequences so the
// terminal can identify prompt, input, and output regions.  They are stored
// as comptime string constants and can be injected into the respective
// shell's startup environment.

const std = @import("std");
const testing = std.testing;

// ---------------------------------------------------------------------------
// Bash integration  (uses PROMPT_COMMAND + DEBUG trap)
// ---------------------------------------------------------------------------

pub const bash_integration =
    \\# Termania shell integration for Bash
    \\# Emits OSC 133 semantic prompt markers.
    \\
    \\__termania_prompt_start() { printf '\033]133;A\007'; }
    \\__termania_prompt_end()   { printf '\033]133;B\007'; }
    \\__termania_preexec()      { printf '\033]133;C\007'; }
    \\__termania_precmd()       { printf '\033]133;D;%s\007' "$?"; }
    \\
    \\# Install hooks -------------------------------------------------------
    \\if [[ ! "${PROMPT_COMMAND[*]}" == *__termania_precmd* ]]; then
    \\  PROMPT_COMMAND=("__termania_precmd" "${PROMPT_COMMAND[@]}")
    \\fi
    \\if [[ ! "${PROMPT_COMMAND[*]}" == *__termania_prompt_start* ]]; then
    \\  PROMPT_COMMAND+=("__termania_prompt_start")
    \\fi
    \\
    \\# PS1 wrapper: emit prompt-end (B) right before the user types.
    \\if [[ "$PS1" != *'__termania_prompt_end'* ]]; then
    \\  PS1="$PS1\[$(__termania_prompt_end)\]"
    \\fi
    \\
    \\# DEBUG trap fires just before each command — mark command-executed (C).
    \\trap '__termania_preexec' DEBUG
;

// ---------------------------------------------------------------------------
// Zsh integration  (uses precmd / preexec hooks)
// ---------------------------------------------------------------------------

pub const zsh_integration =
    \\# Termania shell integration for Zsh
    \\# Emits OSC 133 semantic prompt markers.
    \\
    \\__termania_prompt_start() { printf '\033]133;A\007'; }
    \\__termania_prompt_end()   { printf '\033]133;B\007'; }
    \\__termania_preexec()      { printf '\033]133;C\007'; }
    \\__termania_precmd()       { printf '\033]133;D;%s\007' "$?"; __termania_prompt_start; }
    \\
    \\# Install hooks -------------------------------------------------------
    \\autoload -Uz add-zsh-hook
    \\add-zsh-hook precmd  __termania_precmd
    \\add-zsh-hook preexec __termania_preexec
    \\
    \\# Emit prompt-end marker via PROMPT escape.
    \\PROMPT="${PROMPT}%{$(__termania_prompt_end)%}"
;

// ---------------------------------------------------------------------------
// Fish integration  (uses fish event functions)
// ---------------------------------------------------------------------------

pub const fish_integration =
    \\# Termania shell integration for Fish
    \\# Emits OSC 133 semantic prompt markers.
    \\
    \\function __termania_prompt_start --on-event fish_prompt
    \\    printf '\033]133;A\007'
    \\end
    \\
    \\function __termania_prompt_end --on-event fish_prompt
    \\    printf '\033]133;B\007'
    \\end
    \\
    \\function __termania_preexec --on-event fish_preexec
    \\    printf '\033]133;C\007'
    \\end
    \\
    \\function __termania_postexec --on-event fish_postexec
    \\    printf '\033]133;D;%d\007' $status
    \\end
;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test "bash integration contains OSC 133 markers" {
    try testing.expect(std.mem.indexOf(u8, bash_integration, "133;A") != null);
    try testing.expect(std.mem.indexOf(u8, bash_integration, "133;B") != null);
    try testing.expect(std.mem.indexOf(u8, bash_integration, "133;C") != null);
    try testing.expect(std.mem.indexOf(u8, bash_integration, "133;D") != null);
}

test "zsh integration contains OSC 133 markers" {
    try testing.expect(std.mem.indexOf(u8, zsh_integration, "133;A") != null);
    try testing.expect(std.mem.indexOf(u8, zsh_integration, "133;B") != null);
    try testing.expect(std.mem.indexOf(u8, zsh_integration, "133;C") != null);
    try testing.expect(std.mem.indexOf(u8, zsh_integration, "133;D") != null);
}

test "fish integration contains OSC 133 markers" {
    try testing.expect(std.mem.indexOf(u8, fish_integration, "133;A") != null);
    try testing.expect(std.mem.indexOf(u8, fish_integration, "133;B") != null);
    try testing.expect(std.mem.indexOf(u8, fish_integration, "133;C") != null);
    try testing.expect(std.mem.indexOf(u8, fish_integration, "133;D") != null);
}
