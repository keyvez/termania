use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::llm::TermaniaAction;

/// Text Tap Server - allows external processes to subscribe to terminal output
/// and send commands to panes.
///
/// Protocol (newline-delimited JSON):
///   Client -> Server:
///     {"subscribe": <pane_index>}     Subscribe to a pane's output
///     {"subscribe": "all"}            Subscribe to all panes
///     {"unsubscribe": <pane_index>}   Unsubscribe from a pane
///     {"list": true}                  List available panes
///     {"read": <pane_index>}          Read current screen content once
///     {"send": <pane_index>, "input": "<text>"}  Send input to a pane
///     {"send": "all", "input": "<text>"}         Send input to all panes
///     {"action": {<TermaniaAction>}}             Execute a TermaniaAction
///
///   Server -> Client:
///     {"pane": <index>, "content": "<text>"}  Screen content update
///     {"panes": <count>}                       Response to list
///     {"screen": "<text>"}                     Response to read
///     {"ok": true}                             Ack for send/action
pub struct TextTapServer {
    socket_path: String,
    clients: Arc<Mutex<Vec<TapClient>>>,
    running: Arc<Mutex<bool>>,
    /// Pending commands from tap clients to be executed on panes
    pending_commands: Arc<Mutex<Vec<TapCommand>>>,
    /// Pane count for list responses
    pane_count: Arc<Mutex<usize>>,
    /// Last broadcast per pane — used for throttling
    last_broadcast: Arc<Mutex<std::collections::HashMap<usize, (Instant, String)>>>,
}

struct TapClient {
    stream: UnixStream,
    subscriptions: Vec<TapSubscription>,
    id: u64,
}

/// Command from a tap client — either a legacy send or a full TermaniaAction
pub enum TapCommand {
    /// Legacy: send raw input to a target
    Send {
        target: TapTarget,
        input: String,
    },
    /// Full TermaniaAction (from {"action": {...}} protocol)
    Action(TermaniaAction),
}

#[derive(Clone)]
pub enum TapTarget {
    Pane(usize),
    All,
}

#[derive(Clone)]
enum TapSubscription {
    Pane(usize),
    All,
}

impl TextTapServer {
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            clients: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(Mutex::new(false)),
            pending_commands: Arc::new(Mutex::new(Vec::new())),
            pane_count: Arc::new(Mutex::new(0)),
            last_broadcast: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn set_pane_count(&self, count: usize) {
        *self.pane_count.lock().unwrap() = count;
    }

    /// Drain pending commands from tap clients
    pub fn drain_commands(&self) -> Vec<TapCommand> {
        let mut cmds = self.pending_commands.lock().unwrap();
        std::mem::take(&mut *cmds)
    }

    pub fn start(&self) {
        let mut running = self.running.lock().unwrap();
        if *running {
            return;
        }
        *running = true;
        drop(running);

        // Remove old socket if it exists
        let _ = std::fs::remove_file(&self.socket_path);

        let socket_path = self.socket_path.clone();
        let clients = self.clients.clone();
        let running = self.running.clone();
        let pending_commands = self.pending_commands.clone();
        let pane_count = self.pane_count.clone();

        thread::spawn(move || {
            let listener = match UnixListener::bind(&socket_path) {
                Ok(l) => l,
                Err(e) => {
                    log::error!("Failed to bind text tap socket at {}: {}", socket_path, e);
                    return;
                }
            };

            log::info!("Text tap server listening on {}", socket_path);

            listener
                .set_nonblocking(true)
                .expect("Failed to set non-blocking");

            let mut next_client_id: u64 = 0;

            while *running.lock().unwrap() {
                match listener.accept() {
                    Ok((stream, _)) => {
                        log::info!("Text tap client connected (id={})", next_client_id);
                        let client_id = next_client_id;
                        next_client_id += 1;

                        let reader_stream = stream.try_clone().unwrap();
                        let clients_clone = clients.clone();
                        let pending_clone = pending_commands.clone();
                        let pane_count_clone = pane_count.clone();

                        // Register the client
                        {
                            let mut cls = clients.lock().unwrap();
                            cls.push(TapClient {
                                stream,
                                subscriptions: vec![TapSubscription::All],
                                id: client_id,
                            });
                        }

                        // Handle client reads in a separate thread
                        thread::spawn(move || {
                            Self::handle_client(
                                reader_stream,
                                client_id,
                                clients_clone,
                                pending_clone,
                                pane_count_clone,
                            );
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(e) => {
                        log::error!("Text tap accept error: {}", e);
                    }
                }
            }

            let _ = std::fs::remove_file(&socket_path);
        });
    }

    fn handle_client(
        mut stream: UnixStream,
        client_id: u64,
        clients: Arc<Mutex<Vec<TapClient>>>,
        pending_commands: Arc<Mutex<Vec<TapCommand>>>,
        pane_count: Arc<Mutex<usize>>,
    ) {
        let mut buf = [0u8; 4096];
        let mut leftover = String::new();
        stream.set_nonblocking(false).ok();

        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    log::info!("Text tap client {} disconnected", client_id);
                    break;
                }
                Ok(n) => {
                    if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                        leftover.push_str(text);

                        // Process complete lines
                        while let Some(nl_pos) = leftover.find('\n') {
                            let line = leftover[..nl_pos].trim().to_string();
                            leftover = leftover[nl_pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            Self::process_command(
                                &line,
                                client_id,
                                &clients,
                                &pending_commands,
                                &pane_count,
                                &mut stream,
                            );
                        }
                    }
                }
                Err(_) => break,
            }
        }

        // Remove disconnected client
        let mut cls = clients.lock().unwrap();
        cls.retain(|c| c.id != client_id);
    }

