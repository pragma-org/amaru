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

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
};

use super::theme::{border_primary, border_secondary, emphasis_primary, emphasis_white_color};
use crate::model::{InteractionMode, LevelFilter};

pub(super) fn button_label(label: &str) -> String {
    format!("[ {} ]", label.to_uppercase())
}

pub(super) fn border_title_line(spans: Vec<Span<'static>>, mode: InteractionMode, focused: bool) -> Line<'static> {
    let style = scroll_panel_border(focused, mode);
    let horizontal = if focused { "═" } else { "─" };
    let mut line = Vec::with_capacity(spans.len() + 2);
    line.push(Span::styled(format!("{horizontal} "), style));
    line.extend(spans);
    line.push(Span::styled(format!(" {horizontal}"), style));
    Line::from(line)
}

pub(super) fn panel_title(mode: InteractionMode, focused: bool, title: &str) -> Line<'static> {
    let style = scroll_panel_border(focused, mode);
    let horizontal = if focused { "═" } else { "─" };
    Line::from(vec![
        Span::styled(format!("{horizontal} "), style),
        Span::styled(title.to_string(), emphasis_primary(mode)),
        Span::styled(format!(" {horizontal}"), style),
    ])
}

pub(super) fn render_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    total: usize,
    visible: usize,
    position: usize,
    mode: InteractionMode,
) {
    if total <= visible || visible == 0 {
        return;
    }

    let height = area.height as usize;
    if height == 0 || area.width == 0 {
        return;
    }

    let x = area.x + area.width.saturating_sub(1);
    let track_style = border_secondary(mode);
    let thumb_style = Style::default().fg(accent_primary(mode)).add_modifier(Modifier::BOLD);
    let thumb_height = height.min(2);
    let max_position = total.saturating_sub(visible);
    let max_offset = height.saturating_sub(thumb_height);
    let top_offset = if max_position == 0 { 0 } else { position.saturating_mul(max_offset) / max_position.max(1) };
    let buffer = frame.buffer_mut();

    for offset in 0..height {
        let symbol = if (top_offset..top_offset + thumb_height).contains(&offset) { "█" } else { "│" };
        let style = if symbol == "█" { thumb_style } else { track_style };
        buffer.set_string(x, area.y + offset as u16, symbol, style);
    }
}

pub(super) fn blit_buffer(frame: &mut Frame<'_>, area: Rect, source: &Buffer, scroll: usize) {
    let target = frame.buffer_mut();

    for y in 0..area.height {
        let source_y = y as usize + scroll;
        if source_y >= source.area.height as usize {
            break;
        }

        for x in 0..area.width {
            let Some(cell) = source.cell((x, source_y as u16)) else {
                continue;
            };
            let Some(target_cell) = target.cell_mut((area.x + x, area.y + y)) else {
                continue;
            };
            *target_cell = cell.clone();
        }
    }
}

pub(super) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);

    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn spans_width(lengths: impl Iterator<Item = u16>) -> u16 {
    lengths.sum()
}

pub(super) fn border_title_prefix_width() -> u16 {
    2
}

pub(super) fn border_title_chrome_width() -> u16 {
    4
}

pub(super) fn show_config_env_column(area: Rect) -> bool {
    area.width >= 140
}

pub(super) fn level_controls_width() -> u16 {
    spans_width(LevelFilter::ALL.into_iter().map(|filter| button_label(filter.label()).len() as u16))
}

pub(super) fn table_body_area(inner: Rect) -> Rect {
    Rect { x: inner.x, y: inner.y.saturating_add(1), width: inner.width, height: inner.height.saturating_sub(1) }
}

pub(super) fn scroll_panel_border(focused: bool, mode: InteractionMode) -> Style {
    if focused { emphasis_primary(mode) } else { border_primary(mode) }
}

pub(super) fn scroll_panel_border_type(focused: bool) -> BorderType {
    if focused { BorderType::Double } else { BorderType::Plain }
}

pub(super) fn render_horizontal_separator(frame: &mut Frame<'_>, area: Rect, mode: InteractionMode, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let horizontal = if focused { "═" } else { "─" }.repeat(area.width as usize);
    frame.render_widget(Paragraph::new(horizontal).style(border_secondary(mode)), area);
}

pub(super) fn format_vote_status(status: Option<bool>) -> &'static str {
    match status {
        Some(true) => "yes",
        Some(false) => "no",
        None => "-",
    }
}

pub(super) fn render_gradient_progress_bar(frame: &mut Frame<'_>, area: Rect, ratio: f64, label: &str) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let filled = ((ratio.clamp(0.0, 1.0) * area.width as f64).round() as usize).min(width);
    let label_chars = label.chars().collect::<Vec<_>>();
    let label_len = label_chars.len().min(width);
    let label_start = width.saturating_sub(label_len) / 2;
    let label_end = label_start + label_len;

    let spans = (0..width)
        .map(|index| {
            let is_filled = index < filled;
            let background = if is_filled {
                amaru_gradient_color(if width <= 1 { 0.0 } else { index as f32 / (width - 1) as f32 })
            } else {
                Color::Rgb(12, 22, 18)
            };

            if (label_start..label_end).contains(&index) {
                let glyph = label_chars[index - label_start];
                let foreground = if is_filled { Color::Rgb(8, 17, 14) } else { emphasis_white_color() };
                Span::styled(
                    glyph.to_string(),
                    Style::default().fg(foreground).bg(background).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(" ", Style::default().bg(background))
            }
        })
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn render_solid_progress_bar(
    frame: &mut Frame<'_>,
    area: Rect,
    ratio: f64,
    filled_color: Color,
    empty: Color,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let filled_width = ((ratio.clamp(0.0, 1.0) * area.width as f64).round() as usize).min(width);
    let line = Line::from(
        (0..width)
            .map(|index| {
                let background = if index < filled_width { filled_color } else { empty };
                Span::styled(" ", Style::default().bg(background))
            })
            .collect::<Vec<_>>(),
    );

    for row in 0..area.height {
        frame.render_widget(Paragraph::new(line.clone()), Rect { y: area.y + row, ..area });
    }
}

pub(super) fn accent_primary(mode: InteractionMode) -> Color {
    match mode {
        InteractionMode::Normal => Color::Rgb(110, 228, 150),
        InteractionMode::Copy => Color::Rgb(96, 171, 255),
        InteractionMode::Shutdown => Color::Rgb(180, 184, 192),
    }
}

pub(super) fn amaru_gradient_color(position: f32) -> Color {
    interpolate_gradient(
        [(0.0, (48_u8, 228_u8, 161_u8)), (0.55, (48_u8, 207_u8, 178_u8)), (1.0, (49_u8, 130_u8, 243_u8))],
        position,
    )
}

fn interpolate_gradient(stops: [(f32, (u8, u8, u8)); 3], position: f32) -> Color {
    let clamped = position.clamp(0.0, 1.0);

    for [(start_offset, start), (end_offset, end)] in stops.array_windows() {
        if clamped <= *end_offset {
            let span = (end_offset - start_offset).max(f32::EPSILON);
            let t = ((clamped - start_offset) / span).clamp(0.0, 1.0);
            return Color::Rgb(
                interpolate_channel(start.0, end.0, t),
                interpolate_channel(start.1, end.1, t),
                interpolate_channel(start.2, end.2, t),
            );
        }
    }

    let (_, (r, g, b)) = stops[stops.len() - 1];
    Color::Rgb(r, g, b)
}

fn interpolate_channel(start: u8, end: u8, t: f32) -> u8 {
    (start as f32 + ((end as f32 - start as f32) * t)).round() as u8
}
