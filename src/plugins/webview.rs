use crate::config::PaneConfig;
use crate::plugin::{PanePlugin, PanePluginRenderData, PaneType};

/// WebView plugin — embeds a WKWebView in a pane
pub struct WebViewPlugin {
    title: String,
    url: String,
    view_id: u64,
    /// The raw pointer to the WKWebView (as NSView)
    ns_view: Option<*mut std::ffi::c_void>,
    dirty: bool,
}

// Safety: The NSView pointer is only accessed from the main thread,
// which is the same thread that owns the App/event loop.
unsafe impl Send for WebViewPlugin {}

impl WebViewPlugin {
    pub fn new(_index: usize, pane_config: Option<&PaneConfig>) -> Self {
        static NEXT_VIEW_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

        let title = pane_config
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| "WebView".to_string());

        let url = pane_config
            .and_then(|p| p.url.clone())
            .unwrap_or_else(|| "https://example.com".to_string());

        Self {
            title,
            url,
            view_id: NEXT_VIEW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ns_view: None,
            dirty: true,
        }
    }

    /// Initialize the WKWebView and add it as a subview of the given parent NSView.
    /// `parent_view` is a raw pointer to the window's contentView (NSView*).
    pub fn init_webview(&mut self, parent_view: *mut std::ffi::c_void) {
        #[cfg(target_os = "macos")]
        {
            use objc2::rc::Retained;
            use objc2::MainThreadMarker;
            use objc2_foundation::{NSString, NSURL, NSURLRequest};
            use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
            use objc2_app_kit::NSView;
            use objc2_foundation::{NSRect, NSPoint, NSSize};

            unsafe {
                let mtm = MainThreadMarker::new_unchecked();

                // Create WKWebView configuration
                let config = WKWebViewConfiguration::new(mtm);

                // Create WKWebView with a frame (will be positioned later)
                let frame = NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(100.0, 100.0),
                );
                let webview = WKWebView::initWithFrame_configuration(
                    mtm.alloc(),
                    frame,
                    &config,
                );

                // Start hidden — will be shown after position_native_view is called
                webview.setHidden(true);

                // Load the URL
                let url_string = NSString::from_str(&self.url);
                if let Some(url) = NSURL::URLWithString(&url_string) {
                    let request = NSURLRequest::requestWithURL(&url);
                    webview.loadRequest(&request);
                }

                // Add as subview
                let parent: &NSView = &*(parent_view as *const NSView);
                let webview_as_view: &NSView = webview.as_ref();
                parent.addSubview(webview_as_view);

                // Store the raw pointer
                let ptr: *mut WKWebView = Retained::into_raw(webview);
                self.ns_view = Some(ptr as *mut std::ffi::c_void);
            }
        }
    }

    /// Navigate the existing WKWebView to a new URL
    pub fn navigate_to(&mut self, url: &str) {
        self.url = url.to_string();
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view {
                unsafe {
                    use objc2_foundation::{NSString, NSURL, NSURLRequest};
                    use objc2_web_kit::WKWebView;

                    let webview = &*(ptr as *const WKWebView);
                    let url_string = NSString::from_str(url);
                    if let Some(url_obj) = NSURL::URLWithString(&url_string) {
                        let request = NSURLRequest::requestWithURL(&url_obj);
                        webview.loadRequest(&request);
                    }
                }
            }
        }
        self.dirty = true;
    }

    /// Remove the webview from its superview
    pub fn remove_webview(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if let Some(ptr) = self.ns_view.take() {
                unsafe {
                    use objc2_app_kit::NSView;
                    let view = &*(ptr as *const NSView);
                    view.removeFromSuperview();
                    // Re-create the Retained to drop it properly
                    use objc2::rc::Retained;
                    use objc2_web_kit::WKWebView;
                    let _ = Retained::from_raw(ptr as *mut WKWebView);
                }
            }
        }
    }
}

impl PanePlugin for WebViewPlugin {
    fn pane_type(&self) -> PaneType {
        PaneType::WebView
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn init(&mut self) {}

    fn init_native_view_with_parent(&mut self, parent_view: *mut std::ffi::c_void) {
        self.init_webview(parent_view);
    }

    fn shutdown(&mut self) {
        self.remove_webview();
    }

    fn render_data(&self) -> PanePluginRenderData {
        PanePluginRenderData::NativeView {
            view_id: self.view_id,
        }
    }

    fn visible_text(&self) -> String {
        format!("[WebView: {}]", self.url)
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
                    use objc2_foundation::{NSRect, NSPoint, NSSize};

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

    fn navigate(&mut self, url: &str) -> bool {
        self.navigate_to(url);
        true
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
}

impl Drop for WebViewPlugin {
    fn drop(&mut self) {
        self.remove_webview();
    }
}
