use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};

/// Notes plugin — embeds an NSTextView in a pane for editable text
pub struct NotesPlugin {
    title: String,
    file_path: Option<String>,
    content: String,
    view_id: u64,
    /// Raw pointer to the NSScrollView containing the NSTextView
    ns_view: Option<*mut std::ffi::c_void>,
    dirty: bool,
}

// Safety: The NSView pointer is only accessed from the main thread.
unsafe impl Send for NotesPlugin {}

impl NotesPlugin {
    pub fn new(_index: usize, pane_config: Option<&PaneConfig>) -> Self {
        static NEXT_VIEW_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1000);

        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| "Notes".to_string());

        let file_path = pane_config.and_then(|p| p.file.clone());

        // Load content from file if specified, otherwise use initial content from config
        let content = if let Some(ref path) = file_path {
            let expanded = crate::config::expand_tilde(path);
            std::fs::read_to_string(&expanded).unwrap_or_default()
        } else {
            pane_config
                .and_then(|p| p.content.clone())
                .unwrap_or_default()
        };

        Self {
            title,
            file_path,
            content,
            view_id: NEXT_VIEW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ns_view: None,
            dirty: true,
        }
    }

    /// Initialize the NSTextView and add it as a subview
    pub fn init_notes_view(&mut self, parent_view: *mut std::ffi::c_void) {
        #[cfg(target_os = "macos")]
        {
            use objc2::rc::Retained;
            use objc2::MainThreadMarker;
            use objc2_app_kit::{
                NSAutoresizingMaskOptions, NSColor, NSFont, NSScrollView, NSTextView, NSView,
            };
            use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

            unsafe {
                let mtm = MainThreadMarker::new_unchecked();

                let frame = NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(100.0, 100.0),
                );

                // Create NSScrollView
                let scroll_view = NSScrollView::initWithFrame(
                    mtm.alloc(),
                    frame,
                );
                scroll_view.setHasVerticalScroller(true);
                scroll_view.setHasHorizontalScroller(false);

                // Create NSTextView
                let text_view = NSTextView::initWithFrame(
                    mtm.alloc(),
                    frame,
                );

                // Dark theme styling
                let bg_color = NSColor::colorWithSRGBRed_green_blue_alpha(
                    0.004, 0.016, 0.035, 1.0,
                );
                let fg_color = NSColor::colorWithSRGBRed_green_blue_alpha(
                    0.9, 0.93, 0.95, 1.0,
                );
                text_view.setBackgroundColor(&bg_color);
                text_view.setTextColor(Some(&fg_color));
                text_view.setInsertionPointColor(Some(&fg_color));

                // Set font
                let font = NSFont::monospacedSystemFontOfSize_weight(14.0, 0.0);
                text_view.setFont(Some(&font));

                // Set initial content
                if !self.content.is_empty() {
                    let ns_string = NSString::from_str(&self.content);
                    text_view.setString(&ns_string);
                }

                // Make editable
                text_view.setEditable(true);
                text_view.setSelectable(true);
                text_view.setRichText(false);

                // Auto-resize
                text_view.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable,
                );

                // Set as document view of scroll view
                let text_as_view: &NSView = text_view.as_ref();
                scroll_view.setDocumentView(Some(text_as_view));

                // Start hidden — will be shown after position_native_view is called
                scroll_view.setHidden(true);

                // Add scroll view as subview
                let parent: &NSView = &*(parent_view as *const NSView);
                let scroll_as_view: &NSView = scroll_view.as_ref();
                parent.addSubview(scroll_as_view);

                // Store the raw pointer to the scroll view
                let ptr: *mut NSScrollView = Retained::into_raw(scroll_view);
                self.ns_view = Some(ptr as *mut std::ffi::c_void);
            }
        }
    }

    /// Remove the notes view from its superview
    pub fn remove_notes_view(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view.take() {
                unsafe {
                    use objc2_app_kit::NSView;
                    let view = &*(ptr as *const NSView);
                    view.removeFromSuperview();
                    use objc2::rc::Retained;
                    use objc2_app_kit::NSScrollView;
                    let _ = Retained::from_raw(ptr as *mut NSScrollView);
                }
            }
        }
    }

    /// Save content to file if a file path is configured
    pub fn save_to_file(&self) {
        if let Some(ref path) = self.file_path {
            let expanded = crate::config::expand_tilde(path);
            if let Err(e) = std::fs::write(&expanded, &self.content) {
                log::error!("Failed to save notes to {}: {}", expanded, e);
            }
        }
    }

    /// Set the text content programmatically
    pub fn set_text_content(&mut self, content: &str) {
        self.content = content.to_string();
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view {
                unsafe {
                    use objc2_app_kit::{NSScrollView, NSTextView};
                    use objc2_foundation::NSString;

                    let scroll_view = &*(ptr as *const NSScrollView);
                    if let Some(doc_view) = scroll_view.documentView() {
                        let text_view: &NSTextView = &*((&*doc_view) as *const _ as *const NSTextView);
                        let ns_string = NSString::from_str(content);
                        text_view.setString(&ns_string);
                    }
                }
            }
        }
        self.dirty = true;
    }

    /// Read current text content from the NSTextView
    fn read_text_from_view(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view {
                unsafe {
                    use objc2_app_kit::{NSScrollView, NSTextView};

                    let scroll_view = &*(ptr as *const NSScrollView);
                    if let Some(doc_view) = scroll_view.documentView() {
                        // Cast to NSTextView
                        let text_view: &NSTextView = &*((&*doc_view) as *const _ as *const NSTextView);
                        let ns_string = text_view.string();
                        self.content = ns_string.to_string();
                    }
                }
            }
        }
    }
}

