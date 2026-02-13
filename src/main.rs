#![allow(dead_code)]

mod config;
mod grid;
mod pty;
mod renderer;
mod terminal;
mod text_tap;

use std::sync::Arc;

use config::Config;
use grid::GridManager;
use log::info;
use renderer::Renderer;
use terminal::TerminalManager;
use text_tap::TextTapServer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

struct App {
    config: Arc<Config>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    grid: GridManager,
    terminals: TerminalManager,
    text_tap: TextTapServer,
    modifiers: ModifiersState,
    focused_pane: usize,
}

impl App {
    fn new(config: Config) -> Self {
        let config = Arc::new(config);
        let grid = GridManager::new(config.grid.rows, config.grid.cols);
        let pane_count = config.grid.rows * config.grid.cols;
        let terminals = TerminalManager::new(pane_count, &config);
        let text_tap = TextTapServer::new(&config.text_tap.socket_path);

        Self {
            config,
            window: None,
            renderer: None,
            grid,
            terminals,
            text_tap,
            modifiers: ModifiersState::empty(),
            focused_pane: 0,
        }
    }

    fn handle_key_input(&mut self, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        let mods = self.modifiers;
        let is_super = mods.super_key();
        let is_ctrl = mods.control_key();
        let is_shift = mods.shift_key();

        // Termania keybindings (Cmd+key on macOS)
        if is_super {
            match &event.logical_key {
                // Cmd+N: new pane
                Key::Character(c) if c.as_str() == "n" => {
                    self.terminals.spawn_pane(&self.config);
                    self.grid.set_dimensions(
                        self.config.grid.rows,
                        self.config.grid.cols,
                    );
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Cmd+W: close focused pane
                Key::Character(c) if c.as_str() == "w" => {
                    self.terminals.close_pane(self.focused_pane);
                    if self.focused_pane > 0 {
                        self.focused_pane -= 1;
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Cmd+]: next pane
                Key::Character(c) if c.as_str() == "]" => {
                    let count = self.terminals.pane_count();
                    if count > 0 {
                        self.focused_pane = (self.focused_pane + 1) % count;
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Cmd+[: previous pane
                Key::Character(c) if c.as_str() == "[" => {
                    let count = self.terminals.pane_count();
                    if count > 0 {
                        self.focused_pane = if self.focused_pane == 0 {
                            count - 1
                        } else {
                            self.focused_pane - 1
                        };
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                // Cmd++: increase font size
                Key::Character(c) if c.as_str() == "+" || c.as_str() == "=" => {
                    if let Some(renderer) = &mut self.renderer {
                        renderer.adjust_font_size(2.0);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // Cmd+-: decrease font size
                Key::Character(c) if c.as_str() == "-" => {
                    if let Some(renderer) = &mut self.renderer {
                        renderer.adjust_font_size(-2.0);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // Cmd+0: reset font size
                Key::Character(c) if c.as_str() == "0" => {
                    if let Some(renderer) = &mut self.renderer {
                        renderer.set_font_size(self.config.font.size);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                    return;
                }
                // Cmd+1..9: jump to pane by number
                Key::Character(c) => {
                    if let Ok(num) = c.as_str().parse::<usize>() {
                        if num >= 1 && num <= self.terminals.pane_count() {
                            self.focused_pane = num - 1;
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                    }
                }
                _ => {}
            }
        }

        // Forward input to focused terminal
        let bytes = key_event_to_bytes(&event, is_ctrl, is_shift);
        if !bytes.is_empty() {
            self.terminals.write_to_pane(self.focused_pane, &bytes);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("Termania")
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.window.width as f64,
                self.config.window.height as f64,
            ));

        let window = Arc::new(event_loop.create_window(attrs).expect("Failed to create window"));
        self.window = Some(window.clone());

        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            &self.config,
        ));
        self.renderer = Some(renderer);

        // Start the text tap server
        self.text_tap.start();

        info!("Termania started with {}x{} grid", self.config.grid.rows, self.config.grid.cols);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                info!("Window close requested");
                self.text_tap.stop();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key_input(event);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Poll all terminals for new output
                self.terminals.poll_all();

                // Broadcast to text tap listeners
                for i in 0..self.terminals.pane_count() {
                    if let Some(content) = self.terminals.get_visible_text(i) {
                        self.text_tap.broadcast(i, &content);
                    }
                }

                // Gather pane data for rendering
                let pane_data: Vec<renderer::PaneRenderData> = (0..self.terminals.pane_count())
                    .map(|i| {
                        let term = self.terminals.get_terminal(i);
                        let term = term.lock().unwrap();
                        renderer::PaneRenderData {
                            title: term.title().to_string(),
                            lines: term.visible_lines(),
                            cursor: term.cursor_position(),
                            is_focused: i == self.focused_pane,
                        }
                    })
                    .collect();

                if let Some(renderer) = &mut self.renderer {
                    let grid_layout = self.grid.compute_layout(
                        renderer.width(),
                        renderer.height(),
                        &self.config,
                    );
                    renderer.render(&pane_data, &grid_layout);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Request periodic redraws to update terminal content
        if let Some(window) = &self.window {
            window.request_redraw();
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

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config::load();
    info!("Loaded config: {}x{} grid, font: {} @ {}pt",
        config.grid.rows, config.grid.cols,
        config.font.family, config.font.size);

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(config);
    event_loop.run_app(&mut app).expect("Event loop error");
}
