use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};

use crate::config::LlmConfig;

/// A comprehensive action the LLM (or text tap) can perform on Termania
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TermaniaAction {
    // Terminal I/O
    /// Send a command to a specific pane (appends \r)
    SendCommand { pane: usize, command: String },
    /// Send a command to all terminal panes
    SendToAll { command: String },

    // Pane metadata
    /// Rename a pane's title bar
    SetTitle { pane: usize, title: String },
    /// Set a watermark on a pane
    SetWatermark { pane: usize, watermark: String },
    /// Remove a pane's watermark
    ClearWatermark { pane: usize },

    // WebView control
    /// Navigate a webview pane to a new URL
    Navigate { pane: usize, url: String },

    // Notes control
    /// Set the text content of a notes pane
    SetContent { pane: usize, content: String },

    // Pane lifecycle
    /// Spawn a new pane
    SpawnPane {
        pane_type: String,
        title: Option<String>,
        command: Option<String>,
        cwd: Option<String>,
        url: Option<String>,
        content: Option<String>,
        watermark: Option<String>,
        row: Option<usize>,
    },
    /// Close a pane
    ClosePane { pane: usize },
    /// Replace a pane's type in-place (e.g., terminal -> webview)
    ReplacePane {
        pane: usize,
        pane_type: String,
        title: Option<String>,
        command: Option<String>,
        cwd: Option<String>,
        url: Option<String>,
        content: Option<String>,
        watermark: Option<String>,
    },

    // Layout
    /// Swap two panes' positions
    SwapPanes { a: usize, b: usize },
    /// Focus a specific pane
    FocusPane { pane: usize },

    // User communication
    /// Display a message to the user (no side effect on panes)
    Message { text: String },
}

/// Result of executing a TermaniaAction
#[derive(Debug, Clone)]
pub enum ActionResult {
    Ok,
    Error { message: String },
}

/// Information about a pane (for LLM context)
pub struct PaneInfo {
    pub index: usize,
    pub pane_type: String,
    pub title: String,
}

/// A parsed LLM response
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub explanation: String,
    pub actions: Vec<TermaniaAction>,
}

/// Status of an in-flight LLM request
#[derive(Debug)]
pub enum LlmStatus {
    Thinking,
    Complete(LlmResponse),
    Failed(String),
}

/// Pane context sent to the LLM
pub struct PaneContext {
    pub index: usize,
    pub pane_type: String,
    pub title: String,
    pub visible_text: String,
    pub subprocess_info: Option<String>,
}

/// Non-blocking LLM client using a background thread
pub struct LlmClient {
    request_tx: mpsc::Sender<LlmRequest>,
    pub status_rx: mpsc::Receiver<LlmStatus>,
}

struct LlmRequest {
    system_prompt: String,
    user_message: String,
}

trait LlmProvider: Send {
    fn send_message(&self, system: &str, user_message: &str) -> Result<String, String>;
}

struct AnthropicProvider {
    token: String,
    /// If true, use `Authorization: Bearer` (OAuth). Otherwise use `x-api-key`.
    use_oauth: bool,
    model: String,
    max_tokens: u32,
}

impl LlmProvider for AnthropicProvider {
    fn send_message(&self, system: &str, user_message: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": system,
            "messages": [
                {"role": "user", "content": user_message}
            ]
        });

        let req = ureq::post("https://api.anthropic.com/v1/messages")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");

        let req = if self.use_oauth {
            req.header("Authorization", &format!("Bearer {}", self.token))
                .header("anthropic-beta", "oauth-2025-04-20")
        } else {
            req.header("x-api-key", &self.token)
        };

        let mut resp = req
            .send(body.to_string().as_bytes())
            .map_err(|e| format!("HTTP error: {}", e))?;

        let status = resp.status();
        let body_str = resp.body_mut().read_to_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;

        if status != 200 {
            return Err(format!("API error ({}): {}", status, body_str));
        }

        let json: serde_json::Value = serde_json::from_str(&body_str)
            .map_err(|e| format!("JSON parse error: {}", e))?;

        // Extract text from Anthropic response format
        json["content"]
            .as_array()
            .and_then(|arr| arr.iter().find(|c| c["type"] == "text"))
            .and_then(|c| c["text"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Unexpected response format: {}", body_str))
    }
}

