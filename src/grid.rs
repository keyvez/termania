use crate::config::Config;

/// Describes the layout rectangle for a single pane
#[derive(Debug, Clone)]
pub struct PaneLayout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub title_height: f32,
}

/// Manages the grid layout of terminal panes
pub struct GridManager {
    pub rows: usize,
    pub cols: usize,
}

impl GridManager {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self { rows, cols }
    }

    pub fn set_dimensions(&mut self, rows: usize, cols: usize) {
        self.rows = rows;
        self.cols = cols;
    }

    /// Compute pixel layout for each pane given the total window size
    pub fn compute_layout(
        &self,
        window_width: u32,
        window_height: u32,
        config: &Config,
    ) -> Vec<PaneLayout> {
        let outer = config.grid.outer_padding as f32;
        let gap = config.grid.gap as f32;
        let title_h = config.grid.title_bar_height as f32;

        let total_w = window_width as f32 - 2.0 * outer;
        let total_h = window_height as f32 - 2.0 * outer;

        let pane_w = (total_w - (self.cols as f32 - 1.0) * gap) / self.cols as f32;
        let pane_h = (total_h - (self.rows as f32 - 1.0) * gap) / self.rows as f32;

        let mut layouts = Vec::new();

        for row in 0..self.rows {
            for col in 0..self.cols {
                let x = outer + col as f32 * (pane_w + gap);
                let y = outer + row as f32 * (pane_h + gap);

                layouts.push(PaneLayout {
                    x,
                    y,
                    width: pane_w,
                    height: pane_h,
                    title_height: title_h,
                });
            }
        }

        layouts
    }

    /// Get the number of panes in the grid
    pub fn total_panes(&self) -> usize {
        self.rows * self.cols
    }
}
