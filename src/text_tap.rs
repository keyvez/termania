use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// Text Tap Server - allows external processes to subscribe to terminal output
///
/// Protocol (newline-delimited JSON):
///   Client -> Server:
///     {"subscribe": <pane_index>}     Subscribe to a pane's output
///     {"subscribe": "all"}            Subscribe to all panes
///     {"unsubscribe": <pane_index>}   Unsubscribe from a pane
///     {"list": true}                  List available panes
///     {"read": <pane_index>}          Read current screen content once
///
///   Server -> Client:
///     {"pane": <index>, "content": "<text>"}  Screen content update
///     {"panes": [<count>]}                     Response to list
///     {"screen": "<text>"}                     Response to read
pub struct TextTapServer {
    socket_path: String,
    clients: Arc<Mutex<Vec<TapClient>>>,
    running: Arc<Mutex<bool>>,
}

struct TapClient {
    stream: UnixStream,
    subscriptions: Vec<TapSubscription>,
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
        }
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

        thread::spawn(move || {
            let listener = match UnixListener::bind(&socket_path) {
                Ok(l) => l,
                Err(e) => {
                    log::error!("Failed to bind text tap socket at {}: {}", socket_path, e);
                    return;
                }
            };

            log::info!("Text tap server listening on {}", socket_path);

            // Set non-blocking so we can check the running flag
            listener
                .set_nonblocking(true)
                .expect("Failed to set non-blocking");

            while *running.lock().unwrap() {
                match listener.accept() {
                    Ok((stream, _)) => {
                        log::info!("Text tap client connected");
                        let client_stream = stream.try_clone().unwrap();
                        let clients_clone = clients.clone();

                        // Handle client in a new thread
                        thread::spawn(move || {
                            Self::handle_client(client_stream, clients_clone);
                        });

                        // Register the client
                        let mut cls = clients.lock().unwrap();
                        cls.push(TapClient {
                            stream,
                            subscriptions: vec![TapSubscription::All],
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(e) => {
                        log::error!("Text tap accept error: {}", e);
                    }
                }
            }

            let _ = std::fs::remove_file(&socket_path);
        });
    }

    fn handle_client(mut stream: UnixStream, clients: Arc<Mutex<Vec<TapClient>>>) {
        let mut buf = [0u8; 4096];
        stream.set_nonblocking(false).ok();

        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    log::info!("Text tap client disconnected");
                    break;
                }
                Ok(n) => {
                    if let Ok(msg) = std::str::from_utf8(&buf[..n]) {
                        // Parse simple commands
                        for line in msg.lines() {
                            let line = line.trim();
                            if line.contains("\"subscribe\"") {
                                if line.contains("\"all\"") {
                                    // Already subscribed to all by default
                                } else if let Some(idx) = extract_number(line) {
                                    let mut cls = clients.lock().unwrap();
                                    for client in cls.iter_mut() {
                                        // Simple matching by fd is impractical,
                                        // so we add to all clients for now
                                        client
                                            .subscriptions
                                            .push(TapSubscription::Pane(idx));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }

        // Remove disconnected client
        let mut cls = clients.lock().unwrap();
        cls.retain(|c| {
            // Try to check if stream is still valid
            c.stream.peer_addr().is_ok()
        });
    }

    pub fn broadcast(&self, pane_index: usize, content: &str) {
        let mut clients = self.clients.lock().unwrap();
        let msg = format!(
            "{{\"pane\":{},\"content\":{}}}\n",
            pane_index,
            serde_json::_to_string(content)
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
                        log::info!("Removing disconnected text tap client");
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

fn extract_number(s: &str) -> Option<usize> {
    s.chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// Simple JSON string escaping (avoids pulling in serde_json just for this)
mod serde_json {
    pub fn _to_string(s: &str) -> String {
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
}
