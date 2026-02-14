use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};

/// Screen Capture plugin — mirrors another macOS app window into a pane
pub struct ScreenCapturePlugin {
    title: String,
    /// Bundle ID of the target app (e.g., "com.apple.Safari")
    target_bundle_id: Option<String>,
    /// Window title to capture (alternative to bundle ID)
    target_title: Option<String>,
    dirty: bool,
    /// Whether the capture stream is active
    capturing: bool,
}

impl ScreenCapturePlugin {
    pub fn new(_index: usize, pane_config: Option<&PaneConfig>) -> Self {
        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| "Screen Capture".to_string());

        let target_bundle_id = pane_config.and_then(|p| p.target.clone());
        let target_title = pane_config.and_then(|p| p.target_title.clone());

        Self {
            title,
            target_bundle_id,
            target_title,
            dirty: true,
            capturing: false,
        }
    }

    /// Start the capture stream using ScreenCaptureKit
    pub fn start_capture(&mut self) {
        if self.capturing {
            return;
        }

        log::info!(
            "Screen capture: target_bundle_id={:?}, target_title={:?}",
            self.target_bundle_id,
            self.target_title
        );

        // TODO: Implement ScreenCaptureKit integration
        // 1. Find target window by bundle ID or title
        // 2. Create SCContentFilter for that window
        // 3. Create SCStreamConfiguration with appropriate resolution
        // 4. Start SCStream and capture frames
        // 5. Convert frames to wgpu textures

        self.capturing = true;
    }

    /// Stop the capture stream
    pub fn stop_capture(&mut self) {
        if !self.capturing {
            return;
        }

        // TODO: Stop SCStream
        self.capturing = false;
    }
}

impl PanePlugin for ScreenCapturePlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::ScreenCapture
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn init(&mut self) {
        self.start_capture();
    }

    fn shutdown(&mut self) {
        self.stop_capture();
    }

    fn render_data(&self) -> PanePluginRenderData {
        // TODO: Return GpuTexture with captured frame data
        PanePluginRenderData::GpuTexture {
            _placeholder: (),
        }
    }

    fn visible_text(&self) -> String {
        let target = self.target_bundle_id.as_deref()
            .or(self.target_title.as_deref())
            .unwrap_or("none");
        format!("[Screen Capture: {}]", target)
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}
