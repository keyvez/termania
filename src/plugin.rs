use crate::terminal::{Cell};

/// Types of pane plugins
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaneType {
    Terminal,
    WebView,
    Notes,
    ScreenCapture,
    FileBrowser,
    ProcessMonitor,
    LogViewer,
    MarkdownPreview,
    SystemInfo,
    GitStatus,
}

/// Data returned by a plugin for rendering
pub enum PanePluginRenderData {
    /// Terminal cell grid — rendered via wgpu (existing cell pipeline)
    Terminal {
        lines: Vec<Vec<Cell>>,
        cursor: (usize, usize),
        watermark: Option<String>,
    },
    /// A native NSView subview is managed by the plugin.
    /// The renderer draws border/title but skips cell rendering.
    /// The `view_id` is an opaque identifier used to track the view.
    NativeView {
        view_id: u64,
    },
    /// Plugin provides a wgpu texture to blit into the pane area.
    GpuTexture {
        // Future: wgpu::TextureView handle
        _placeholder: (),
    },
}

/// Trait that all pane plugins must implement
pub trait PanePlugin: Send {
    /// What type of plugin this is
    fn pane_type(&self) -> PaneType;

    /// Title for the pane's title bar
    fn title(&self) -> &str;

    /// Rename the pane
    fn set_title(&mut self, _title: String) {}

    /// Initialize the plugin (called after construction)
    fn init(&mut self) {}

    /// Shutdown and clean up resources
    fn shutdown(&mut self) {}

    /// Handle a character/text key press
    fn handle_key(&mut self, _text: &str) {}

    /// Resize notification with pixel dimensions and cell dimensions
    fn resize(&mut self, _width_px: f32, _height_px: f32, _cell_w: f32, _cell_h: f32) {}

    /// Get render data for this frame
    fn render_data(&self) -> PanePluginRenderData;

    /// Get visible text content (for text tap API)
    fn visible_text(&self) -> String;

    /// Write raw input bytes (for broadcast/command overlay)
    fn write_input(&mut self, _data: &[u8]) {}

    /// Poll for updates. Returns true if content changed.
    fn poll(&mut self) -> bool { false }

    /// Whether the plugin has detected an error
    fn has_error(&self) -> bool { false }

    /// Whether this plugin's content is "dirty" (needs redraw)
    fn is_dirty(&self) -> bool { false }

    /// Clear the dirty flag after rendering
    fn clear_dirty(&mut self) {}

    /// Scroll the view up into history by `lines` lines
    fn scroll_up(&mut self, _lines: usize) {}

    /// Scroll the view down toward live by `lines` lines
    fn scroll_down(&mut self, _lines: usize) {}

    /// Whether the plugin's process has exited (terminal shell died, etc.)
    fn is_exited(&self) -> bool { false }

    /// Whether this plugin uses a native macOS view (WKWebView, NSTextView, etc.)
    /// Native view plugins handle their own keyboard input when focused.
    fn is_native_view(&self) -> bool { false }

    /// Position the native view at the given pixel coordinates.
    /// Called on resize and layout changes. Only relevant for native view plugins.
    /// `window_height` is needed for macOS coordinate conversion (origin at bottom-left).
    fn position_native_view(
        &mut self,
        _x: f32,
        _y: f32,
        _width: f32,
        _height: f32,
        _window_height: f32,
    ) {}

    /// Show or hide the native view (e.g., when the window is resized or plugin is removed)
    fn set_native_view_visible(&mut self, _visible: bool) {}

    /// Set focus state on the native view (dims unfocused native views)
    fn set_native_view_focused(&mut self, _focused: bool) {}

    /// Initialize the native view with the parent NSView pointer.
    /// Called once during startup for native-view plugins (WebView, Notes).
    fn init_native_view_with_parent(&mut self, _parent_view: *mut std::ffi::c_void) {}

    /// Navigate to a URL (webview panes only). Returns true if supported.
    fn navigate(&mut self, _url: &str) -> bool { false }

    /// Set text content (notes panes only). Returns true if supported.
    fn set_content(&mut self, _content: &str) -> bool { false }

    /// Get the child process PID (terminal panes only)
    fn child_pid(&self) -> Option<u32> { None }

    /// Return the pane type as a string identifier
    fn pane_type_str(&self) -> &str {
        match self.pane_type() {
            PaneType::Terminal => "terminal",
            PaneType::WebView => "webview",
            PaneType::Notes => "notes",
            PaneType::ScreenCapture => "screen_capture",
            PaneType::FileBrowser => "file_browser",
            PaneType::ProcessMonitor => "process_monitor",
            PaneType::LogViewer => "log_viewer",
            PaneType::MarkdownPreview => "markdown_preview",
            PaneType::SystemInfo => "system_info",
            PaneType::GitStatus => "git_status",
        }
    }
}