struct OpenAiProvider {
    api_key: String,
    model: String,
    base_url: String,
    max_tokens: u32,
}

impl LlmProvider for OpenAiProvider {
    fn send_message(&self, system: &str, user_message: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user_message}
            ]
        });

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut resp = ureq::post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(|e| format!("HTTP error: {}", e))?;

        let status = resp.status();
        let body_str = resp.body_mut().read_to_string()
            .map_err(|e| format!("Failed to read response: {}", e))?;

        if status != 200 {
            return Err(format!("API error ({}): {}", status, body_str));
        }

        let json: serde_json::Value = serde_json::from_str(&body_str)
            .map_err(|e| format!("JSON parse error: {}", e))?;

        json["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["message"]["content"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Unexpected response format: {}", body_str))
    }
}

/// Try to read the OAuth access token from ~/.claude/.credentials.json
fn read_claude_credentials() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".claude/.credentials.json");
    let data = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    json["claudeAiOauth"]["accessToken"].as_str().map(|s| s.to_string())
}

fn termania_token_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".config/termania/oauth_token.json"))
}

/// Read a saved OAuth token from Termania's own config, checking expiry.
fn read_saved_token() -> Option<String> {
    let path = termania_token_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    let token = json["token"].as_str()?;
    // Check expiry if present
    if let Some(expires_at) = json["expires_at"].as_i64() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if now >= expires_at {
            log::info!("Saved OAuth token has expired, ignoring");
            // Clean up expired token
            let _ = std::fs::remove_file(&path);
            return None;
        }
    }
    Some(token.to_string())
}

/// Save an OAuth token to Termania's config with an 8-hour expiry.
pub fn save_oauth_token(token: &str) {
    let Some(path) = termania_token_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let json = serde_json::json!({
        "token": token,
        "expires_at": now + 8 * 3600, // 8 hours
    });
    if let Err(e) = std::fs::write(&path, json.to_string()) {
        log::warn!("Failed to save OAuth token: {}", e);
    } else {
        log::info!("OAuth token saved to {:?}", path);
    }
}

fn create_provider(config: &LlmConfig) -> Result<Box<dyn LlmProvider>, String> {
    match config.provider.as_str() {
        "anthropic" => {
            // Try OAuth token first, then saved token, then credentials file, then API key
            let (token, use_oauth) = if let Some(oauth) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok().filter(|s| !s.is_empty()) {
                (oauth, true)
            } else if let Some(oauth) = read_saved_token() {
                log::info!("Using saved OAuth token from ~/.config/termania/");
                (oauth, true)
            } else if let Some(oauth) = read_claude_credentials() {
                log::info!("Using OAuth token from ~/.claude/.credentials.json");
                (oauth, true)
            } else if let Some(key) = config.api_key.clone().or_else(|| std::env::var("ANTHROPIC_API_KEY").ok()) {
                (key, false)
            } else {
                return Err("No Anthropic credentials. Set CLAUDE_CODE_OAUTH_TOKEN for OAuth, ANTHROPIC_API_KEY for API key auth, or api_key in [llm] config.".to_string());
            };
            let model = config.model.clone()
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());
            Ok(Box::new(AnthropicProvider {
                token,
                use_oauth,
                model,
                max_tokens: config.max_tokens,
            }))
        }
        "openai" => {
            let api_key = config.api_key.clone()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .ok_or("No OpenAI API key. Set OPENAI_API_KEY env var or api_key in [llm] config.")?;
            let model = config.model.clone()
                .unwrap_or_else(|| "gpt-4o".to_string());
            Ok(Box::new(OpenAiProvider {
                api_key,
                model,
                base_url: "https://api.openai.com/v1".to_string(),
                max_tokens: config.max_tokens,
            }))
        }
        "ollama" | "custom" => {
            let base_url = config.base_url.clone()
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string());
            let model = config.model.clone()
                .unwrap_or_else(|| "llama3".to_string());
            let api_key = config.api_key.clone()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .unwrap_or_else(|| "ollama".to_string());
            Ok(Box::new(OpenAiProvider {
                api_key,
                model,
                base_url,
                max_tokens: config.max_tokens,
            }))
        }
        other => Err(format!("Unknown LLM provider: {}. Use anthropic, openai, ollama, or custom.", other)),
    }
}

