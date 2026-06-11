//! Terminal grid reflow for mobile rendering.
//!
//! Takes CompactLine data from the desktop terminal (e.g. 120 cols) and reflows
//! it to a different width (e.g. 50 cols for mobile). Uses the `wrapped` flag
//! on each line to reconstruct logical lines, then re-wraps them at the target width.
//!
//! Based on WezTerm's `Screen::rewrap_lines()` algorithm (MIT licensed).

use super::grid_types::{CompactLine, GridUpdate, StyleSpan};

/// A logical line — one or more physical rows joined together.
/// Represents content from one hard line break to the next.
struct LogicalLine {
    text: String,
    spans: Vec<StyleSpan>,
}

/// Reflow a GridUpdate from its original column width to a target width.
/// Returns a new GridUpdate with lines re-wrapped at `target_cols`.
pub fn reflow_grid(grid: &GridUpdate, target_cols: u16, target_rows: u16) -> GridUpdate {
    let _h = crate::perf_hist!("reflow");
    if target_cols == 0 || target_rows == 0 {
        return grid.clone();
    }

    // Phase 1: Join soft-wrapped lines into logical lines
    let logical_lines = join_wrapped_lines(&grid.lines);

    // Phase 2: Re-wrap logical lines at target width
    let mut reflowed: Vec<CompactLine> = Vec::new();
    for logical in &logical_lines {
        let wrapped_lines = wrap_logical_line(logical, target_cols);
        reflowed.extend(wrapped_lines);
    }

    // Assign row indices and trim to target_rows (show the last N rows, like a terminal).
    // The number of lines trimmed off the top becomes the display_offset —
    // this tells the mobile app that row 0 is actually absolute row `start`
    // in the full reflowed history. The mobile app can use this to build a
    // continuous scrollable buffer: absolute_row = display_offset + row.
    let total = reflowed.len();
    let start = if total > target_rows as usize { total - target_rows as usize } else { 0 };
    let visible: Vec<CompactLine> = reflowed[start..]
        .iter()
        .enumerate()
        .map(|(i, line)| CompactLine {
            row: i as u16,
            text: line.text.clone(),
            spans: line.spans.clone(),
            wrapped: line.wrapped,
        })
        .collect();

    // Adjust cursor position for the reflow
    let cursor_col = grid.cursor_col.min(target_cols.saturating_sub(1));
    let cursor_row = grid.cursor_row.min(target_rows.saturating_sub(1));

    GridUpdate {
        cols: target_cols,
        rows: target_rows,
        cursor_col,
        cursor_row,
        cursor_visible: grid.cursor_visible,
        cursor_shape: grid.cursor_shape.clone(),
        lines: visible,
        full: true,
        mode: grid.mode,
        display_offset: start,
        selection: None,
        perf: None,
        // Reflowed grid inherits its source's seqno — if the source hasn't
        // changed, neither has the reflow, which is exactly the invariant
        // P2.3's reflow cache relies on for key equality.
        seqno: grid.seqno,
    }
}

/// Phase 1: Join soft-wrapped CompactLines into logical lines.
/// A line with `wrapped: true` means the next line is a continuation.
fn join_wrapped_lines(lines: &[CompactLine]) -> Vec<LogicalLine> {
    let mut logical_lines: Vec<LogicalLine> = Vec::new();
    let mut current: Option<LogicalLine> = None;

    for line in lines {
        if let Some(ref mut cur) = current {
            // Trim the previous row's trailing padding before appending.
            // Desktop terminals pad rows with spaces to fill the full width;
            // without trimming, padding ends up in the middle of the logical line.
            let trimmed_prev = cur.text.trim_end().len();
            if trimmed_prev < cur.text.len() {
                cur.text.truncate(trimmed_prev);
                // Clamp spans to trimmed length
                let trimmed_chars = cur.text.chars().count() as u16;
                cur.spans.retain(|s| s.s < trimmed_chars);
                for span in &mut cur.spans {
                    if span.e >= trimmed_chars { span.e = trimmed_chars.saturating_sub(1); }
                }
            }

            // Append this line to the current logical line
            let offset = cur.text.chars().count() as u16;
            cur.text.push_str(&line.text);
            // Shift span positions by the current text offset
            for span in &line.spans {
                cur.spans.push(StyleSpan {
                    s: span.s + offset,
                    e: span.e + offset,
                    fg: span.fg,
                    bg: span.bg,
                    fl: span.fl,
                });
            }

            if !line.wrapped {
                // End of logical line — flush
                logical_lines.push(current.take().unwrap());
            }
        } else {
            // Start a new logical line
            let logical = LogicalLine {
                text: line.text.clone(),
                spans: line.spans.clone(),
            };

            if line.wrapped {
                // This line continues — hold it
                current = Some(logical);
            } else {
                // Single-row logical line
                logical_lines.push(logical);
            }
        }
    }

    // Flush any remaining
    if let Some(cur) = current {
        logical_lines.push(cur);
    }

    // Trim trailing spaces from each logical line.
    // Desktop terminals pad rows with spaces to fill the full width (e.g. status bars,
    // prompt lines). Without trimming, a 120-col padded line wraps at 50 cols even
    // though the actual content is much shorter.
    for line in &mut logical_lines {
        let trimmed = line.text.trim_end().to_string();
        if trimmed.len() < line.text.len() {
            let trimmed_chars = trimmed.chars().count() as u16;
            // Remove spans that fall entirely in the trimmed region
            line.spans.retain(|span| span.s < trimmed_chars);
            // Clamp spans that extend past the trimmed end
            for span in &mut line.spans {
                if span.e >= trimmed_chars {
                    span.e = trimmed_chars.saturating_sub(1);
                }
            }
            line.text = trimmed;
        }
    }

    logical_lines
}

