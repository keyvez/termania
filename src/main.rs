#![allow(dead_code)]

mod config;
mod grid;
mod llm;
mod plugin;
mod plugins;
mod pty;
mod renderer;
mod terminal;
mod text_tap;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use config::{Config, PaneConfig, SessionConfig};
use plugin::PanePlugin;
use plugins::terminal::TerminalPlugin;
use plugins::webview::WebViewPlugin;
use plugins::notes::NotesPlugin;
use plugins::screen_capture::ScreenCapturePlugin;

/// Overlay mode: LLM-assisted or raw command
#[derive(Debug, Clone, PartialEq)]
enum OverlayMode {
    Llm,
    RawCommand,
    TokenInput,
}

/// Command overlay for sending commands to selected/all panes
struct CommandOverlay {
    /// The text being typed
    input: String,
    /// Target panes (None = all, Some = specific set)
    targets: Option<HashSet<usize>>,
    /// Current mode (LLM or raw command)
    mode: OverlayMode,
    /// Whether the LLM is currently processing
    llm_thinking: bool,
    /// LLM response ready for review/execution
    llm_response: Option<llm::LlmResponse>,
}
/// Text selection within a terminal pane
struct TextSelection {
    pane_idx: usize,
    start: (usize, usize), // (row, col) in displayed lines
    end: (usize, usize),
    in_progress: bool,
}

use grid::GridManager;
use log::info;
use renderer::Renderer;
use text_tap::TextTapServer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// Per-window state: each window has its own renderer, grid, panes, and UI state
struct TermaniaWindow {
    window: Arc<Window>,
    renderer: Renderer,
    grid: GridManager,
    panes: Vec<Box<dyn PanePlugin>>,
    focused_pane: usize,
    cursor_position: (f64, f64),
    last_resize: Option<Instant>,
    resize_pending: bool,
    broadcast_mode: bool,
    selected_panes: HashSet<usize>,
    drag_selecting: bool,
    drag_start_pane: Option<usize>,
    command_overlay: Option<CommandOverlay>,
    show_help: bool,
    help_scroll: usize,
    rename_overlay: Option<String>,
    effective_panes_config: Vec<PaneConfig>,
    /// Session config used by this window (for grid dimension queries)
    session: Option<SessionConfig>,
    /// Accumulated trackpad scroll delta (pixels) for smooth scrolling
    scroll_accumulator: f64,
    /// Parent NSView pointer for initializing native views at runtime
    parent_ns_view: Option<*mut std::ffi::c_void>,
    /// Current text selection (if any)
    text_selection: Option<TextSelection>,
    /// Timestamp of last left click (for double-click detection)
    last_click_time: Option<Instant>,
    /// Position of last left click (for double-click detection)
    last_click_pos: Option<(f64, f64)>,
}

struct App {
    config: Arc<Config>,
    /// Session config for the initial window
    initial_session: Option<SessionConfig>,
    /// All windows, keyed by WindowId
    windows: HashMap<WindowId, TermaniaWindow>,
    /// Shared text tap server
    text_tap: TextTapServer,
    /// Shared modifier state (tracked globally)
    modifiers: ModifiersState,
    /// Timestamp of last Option key press (for double-tap detection)
    last_option_press: Option<Instant>,
    /// Whether the initial window has been created
    initial_window_created: bool,
    /// LLM client for AI-assisted commands (None if no API key)
    llm_client: Option<llm::LlmClient>,
}

/// Create a pane plugin from a pane config entry
fn create_pane_plugin(index: usize, pane_config: Option<&PaneConfig>, config: &Config) -> Box<dyn PanePlugin> {
    let pane_type = pane_config
        .map(|p| p.pane_type.as_str())
        .unwrap_or("terminal");

    match pane_type {
        "webview" => Box::new(WebViewPlugin::new(index, pane_config)),
        "notes" => Box::new(NotesPlugin::new(index, pane_config)),
        "screen_capture" => Box::new(ScreenCapturePlugin::new(index, pane_config)),
        _ => Box::new(TerminalPlugin::new(index, pane_config, config)),
    }
}

impl TermaniaWindow {
    /// Initialize native views (WebView, Notes) by providing the parent NSView
    #[cfg(target_os = "macos")]
    fn init_native_views(&mut self, parent_view: *mut std::ffi::c_void) {
        for pane in &mut self.panes {
            if pane.is_native_view() {
                pane.init_native_view_with_parent(parent_view);
            }
        }
    }

    fn sync_pane_sizes(&mut self, config: &Config) {
        let scale = self.renderer.scale_factor();
        let grid_layout = self.grid.compute_layout(
            self.renderer.width(),
            self.renderer.height(),
            config,
            scale,
        );
        let border_width = 2.0f32 * scale;
        let inner_pad = config.grid.inner_padding as f32 * scale;
        let cell_w = self.renderer.cell_width();
        let cell_h = self.renderer.cell_height();
        let window_height = self.renderer.height() as f32;

        for (i, layout) in grid_layout.iter().enumerate() {
            if i >= self.panes.len() {
                break;
            }
            let content_w = layout.width - 2.0 * border_width - 2.0 * inner_pad;
            let content_h = layout.height - 2.0 * border_width - layout.title_height - 2.0 * inner_pad;

            // Resize the plugin (terminal plugins use cell dimensions, others use pixel dimensions)
            self.panes[i].resize(content_w, content_h, cell_w, cell_h);

            // Position native views at the correct pixel coordinates
            if self.panes[i].is_native_view() {
                let view_x = layout.x + border_width + inner_pad;
                let view_y = layout.y + border_width + layout.title_height + inner_pad;
                let view_w = layout.width - 2.0 * border_width - 2.0 * inner_pad;
                let view_h = layout.height - 2.0 * border_width - layout.title_height - 2.0 * inner_pad;
                log::debug!(
                    "Positioning native view pane {} ({}) at ({}, {}, {}x{}) window_h={}",
                    i, self.panes[i].title(), view_x, view_y, view_w, view_h, window_height
                );
                self.panes[i].position_native_view(
                    view_x, view_y, view_w, view_h, window_height,
                );
            }
        }
    }

    /// Send input to the appropriate pane(s) based on broadcast mode.
    fn send_input(&mut self, data: &[u8]) {
        if self.broadcast_mode {
            for pane in &mut self.panes {
                if !pane.is_native_view() {
                    pane.write_input(data);
                }
            }
        } else if !self.selected_panes.is_empty() {
            for &i in &self.selected_panes.clone() {
                if i < self.panes.len() && !self.panes[i].is_native_view() {
                    self.panes[i].write_input(data);
                }
            }
            if !self.selected_panes.contains(&self.focused_pane)
                && self.focused_pane < self.panes.len()
            {
                self.panes[self.focused_pane].write_input(data);
            }
        } else if self.focused_pane < self.panes.len() {
            self.panes[self.focused_pane].write_input(data);
        }
    }

    /// Convert pixel coordinates to (pane_idx, row, col) in the terminal grid.
    /// Returns None if the position is outside any terminal pane's content area.
    fn pixel_to_cell(&self, px: f64, py: f64, config: &Config) -> Option<(usize, usize, usize)> {
        let scale = self.renderer.scale_factor();
        let grid_layout = self.grid.compute_layout(
            self.renderer.width(),
            self.renderer.height(),
            config,
            scale,
        );
        let x = px as f32;
        let y = py as f32;
        let border_width = 2.0f32 * scale;
        let inner_pad = config.grid.inner_padding as f32 * scale;
        let cell_w = self.renderer.cell_width();
        let cell_h = self.renderer.cell_height();
        let title_height = config.grid.title_bar_height as f32 * scale;

        for (i, layout) in grid_layout.iter().enumerate() {
            if i >= self.panes.len() { break; }
            // Skip native-view panes
            if self.panes[i].is_native_view() { continue; }

            let content_x = layout.x + border_width + inner_pad;
            let content_y = layout.y + border_width + title_height + inner_pad;
            let content_w = layout.width - 2.0 * border_width - 2.0 * inner_pad;
            let content_h = layout.height - 2.0 * border_width - title_height - 2.0 * inner_pad;

            if x >= content_x && x < content_x + content_w
                && y >= content_y && y < content_y + content_h
            {
                let col = ((x - content_x) / cell_w) as usize;
                let row = ((y - content_y) / cell_h) as usize;
                return Some((i, row, col));
            }
        }
        None
    }

