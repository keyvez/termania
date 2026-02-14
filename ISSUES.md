# Termania Issue Tracker

## Open Issues

### #1 - Trackpad scroll not working [FIXED]
Mouse/trackpad scroll events are not scrolling through the scrollback buffer. The `MouseWheel` handler was added but scroll gestures have no visible effect.

**Root causes found and fixed:**
1. `PixelDelta` from trackpad gave small fractional values that truncated to 0 lines — added `scroll_accumulator` to `TermaniaWindow` to accumulate sub-line deltas until they reach a full line threshold.
2. `process_output()` reset `scroll_offset` to 0 on every PTY read — even idle cursor blink escapes would snap back instantly. Removed auto-scroll from `process_output`; scroll-to-bottom now only happens on user input (`write_input`).
3. If `pane_at_position` missed (returns `None`), scroll was silently discarded — added fallback to `focused_pane`.

---

### #2 - Clicking on a pane activates the wrong pane [FIXED]
Tapping/clicking on a pane focuses a different pane than the one clicked. Clicking again selects yet another pane. The hit-testing in `pane_at_position()` is returning incorrect pane indices.

**Root cause:** Double-scaling on Retina. winit 0.30's `CursorMoved` gives `PhysicalPosition<f64>` — coordinates are already in physical pixels. But `pane_at_position()` was multiplying them by `scale_factor` again, so a click at physical (400, 400) was being tested against (800, 800), landing on the wrong pane.

---

### #3 - Character spacing gaps in Powerlevel10k prompt [FIXED]
Visible gaps between characters in colored prompt segments (e.g., git branch, status icons). Only affects regions with ANSI color codes / special Unicode glyphs from the P10K lean theme.

**Root causes found and fixed:**
1. **Wrong font for cell measurement:** `measure_cell_size()` used cosmic-text layout which resolved "SF Mono" to `.SFNS-Regular` (system proportional sans-serif) instead of `.SF NS Mono` (the actual monospace font). This gave cell_w=23.3px instead of 17.3px. Fixed by creating `resolve_font_family()` that maps "SF Mono" → ".SF NS Mono" via fontdb alias lookup, and using `fontdb::with_face_data()` + ttf-parser to read the correct advance width directly from the font tables.
2. **Wrong font for glyph rendering:** cosmic-text fell back to `Menlo-Bold` and `.SFNS-Regular` for glyph rasterization because `.SF NS Mono` only exposes Light (weight 295) faces via fontdb on macOS — no Regular (400) or Bold (700). Requesting `Weight::NORMAL` or `Weight::BOLD` caused cosmic-text to pick different fonts entirely. Fixed by discovering available font weights at startup and using the actual available weight (295) in all `Attrs` for cosmic-text rendering.
