// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

//! Gutter rendering support for line numbers and status indicators.
//!
//! The gutter is displayed on the left side of file-backed buffers and shows:
//! - Line numbers (right-aligned)
//! - Line modification status (modified, saved, conflict)

use std::collections::HashSet;

/// Status of a line relative to the base version on disk
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStatus {
    /// Line is unchanged from the base version
    Clean,
    /// Line has been modified locally (unsaved changes)
    Modified,
    /// Line was modified and merged but not yet saved
    ModifiedSaved,
    /// Line contains conflict markers
    Conflict,
}

/// Information needed to render a line's gutter
#[derive(Debug, Clone)]
pub struct GutterLine {
    /// 1-based line number
    pub line_number: usize,
    /// Current status of the line
    pub status: LineStatus,
}

/// Configuration for gutter rendering
#[derive(Debug, Clone)]
pub struct GutterConfig {
    /// Whether to show line numbers
    pub show_line_numbers: bool,
    /// Whether to show status indicators
    pub show_status: bool,
    /// Minimum width for line numbers (in characters)
    pub min_line_number_width: usize,
}

impl Default for GutterConfig {
    fn default() -> Self {
        Self {
            show_line_numbers: true,
            show_status: true,
            min_line_number_width: 3,
        }
    }
}

/// Calculate the width of the gutter in characters
///
/// Gutter layout: [status][line_number][separator]
/// - status: 1 char (modification indicator)
/// - line_number: max(min_width, digits needed for total_lines)
/// - separator: 1 char (space or line)
pub fn calculate_gutter_width(total_lines: usize, config: &GutterConfig) -> usize {
    if !config.show_line_numbers && !config.show_status {
        return 0;
    }

    let mut width = 0;

    // Status indicator
    if config.show_status {
        width += 1;
    }

    // Line number width
    if config.show_line_numbers {
        let digits_needed = if total_lines == 0 {
            1
        } else {
            ((total_lines as f64).log10().floor() as usize) + 1
        };
        width += digits_needed.max(config.min_line_number_width);
    }

    // Separator
    width += 1;

    width
}

/// Determine line status by checking if the line is in the modified set
/// and if it contains conflict markers
pub fn get_line_status(
    line_content: &str,
    line_index: usize,
    modified_lines: &HashSet<usize>,
    merged_lines: &HashSet<usize>,
) -> LineStatus {
    // Check for conflict markers first
    let trimmed = line_content.trim_start();
    if trimmed.starts_with("<<<<<<<")
        || trimmed.starts_with("=======")
        || trimmed.starts_with(">>>>>>>")
    {
        return LineStatus::Conflict;
    }

    // Check if line was part of a merge (modified but from external source)
    if merged_lines.contains(&line_index) {
        return LineStatus::ModifiedSaved;
    }

    // Check if line is modified locally
    if modified_lines.contains(&line_index) {
        return LineStatus::Modified;
    }

    LineStatus::Clean
}

/// Format line number for display (right-aligned)
pub fn format_line_number(line_number: usize, width: usize) -> String {
    format!("{:>width$}", line_number, width = width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gutter_width_small_file() {
        let config = GutterConfig::default();
        // 10 lines needs 2 digits, but min is 3, plus 1 status + 1 separator = 5
        assert_eq!(calculate_gutter_width(10, &config), 5);
    }

    #[test]
    fn test_gutter_width_large_file() {
        let config = GutterConfig::default();
        // 1000 lines needs 4 digits, plus 1 status + 1 separator = 6
        assert_eq!(calculate_gutter_width(1000, &config), 6);
    }

    #[test]
    fn test_gutter_width_no_status() {
        let config = GutterConfig {
            show_status: false,
            ..Default::default()
        };
        // No status, 3 digit min + 1 separator = 4
        assert_eq!(calculate_gutter_width(10, &config), 4);
    }

    #[test]
    fn test_line_status_conflict() {
        let modified = HashSet::new();
        let merged = HashSet::new();
        assert_eq!(
            get_line_status("<<<<<<< LOCAL", 0, &modified, &merged),
            LineStatus::Conflict
        );
        assert_eq!(
            get_line_status("=======", 0, &modified, &merged),
            LineStatus::Conflict
        );
        assert_eq!(
            get_line_status(">>>>>>> EXTERNAL", 0, &modified, &merged),
            LineStatus::Conflict
        );
    }

    #[test]
    fn test_line_status_modified() {
        let mut modified = HashSet::new();
        modified.insert(5);
        let merged = HashSet::new();

        assert_eq!(
            get_line_status("some content", 5, &modified, &merged),
            LineStatus::Modified
        );
        assert_eq!(
            get_line_status("other content", 6, &modified, &merged),
            LineStatus::Clean
        );
    }

    #[test]
    fn test_format_line_number() {
        assert_eq!(format_line_number(1, 3), "  1");
        assert_eq!(format_line_number(42, 3), " 42");
        assert_eq!(format_line_number(999, 3), "999");
        assert_eq!(format_line_number(1000, 4), "1000");
    }
}