    /// Extract the selected text from the terminal pane
    fn selected_text(&self) -> Option<String> {
        let sel = self.text_selection.as_ref()?;
        let pane = self.panes.get(sel.pane_idx)?;
        let render_data = pane.render_data();
        if let crate::plugin::PanePluginRenderData::Terminal { ref lines, .. } = render_data {
            let (sr, sc, er, ec) = Self::normalized_selection(sel);
            let mut result = String::new();
            for row in sr..=er {
                if row >= lines.len() { break; }
                let line = &lines[row];
                let col_start = if row == sr { sc } else { 0 };
                let col_end = if row == er { ec.min(line.len().saturating_sub(1)) } else { line.len().saturating_sub(1) };
                if col_start > col_end || col_start >= line.len() { continue; }
                let mut row_text = String::new();
                for col in col_start..=col_end {
                    if col < line.len() {
                        row_text.push(line[col].ch);
                    }
                }
                // Trim trailing spaces per line
                let trimmed = row_text.trim_end();
                result.push_str(trimmed);
                if row < er {
                    result.push('\n');
                }
            }
            if result.is_empty() { None } else { Some(result) }
        } else {
            None
        }
    }

    /// Normalize selection so start <= end in reading order
    fn normalized_selection(sel: &TextSelection) -> (usize, usize, usize, usize) {
        if sel.start.0 < sel.end.0 || (sel.start.0 == sel.end.0 && sel.start.1 <= sel.end.1) {
            (sel.start.0, sel.start.1, sel.end.0, sel.end.1)
        } else {
            (sel.end.0, sel.end.1, sel.start.0, sel.start.1)
        }
    }

    /// Find which pane index the given screen coordinates fall in
    fn pane_at_position(&self, x: f64, y: f64, config: &Config) -> Option<usize> {
        let scale = self.renderer.scale_factor();
        let grid_layout = self.grid.compute_layout(
            self.renderer.width(),
            self.renderer.height(),
            config,
            scale,
        );
        // CursorMoved gives PhysicalPosition — already in physical pixels
        let px = x as f32;
        let py = y as f32;
        for (i, layout) in grid_layout.iter().enumerate() {
            if i >= self.panes.len() {
                break;
            }
            if px >= layout.x
                && px <= layout.x + layout.width
                && py >= layout.y
                && py <= layout.y + layout.height
            {
                return Some(i);
            }
        }
        None
    }

    /// Swap the focused pane with a neighbor in the grid.
    fn swap_pane_in_direction(&mut self, dcol: i32, drow: i32, config: &Config) {
        let idx = self.focused_pane;
        if idx >= self.panes.len() {
            return;
        }
        let (cur_row, cur_col) = match self.grid.pane_position(idx) {
            Some(pos) => pos,
            None => return,
        };
        let new_row = cur_row as i32 + drow;
        let new_col = cur_col as i32 + dcol;
        if new_row < 0 || new_row >= self.grid.rows() as i32 {
            return;
        }
        let nr = new_row as usize;
        if new_col < 0 || new_col >= self.grid.cols_in_row(nr) as i32 {
            return;
        }
        let target = match self.grid.flat_index(nr, new_col as usize) {
            Some(t) => t,
            None => return,
        };
        if target >= self.panes.len() {
            return;
        }
        self.panes.swap(idx, target);
        self.effective_panes_config.resize(self.panes.len().max(self.effective_panes_config.len()), PaneConfig {
            pane_type: "terminal".to_string(),
            title: None, command: None, cwd: None, env: None,
            initial_commands: None, watermark: None, url: None,
            file: None, content: None, target: None, target_title: None,
        });
        self.effective_panes_config.swap(idx, target);
        self.focused_pane = target;
        // Reposition native views after swap
        self.sync_pane_sizes(config);
        self.window.request_redraw();
    }

    /// Execute a TermaniaAction and return the result
    fn execute_action(&mut self, action: &llm::TermaniaAction, config: &Config) -> llm::ActionResult {
        match action {
            llm::TermaniaAction::SendCommand { pane, command } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range (have {})", pane, self.panes.len()),
                    };
                }
                let cmd = format!("{}\r", command);
                self.panes[*pane].write_input(cmd.as_bytes());
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::SendToAll { command } => {
                let cmd = format!("{}\r", command);
                for pane in &mut self.panes {
                    pane.write_input(cmd.as_bytes());
                }
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::SetTitle { pane, title } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                self.panes[*pane].set_title(title.clone());
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::SetWatermark { pane, watermark } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                // Ensure effective_panes_config has enough entries
                while self.effective_panes_config.len() <= *pane {
                    self.effective_panes_config.push(PaneConfig {
                        pane_type: "terminal".to_string(),
                        title: None, command: None, cwd: None, env: None,
                        initial_commands: None, watermark: None, url: None,
                        file: None, content: None, target: None, target_title: None,
                    });
                }
                self.effective_panes_config[*pane].watermark = Some(watermark.clone());
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::ClearWatermark { pane } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                if *pane < self.effective_panes_config.len() {
                    self.effective_panes_config[*pane].watermark = None;
                }
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::Navigate { pane, url } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                if !self.panes[*pane].navigate(url) {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} does not support navigation (not a webview)", pane),
                    };
                }
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::SetContent { pane, content } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                if !self.panes[*pane].set_content(content) {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} does not support set_content (not a notes pane)", pane),
                    };
                }
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::SpawnPane {
                pane_type, title, command, cwd, url, content, watermark, row,
            } => {
                let pane_config = PaneConfig {
                    pane_type: pane_type.clone(),
                    title: title.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                    env: None,
                    initial_commands: None,
                    watermark: watermark.clone(),
                    url: url.clone(),
                    file: None,
                    content: content.clone(),
                    target: None,
                    target_title: None,
                };
                let idx = self.panes.len();
                let mut new_pane = create_pane_plugin(idx, Some(&pane_config), config);
                new_pane.init();

                // Initialize native view if needed
                if new_pane.is_native_view() {
                    if let Some(parent) = self.parent_ns_view {
                        new_pane.init_native_view_with_parent(parent);
                    }
                }

                if let Some(target_row) = row {
                    // Insert into an existing row
                    if *target_row < self.grid.rows() {
                        let insert_at = self.grid.flat_index(*target_row, self.grid.cols_in_row(*target_row).saturating_sub(1))
                            .map(|i| i + 1)
                            .unwrap_or(self.panes.len());
                        self.panes.insert(insert_at, new_pane);
                        // Keep effective_panes_config in sync
                        if insert_at <= self.effective_panes_config.len() {
                            self.effective_panes_config.insert(insert_at, pane_config);
                        } else {
                            self.effective_panes_config.push(pane_config);
                        }
                        self.grid.add_col_to_row(*target_row);
                    } else {
                        // Row out of range, add a new row
                        self.panes.push(new_pane);
                        self.effective_panes_config.push(pane_config);
                        self.grid.add_row();
                    }
                } else {
                    // Add a new row
                    self.panes.push(new_pane);
                    self.effective_panes_config.push(pane_config);
                    self.grid.add_row();
                }

                self.sync_pane_sizes(config);
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::ClosePane { pane } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                if self.panes.len() <= 1 {
                    return llm::ActionResult::Error {
                        message: "Cannot close the last pane".to_string(),
                    };
                }
                let row = self.grid.pane_position(*pane)
                    .map(|(r, _)| r)
                    .unwrap_or(0);
                self.panes[*pane].shutdown();
                self.panes.remove(*pane);
                if *pane < self.effective_panes_config.len() {
                    self.effective_panes_config.remove(*pane);
                }
                self.grid.remove_col_from_row(row);
                if self.focused_pane >= self.panes.len() && self.focused_pane > 0 {
                    self.focused_pane -= 1;
                }
                self.sync_pane_sizes(config);
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::ReplacePane {
                pane, pane_type, title, command, cwd, url, content, watermark,
            } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                // Shutdown old pane
                self.panes[*pane].shutdown();

                // Create new pane
                let pane_config = PaneConfig {
                    pane_type: pane_type.clone(),
                    title: title.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                    env: None,
                    initial_commands: None,
                    watermark: watermark.clone(),
                    url: url.clone(),
                    file: None,
                    content: content.clone(),
                    target: None,
                    target_title: None,
                };
                let mut new_pane = create_pane_plugin(*pane, Some(&pane_config), config);
                new_pane.init();

                // Initialize native view if needed
                if new_pane.is_native_view() {
                    if let Some(parent) = self.parent_ns_view {
                        new_pane.init_native_view_with_parent(parent);
                    }
                }

                self.panes[*pane] = new_pane;

                // Update effective config
                while self.effective_panes_config.len() <= *pane {
                    self.effective_panes_config.push(PaneConfig {
                        pane_type: "terminal".to_string(),
                        title: None, command: None, cwd: None, env: None,
                        initial_commands: None, watermark: None, url: None,
                        file: None, content: None, target: None, target_title: None,
                    });
                }
                self.effective_panes_config[*pane] = pane_config;

                self.sync_pane_sizes(config);
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::SwapPanes { a, b } => {
                if *a >= self.panes.len() || *b >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane index out of range (a={}, b={}, have {})", a, b, self.panes.len()),
                    };
                }
                self.panes.swap(*a, *b);
                // Keep effective_panes_config in sync
                let max_idx = (*a).max(*b);
                while self.effective_panes_config.len() <= max_idx {
                    self.effective_panes_config.push(PaneConfig {
                        pane_type: "terminal".to_string(),
                        title: None, command: None, cwd: None, env: None,
                        initial_commands: None, watermark: None, url: None,
                        file: None, content: None, target: None, target_title: None,
                    });
                }
                self.effective_panes_config.swap(*a, *b);
                self.sync_pane_sizes(config);
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::FocusPane { pane } => {
                if *pane >= self.panes.len() {
                    return llm::ActionResult::Error {
                        message: format!("Pane {} out of range", pane),
                    };
                }
                self.focused_pane = *pane;
                llm::ActionResult::Ok
            }
            llm::TermaniaAction::Message { .. } => {
                // Messages are display-only, no side effect
                llm::ActionResult::Ok
            }
        }
    }
}

