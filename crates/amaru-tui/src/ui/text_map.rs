// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Plain-text hit targets for the live dashboard.
//!
//! A drag reads the glyphs on screen. A double-click reads the source string,
//! so a clipped peer address still copies the whole `ip:port`.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Flex, Layout, Rect},
    text::Span,
};

use crate::model::TextPane;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextSpan {
    pub x: u16,
    pub y: u16,
    /// Column width, including padding. A drag may cover this whole box.
    pub width: u16,
    /// Display columns of `plain` that accept a double-click, starting at `x`.
    pub text_cols: u16,
    pub plain: String,
    /// Display columns of `plain` scrolled off the left of this visual row.
    pub skip_cols: u16,
    /// When set, a double-click copies this instead of the word under the pointer.
    pub token: Option<String>,
    /// Visual rows that share a record are one logical line.
    pub record: u32,
    pub pane: TextPane,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WordHit {
    pub text: String,
    pub key: String,
    pub cells: Vec<(u16, u16)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TextMap {
    spans: Vec<TextSpan>,
    excluded: Vec<Rect>,
    next_record: u32,
}

impl TextMap {
    pub(crate) fn clear(&mut self) {
        self.spans.clear();
        self.excluded.clear();
        self.next_record = 0;
    }

    pub(crate) fn next_record(&mut self) -> u32 {
        self.next_record = self.next_record.saturating_add(1);
        self.next_record
    }

    pub(crate) fn push(&mut self, span: TextSpan) {
        if span.width == 0 {
            return;
        }
        self.spans.push(span);
    }

    pub(crate) fn exclude(&mut self, rect: Rect) {
        if rect.width > 0 && rect.height > 0 {
            self.excluded.push(rect);
        }
    }

    pub(crate) fn pane_at(&self, x: u16, y: u16) -> Option<TextPane> {
        self.spans
            .iter()
            .find(|span| span.y == y && x >= span.x && x < span.x.saturating_add(span.width))
            .map(|span| span.pane)
    }

    /// Keep a drag inside the pane that owns `pane`, on a row that has text.
    pub(crate) fn clamp(&self, pane: TextPane, x: u16, y: u16) -> (u16, u16) {
        let spans = self.spans.iter().filter(|span| span.pane == pane).collect::<Vec<_>>();
        let Some(min_y) = spans.iter().map(|span| span.y).min() else {
            return (x, y);
        };
        let max_y = spans.iter().map(|span| span.y).max().unwrap_or(min_y);
        let y = y.clamp(min_y, max_y);
        let Some((start, end, _)) = row_extent(&spans, y) else {
            return (x, y);
        };
        (x.clamp(start, end), y)
    }

    /// Screen cell where a peer-address token was recorded.
    #[cfg(test)]
    pub(crate) fn token_origin(&self, token: &str) -> Option<(u16, u16)> {
        self.spans.iter().find(|span| span.token.as_deref() == Some(token)).map(|span| (span.x, span.y))
    }

    pub(crate) fn word_at(&self, x: u16, y: u16) -> Option<WordHit> {
        let span = self.spans.iter().find(|span| {
            span.y == y && x >= span.x && x < span.x.saturating_add(span.width) && x - span.x < span.text_cols
        })?;
        let local = x - span.x;
        if let Some(token) = &span.token {
            return Some(WordHit {
                text: token.clone(),
                key: format!("token:{}:{token}", span.record),
                cells: (0..span.text_cols).map(|col| (span.x.saturating_add(col), span.y)).collect(),
            });
        }
        let column = span.skip_cols.saturating_add(local);
        let byte = byte_at_column(&span.plain, column)?;
        let (start, end) = word_bounds(&span.plain, byte)?;
        let text = span.plain[start..end].to_string();
        let start_col = display_columns(&span.plain[..start]);
        let end_col = start_col.saturating_add(display_columns(&text));
        let mut cells = Vec::new();
        for other in self.spans.iter().filter(|other| other.record == span.record && other.plain == span.plain) {
            cells.extend(visible_cells(other, start_col, end_col));
        }
        Some(WordHit { key: format!("word:{}:{start}:{text}", span.record), text, cells })
    }
}

pub(crate) fn push_plain_cell(map: &mut TextMap, x: u16, y: u16, width: u16, plain: &str, pane: TextPane, record: u32) {
    map.push(TextSpan {
        x,
        y,
        width,
        text_cols: display_columns(plain).min(width),
        plain: plain.to_string(),
        skip_cols: 0,
        token: None,
        record,
        pane,
    });
}

/// The address token is the full `ip:port` even when the column clips it.
/// A candidate name after the address is its own word.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_address_cell(
    map: &mut TextMap,
    x: u16,
    y: u16,
    width: u16,
    address: &str,
    candidate: Option<&str>,
    pane: TextPane,
    record: u32,
) {
    if width == 0 || address.is_empty() {
        return;
    }
    let address_cols = display_columns(address);
    map.push(TextSpan {
        x,
        y,
        width,
        text_cols: address_cols.min(width),
        plain: address.to_string(),
        skip_cols: 0,
        token: Some(address.to_string()),
        record,
        pane,
    });
    let Some(candidate) = candidate.filter(|candidate| !candidate.is_empty()) else {
        return;
    };
    let start = address_cols.saturating_add(2);
    if start >= width {
        return;
    }
    let room = width - start;
    let candidate_cols = display_columns(candidate).min(room);
    if candidate_cols == 0 {
        return;
    }
    map.push(TextSpan {
        x: x.saturating_add(start),
        y,
        width: candidate_cols,
        text_cols: candidate_cols,
        plain: candidate.to_string(),
        skip_cols: 0,
        token: None,
        record,
        pane,
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_log_row(
    map: &mut TextMap,
    x: u16,
    y: u16,
    width: u16,
    plain: &str,
    skip_cols: u16,
    pane: TextPane,
    record: u32,
) {
    let remaining = display_columns(plain).saturating_sub(skip_cols);
    map.push(TextSpan {
        x,
        y,
        width,
        text_cols: remaining.min(width),
        plain: plain.to_string(),
        skip_cols,
        token: None,
        record,
        pane,
    });
}

/// Column `(x, width)` pairs inside `width`, matching a ratatui `Table` with one column of spacing.
pub(crate) fn column_offsets(width: u16, constraints: &[Constraint]) -> Vec<(u16, u16)> {
    if width == 0 || constraints.is_empty() {
        return Vec::new();
    }
    let area = Rect::new(0, 0, width, 1);
    let [_, columns] = Layout::horizontal([Constraint::Length(0), Constraint::Fill(0)]).areas(area);
    Layout::horizontal(constraints)
        .flex(Flex::Start)
        .spacing(1)
        .split(columns)
        .iter()
        .map(|rect| (rect.x, rect.width))
        .collect()
}

pub(crate) trait Glyphs {
    fn symbol_at(&self, x: u16, y: u16) -> &str;
}

impl Glyphs for Buffer {
    fn symbol_at(&self, x: u16, y: u16) -> &str {
        match self.cell((x, y)) {
            Some(cell) => cell.symbol(),
            None => "",
        }
    }
}

pub(crate) fn selection_cells(map: &TextMap, anchor: (u16, u16), current: (u16, u16)) -> Vec<(u16, u16)> {
    selected(map, anchor, current).into_iter().map(|cell| (cell.x, cell.y)).collect()
}

/// Visible glyphs covered by the drag. Wrapped rows of one record stay on one line.
/// Trailing spaces on each logical line are dropped.
pub(crate) fn drag_plain(map: &TextMap, glyphs: &impl Glyphs, anchor: (u16, u16), current: (u16, u16)) -> String {
    let mut lines: Vec<(u32, String)> = Vec::new();
    let mut skip_following = 0u16;
    for cell in selected(map, anchor, current) {
        if skip_following > 0 {
            skip_following -= 1;
            continue;
        }
        let symbol = glyphs.symbol_at(cell.x, cell.y);
        let width = display_columns(symbol);
        if symbol.is_empty() || width == 0 {
            continue;
        }
        if lines.last().is_none_or(|line| line.0 != cell.record) {
            lines.push((cell.record, String::new()));
        }
        if let Some(line) = lines.last_mut() {
            line.1.push_str(symbol);
        }
        skip_following = width.saturating_sub(1);
    }

    let mut text = String::new();
    for (_, line) in lines {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(trimmed);
    }
    text
}

struct SelectedCell {
    x: u16,
    y: u16,
    record: u32,
}

fn selected(map: &TextMap, anchor: (u16, u16), current: (u16, u16)) -> Vec<SelectedCell> {
    let Some(pane) = map.pane_at(anchor.0, anchor.1) else {
        return Vec::new();
    };
    let spans = map.spans.iter().filter(|span| span.pane == pane).collect::<Vec<_>>();
    let top = anchor.1.min(current.1);
    let bottom = anchor.1.max(current.1);
    let mut cells = Vec::new();
    for y in top..=bottom {
        let Some((row_start, row_end, record)) = row_extent(&spans, y) else {
            continue;
        };
        let (from, to) = x_range(y, top, bottom, anchor, current, row_start, row_end);
        if from > to {
            continue;
        }
        for x in from..=to {
            if map.excluded(x, y) {
                continue;
            }
            cells.push(SelectedCell { x, y, record });
        }
    }
    cells
}

fn row_extent(spans: &[&TextSpan], y: u16) -> Option<(u16, u16, u32)> {
    let mut on_row = spans.iter().filter(|span| span.y == y);
    let first = on_row.next()?;
    let mut start = first.x;
    let mut end = first.x.saturating_add(first.width);
    let record = first.record;
    for span in on_row {
        start = start.min(span.x);
        end = end.max(span.x.saturating_add(span.width));
    }
    if end <= start {
        return None;
    }
    Some((start, end - 1, record))
}

fn x_range(
    y: u16,
    top: u16,
    bottom: u16,
    anchor: (u16, u16),
    current: (u16, u16),
    row_start: u16,
    row_end: u16,
) -> (u16, u16) {
    let (from, to) = if anchor.1 == current.1 {
        (anchor.0.min(current.0), anchor.0.max(current.0))
    } else if y == top {
        let x = if anchor.1 < current.1 { anchor.0 } else { current.0 };
        (x, row_end)
    } else if y == bottom {
        let x = if anchor.1 < current.1 { current.0 } else { anchor.0 };
        (row_start, x)
    } else {
        (row_start, row_end)
    };
    (from.max(row_start), to.min(row_end))
}

impl TextMap {
    fn excluded(&self, x: u16, y: u16) -> bool {
        self.excluded.iter().any(|rect| {
            x >= rect.x
                && x < rect.x.saturating_add(rect.width)
                && y >= rect.y
                && y < rect.y.saturating_add(rect.height)
        })
    }
}

fn visible_cells(span: &TextSpan, start_col: u16, end_col: u16) -> Vec<(u16, u16)> {
    let span_start = span.skip_cols;
    let span_end = span.skip_cols.saturating_add(span.text_cols);
    let from = start_col.max(span_start);
    let to = end_col.min(span_end);
    (from..to).map(|col| (span.x.saturating_add(col - span_start), span.y)).collect()
}

fn display_columns(text: &str) -> u16 {
    u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX)
}

fn byte_at_column(text: &str, column: u16) -> Option<usize> {
    let mut col = 0u16;
    for (index, ch) in text.char_indices() {
        let width = char_columns(ch);
        if width == 0 {
            continue;
        }
        if column < col.saturating_add(width) {
            return Some(index);
        }
        col = col.saturating_add(width);
    }
    None
}

fn char_columns(ch: char) -> u16 {
    let mut buf = [0u8; 4];
    display_columns(ch.encode_utf8(&mut buf))
}

fn word_bounds(text: &str, byte_index: usize) -> Option<(usize, usize)> {
    let ch = text[byte_index..].chars().next()?;
    if ch.is_whitespace() {
        return None;
    }
    let mut start = byte_index;
    while start > 0 {
        let prev = text[..start].chars().next_back()?;
        if prev.is_whitespace() {
            break;
        }
        start -= prev.len_utf8();
    }
    let mut end = byte_index + ch.len_utf8();
    while end < text.len() {
        let next = text[end..].chars().next()?;
        if next.is_whitespace() {
            break;
        }
        end += next.len_utf8();
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct Grid(BTreeMap<(u16, u16), String>);

    impl Grid {
        fn paint(&mut self, y: u16, x: u16, text: &str) {
            for (offset, ch) in text.chars().enumerate() {
                self.0.insert((x + offset as u16, y), ch.to_string());
            }
        }
    }

    impl Glyphs for Grid {
        fn symbol_at(&self, x: u16, y: u16) -> &str {
            self.0.get(&(x, y)).map(String::as_str).unwrap_or("")
        }
    }

    fn address_map(column_width: u16) -> TextMap {
        let mut map = TextMap::default();
        let record = map.next_record();
        push_address_cell(&mut map, 4, 2, column_width, "1.2.3.4:3001", Some("relay"), TextPane::Peers, record);
        map
    }

    #[test]
    fn double_click_on_a_clipped_peer_address_copies_the_whole_value() {
        let map = address_map(8);
        let hit = map.word_at(4, 2).expect("address");
        assert_eq!(hit.text, "1.2.3.4:3001");
        assert_eq!(hit.cells.len(), 8);
        assert!(map.word_at(4 + 8, 2).is_none(), "padding past the clipped address is not the token");
    }

    #[test]
    fn double_click_on_the_candidate_does_not_copy_the_address() {
        let map = address_map(24);
        let candidate_x = 4 + display_columns("1.2.3.4:3001") + 2;
        let hit = map.word_at(candidate_x, 2).expect("candidate");
        assert_eq!(hit.text, "relay");
    }

    #[test]
    fn drag_across_a_peer_row_copies_plain_characters_without_the_border() {
        let mut map = TextMap::default();
        let record = map.next_record();
        push_plain_cell(&mut map, 2, 1, 4, "out+", TextPane::Peers, record);
        push_address_cell(&mut map, 7, 1, 14, "1.2.3.4:3001", None, TextPane::Peers, record);
        push_plain_cell(&mut map, 22, 1, 6, "3.2 ms", TextPane::Peers, record);
        map.exclude(Rect { x: 0, y: 1, width: 1, height: 1 });

        let mut grid = Grid::default();
        grid.paint(1, 0, "│");
        grid.paint(1, 2, "out+");
        grid.paint(1, 6, " ");
        grid.paint(1, 7, "1.2.3.4:3001  ");
        grid.paint(1, 21, " ");
        grid.paint(1, 22, "3.2 ms");
        let text = drag_plain(&map, &grid, (2, 1), (27, 1));
        assert_eq!(text, "out+ 1.2.3.4:3001   3.2 ms");
        assert!(!text.contains('│'));
    }

    #[test]
    fn drag_across_a_wrapped_log_does_not_break_the_record() {
        let mut map = TextMap::default();
        let record = map.next_record();
        let plain = "alpha beta gamma delta";
        push_log_row(&mut map, 0, 0, 12, plain, 0, TextPane::Logs, record);
        push_log_row(&mut map, 0, 1, 12, plain, 12, TextPane::Logs, record);
        let next = map.next_record();
        push_log_row(&mut map, 0, 2, 12, "next line", 0, TextPane::Logs, next);

        let mut grid = Grid::default();
        grid.paint(0, 0, "alpha beta g");
        grid.paint(1, 0, "amma delta   ");
        grid.paint(2, 0, "next line    ");
        let text = drag_plain(&map, &grid, (0, 0), (4, 2));
        assert_eq!(text, "alpha beta gamma delta\nnext");
    }

    #[test]
    fn double_click_a_wrapped_word_uses_the_source_line() {
        let mut map = TextMap::default();
        let record = map.next_record();
        let plain = "alpha beta gamma";
        push_log_row(&mut map, 0, 0, 8, plain, 0, TextPane::Logs, record);
        push_log_row(&mut map, 0, 1, 8, plain, 8, TextPane::Logs, record);
        let hit = map.word_at(0, 1).expect("the continuation still belongs to beta");
        assert_eq!(hit.text, "beta");
        assert!(hit.cells.contains(&(6, 0)), "the word includes the letters on the previous row: {:?}", hit.cells);
        assert!(hit.cells.contains(&(0, 1)), "the word includes the letters on this row: {:?}", hit.cells);
    }
}