impl LlmClient {
    /// Create a new LLM client. Returns None if no API key is available.
    pub fn new(config: &LlmConfig) -> Option<Self> {
        let provider = match create_provider(config) {
            Ok(p) => p,
            Err(e) => {
                log::info!("LLM not available: {}", e);
                return None;
            }
        };

        let (request_tx, request_rx) = mpsc::channel::<LlmRequest>();
        let (status_tx, status_rx) = mpsc::channel::<LlmStatus>();

        thread::spawn(move || {
            for req in request_rx {
                let _ = status_tx.send(LlmStatus::Thinking);
                match provider.send_message(&req.system_prompt, &req.user_message) {
                    Ok(response_text) => {
                        let response = parse_llm_response(&response_text);
                        let _ = status_tx.send(LlmStatus::Complete(response));
                    }
                    Err(e) => {
                        log::error!("LLM request failed: {}", e);
                        let _ = status_tx.send(LlmStatus::Failed(e));
                    }
                }
            }
        });

        Some(Self { request_tx, status_rx })
    }

    /// Send a prompt to the LLM with pane context
    pub fn send(&self, prompt: &str, panes: &[PaneContext], custom_system: Option<&str>) {
        let system_prompt = if let Some(custom) = custom_system {
            custom.to_string()
        } else {
            build_system_prompt(panes)
        };

        let _ = self.request_tx.send(LlmRequest {
            system_prompt,
            user_message: prompt.to_string(),
        });
    }
}

fn build_system_prompt(panes: &[PaneContext]) -> String {
    let mut prompt = String::from(
        "You are an AI assistant integrated into Termania, a multi-pane terminal emulator. \
         You have deep programmatic control over the entire application.\n\n\
         Current panes:\n"
    );

    for pane in panes {
        prompt.push_str(&format!(
            "\n--- Pane {} [{}] (\"{}\") ---\n",
            pane.index, pane.pane_type, pane.title,
        ));
        if let Some(ref info) = pane.subprocess_info {
            if !info.is_empty() {
                prompt.push_str(&format!("{}\n", info));
            }
        }
        prompt.push_str(&format!(
            "Last visible output:\n{}\n",
            truncate_visible_text(&pane.visible_text, 50)
        ));
    }

    prompt.push_str(
        "\n\nRespond with JSON in this exact format:\n\
         ```json\n\
         {\n\
           \"explanation\": \"Brief description of what you'll do\",\n\
           \"actions\": [\n\
             {\"type\": \"send_command\", \"pane\": 0, \"command\": \"ls -la\"},\n\
             {\"type\": \"message\", \"text\": \"Done!\"}\n\
           ]\n\
         }\n\
         ```\n\n\
         Available action types:\n\n\
         TERMINAL I/O:\n\
         - {\"type\": \"send_command\", \"pane\": <index>, \"command\": \"<shell command>\"}\n\
           Send a command to a specific terminal pane (appends Enter).\n\
         - {\"type\": \"send_to_all\", \"command\": \"<shell command>\"}\n\
           Send a command to all terminal panes.\n\n\
         PANE METADATA:\n\
         - {\"type\": \"set_title\", \"pane\": <index>, \"title\": \"<new title>\"}\n\
           Rename a pane's title bar.\n\
         - {\"type\": \"set_watermark\", \"pane\": <index>, \"watermark\": \"<text>\"}\n\
           Set a large faded watermark behind a pane's content.\n\
         - {\"type\": \"clear_watermark\", \"pane\": <index>}\n\
           Remove a pane's watermark.\n\n\
         WEBVIEW CONTROL:\n\
         - {\"type\": \"navigate\", \"pane\": <index>, \"url\": \"<url>\"}\n\
           Navigate a webview pane to a new URL. Only works on webview panes.\n\n\
         NOTES CONTROL:\n\
         - {\"type\": \"set_content\", \"pane\": <index>, \"content\": \"<text>\"}\n\
           Set the text content of a notes pane. Only works on notes panes.\n\n\
         PANE LIFECYCLE:\n\
         - {\"type\": \"spawn_pane\", \"pane_type\": \"terminal|webview|notes\", \"title\": \"<opt>\", \
           \"command\": \"<opt>\", \"cwd\": \"<opt>\", \"url\": \"<opt>\", \"content\": \"<opt>\", \
           \"watermark\": \"<opt>\", \"row\": <opt index>}\n\
           Spawn a new pane. If row is given, adds to that row; otherwise adds a new row.\n\
         - {\"type\": \"close_pane\", \"pane\": <index>}\n\
           Close and remove a pane.\n\
         - {\"type\": \"replace_pane\", \"pane\": <index>, \"pane_type\": \"terminal|webview|notes\", \
           \"title\": \"<opt>\", \"command\": \"<opt>\", \"cwd\": \"<opt>\", \"url\": \"<opt>\", \
           \"content\": \"<opt>\", \"watermark\": \"<opt>\"}\n\
           Hot-replace a pane's type in-place (e.g., terminal -> webview).\n\n\
         LAYOUT:\n\
         - {\"type\": \"swap_panes\", \"a\": <index>, \"b\": <index>}\n\
           Swap two panes' positions in the grid.\n\
         - {\"type\": \"focus_pane\", \"pane\": <index>}\n\
           Focus a specific pane.\n\n\
         USER COMMUNICATION:\n\
         - {\"type\": \"message\", \"text\": \"<message>\"}\n\
           Display a message to the user.\n\n\
         Always include an explanation. Use the pane type info to choose appropriate actions \
         (e.g., navigate only for webview panes, send_command only for terminal panes). \
         If the user's request is a question or doesn't need an action, use a \"message\" action.\n\
         Return ONLY the JSON, no other text."
    );

    prompt
}

