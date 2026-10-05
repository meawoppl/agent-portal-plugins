//! Port of `kicad_tools.router.optimizer.config`.

/// Configuration for trace optimization.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizationConfig {
    pub merge_collinear: bool,
    pub eliminate_zigzags: bool,
    pub convert_45_corners: bool,
    pub compress_staircase: bool,
    pub pull_tight: bool,
    pub pull_tight_max_iterations: usize,
    pub minimize_vias: bool,
    pub min_staircase_segments: usize,
    pub min_segment_length: f64,
    pub corner_chamfer_size: f64,
    pub via_max_detour_factor: f64,
    pub via_pair_threshold: f64,
    pub tolerance: f64,
    pub drc_aware: bool,
    pub drc_manufacturer: Option<String>,
    pub drc_layers: i64,
    pub drc_copper_oz: f64,
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        OptimizationConfig {
            merge_collinear: true,
            eliminate_zigzags: true,
            convert_45_corners: true,
            compress_staircase: true,
            pull_tight: true,
            pull_tight_max_iterations: 10,
            minimize_vias: true,
            min_staircase_segments: 3,
            min_segment_length: 0.05,
            corner_chamfer_size: 0.5,
            via_max_detour_factor: 1.5,
            via_pair_threshold: 2.0,
            tolerance: 1e-4,
            drc_aware: false,
            drc_manufacturer: None,
            drc_layers: 2,
            drc_copper_oz: 1.0,
        }
    }
}

/// Statistics from trace optimization.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptimizationStats {
    pub segments_before: usize,
    pub segments_after: usize,
    pub corners_before: usize,
    pub corners_after: usize,
    pub length_before: f64,
    pub length_after: f64,
    pub nets_optimized: usize,
    pub vias_before: usize,
    pub vias_after: usize,
    pub nets_rolled_back: usize,
    pub drc_errors_before: usize,
    pub drc_errors_after: usize,
}

impl OptimizationStats {
    /// Percentage reduction in segment count.
    pub fn segment_reduction(&self) -> f64 {
        if self.segments_before == 0 {
            return 0.0;
        }
        (1.0 - self.segments_after as f64 / self.segments_before as f64) * 100.0
    }

    /// Percentage reduction in total length.
    pub fn length_reduction(&self) -> f64 {
        if self.length_before == 0.0 {
            return 0.0;
        }
        (1.0 - self.length_after / self.length_before) * 100.0
    }

    /// Percentage reduction in via count.
    pub fn via_reduction(&self) -> f64 {
        if self.vias_before == 0 {
            return 0.0;
        }
        (1.0 - self.vias_after as f64 / self.vias_before as f64) * 100.0
    }
}