    fn process_command(
        line: &str,
        client_id: u64,
        clients: &Arc<Mutex<Vec<TapClient>>>,
        pending_commands: &Arc<Mutex<Vec<TapCommand>>>,
        pane_count: &Arc<Mutex<usize>>,
        response_stream: &mut UnixStream,
    ) {
        // Try to parse as JSON for the "action" command first
        if line.contains("\"action\"") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(action_val) = json.get("action") {
                    if let Ok(action) = serde_json::from_value::<TermaniaAction>(action_val.clone()) {
                        pending_commands.lock().unwrap().push(TapCommand::Action(action));
                        let _ = response_stream.write_all(b"{\"ok\":true}\n");
                        return;
                    } else {
                        let _ = response_stream.write_all(b"{\"error\":\"invalid action format\"}\n");
                        return;
                    }
                }
            }
        }

        // Legacy command parsing
        if line.contains("\"list\"") {
            let count = *pane_count.lock().unwrap();
            let response = format!("{{\"panes\":{}}}\n", count);
            let _ = response_stream.write_all(response.as_bytes());
        } else if line.contains("\"subscribe\"") {
            let mut cls = clients.lock().unwrap();
            if let Some(client) = cls.iter_mut().find(|c| c.id == client_id) {
                if line.contains("\"all\"") {
                    client.subscriptions = vec![TapSubscription::All];
                } else if let Some(idx) = extract_number_after(line, "subscribe") {
                    // Replace subscriptions with specific pane
                    client.subscriptions.retain(|s| !matches!(s, TapSubscription::All));
                    client.subscriptions.push(TapSubscription::Pane(idx));
                }
            }
        } else if line.contains("\"unsubscribe\"") {
            let mut cls = clients.lock().unwrap();
            if let Some(client) = cls.iter_mut().find(|c| c.id == client_id) {
                if let Some(idx) = extract_number_after(line, "unsubscribe") {
                    client.subscriptions.retain(|s| !matches!(s, TapSubscription::Pane(i) if *i == idx));
                }
            }
        } else if line.contains("\"send\"") {
            // Extract input text between quotes after "input"
            if let Some(input) = extract_quoted_value(line, "input") {
                let target = if line.contains("\"all\"") {
                    TapTarget::All
                } else if let Some(idx) = extract_number_after(line, "send") {
                    TapTarget::Pane(idx)
                } else {
                    return;
                };

                pending_commands.lock().unwrap().push(TapCommand::Send {
                    target,
                    input,
                });

                let _ = response_stream.write_all(b"{\"ok\":true}\n");
            }
        }
    }

    /// Broadcast pane content to subscribed clients.
    /// Throttled: only sends if content changed or 250ms elapsed since last broadcast.
    pub fn broadcast(&self, pane_index: usize, content: &str) {
        // Throttle check
        {
            let mut last = self.last_broadcast.lock().unwrap();
            if let Some((last_time, last_content)) = last.get(&pane_index) {
                if last_content == content && last_time.elapsed().as_millis() < 250 {
                    return;
                }
            }
            last.insert(pane_index, (Instant::now(), content.to_string()));
        }

        let mut clients = self.clients.lock().unwrap();
        let msg = format!(
            "{{\"pane\":{},\"content\":{}}}\n",
            pane_index,
            json_escape_string(content)
        );

        clients.retain_mut(|client| {
            let should_send = client.subscriptions.iter().any(|s| match s {
                TapSubscription::All => true,
                TapSubscription::Pane(idx) => *idx == pane_index,
            });

            if should_send {
                match client.stream.write_all(msg.as_bytes()) {
                    Ok(_) => true,
                    Err(_) => {
                        log::info!("Removing disconnected text tap client {}", client.id);
                        false
                    }
                }
            } else {
                true
            }
        });
    }

    pub fn stop(&self) {
        let mut running = self.running.lock().unwrap();
        *running = false;
    }
}

/// Extract a number value after a given key in JSON-like text
fn extract_number_after(s: &str, key: &str) -> Option<usize> {
    let key_pattern = format!("\"{}\"", key);
    if let Some(pos) = s.find(&key_pattern) {
        let after = &s[pos + key_pattern.len()..];
        // Find the colon, then the number
        if let Some(colon_pos) = after.find(':') {
            let value_str = after[colon_pos + 1..].trim();
            // Extract digits
            let num_str: String = value_str.chars().take_while(|c| c.is_ascii_digit()).collect();
            return num_str.parse().ok();
        }
    }
    None
}

/// Extract a quoted string value after a given key in JSON-like text
fn extract_quoted_value(s: &str, key: &str) -> Option<String> {
    let key_pattern = format!("\"{}\"", key);
    if let Some(pos) = s.find(&key_pattern) {
        let after = &s[pos + key_pattern.len()..];
        if let Some(colon_pos) = after.find(':') {
            let value_str = after[colon_pos + 1..].trim();
            if value_str.starts_with('"') {
                // Find matching close quote (handling escapes)
                let inner = &value_str[1..];
                let mut result = String::new();
                let mut chars = inner.chars();
                while let Some(ch) = chars.next() {
                    if ch == '\\' {
                        if let Some(next) = chars.next() {
                            match next {
                                'n' => result.push('\n'),
                                'r' => result.push('\r'),
                                't' => result.push('\t'),
                                '"' => result.push('"'),
                                '\\' => result.push('\\'),
                                _ => {
                                    result.push('\\');
                                    result.push(next);
                                }
                            }
                        }
                    } else if ch == '"' {
                        return Some(result);
                    } else {
                        result.push(ch);
                    }
                }
            }
        }
    }
    None
}

/// JSON string escaping
fn json_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\x20' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