fn truncate_visible_text(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    let start = lines.len() - max_lines;
    lines[start..].join("\n")
}

fn parse_llm_response(text: &str) -> LlmResponse {
    // Try to extract JSON from the response, stripping markdown code fences if present
    let json_str = extract_json(text);

    if let Some(json_str) = json_str {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_str) {
            let explanation = json["explanation"]
                .as_str()
                .unwrap_or("(no explanation)")
                .to_string();

            let mut actions = Vec::new();

            if let Some(arr) = json["actions"].as_array() {
                for action_val in arr {
                    // Try serde deserialization first (new format)
                    if let Ok(action) = serde_json::from_value::<TermaniaAction>(action_val.clone()) {
                        actions.push(action);
                        continue;
                    }
                    // Fallback: parse legacy format
                    if let Some(action) = parse_legacy_action(action_val) {
                        actions.push(action);
                    }
                }
            }

            return LlmResponse { explanation, actions };
        }
    }

    // Fallback: treat entire response as a message
    LlmResponse {
        explanation: "Response from AI".to_string(),
        actions: vec![TermaniaAction::Message { text: text.to_string() }],
    }
}

/// Parse the old Phase-1 action format for backward compatibility
fn parse_legacy_action(action: &serde_json::Value) -> Option<TermaniaAction> {
    match action["type"].as_str() {
        Some("send") => {
            let pane = action["pane"].as_u64()? as usize;
            let command = action["command"].as_str()?.to_string();
            Some(TermaniaAction::SendCommand { pane, command })
        }
        Some("send_all") => {
            let command = action["command"].as_str()?.to_string();
            Some(TermaniaAction::SendToAll { command })
        }
        Some("message") => {
            let text = action["text"].as_str()?.to_string();
            Some(TermaniaAction::Message { text })
        }
        _ => None,
    }
}