impl App {
    fn new(config: Config, session: Option<SessionConfig>) -> Self {
        let config = Arc::new(config);
        let text_tap = TextTapServer::new(&config.text_tap.socket_path);
        let llm_client = llm::LlmClient::new(&config.llm);
        if llm_client.is_some() {
            log::info!("LLM client initialized (provider: {})", config.llm.provider);
        }

        Self {
            config,
            initial_session: session,
            windows: HashMap::new(),
            text_tap,
            modifiers: ModifiersState::empty(),
            last_option_press: None,
            initial_window_created: false,
            llm_client,
        }
    }

    /// Create panes for a window based on config/session
    fn create_panes_for_session(
        config: &Config,
        session: Option<&SessionConfig>,
    ) -> (GridManager, Vec<Box<dyn PanePlugin>>, Vec<PaneConfig>) {
        let rows = config.effective_rows(session);
        let cols = config.effective_cols(session);
        let pane_count = rows * cols;
        let pane_configs = config.effective_panes(session);

        let grid = GridManager::new(rows, cols);

        let mut panes: Vec<Box<dyn PanePlugin>> = Vec::new();
        for i in 0..pane_count {
            let pane_config = pane_configs.get(i);
            panes.push(create_pane_plugin(i, pane_config, config));
        }

        let effective_panes_config = pane_configs.to_vec();
        (grid, panes, effective_panes_config)
    }