impl PanePlugin for NotesPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::Notes
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn init_native_view_with_parent(&mut self, parent_view: *mut std::ffi::c_void) {
        self.init_notes_view(parent_view);
    }

    fn shutdown(&mut self) {
        // Save content before removing
        self.read_text_from_view();
        self.save_to_file();
        self.remove_notes_view();
    }

    fn render_data(&self) -> PanePluginRenderData {
        PanePluginRenderData::NativeView {
            view_id: self.view_id,
        }
    }

    fn visible_text(&self) -> String {
        self.content.clone()
    }

    fn poll(&mut self) -> bool {
        // Periodically sync content from NSTextView
        let old_content = self.content.clone();
        self.read_text_from_view();
        old_content != self.content
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn is_native_view(&self) -> bool {
        true
    }

    fn position_native_view(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        _window_height: f32,
    ) {
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view {
                unsafe {
                    use objc2_app_kit::NSView;
                    use objc2_foundation::{NSPoint, NSRect, NSSize};

                    let view = &*(ptr as *const NSView);
                    // Winit's content view is flipped (isFlipped=true), so the
                    // coordinate origin is at top-left, matching our grid layout.
                    // Convert physical pixels to logical points.
                    let scale = view.window()
                        .map(|w| w.backingScaleFactor())
                        .unwrap_or(2.0);
                    let lx = x as f64 / scale;
                    let ly = y as f64 / scale;
                    let lw = width as f64 / scale;
                    let lh = height as f64 / scale;
                    let frame = NSRect::new(
                        NSPoint::new(lx, ly),
                        NSSize::new(lw, lh),
                    );
                    view.setFrame(frame);
                    // Show the view now that it has the correct position
                    view.setHidden(false);
                }
            }
        }
    }

    fn set_native_view_visible(&mut self, visible: bool) {
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view {
                unsafe {
                    use objc2_app_kit::NSView;
                    let view = &*(ptr as *const NSView);
                    view.setHidden(!visible);
                }
            }
        }
    }

    fn set_native_view_focused(&mut self, focused: bool) {
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view {
                unsafe {
                    use objc2_app_kit::NSView;
                    let view = &*(ptr as *const NSView);
                    view.setAlphaValue(if focused { 1.0 } else { 0.7 });
                }
            }
        }
    }

    fn set_content(&mut self, content: &str) -> bool {
        self.set_text_content(content);
        true
    }
}

impl Drop for NotesPlugin {
    fn drop(&mut self) {
        self.remove_notes_view();
    }
}
