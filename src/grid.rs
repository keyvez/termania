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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_new() {
        let grid = GridManager::new(2, 3);
        assert_eq!(grid.rows(), 2);
        assert_eq!(grid.total_panes(), 6);
        assert_eq!(grid.cols_in_row(0), 3);
        assert_eq!(grid.cols_in_row(1), 3);
    }

    #[test]
    fn test_grid_1x1() {
        let grid = GridManager::new(1, 1);
        assert_eq!(grid.rows(), 1);
        assert_eq!(grid.total_panes(), 1);
    }

    #[test]
    fn test_pane_position_2x2() {
        let grid = GridManager::new(2, 2);
        assert_eq!(grid.pane_position(0), Some((0, 0)));
        assert_eq!(grid.pane_position(1), Some((0, 1)));
        assert_eq!(grid.pane_position(2), Some((1, 0)));
        assert_eq!(grid.pane_position(3), Some((1, 1)));
        assert_eq!(grid.pane_position(4), None);
    }

    #[test]
    fn test_flat_index() {
        let grid = GridManager::new(2, 3);
        assert_eq!(grid.flat_index(0, 0), Some(0));
        assert_eq!(grid.flat_index(0, 2), Some(2));
        assert_eq!(grid.flat_index(1, 0), Some(3));
        assert_eq!(grid.flat_index(1, 2), Some(5));
    }

    #[test]
    fn test_flat_index_out_of_bounds() {
        let grid = GridManager::new(2, 2);
        assert_eq!(grid.flat_index(2, 0), None);
        assert_eq!(grid.flat_index(0, 3), None);
    }

    #[test]
    fn test_add_col_to_row() {
        let mut grid = GridManager::new(2, 2);
        grid.add_col_to_row(0);
        assert_eq!(grid.cols_in_row(0), 3);
        assert_eq!(grid.cols_in_row(1), 2);
        assert_eq!(grid.total_panes(), 5);
    }

    #[test]
    fn test_remove_col_from_row() {
        let mut grid = GridManager::new(2, 3);
        let removed = grid.remove_col_from_row(0);
        assert!(!removed);
        assert_eq!(grid.cols_in_row(0), 2);
    }

    #[test]
    fn test_remove_col_removes_row() {
        let mut grid = GridManager::new(2, 1);
        let removed = grid.remove_col_from_row(0);
        assert!(removed);
        assert_eq!(grid.rows(), 1);
    }

    #[test]
    fn test_add_row() {
        let mut grid = GridManager::new(1, 2);
        grid.add_row();
        assert_eq!(grid.rows(), 2);
        assert_eq!(grid.cols_in_row(1), 1);
        assert_eq!(grid.total_panes(), 3);
    }

    #[test]
    fn test_cols_in_row_out_of_range() {
        let grid = GridManager::new(1, 2);
        assert_eq!(grid.cols_in_row(5), 0);
    }

    #[test]
    fn test_jagged_grid() {
        let mut grid = GridManager::new(1, 2);
        grid.add_row();
        grid.add_col_to_row(1);
        grid.add_col_to_row(1);
        // Row 0: 2 panes, Row 1: 3 panes
        assert_eq!(grid.total_panes(), 5);
        assert_eq!(grid.pane_position(2), Some((1, 0)));
        assert_eq!(grid.pane_position(4), Some((1, 2)));
    }

    #[test]
    fn test_compute_layout_basic() {
        let config = Config::default();
        let grid = GridManager::new(1, 1);
        let layouts = grid.compute_layout(800, 600, &config, 1.0);
        assert_eq!(layouts.len(), 1);
        assert!(layouts[0].width > 0.0);
        assert!(layouts[0].height > 0.0);
    }

    #[test]
    fn test_compute_layout_2x2() {
        let config = Config::default();
        let grid = GridManager::new(2, 2);
        let layouts = grid.compute_layout(800, 600, &config, 1.0);
        assert_eq!(layouts.len(), 4);
        // Second pane should be to the right of the first
        assert!(layouts[1].x > layouts[0].x);
        // Third pane should be below the first
        assert!(layouts[2].y > layouts[0].y);
    }

    #[test]
    fn test_compute_layout_all_positive() {
        let config = Config::default();
        let grid = GridManager::new(3, 3);
        let layouts = grid.compute_layout(1920, 1080, &config, 2.0);
        for layout in &layouts {
            assert!(layout.x >= 0.0);
            assert!(layout.y >= 0.0);
            assert!(layout.width > 0.0);
            assert!(layout.height > 0.0);
        }
    }
}
