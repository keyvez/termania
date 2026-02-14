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

/// Manages the grid layout of terminal panes.
///
/// The grid is a jagged table: each row can have a different number of columns.
/// All rows share the same height; within a row all panes share the same width.
pub struct GridManager {
    /// Number of columns in each row (length = number of rows).
    pub row_cols: Vec<usize>,
}

impl GridManager {
    /// Create a uniform grid with `rows` rows, each having `cols` columns.
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            row_cols: vec![cols; rows],
        }
    }

    /// Total number of rows.
    pub fn rows(&self) -> usize {
        self.row_cols.len()
    }

    /// Add a column to the given row (insert a pane slot to the right).
    pub fn add_col_to_row(&mut self, row: usize) {
        if row < self.row_cols.len() {
            self.row_cols[row] += 1;
        }
    }

    /// Remove a column from the given row. If the row becomes empty, remove it.
    /// Returns true if the row was removed entirely.
    pub fn remove_col_from_row(&mut self, row: usize) -> bool {
        if row < self.row_cols.len() {
            if self.row_cols[row] > 1 {
                self.row_cols[row] -= 1;
                false
            } else {
                self.row_cols.remove(row);
                true
            }
        } else {
            false
        }
    }

    /// Add a new row with one pane.
    pub fn add_row(&mut self) {
        self.row_cols.push(1);
    }

    /// Given a flat pane index, return (row, col_within_row).
    pub fn pane_position(&self, pane_idx: usize) -> Option<(usize, usize)> {
        let mut offset = 0;
        for (row, &cols) in self.row_cols.iter().enumerate() {
            if pane_idx < offset + cols {
                return Some((row, pane_idx - offset));
            }
            offset += cols;
        }
        None
    }

    /// Given (row, col), return the flat pane index.
    pub fn flat_index(&self, row: usize, col: usize) -> Option<usize> {
        if row >= self.row_cols.len() || col >= self.row_cols[row] {
            return None;
        }
        let offset: usize = self.row_cols[..row].iter().sum();
        Some(offset + col)
    }

    /// Number of columns in a given row.
    pub fn cols_in_row(&self, row: usize) -> usize {
        self.row_cols.get(row).copied().unwrap_or(0)
    }

    /// Compute pixel layout for each pane given the total window size.
    /// `scale` converts config values (logical pixels) to physical pixels.
    pub fn compute_layout(
        &self,
        window_width: u32,
        window_height: u32,
        config: &Config,
        scale: f32,
    ) -> Vec<PaneLayout> {
        let outer = config.grid.outer_padding as f32 * scale;
        let gap = config.grid.gap as f32 * scale;
        let title_h = config.grid.title_bar_height as f32 * scale;

        let num_rows = self.row_cols.len().max(1);

        let total_w = window_width as f32 - 2.0 * outer;
        let total_h = window_height as f32 - 2.0 * outer;

        let pane_h = (total_h - (num_rows as f32 - 1.0) * gap) / num_rows as f32;

        let mut layouts = Vec::new();

        for (row, &cols) in self.row_cols.iter().enumerate() {
            let cols = cols.max(1);
            let pane_w = (total_w - (cols as f32 - 1.0) * gap) / cols as f32;

            for col in 0..cols {
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

    /// Get the total number of pane slots in the grid.
    pub fn total_panes(&self) -> usize {
        self.row_cols.iter().sum()
    }
}
