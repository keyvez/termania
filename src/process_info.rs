use std::process::Command;

/// Information about a child process
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub command: String,
}

/// Get child processes of a given PID using `pgrep -P`
pub fn get_child_processes(parent_pid: u32) -> Vec<ProcessInfo> {
    let output = Command::new("pgrep")
        .args(["-P", &parent_pid.to_string()])
        .output();

    let pids: Vec<u32> = match output {
        Ok(out) => {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        }
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();
    for pid in pids {
        let cmd = get_process_command(pid).unwrap_or_else(|| format!("(pid {})", pid));
        result.push(ProcessInfo { pid, command: cmd });
        // Recurse into grandchildren
        result.extend(get_child_processes(pid));
    }
    result
}

/// Get the command name for a PID using `ps`
fn get_process_command(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if cmd.is_empty() { None } else { Some(cmd) }
}

/// Get listening ports for a PID using `lsof`
pub fn get_listening_ports(pid: u32) -> Vec<u16> {
    let output = Command::new("lsof")
        .args(["-iTCP", "-sTCP:LISTEN", "-P", "-n", "-p", &pid.to_string()])
        .output();

    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .skip(1) // skip header
                .filter_map(|line| {
                    // lsof output has port after the last ':'
                    let name_field = line.split_whitespace().nth(8)?;
                    let port_str = name_field.rsplit(':').next()?;
                    port_str.parse().ok()
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

/// Build a human-readable subprocess context string for a shell PID
pub fn build_process_context(shell_pid: u32) -> String {
    let children = get_child_processes(shell_pid);
    if children.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    for child in &children {
        let ports = get_listening_ports(child.pid);
        if ports.is_empty() {
            lines.push(format!("  pid={} cmd={}", child.pid, child.command));
        } else {
            let port_str: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
            lines.push(format!(
                "  pid={} cmd={} ports=[{}]",
                child.pid,
                child.command,
                port_str.join(", ")
            ));
        }
    }

    format!("Child processes:\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_process_context_returns_string() {
        // Use PID 1 (launchd) which always exists on macOS
        let result = build_process_context(1);
        // Should return something (launchd has children) or empty string
        assert!(result.is_empty() || result.contains("Child processes:"));
    }

    #[test]
    fn test_get_child_processes_nonexistent_pid() {
        let result = get_child_processes(999999999);
        assert!(result.is_empty());
    }

    #[test]
    fn test_get_listening_ports_nonexistent_pid() {
        // lsof might error or return nothing for a nonexistent PID
        let result = get_listening_ports(999999999);
        // Just ensure it doesn't panic — result may or may not be empty
        let _ = result;
    }

    #[test]
    fn test_build_process_context_no_children() {
        // Very high PID unlikely to exist
        let result = build_process_context(999999999);
        assert!(result.is_empty());
    }
}