/// Format a TermaniaAction for display in the command overlay
pub fn format_action_for_display(action: &TermaniaAction) -> String {
    match action {
        TermaniaAction::SendCommand { pane, command } => {
            format!("  [pane {}] $ {}", pane, command)
        }
        TermaniaAction::SendToAll { command } => {
            format!("  [all] $ {}", command)
        }
        TermaniaAction::SetTitle { pane, title } => {
            format!("  [pane {}] title = \"{}\"", pane, title)
        }
        TermaniaAction::SetWatermark { pane, watermark } => {
            format!("  [pane {}] watermark = \"{}\"", pane, watermark)
        }
        TermaniaAction::ClearWatermark { pane } => {
            format!("  [pane {}] clear watermark", pane)
        }
        TermaniaAction::Navigate { pane, url } => {
            format!("  [pane {}] navigate -> {}", pane, url)
        }
        TermaniaAction::SetContent { pane, content } => {
            let preview = if content.len() > 40 {
                format!("{}...", &content[..40])
            } else {
                content.clone()
            };
            format!("  [pane {}] set content: \"{}\"", pane, preview)
        }
        TermaniaAction::SpawnPane { pane_type, title, .. } => {
            let label = title.as_deref().unwrap_or(pane_type);
            format!("  spawn {} (\"{}\")", pane_type, label)
        }
        TermaniaAction::ClosePane { pane } => {
            format!("  close pane {}", pane)
        }
        TermaniaAction::ReplacePane { pane, pane_type, .. } => {
            format!("  [pane {}] replace with {}", pane, pane_type)
        }
        TermaniaAction::SwapPanes { a, b } => {
            format!("  swap pane {} <-> pane {}", a, b)
        }
        TermaniaAction::FocusPane { pane } => {
            format!("  focus pane {}", pane)
        }
        TermaniaAction::Message { text } => {
            format!("  {}", text)
        }
    }
}

