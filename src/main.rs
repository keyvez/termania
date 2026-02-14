#![allow(dead_code)]

mod config;
mod grid;
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

/// Command overlay for sending commands to selected/all panes
struct CommandOverlay {
    /// The text being typed
    input: String,
    /// Target panes (None = all, Some = specific set)
    targets: Option<HashSet<usize>>,
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
}

impl App {
    fn new(config: Config, session: Option<SessionConfig>) -> Self {
        let config = Arc::new(config);
        let text_tap = TextTapServer::new(&config.text_tap.socket_path);

        Self {
            config,
            initial_session: session,
            windows: HashMap::new(),
            text_tap,
            modifiers: ModifiersState::empty(),
            last_option_press: None,
            initial_window_created: false,
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
        };

        // Initialize native views
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = window.window_handle() {
                if let RawWindowHandle::AppKit(appkit_handle) = handle.as_raw() {
                    let ns_view = appkit_handle.ns_view.as_ptr() as *mut std::ffi::c_void;
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
        if let Some(ref mut overlay) = tw.command_overlay {
            match &event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    tw.command_overlay = None;
                    return;
                }
                Key::Named(NamedKey::Enter) => {
                    let cmd = format!("{}\r", overlay.input);
                    let targets = overlay.targets.clone();
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
                    return;
                }
                Key::Named(NamedKey::Backspace) => {
                    overlay.input.pop();
                    return;
                }
                Key::Character(c) => {
                    overlay.input.push_str(c.as_str());
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
                // Cmd+W: close focused pane
                Key::Character(c) if c.as_str() == "w" => {
                    if tw.panes.len() > 1 && tw.focused_pane < tw.panes.len() {
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

        info!("Termania started");
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
                // Check for Cmd+Shift+N (new window) before dispatching to handle_key_input
                // since creating a new window requires &mut self + event_loop
                if event.state == ElementState::Pressed
                    && self.modifiers.super_key()
                    && self.modifiers.shift_key()
                {
                    if let Key::Character(ref c) = event.logical_key {
                        if c.as_str() == "n" {
                            self.create_new_default_window(event_loop);
                            return;
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
                    // Update drag selection if in progress
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
                                    tw.drag_selecting = true;
                                    tw.drag_start_pane = Some(pane_idx);
                                    tw.selected_panes.clear();
                                    tw.selected_panes.insert(pane_idx);
                                } else if self.modifiers.super_key() {
                                    if tw.selected_panes.contains(&pane_idx) {
                                        tw.selected_panes.remove(&pane_idx);
                                    } else {
                                        tw.selected_panes.insert(pane_idx);
                                    }
                                } else {
                                    tw.focused_pane = pane_idx;
                                    tw.selected_panes.clear();
                                }
                                tw.window.request_redraw();
                            }
                        }
                        ElementState::Released => {
                            tw.drag_selecting = false;
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
                            renderer::PaneRenderData {
                                title: tw.panes[i].title().to_string(),
                                plugin_data: tw.panes[i].render_data(),
                                is_focused: i == tw.focused_pane,
                                watermark,
                                has_error: tw.panes[i].has_error(),
                                is_selected: tw.selected_panes.contains(&i),
                                broadcast_mode: tw.broadcast_mode,
                                rename_input,
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
                        let target_label = match &o.targets {
                            None => "Send to ALL panes".to_string(),
                            Some(targets) => {
                                let names: Vec<String> = targets.iter()
                                    .map(|&i| format!("Pane {}", i + 1))
                                    .collect();
                                format!("Send to: {}", names.join(", "))
                            }
                        };
                        renderer::OverlayRenderData {
                            text: o.input.clone(),
                            target_label,
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
                match cmd.target {
                    text_tap::TapTarget::Pane(idx) => {
                        if idx < tw.panes.len() {
                            tw.panes[idx].write_input(cmd.input.as_bytes());
                        }
                    }
                    text_tap::TapTarget::All => {
                        for pane in &mut tw.panes {
                            pane.write_input(cmd.input.as_bytes());
                        }
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

fn print_usage() {
    eprintln!("Usage: termania [OPTIONS] [SESSION_FILE]");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  [SESSION_FILE]    Path to a sessions.toml file");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -h, --help        Print this help message");
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
