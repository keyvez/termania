# Agent Learnings

## Rendering: Opaque Overlays in wgpu

When rendering overlays (command overlay, help panel, etc.) that need to be fully opaque and cover the content behind them, setting `alpha: 1.0` on the background color is **not sufficient** if all geometry is batched into shared vertex/index buffers and drawn in pipeline-order (rounded rects -> rects -> text).

The problem: the overlay background (rounded rect) is drawn in the same batch as pane backgrounds, but pane **text** is drawn later in the text pass, so it renders on top of the overlay background.

The fix: render the overlay into **separate vertex/index buffers** (`cmd_rounded_rect_vertices`, `cmd_rect_vertices`, `cmd_text_vertices`) and issue separate draw calls for them **after** the main text pass. This ensures the overlay's background occludes all pane content, and the overlay's own text renders on top.

Draw order should be:
1. Main rounded rects (pane backgrounds/borders)
2. Main rects (cursors, selections, etc.)
3. Main text (terminal content)
4. **Overlay rounded rects** (overlay background/border)
5. **Overlay rects** (badge backgrounds, separators)
6. **Overlay text** (overlay labels, input, hints)
7. Help panel rects (if shown)
8. Help panel text (if shown)

See `renderer.rs` for the implementation pattern — the help overlay already followed this approach, and the command overlay was refactored to match.