    /// Create and register a new window
    fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        session: Option<SessionConfig>,
    ) -> WindowId {
        let window_title = self.config.effective_title(session.as_ref());
        let attrs = Window::default_attributes()
            .with_title(&window_title)
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.window.width as f64,
                self.config.window.height as f64,
            ));

        let window = Arc::new(event_loop.create_window(attrs).expect("Failed to create window"));
        let window_id = window.id();

        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            &self.config,
        ));

        let (grid, mut panes, effective_panes_config) =
            Self::create_panes_for_session(&self.config, session.as_ref());

        // Initialize all pane plugins
        for pane in &mut panes {
            pane.init();
        }

        let mut tw = TermaniaWindow {
            window: window.clone(),
            renderer,
            grid,
            panes,
            focused_pane: 0,
            cursor_position: (0.0, 0.0),
            last_resize: None,
            resize_pending: false,
            broadcast_mode: false,
            selected_panes: HashSet::new(),
            drag_selecting: false,
            drag_start_pane: None,
            command_overlay: None,
            show_help: false,
            help_scroll: 0,
            rename_overlay: None,
            effective_panes_config,
            session,
            scroll_accumulator: 0.0,
            parent_ns_view: None,
            text_selection: None,
            last_click_time: None,
            last_click_pos: None,
        };

        // Initialize native views and store parent_ns_view for later use
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = window.window_handle() {
                if let RawWindowHandle::AppKit(appkit_handle) = handle.as_raw() {
                    let ns_view = appkit_handle.ns_view.as_ptr() as *mut std::ffi::c_void;
                    tw.parent_ns_view = Some(ns_view);
                    tw.init_native_views(ns_view);
                }
            }
        }

        // Size panes to match initial layout
        tw.sync_pane_sizes(&self.config);

        let rows = self.config.effective_rows(tw.session.as_ref());
        let cols = self.config.effective_cols(tw.session.as_ref());
        info!("Created window with {}x{} grid ({} panes)", rows, cols, tw.panes.len());

        self.windows.insert(window_id, tw);

        // Update text tap pane count (sum of all windows' panes)
        let total_panes: usize = self.windows.values().map(|w| w.panes.len()).sum();
        self.text_tap.set_pane_count(total_panes);

        window_id
    }

    /// Create a new window with a single default terminal
    fn create_new_default_window(&mut self, event_loop: &ActiveEventLoop) {
        self.create_window(event_loop, None);
    }

    fn handle_key_input(&mut self, window_id: WindowId, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        let mods = self.modifiers;
        let is_super = mods.super_key();
        let is_ctrl = mods.control_key();
        let is_shift = mods.shift_key();
        let is_alt = mods.alt_key();

        // Get the window
        let tw = match self.windows.get_mut(&window_id) {
            Some(tw) => tw,
            None => return,
        };

        // Escape clears text selection first
        if matches!(&event.logical_key, Key::Named(NamedKey::Escape)) && tw.text_selection.is_some() {
            tw.text_selection = None;
            tw.window.request_redraw();
            return;
        }

        // Help overlay: Escape to dismiss, arrows/page keys to scroll
        if tw.show_help {
            let scroll_down = matches!(&event.logical_key, Key::Named(NamedKey::ArrowDown))
                || matches!(&event.logical_key, Key::Character(ref c) if c.as_str() == "j");
            let scroll_up = matches!(&event.logical_key, Key::Named(NamedKey::ArrowUp))
                || matches!(&event.logical_key, Key::Character(ref c) if c.as_str() == "k");

            if matches!(&event.logical_key, Key::Named(NamedKey::Escape)) {
                tw.show_help = false;
            } else if scroll_down {
                tw.help_scroll = tw.help_scroll.saturating_add(1);
            } else if scroll_up {
                tw.help_scroll = tw.help_scroll.saturating_sub(1);
            } else if matches!(&event.logical_key, Key::Named(NamedKey::PageDown)) {
                tw.help_scroll = tw.help_scroll.saturating_add(10);
            } else if matches!(&event.logical_key, Key::Named(NamedKey::PageUp)) {
                tw.help_scroll = tw.help_scroll.saturating_sub(10);
            } else if matches!(&event.logical_key, Key::Named(NamedKey::Home)) {
                tw.help_scroll = 0;
            }
            tw.window.request_redraw();
            return;
        }

        // Detect double-tap Option/Alt to open command overlay
        if matches!(&event.logical_key, Key::Named(NamedKey::Alt)) {
            if let Some(last) = self.last_option_press {
                if last.elapsed().as_millis() < 400 {
                    // Double-tap detected — open command overlay
                    self.last_option_press = None;
                    let has_llm = self.llm_client.is_some();
                    let tw = self.windows.get_mut(&window_id).unwrap();
                    if tw.command_overlay.is_none() {
                        let targets = if tw.selected_panes.is_empty() {
                            None
                        } else {
                            Some(tw.selected_panes.clone())
                        };
                        tw.command_overlay = Some(CommandOverlay {
                            input: String::new(),
                            targets,
                            mode: if has_llm { OverlayMode::Llm } else { OverlayMode::TokenInput },
                            llm_thinking: false,
                            llm_response: None,
                        });
                        tw.window.request_redraw();
                    }
                    return;
                }
            }
            self.last_option_press = Some(Instant::now());
            return;
        }

        // Re-borrow after potential re-borrow above
        let tw = match self.windows.get_mut(&window_id) {
            Some(tw) => tw,
            None => return,
        };

        // Handle rename overlay input
        if let Some(ref mut input) = tw.rename_overlay {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    tw.rename_overlay = None;
                    tw.window.request_redraw();
                    return;
                }
                Key::Named(NamedKey::Enter) => {
                    let new_title = std::mem::take(input);
                    tw.rename_overlay = None;
                    if !new_title.is_empty() && tw.focused_pane < tw.panes.len() {
                        tw.panes[tw.focused_pane].set_title(new_title);
                    }
                    tw.window.request_redraw();
                    return;
                }
                Key::Named(NamedKey::Backspace) => {
                    input.pop();
                    tw.window.request_redraw();
                    return;
                }
                Key::Character(c) => {
                    input.push_str(c.as_str());
                    tw.window.request_redraw();
                    return;
                }
                Key::Named(NamedKey::Space) => {
                    input.push(' ');
                    tw.window.request_redraw();
                    return;
                }
                _ => return,
            }
        }

        // Handle command overlay input
        if tw.command_overlay.is_some() {
            let overlay = tw.command_overlay.as_mut().unwrap();

            // If LLM response is shown, Enter executes actions, Escape cancels
            if overlay.llm_response.is_some() {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        tw.command_overlay = None;
                        return;
                    }
                    Key::Named(NamedKey::Enter) => {
                        let response = overlay.llm_response.take().unwrap();
                        tw.command_overlay = None;
                        // Execute all actions via the dispatch engine
                        for action in &response.actions {
                            let result = tw.execute_action(action, &self.config);
                            if let llm::ActionResult::Error { message } = result {
                                log::warn!("Action failed: {}", message);
                            }
                        }
                        tw.window.request_redraw();
                        return;
                    }
                    _ => return,
                }
            }

            // If LLM is thinking, only Escape works
            if overlay.llm_thinking {
                if matches!(&event.logical_key, Key::Named(NamedKey::Escape)) {
                    tw.command_overlay = None;
                }
                return;
            }

            // Cmd+V: paste from clipboard into overlay input
            if is_super && matches!(&event.logical_key, Key::Character(c) if c.as_str() == "v") {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(text) = clipboard.get_text() {
                        // Strip newlines from pasted text
                        let clean = text.replace('\n', "").replace('\r', "");
                        overlay.input.push_str(&clean);
                        // Update mode based on ! prefix
                        if self.llm_client.is_some() {
                            overlay.mode = if overlay.input.starts_with('!') {
                                OverlayMode::RawCommand
                            } else {
                                OverlayMode::Llm
                            };
                        } else {
                            overlay.mode = if overlay.input.starts_with('!') {
                                OverlayMode::RawCommand
                            } else {
                                OverlayMode::TokenInput
                            };
                        }
                    }
                }
                return;
            }

            match &event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    tw.command_overlay = None;
                    return;
                }
                Key::Named(NamedKey::Enter) => {
                    let input = overlay.input.clone();
                    let mode = overlay.mode.clone();
                    let targets = overlay.targets.clone();

                    match mode {
                        OverlayMode::TokenInput => {
                            // Set the OAuth token and initialize the LLM client
                            let token = input.trim().to_string();
                            if token.is_empty() {
                                return;
                            }
                            std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", &token);
                            llm::save_oauth_token(&token);
                            let client = llm::LlmClient::new(&self.config.llm);
                            if client.is_some() {
                                log::info!("LLM client initialized via OAuth token from overlay");
                                self.llm_client = client;
                                // Switch overlay to AI mode
                                let overlay = tw.command_overlay.as_mut().unwrap();
                                overlay.input.clear();
                                overlay.mode = OverlayMode::Llm;
                            } else {
                                // Token didn't work, stay in token input
                                let overlay = tw.command_overlay.as_mut().unwrap();
                                overlay.input.clear();
                            }
                            return;
                        }
                        OverlayMode::RawCommand => {
                            // Raw mode: send directly (strip leading ! if present)
                            let raw = if input.starts_with('!') { &input[1..] } else { &input };
                            let cmd = format!("{}\r", raw);
                            tw.command_overlay = None;

                            if let Some(targets) = targets {
                                for &i in &targets {
                                    if i < tw.panes.len() {
                                        tw.panes[i].write_input(cmd.as_bytes());
                                    }
                                }
                            } else {
                                for pane in &mut tw.panes {
                                    pane.write_input(cmd.as_bytes());
                                }
                            }
                        }
                        OverlayMode::Llm => {
                            if input.is_empty() {
                                return;
                            }
                            // Gather pane context and send to LLM
                            let pane_contexts: Vec<llm::PaneContext> = tw.panes.iter().enumerate()
                                .map(|(i, pane)| llm::PaneContext {
                                    index: i,
                                    pane_type: pane.pane_type_str().to_string(),
                                    title: pane.title().to_string(),
                                    visible_text: pane.visible_text(),
                                })
                                .collect();

                            let custom_system = self.config.llm.system_prompt.as_deref();
                            if let Some(ref client) = self.llm_client {
                                client.send(&input, &pane_contexts, custom_system);
                                let overlay = tw.command_overlay.as_mut().unwrap();
                                overlay.llm_thinking = true;
                            }
                        }
                    }
                    return;
                }
                Key::Named(NamedKey::Backspace) => {
                    overlay.input.pop();
                    // Update mode based on ! prefix
                    if self.llm_client.is_some() {
                        overlay.mode = if overlay.input.starts_with('!') {
                            OverlayMode::RawCommand
                        } else {
                            OverlayMode::Llm
                        };
                    } else {
                        overlay.mode = if overlay.input.starts_with('!') {
                            OverlayMode::RawCommand
                        } else {
                            OverlayMode::TokenInput
                        };
                    }
                    return;
                }
                Key::Character(c) => {
                    overlay.input.push_str(c.as_str());
                    // Update mode based on ! prefix
                    if self.llm_client.is_some() {
                        overlay.mode = if overlay.input.starts_with('!') {
                            OverlayMode::RawCommand
                        } else {
                            OverlayMode::Llm
                        };
                    } else {
                        overlay.mode = if overlay.input.starts_with('!') {
                            OverlayMode::RawCommand
                        } else {
                            OverlayMode::TokenInput
                        };
                    }
                    return;
                }
                Key::Named(NamedKey::Space) => {
                    overlay.input.push(' ');
                    return;
                }
                _ => return,
            }
        }

        // Termania keybindings (Cmd+key on macOS)
        if is_super {
            match &event.logical_key {
                // Cmd+C: copy selected text (or send Ctrl+C if no selection)
                Key::Character(c) if c.as_str() == "c" && !is_shift => {
                    if tw.text_selection.is_some() {
                        if let Some(text) = tw.selected_text() {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let _ = clipboard.set_text(text);
                            }
                        }
                        tw.window.request_redraw();
                        return;
                    }
                    // No selection: fall through to send Ctrl+C to terminal
                }
                // Cmd+V: paste from clipboard
                Key::Character(c) if c.as_str() == "v" && !is_shift => {
                    tw.text_selection = None;
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            tw.send_input(text.as_bytes());
                        }
                    }
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+X: copy selected text then clear selection (can't really cut in terminal)
                Key::Character(c) if c.as_str() == "x" && !is_shift => {
                    if tw.text_selection.is_some() {
                        if let Some(text) = tw.selected_text() {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let _ = clipboard.set_text(text);
                            }
                        }
                        tw.text_selection = None;
                        tw.window.request_redraw();
                        return;
                    }
                }
                // Cmd+Shift+N: new window
                Key::Character(c) if c.as_str() == "n" && is_shift => {
                    // Handled outside this function since we need &mut self + event_loop
                    // Signal via a flag — see window_event handler
                    return;
                }
                // Cmd+Option+N: add a new row with one pane
                _ if is_alt && matches!(event.physical_key, PhysicalKey::Code(KeyCode::KeyN)) => {
                    let idx = tw.panes.len();
                    tw.panes.push(Box::new(TerminalPlugin::new_bare(idx)));
                    tw.grid.add_row();
                    // Focus the new pane (last one, in the new row)
                    tw.focused_pane = tw.panes.len() - 1;
                    tw.sync_pane_sizes(&self.config);
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+N: add a pane to the right in the focused pane's row
                Key::Character(c) if c.as_str() == "n" => {
                    // Find which row the focused pane is in
                    let (row, _col) = tw.grid.pane_position(tw.focused_pane)
                        .unwrap_or((tw.grid.rows().saturating_sub(1), 0));
                    // Insert the new pane right after the last pane in this row
                    let insert_at = tw.grid.flat_index(row, tw.grid.cols_in_row(row).saturating_sub(1))
                        .map(|i| i + 1)
                        .unwrap_or(tw.panes.len());
                    let idx = tw.panes.len();
                    let pane = Box::new(TerminalPlugin::new_bare(idx));
                    tw.panes.insert(insert_at, pane);
                    tw.grid.add_col_to_row(row);
                    // Focus the new pane
                    tw.focused_pane = insert_at;
                    tw.sync_pane_sizes(&self.config);
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+W: close focused pane (or close window if last pane — handled in window_event)
                Key::Character(c) if c.as_str() == "w" => {
                    if tw.panes.len() <= 1 {
                        // Last pane: handled in window_event where event_loop is available
                        return;
                    }
                    if tw.focused_pane < tw.panes.len() {
                        // Find which row this pane is in before removing
                        let row = tw.grid.pane_position(tw.focused_pane)
                            .map(|(r, _c)| r)
                            .unwrap_or(0);
                        tw.panes[tw.focused_pane].shutdown();
                        tw.panes.remove(tw.focused_pane);
                        if tw.focused_pane < tw.effective_panes_config.len() {
                            tw.effective_panes_config.remove(tw.focused_pane);
                        }
                        // Shrink the row (or remove it if it becomes empty)
                        tw.grid.remove_col_from_row(row);
                    }
                    if tw.focused_pane > 0 && tw.focused_pane >= tw.panes.len() {
                        tw.focused_pane -= 1;
                    }
                    tw.sync_pane_sizes(&self.config);
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+R: rename focused pane
                Key::Character(c) if c.as_str() == "r" => {
                    if tw.focused_pane < tw.panes.len() {
                        let current = tw.panes[tw.focused_pane].title().to_string();
                        tw.rename_overlay = Some(current);
                    }
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+]: next pane
                Key::Character(c) if c.as_str() == "]" => {
                    let count = tw.panes.len();
                    if count > 0 {
                        tw.focused_pane = (tw.focused_pane + 1) % count;
                    }
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+[: previous pane
                Key::Character(c) if c.as_str() == "[" => {
                    let count = tw.panes.len();
                    if count > 0 {
                        tw.focused_pane = if tw.focused_pane == 0 {
                            count - 1
                        } else {
                            tw.focused_pane - 1
                        };
                    }
                    tw.window.request_redraw();
                    return;
                }
                // Cmd++: increase font size
                Key::Character(c) if c.as_str() == "+" || c.as_str() == "=" => {
                    tw.renderer.adjust_font_size(2.0);
                    tw.sync_pane_sizes(&self.config);
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+-: decrease font size
                Key::Character(c) if c.as_str() == "-" => {
                    tw.renderer.adjust_font_size(-2.0);
                    tw.sync_pane_sizes(&self.config);
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+Shift+B: toggle broadcast mode
                Key::Character(c) if c.as_str() == "b" && is_shift => {
                    tw.broadcast_mode = !tw.broadcast_mode;
                    log::info!("Broadcast mode: {}", if tw.broadcast_mode { "ON" } else { "OFF" });
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+Shift+Enter: open command overlay
                Key::Named(NamedKey::Enter) if is_shift => {
                    let targets = if tw.selected_panes.is_empty() {
                        None
                    } else {
                        Some(tw.selected_panes.clone())
                    };
                    tw.command_overlay = Some(CommandOverlay {
                        input: String::new(),
                        targets,
                        mode: if self.llm_client.is_some() { OverlayMode::Llm } else { OverlayMode::TokenInput },
                        llm_thinking: false,
                        llm_response: None,
                    });
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+Shift+A: select all panes
                Key::Character(c) if c.as_str() == "a" && is_shift => {
                    tw.selected_panes = (0..tw.panes.len()).collect();
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+Shift+D: deselect all panes
                Key::Character(c) if c.as_str() == "d" && is_shift => {
                    tw.selected_panes.clear();
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+/ or Cmd+?: toggle help overlay
                Key::Character(c) if c.as_str() == "/" || c.as_str() == "?" => {
                    tw.show_help = !tw.show_help;
                    tw.help_scroll = 0;
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+,: open config in default editor
                Key::Character(c) if c.as_str() == "," => {
                    let config_path = Config::config_path();
                    if let Err(e) = std::process::Command::new("open")
                        .arg(&config_path)
                        .spawn()
                    {
                        log::error!("Failed to open config file: {}", e);
                    }
                    return;
                }
                // Cmd+0: reset font size
                Key::Character(c) if c.as_str() == "0" => {
                    tw.renderer.set_font_size(self.config.font.size);
                    tw.sync_pane_sizes(&self.config);
                    tw.window.request_redraw();
                    return;
                }
                // Cmd+Shift+Arrow: swap focused pane with neighbor
                Key::Named(NamedKey::ArrowLeft) if is_shift => {
                    tw.swap_pane_in_direction(-1, 0, &self.config);
                    return;
                }
                Key::Named(NamedKey::ArrowRight) if is_shift => {
                    tw.swap_pane_in_direction(1, 0, &self.config);
                    return;
                }
                Key::Named(NamedKey::ArrowUp) if is_shift => {
                    tw.swap_pane_in_direction(0, -1, &self.config);
                    return;
                }
                Key::Named(NamedKey::ArrowDown) if is_shift => {
                    tw.swap_pane_in_direction(0, 1, &self.config);
                    return;
                }
                // Cmd+1..9: jump to pane by number
                Key::Character(c) => {
                    if let Ok(num) = c.as_str().parse::<usize>() {
                        if num >= 1 && num <= tw.panes.len() {
                            tw.focused_pane = num - 1;
                            tw.window.request_redraw();
                            return;
                        }
                    }
                }
                _ => {}
            }
        }

        // Skip forwarding to native-view panes (they handle their own input)
        if tw.focused_pane < tw.panes.len() && tw.panes[tw.focused_pane].is_native_view() {
            return;
        }

        // Forward input to focused terminal (or all if broadcast mode)
        let bytes = key_event_to_bytes(&event, is_ctrl, is_shift);
        if !bytes.is_empty() {
            // Clear text selection when actual input is sent to the terminal
            tw.text_selection = None;
            tw.send_input(&bytes);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.initial_window_created {
            return;
        }
        self.initial_window_created = true;

        let session = self.initial_session.take();
        self.create_window(event_loop, session);

        // Start the text tap server
        self.text_tap.start();

        info!("Termania v{} started", VERSION);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                info!("Window close requested");
                // Shutdown panes for this window
                if let Some(mut tw) = self.windows.remove(&window_id) {
                    for pane in &mut tw.panes {
                        pane.shutdown();
                    }
                }
                // Exit if no windows remain
                if self.windows.is_empty() {
                    self.text_tap.stop();
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(tw) = self.windows.get_mut(&window_id) {
                    tw.renderer.resize(size.width, size.height);
                    tw.last_resize = Some(Instant::now());
                    tw.resize_pending = true;
                    tw.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed && self.modifiers.super_key() {
                    // Cmd+Shift+N: new window (needs event_loop)
                    if self.modifiers.shift_key() {
                        if let Key::Character(ref c) = event.logical_key {
                            if c.as_str() == "n" {
                                self.create_new_default_window(event_loop);
                                return;
                            }
                        }
                    }
                    // Cmd+W on last pane: close the window (needs event_loop)
                    if let Key::Character(ref c) = event.logical_key {
                        if c.as_str() == "w" {
                            let is_last_pane = self.windows.get(&window_id)
                                .map(|tw| tw.panes.len() <= 1)
                                .unwrap_or(false);
                            if is_last_pane {
                                if let Some(mut tw) = self.windows.remove(&window_id) {
                                    for pane in &mut tw.panes {
                                        pane.shutdown();
                                    }
                                }
                                if self.windows.is_empty() {
                                    self.text_tap.stop();
                                    event_loop.exit();
                                }
                                return;
                            }
                        }
                    }
                }
                self.handle_key_input(window_id, event);
                if let Some(tw) = self.windows.get(&window_id) {
                    tw.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(tw) = self.windows.get_mut(&window_id) {
                    tw.cursor_position = (position.x, position.y);
                    // Update text selection drag
                    let sel_in_progress = tw.text_selection.as_ref().map(|s| s.in_progress).unwrap_or(false);
                    if sel_in_progress {
                        if let Some((_pi, row, col)) = tw.pixel_to_cell(position.x, position.y, &self.config) {
                            if let Some(ref mut sel) = tw.text_selection {
                                sel.end = (row, col);
                            }
                            tw.window.request_redraw();
                        }
                    }
                    // Update pane drag selection if in progress
                    if tw.drag_selecting {
                        if let Some(start_pane) = tw.drag_start_pane {
                            if let Some(current_pane) = tw.pane_at_position(position.x, position.y, &self.config) {
                                // Use grid positions for rectangle selection
                                if let (Some((sr, sc)), Some((cr, cc))) = (
                                    tw.grid.pane_position(start_pane),
                                    tw.grid.pane_position(current_pane),
                                ) {
                                    let min_row = sr.min(cr);
                                    let max_row = sr.max(cr);
                                    let min_col = sc.min(cc);
                                    let max_col = sc.max(cc);

                                    tw.selected_panes.clear();
                                    for r in min_row..=max_row.min(tw.grid.rows().saturating_sub(1)) {
                                        let row_cols = tw.grid.cols_in_row(r);
                                        for c in min_col..=max_col.min(row_cols.saturating_sub(1)) {
                                            if let Some(flat) = tw.grid.flat_index(r, c) {
                                                tw.selected_panes.insert(flat);
                                            }
                                        }
                                    }
                                }
                                tw.window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if let Some(tw) = self.windows.get_mut(&window_id) {
                    match state {
                        ElementState::Pressed => {
                            let (cx, cy) = tw.cursor_position;
                            if let Some(pane_idx) = tw.pane_at_position(cx, cy, &self.config) {
                                if self.modifiers.shift_key() {
                                    // Shift+click: pane drag selection (existing behavior)
                                    tw.drag_selecting = true;
                                    tw.drag_start_pane = Some(pane_idx);
                                    tw.selected_panes.clear();
                                    tw.selected_panes.insert(pane_idx);
                                } else if self.modifiers.super_key() && self.modifiers.control_key() {
                                    // Cmd+Ctrl+Click: toggle pane selection (moved from Cmd+Click)
                                    if tw.selected_panes.contains(&pane_idx) {
                                        tw.selected_panes.remove(&pane_idx);
                                    } else {
                                        tw.selected_panes.insert(pane_idx);
                                    }
                                } else if self.modifiers.control_key() {
                                    // Ctrl+Click: select non-space token within the same line
                                    tw.focused_pane = pane_idx;
                                    tw.selected_panes.clear();
                                    if let Some((pi, row, col)) = tw.pixel_to_cell(cx, cy, &self.config) {
                                        let render_data = tw.panes[pi].render_data();
                                        if let crate::plugin::PanePluginRenderData::Terminal { ref lines, .. } = render_data {
                                            if row < lines.len() {
                                                let line = &lines[row];
                                                let is_non_space = |c: char| c != ' ' && c != '\0';
                                                if col < line.len() && is_non_space(line[col].ch) {
                                                    // Scan left within this line
                                                    let mut sc = col;
                                                    while sc > 0 && sc - 1 < line.len() && is_non_space(line[sc - 1].ch) {
                                                        sc -= 1;
                                                    }
                                                    // Scan right within this line
                                                    let mut ec = col;
                                                    while ec + 1 < line.len() && is_non_space(line[ec + 1].ch) {
                                                        ec += 1;
                                                    }
                                                    tw.text_selection = Some(TextSelection {
                                                        pane_idx: pi,
                                                        start: (row, sc),
                                                        end: (row, ec),
                                                        in_progress: false,
                                                    });
                                                } else {
                                                    tw.text_selection = None;
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    // Plain click: focus pane + start text selection
                                    tw.focused_pane = pane_idx;
                                    tw.selected_panes.clear();

                                    // Check for double-click → word select
                                    let now = Instant::now();
                                    let is_double_click = tw.last_click_time
                                        .map(|t| now.duration_since(t).as_millis() < 400)
                                        .unwrap_or(false)
                                        && tw.last_click_pos
                                            .map(|(lx, ly)| (cx - lx).abs() < 4.0 && (cy - ly).abs() < 4.0)
                                            .unwrap_or(false);

                                    if is_double_click {
                                        // Double-click: word select
                                        if let Some((pi, row, col)) = tw.pixel_to_cell(cx, cy, &self.config) {
                                            let render_data = tw.panes[pi].render_data();
                                            if let crate::plugin::PanePluginRenderData::Terminal { ref lines, .. } = render_data {
                                                if row < lines.len() {
                                                    let line = &lines[row];
                                                    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
                                                    if col < line.len() && is_word_char(line[col].ch) {
                                                        let mut sc = col;
                                                        while sc > 0 && sc - 1 < line.len() && is_word_char(line[sc - 1].ch) {
                                                            sc -= 1;
                                                        }
                                                        let mut ec = col;
                                                        while ec + 1 < line.len() && is_word_char(line[ec + 1].ch) {
                                                            ec += 1;
                                                        }
                                                        tw.text_selection = Some(TextSelection {
                                                            pane_idx: pi,
                                                            start: (row, sc),
                                                            end: (row, ec),
                                                            in_progress: false,
                                                        });
                                                    } else {
                                                        tw.text_selection = None;
                                                    }
                                                }
                                            }
                                        }
                                        tw.last_click_time = None;
                                        tw.last_click_pos = None;
                                    } else {
                                        // Single click: start drag selection
                                        tw.last_click_time = Some(now);
                                        tw.last_click_pos = Some((cx, cy));
                                        if let Some((pi, row, col)) = tw.pixel_to_cell(cx, cy, &self.config) {
                                            tw.text_selection = Some(TextSelection {
                                                pane_idx: pi,
                                                start: (row, col),
                                                end: (row, col),
                                                in_progress: true,
                                            });
                                        } else {
                                            tw.text_selection = None;
                                        }
                                    }
                                }
                                tw.window.request_redraw();
                            }
                        }
                        ElementState::Released => {
                            tw.drag_selecting = false;
                            // Finalize text selection
                            if let Some(ref mut sel) = tw.text_selection {
                                sel.in_progress = false;
                                // If start == end (click with no drag), clear selection
                                if sel.start == sel.end {
                                    tw.text_selection = None;
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(tw) = self.windows.get_mut(&window_id) {
                    // Find which pane the cursor is over, fall back to focused pane
                    let (cx, cy) = tw.cursor_position;
                    let pane_idx = tw.pane_at_position(cx, cy, &self.config)
                        .unwrap_or(tw.focused_pane);

                    // Compute scroll lines from delta.
                    // winit gives the "content movement" direction:
                    //   y > 0 → content moves down → view scrolls toward history
                    //   y < 0 → content moves up → view scrolls toward live
                    // On macOS with natural scrolling, winit handles the inversion
                    // internally, so the convention is consistent.
                    let lines: i32 = match delta {
                        MouseScrollDelta::LineDelta(_x, y) => {
                            (y * -3.0) as i32
                        }
                        MouseScrollDelta::PixelDelta(pos) => {
                            tw.scroll_accumulator -= pos.y;
                            let cell_h = tw.renderer.cell_height() as f64;
                            let line_threshold = cell_h.max(10.0);
                            let l = (tw.scroll_accumulator / line_threshold) as i32;
                            if l != 0 {
                                tw.scroll_accumulator -= l as f64 * line_threshold;
                            }
                            l
                        }
                    };

                    if lines != 0 && pane_idx < tw.panes.len() {
                        // Clear text selection if scrolling the pane that has selection
                        if let Some(ref sel) = tw.text_selection {
                            if sel.pane_idx == pane_idx {
                                tw.text_selection = None;
                            }
                        }
                        if lines > 0 {
                            tw.panes[pane_idx].scroll_up(lines as usize);
                        } else {
                            tw.panes[pane_idx].scroll_down((-lines) as usize);
                        }
                    }
                    tw.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(tw) = self.windows.get_mut(&window_id) {
                    // Hide native views when help overlay is shown, otherwise update focus dimming
                    for i in 0..tw.panes.len() {
                        if tw.panes[i].is_native_view() {
                            if tw.show_help {
                                tw.panes[i].set_native_view_visible(false);
                            } else {
                                tw.panes[i].set_native_view_visible(true);
                                tw.panes[i].set_native_view_focused(i == tw.focused_pane);
                            }
                        }
                    }

                    // Broadcast to text tap listeners
                    for i in 0..tw.panes.len() {
                        if tw.panes[i].is_dirty() {
                            let content = tw.panes[i].visible_text();
                            self.text_tap.broadcast(i, &content);
                        }
                    }

                    // Gather pane data for rendering
                    let pane_data: Vec<renderer::PaneRenderData> = (0..tw.panes.len())
                        .map(|i| {
                            let watermark = tw.effective_panes_config.get(i)
                                .and_then(|p| p.watermark.clone());
                            let rename_input = if i == tw.focused_pane {
                                tw.rename_overlay.clone()
                            } else {
                                None
                            };
                            let text_selection = tw.text_selection.as_ref()
                                .filter(|s| s.pane_idx == i)
                                .map(|s| {
                                    let (sr, sc, er, ec) = TermaniaWindow::normalized_selection(s);
                                    (sr, sc, er, ec)
                                });
                            renderer::PaneRenderData {
                                title: tw.panes[i].title().to_string(),
                                plugin_data: tw.panes[i].render_data(),
                                is_focused: i == tw.focused_pane,
                                watermark,
                                has_error: tw.panes[i].has_error(),
                                is_selected: tw.selected_panes.contains(&i),
                                broadcast_mode: tw.broadcast_mode,
                                rename_input,
                                text_selection,
                            }
                        })
                        .collect();

                    let scale = tw.renderer.scale_factor();
                    let grid_layout = tw.grid.compute_layout(
                        tw.renderer.width(),
                        tw.renderer.height(),
                        &self.config,
                        scale,
                    );
                    let overlay = tw.command_overlay.as_ref().map(|o| {
                        let target_label = match o.mode {
                            OverlayMode::TokenInput => "Paste Anthropic OAuth token".to_string(),
                            _ => match &o.targets {
                                None => "Send to ALL panes".to_string(),
                                Some(targets) => {
                                    let names: Vec<String> = targets.iter()
                                        .map(|&i| format!("Pane {}", i + 1))
                                        .collect();
                                    format!("Send to: {}", names.join(", "))
                                }
                            },
                        };
                        let mode_label = match o.mode {
                            OverlayMode::Llm => "AI".to_string(),
                            OverlayMode::RawCommand => "CMD".to_string(),
                            OverlayMode::TokenInput => "TOKEN".to_string(),
                        };
                        let mut response_lines = Vec::new();
                        if let Some(ref resp) = o.llm_response {
                            response_lines.push(resp.explanation.clone());
                            for action in &resp.actions {
                                response_lines.push(llm::format_action_for_display(action));
                            }
                        }
                        let no_llm_hint = if matches!(o.mode, OverlayMode::TokenInput) {
                            Some("Run `claude login` to authenticate, then relaunch. Or paste a token and hit Enter.".to_string())
                        } else if self.llm_client.is_none() {
                            Some("Set CLAUDE_CODE_OAUTH_TOKEN or ANTHROPIC_API_KEY for AI mode".to_string())
                        } else {
                            None
                        };
                        // Mask token input
                        let display_text = if matches!(o.mode, OverlayMode::TokenInput) && !o.input.is_empty() {
                            "\u{2022}".repeat(o.input.len().min(40))
                        } else {
                            o.input.clone()
                        };
                        renderer::OverlayRenderData {
                            text: display_text,
                            target_label,
                            mode_label,
                            is_thinking: o.llm_thinking,
                            response_lines,
                            no_llm_hint,
                        }
                    });
                    tw.renderer.render(&pane_data, &grid_layout, overlay.as_ref(), tw.show_help, tw.help_scroll);

                    // Clear dirty flags after rendering
                    for pane in &mut tw.panes {
                        pane.clear_dirty();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let mut any_dirty_global = false;

        // Poll LLM status
        if let Some(ref client) = self.llm_client {
            if let Ok(status) = client.status_rx.try_recv() {
                // Find the window with an active LLM overlay
                for tw in self.windows.values_mut() {
                    if let Some(ref mut overlay) = tw.command_overlay {
                        if overlay.llm_thinking {
                            match status {
                                llm::LlmStatus::Thinking => {}
                                llm::LlmStatus::Complete(response) => {
                                    overlay.llm_thinking = false;
                                    overlay.llm_response = Some(response);
                                    tw.window.request_redraw();
                                }
                                llm::LlmStatus::Failed(err) => {
                                    overlay.llm_thinking = false;
                                    overlay.llm_response = Some(llm::LlmResponse {
                                        explanation: format!("Error: {}", err),
                                        actions: vec![],
                                    });
                                    tw.window.request_redraw();
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }

        for tw in self.windows.values_mut() {
            // Debounced resize
            if tw.resize_pending {
                if let Some(last) = tw.last_resize {
                    if last.elapsed().as_millis() >= 150 {
                        tw.sync_pane_sizes(&self.config);
                        tw.resize_pending = false;
                    }
                }
            }

            // Poll all pane plugins for new output
            for pane in &mut tw.panes {
                pane.poll();
            }

            // Remove exited panes
            let mut i = 0;
            let mut removed = false;
            while i < tw.panes.len() {
                if tw.panes[i].is_exited() && tw.panes.len() > 1 {
                    let row = tw.grid.pane_position(i).map(|(r, _)| r).unwrap_or(0);
                    tw.panes[i].shutdown();
                    tw.panes.remove(i);
                    if i < tw.effective_panes_config.len() {
                        tw.effective_panes_config.remove(i);
                    }
                    tw.grid.remove_col_from_row(row);
                    if tw.focused_pane >= tw.panes.len() && tw.focused_pane > 0 {
                        tw.focused_pane -= 1;
                    }
                    tw.selected_panes.clear();
                    removed = true;
                } else {
                    i += 1;
                }
            }
            if removed {
                tw.sync_pane_sizes(&self.config);
            }

            // Check for screenshot trigger file
            let screenshot_trigger = std::path::Path::new("/tmp/termania_screenshot_trigger").exists();

            let any_dirty = tw.panes.iter().any(|p| p.is_dirty());
            if any_dirty || removed || tw.command_overlay.is_some() || tw.resize_pending || screenshot_trigger {
                tw.window.request_redraw();
                any_dirty_global = true;
            }
        }

        // Process commands from text tap clients
        for cmd in self.text_tap.drain_commands() {
            // Text tap commands target panes by global index
            // For now, route to the first window's panes
            if let Some(tw) = self.windows.values_mut().next() {
                match cmd {
                    text_tap::TapCommand::Send { target, input } => {
                        match target {
                            text_tap::TapTarget::Pane(idx) => {
                                if idx < tw.panes.len() {
                                    tw.panes[idx].write_input(input.as_bytes());
                                }
                            }
                            text_tap::TapTarget::All => {
                                for pane in &mut tw.panes {
                                    pane.write_input(input.as_bytes());
                                }
                            }
                        }
                    }
                    text_tap::TapCommand::Action(action) => {
                        let result = tw.execute_action(&action, &self.config);
                        if let llm::ActionResult::Error { message } = result {
                            log::warn!("Text tap action failed: {}", message);
                        }
                        tw.window.request_redraw();
                    }
                }
            }
        }

        if !any_dirty_global {
            // Sleep briefly when idle to reduce CPU usage
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }
}

/// Convert a winit key event into bytes to send to the PTY
fn key_event_to_bytes(event: &KeyEvent, is_ctrl: bool, _is_shift: bool) -> Vec<u8> {
    match &event.logical_key {
        Key::Character(c) => {
            let s = c.as_str();
            if is_ctrl && s.len() == 1 {
                let ch = s.bytes().next().unwrap();
                // Ctrl+A..Z -> 0x01..0x1A
                if ch >= b'a' && ch <= b'z' {
                    return vec![ch - b'a' + 1];
                }
                if ch >= b'A' && ch <= b'Z' {
                    return vec![ch - b'A' + 1];
                }
            }
            s.as_bytes().to_vec()
        }
        Key::Named(named) => match named {
            NamedKey::Enter => vec![b'\r'],
            NamedKey::Backspace => vec![0x7f],
            NamedKey::Tab => vec![b'\t'],
            NamedKey::Escape => vec![0x1b],
            NamedKey::ArrowUp => b"\x1b[A".to_vec(),
            NamedKey::ArrowDown => b"\x1b[B".to_vec(),
            NamedKey::ArrowRight => b"\x1b[C".to_vec(),
            NamedKey::ArrowLeft => b"\x1b[D".to_vec(),
            NamedKey::Home => b"\x1b[H".to_vec(),
            NamedKey::End => b"\x1b[F".to_vec(),
            NamedKey::PageUp => b"\x1b[5~".to_vec(),
            NamedKey::PageDown => b"\x1b[6~".to_vec(),
            NamedKey::Insert => b"\x1b[2~".to_vec(),
            NamedKey::Delete => b"\x1b[3~".to_vec(),
            NamedKey::F1 => b"\x1bOP".to_vec(),
            NamedKey::F2 => b"\x1bOQ".to_vec(),
            NamedKey::F3 => b"\x1bOR".to_vec(),
            NamedKey::F4 => b"\x1bOS".to_vec(),
            NamedKey::F5 => b"\x1b[15~".to_vec(),
            NamedKey::F6 => b"\x1b[17~".to_vec(),
            NamedKey::F7 => b"\x1b[18~".to_vec(),
            NamedKey::F8 => b"\x1b[19~".to_vec(),
            NamedKey::F9 => b"\x1b[20~".to_vec(),
            NamedKey::F10 => b"\x1b[21~".to_vec(),
            NamedKey::F11 => b"\x1b[23~".to_vec(),
            NamedKey::F12 => b"\x1b[24~".to_vec(),
            NamedKey::Space => vec![b' '],
            _ => vec![],
        },
        _ => vec![],
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_version() {
    println!("termania {}", VERSION);
}

fn print_usage() {
    eprintln!("termania {}", VERSION);
    eprintln!();
    eprintln!("Usage: termania [OPTIONS] [SESSION_FILE]");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  [SESSION_FILE]    Path to a sessions.toml file");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -h, --help        Print this help message");
    eprintln!("  -v, --version     Print version");
    eprintln!();
    eprintln!("If no session file is given, Termania looks for:");
    eprintln!("  1. ./termania.toml");
    eprintln!("  2. ~/.config/termania/termania.toml");
    eprintln!("  3. Falls back to panes defined in config.toml");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  termania");
    eprintln!("  termania examples/gallery.toml");
    eprintln!("  termania ~/sessions/dev.toml");
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Parse CLI arguments
    let args: Vec<String> = std::env::args().collect();
    let mut session_path: Option<String> = None;

    for arg in &args[1..] {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-v" | "--version" => {
                print_version();
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => {
                eprintln!("error: unknown option: {}", arg);
                print_usage();
                std::process::exit(1);
            }
            _ => {
                if session_path.is_some() {
                    eprintln!("error: multiple session files specified");
                    print_usage();
                    std::process::exit(1);
                }
                session_path = Some(arg.clone());
            }
        }
    }

    let config = Config::load();
    let session = SessionConfig::load(session_path.as_deref());

    if session.is_some() {
        info!("Loaded session configuration");
    }

    let rows = config.effective_rows(session.as_ref());
    let cols = config.effective_cols(session.as_ref());
    info!("Config: {}x{} grid, font: {} @ {}pt",
        rows, cols, config.font.family, config.font.size);

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(config, session);
    event_loop.run_app(&mut app).expect("Event loop error");
}