/// Phase 2: Wrap a logical line at the target column width.
/// Returns one or more CompactLines, with `wrapped: true` on all but the last.
fn wrap_logical_line(logical: &LogicalLine, target_cols: u16) -> Vec<CompactLine> {
    let chars: Vec<char> = logical.text.chars().collect();
    let target = target_cols as usize;

    if chars.len() <= target {
        // Fits in one line — no wrapping needed
        return vec![CompactLine {
            row: 0,
            text: logical.text.clone(),
            spans: logical.spans.clone(),
            wrapped: false,
        }];
    }

    let mut result: Vec<CompactLine> = Vec::new();
    let mut char_offset: usize = 0;

    while char_offset < chars.len() {
        let end = (char_offset + target).min(chars.len());
        let chunk_text: String = chars[char_offset..end].iter().collect();
        let is_last = end >= chars.len();

        // Extract spans that fall within this chunk, adjusting positions
        let chunk_start = char_offset as u16;
        let chunk_end = end as u16;
        let chunk_spans: Vec<StyleSpan> = logical.spans.iter()
            .filter_map(|span| {
                // Check if span overlaps with this chunk
                if span.e < chunk_start || span.s >= chunk_end {
                    return None; // No overlap
                }
                // Clamp span to chunk boundaries and shift to local positions
                let local_s = if span.s >= chunk_start { span.s - chunk_start } else { 0 };
                let local_e = if span.e < chunk_end { span.e - chunk_start } else { chunk_end - chunk_start - 1 };
                Some(StyleSpan {
                    s: local_s,
                    e: local_e,
                    fg: span.fg,
                    bg: span.bg,
                    fl: span.fl,
                })
            })
            .collect();

        result.push(CompactLine {
            row: 0, // will be reassigned by caller
            text: chunk_text.trim_end().to_string(),
            spans: chunk_spans,
            wrapped: !is_last,
        });

        char_offset = end;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(row: u16, text: &str, wrapped: bool) -> CompactLine {
        CompactLine { row, text: text.to_string(), spans: vec![], wrapped }
    }

    fn colored_line(row: u16, text: &str, wrapped: bool, fg: u32) -> CompactLine {
        CompactLine {
            row,
            text: text.to_string(),
            spans: vec![StyleSpan { s: 0, e: text.len() as u16 - 1, fg: Some(fg), bg: None, fl: None }],
            wrapped,
        }
    }

    #[test]
    fn test_no_reflow_needed() {
        // Line fits within target width
        let grid = GridUpdate {
            cols: 120, rows: 5,
            cursor_col: 0, cursor_row: 0,
            cursor_visible: true, cursor_shape: "block".into(),
            lines: vec![line(0, "hello world", false)],
            full: true, mode: 0, display_offset: 0,
            selection: None, perf: None, seqno: 1,
        };
        let result = reflow_grid(&grid, 50, 20);
        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.lines[0].text, "hello world");
        assert_eq!(result.cols, 50);
    }

    #[test]
    fn test_join_wrapped_lines() {
        // Two physical rows that are one logical line (wrapped at col 10)
        let lines = vec![
            line(0, "hello worl", true),  // wrapped
            line(1, "d!", false),          // end of logical line
        ];
        let logical = join_wrapped_lines(&lines);
        assert_eq!(logical.len(), 1);
        assert_eq!(logical[0].text, "hello world!");
    }

    #[test]
    fn test_rewrap_at_smaller_width() {
        // "hello world!" at width 5 → "hello", " worl", "d!"
        let logical = LogicalLine {
            text: "hello world!".to_string(),
            spans: vec![],
        };
        let result = wrap_logical_line(&logical, 5);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].text, "hello");
        assert!(result[0].wrapped);
        assert_eq!(result[1].text, " worl");
        assert!(result[1].wrapped);
        assert_eq!(result[2].text, "d!");
        assert!(!result[2].wrapped);
    }

    #[test]
    fn test_full_reflow_join_and_rewrap() {
        // Desktop: 10 cols, "abcdefghij" wraps to two rows
        // Mobile: 5 cols, should produce 4 rows
        let grid = GridUpdate {
            cols: 10, rows: 5,
            cursor_col: 0, cursor_row: 0,
            cursor_visible: true, cursor_shape: "block".into(),
            lines: vec![
                line(0, "abcdefghij", true),   // wrapped at col 10
                line(1, "klmno", false),        // end of logical line
                line(2, "xyz", false),           // separate logical line
            ],
            full: true, mode: 0, display_offset: 0,
            selection: None, perf: None, seqno: 1,
        };
        let result = reflow_grid(&grid, 5, 20);
        // "abcdefghijklmno" at width 5 → "abcde", "fghij", "klmno"
        // "xyz" stays as is
        assert_eq!(result.lines.len(), 4);
        assert_eq!(result.lines[0].text, "abcde");
        assert!(result.lines[0].wrapped);
        assert_eq!(result.lines[1].text, "fghij");
        assert!(result.lines[1].wrapped);
        assert_eq!(result.lines[2].text, "klmno");
        assert!(!result.lines[2].wrapped);
        assert_eq!(result.lines[3].text, "xyz");
        assert!(!result.lines[3].wrapped);
    }

    #[test]
    fn test_padded_prompt_line_trimmed() {
        // Desktop pads prompt lines with spaces to fill 120 cols.
        // "$ hello          " (padded to 120 chars) should NOT wrap at 50 cols.
        let padded = format!("{:<120}", "> bypass permissions on");
        let grid = GridUpdate {
            cols: 120, rows: 5,
            cursor_col: 0, cursor_row: 0,
            cursor_visible: true, cursor_shape: "block".into(),
            lines: vec![line(0, &padded, false)],
            full: true, mode: 0, display_offset: 0,
            selection: None, perf: None, seqno: 1,
        };
        let result = reflow_grid(&grid, 50, 20);
        // Should fit in one line after trimming (23 chars < 50 cols)
        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.lines[0].text, "> bypass permissions on");
        assert!(!result.lines[0].wrapped);
    }

    #[test]
    fn test_padded_wrapped_prompt_trimmed() {
        // Desktop wraps a padded prompt across two rows at 60 cols.
        // After joining + trimming, the content is ~30 chars and fits in one line at 50.
        let row1 = format!("{:<60}", "bypass permissions on -");
        let row2 = format!("{:<60}", " 1 monitor");
        let grid = GridUpdate {
            cols: 60, rows: 5,
            cursor_col: 0, cursor_row: 0,
            cursor_visible: true, cursor_shape: "block".into(),
            lines: vec![
                line(0, &row1, true),  // wrapped
                line(1, &row2, false), // end
            ],
            full: true, mode: 0, display_offset: 0,
            selection: None, perf: None, seqno: 1,
        };
        let result = reflow_grid(&grid, 50, 20);
        // Joined: "bypass permissions on - 1 monitor" (trimmed, ~33 chars)
        // Should fit in one line at 50 cols
        assert_eq!(result.lines.len(), 1);
        assert!(result.lines[0].text.contains("monitor"));
        assert!(!result.lines[0].wrapped);
    }

    #[test]
    fn test_color_spans_preserved_across_reflow() {
        // A colored line that wraps — spans should be split correctly
        let grid = GridUpdate {
            cols: 10, rows: 1,
            cursor_col: 0, cursor_row: 0,
            cursor_visible: true, cursor_shape: "block".into(),
            lines: vec![colored_line(0, "hello world!", false, 0xff0000)],
            full: true, mode: 0, display_offset: 0,
            selection: None, perf: None, seqno: 1,
        };
        let result = reflow_grid(&grid, 5, 20);
        // "hello world!" → "hello", " worl", "d!"
        assert_eq!(result.lines.len(), 3);
        // Each chunk should have a span with the red color
        assert!(!result.lines[0].spans.is_empty());
        assert_eq!(result.lines[0].spans[0].fg, Some(0xff0000));
        assert!(!result.lines[1].spans.is_empty());
        assert!(!result.lines[2].spans.is_empty());
    }
}