/// Extract JSON from text, handling markdown code fences
fn extract_json(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Try direct parse first
    if trimmed.starts_with('{') {
        return Some(trimmed.to_string());
    }

    // Strip markdown code fences
    if let Some(start) = trimmed.find("```json") {
        let after = &trimmed[start + 7..];
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        // Skip optional language tag on the same line
        let after = if let Some(nl) = after.find('\n') {
            &after[nl + 1..]
        } else {
            after
        };
        if let Some(end) = after.find("```") {
            let candidate = after[..end].trim();
            if candidate.starts_with('{') {
                return Some(candidate.to_string());
            }
        }
    }

    // Look for first { to last }
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                return Some(trimmed[start..=end].to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let input = r#"{"explanation": "test", "actions": []}"#;
        let result = extract_json(input);
        assert!(result.is_some());
        assert!(result.unwrap().starts_with('{'));
    }

    #[test]
    fn test_extract_json_markdown_fenced() {
        let input = "Here is the response:\n```json\n{\"explanation\": \"test\", \"actions\": []}\n```\n";
        let result = extract_json(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert!(json.contains("explanation"));
    }

    #[test]
    fn test_extract_json_generic_fence() {
        let input = "```\n{\"explanation\": \"hi\", \"actions\": []}\n```";
        let result = extract_json(input);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_json_embedded() {
        let input = "Sure, here you go: {\"explanation\": \"ok\", \"actions\": []} done.";
        let result = extract_json(input);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_json_no_json() {
        let input = "This is plain text with no JSON";
        let result = extract_json(input);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_llm_response_valid() {
        let input = r#"{"explanation": "Running ls", "actions": [{"type": "send_command", "pane": 0, "command": "ls"}]}"#;
        let resp = parse_llm_response(input);
        assert_eq!(resp.explanation, "Running ls");
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            TermaniaAction::SendCommand { pane, command } => {
                assert_eq!(*pane, 0);
                assert_eq!(command, "ls");
            }
            _ => panic!("Expected SendCommand"),
        }
    }

    #[test]
    fn test_parse_llm_response_fallback() {
        let input = "Just some random text response";
        let resp = parse_llm_response(input);
        assert_eq!(resp.explanation, "Response from AI");
        assert_eq!(resp.actions.len(), 1);
        match &resp.actions[0] {
            TermaniaAction::Message { text } => assert_eq!(text, input),
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_parse_legacy_action_send() {
        let val: serde_json::Value = serde_json::json!({"type": "send", "pane": 0, "command": "ls"});
        let action = parse_legacy_action(&val);
        assert!(action.is_some());
        match action.unwrap() {
            TermaniaAction::SendCommand { pane, command } => {
                assert_eq!(pane, 0);
                assert_eq!(command, "ls");
            }
            _ => panic!("Expected SendCommand"),
        }
    }

    #[test]
    fn test_parse_legacy_action_send_all() {
        let val: serde_json::Value = serde_json::json!({"type": "send_all", "command": "clear"});
        let action = parse_legacy_action(&val);
        assert!(action.is_some());
        match action.unwrap() {
            TermaniaAction::SendToAll { command } => assert_eq!(command, "clear"),
            _ => panic!("Expected SendToAll"),
        }
    }

    #[test]
    fn test_parse_legacy_action_unknown() {
        let val: serde_json::Value = serde_json::json!({"type": "unknown_action"});
        assert!(parse_legacy_action(&val).is_none());
    }

    #[test]
    fn test_action_serialization_roundtrip() {
        let action = TermaniaAction::SendCommand { pane: 2, command: "echo hello".to_string() };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: TermaniaAction = serde_json::from_str(&json).unwrap();
        match parsed {
            TermaniaAction::SendCommand { pane, command } => {
                assert_eq!(pane, 2);
                assert_eq!(command, "echo hello");
            }
            _ => panic!("Roundtrip failed"),
        }
    }

    #[test]
    fn test_format_action_send_command() {
        let action = TermaniaAction::SendCommand { pane: 0, command: "ls -la".to_string() };
        let display = format_action_for_display(&action);
        assert!(display.contains("[pane 0]"));
        assert!(display.contains("ls -la"));
    }

    #[test]
    fn test_format_action_message() {
        let action = TermaniaAction::Message { text: "Hello world".to_string() };
        let display = format_action_for_display(&action);
        assert!(display.contains("Hello world"));
    }

    #[test]
    fn test_format_action_spawn() {
        let action = TermaniaAction::SpawnPane {
            pane_type: "terminal".to_string(),
            title: Some("Dev".to_string()),
            command: None, cwd: None, url: None, content: None, watermark: None, row: None,
        };
        let display = format_action_for_display(&action);
        assert!(display.contains("spawn"));
        assert!(display.contains("Dev"));
    }

    #[test]
    fn test_truncate_visible_text_short() {
        let text = "line1\nline2\nline3";
        assert_eq!(truncate_visible_text(text, 10), text);
    }

    #[test]
    fn test_truncate_visible_text_long() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        let text = lines.join("\n");
        let result = truncate_visible_text(&text, 5);
        assert_eq!(result.lines().count(), 5);
        assert!(result.contains("line 99"));
    }

    #[test]
    fn test_pane_context_with_subprocess_info() {
        let ctx = PaneContext {
            index: 0,
            pane_type: "terminal".to_string(),
            title: "Dev".to_string(),
            visible_text: "$ ".to_string(),
            subprocess_info: Some("Child processes:\n  pid=1234 cmd=node".to_string()),
        };
        assert!(ctx.subprocess_info.is_some());
        assert!(ctx.subprocess_info.unwrap().contains("node"));
    }

    #[test]
    fn test_build_system_prompt_includes_panes() {
        let panes = vec![
            PaneContext {
                index: 0,
                pane_type: "terminal".to_string(),
                title: "Shell".to_string(),
                visible_text: "$ hello\n".to_string(),
                subprocess_info: None,
            },
        ];
        let prompt = build_system_prompt(&panes);
        assert!(prompt.contains("Pane 0"));
        assert!(prompt.contains("[terminal]"));
        assert!(prompt.contains("Shell"));
        assert!(prompt.contains("send_command"));
    }

    #[test]
    fn test_build_system_prompt_includes_subprocess_info() {
        let panes = vec![
            PaneContext {
                index: 0,
                pane_type: "terminal".to_string(),
                title: "Dev".to_string(),
                visible_text: "running".to_string(),
                subprocess_info: Some("Child processes:\n  pid=5678 cmd=flutter ports=[8080]".to_string()),
            },
        ];
        let prompt = build_system_prompt(&panes);
        assert!(prompt.contains("flutter"));
        assert!(prompt.contains("8080"));
    }
}
