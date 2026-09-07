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
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::super::{
    common::{
        border_title_chrome_width, border_title_line, border_title_prefix_width, button_label, level_controls_width,
        render_horizontal_separator, render_scrollbar, scroll_panel_border, scroll_panel_border_type, spans_width,
        target_controls_width,
    },
    format::format_log_wall_time,
    theme::{
        emphasis_primary, muted, style_for_level, style_for_level_filter, style_for_log_match, style_for_target,
        style_for_tier_boundary,
    },
};
use crate::{
    events::TelemetryRecord,
    model::{LevelFilter, LogViewItem, Model, RetentionTier, ScrollFocus, TargetFilter},
    ui::Views,
};

pub(in crate::ui) fn render_logs(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    views.logs_area = area;
    let focused = model.scroll_focus == ScrollFocus::Logs;
    let scrollbar_focused = focused && model.log_scrollbar_focused;
    let title = border_title_line(log_title_spans(model), model.interaction_mode, focused);
    let toggle_label = button_label(log_toggle_label(model));
    let toggle = border_title_line(
        vec![Span::styled(toggle_label.clone(), emphasis_primary(model.interaction_mode))],
        model.interaction_mode,
        focused,
    );
    let block = Block::default()
        .title(title)
        .title_top(toggle.right_aligned())
        .borders(Borders::ALL)
        .border_style(scroll_panel_border(focused, model.interaction_mode))
        .border_type(scroll_panel_border_type(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    views.log_toggle = Rect {
        x: area.x
            + area.width.saturating_sub(toggle_label.len() as u16 + border_title_chrome_width() + 1)
            + border_title_prefix_width(),
        y: area.y,
        width: toggle_label.len() as u16,
        height: 1,
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    render_log_controls(frame, layout[0], model, views);
    render_horizontal_separator(frame, layout[1], model.interaction_mode, focused);

    let items = model.log_view();
    let body = layout[2];
    views.logs_body = body;
    if items.len() > body.height as usize && body.width > 0 && body.height > 0 {
        views.logs_scrollbar =
            Rect { x: body.x + body.width.saturating_sub(1), y: body.y, width: 1, height: body.height };
    }

    let window = log_window(items.len(), body.height, model.log_scroll);
    let lines =
        items[window.start..window.end].iter().map(|item| log_view_line(item, model, body.width)).collect::<Vec<_>>();
    let (paragraph, _, _) = log_paragraph(lines, body, window.scroll_from_bottom);
    frame.render_widget(paragraph, body);

    let total = items.len();
    let visible = body.height as usize;
    let max_position = total.saturating_sub(visible);
    let position = max_position.saturating_sub(model.log_scroll.min(max_position));
    render_scrollbar(frame, body, total, visible, position, model.interaction_mode, scrollbar_focused);
}

/// Ratatui [`Paragraph`] scroll is `u16`, and `area.height + scroll.y` must not overflow.
/// Keep the rendered window well below that even when lines wrap a few times.
const MAX_LOG_WINDOW_ITEMS: usize = (u16::MAX as usize) / 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LogWindow {
    start: usize,
    end: usize,
    scroll_from_bottom: usize,
}

fn log_window(item_count: usize, height: u16, scroll_from_bottom: usize) -> LogWindow {
    let height = height as usize;
    if item_count == 0 || height == 0 {
        return LogWindow { start: 0, end: 0, scroll_from_bottom: 0 };
    }

    let take = scroll_from_bottom.saturating_add(height).min(item_count);
    if take <= MAX_LOG_WINDOW_ITEMS {
        return LogWindow { start: item_count - take, end: item_count, scroll_from_bottom };
    }

    let skip = scroll_from_bottom.min(item_count);
    let end = item_count - skip;
    let start = end.saturating_sub(height.min(MAX_LOG_WINDOW_ITEMS));
    LogWindow { start, end, scroll_from_bottom: 0 }
}

fn paragraph_vertical_scroll(position: usize, area_height: u16) -> u16 {
    let max_scroll = u16::MAX.saturating_sub(area_height);
    u16::try_from(position).unwrap_or(u16::MAX).min(max_scroll)
}

fn log_paragraph(
    lines: Vec<Line<'static>>,
    area: Rect,
    scroll_from_bottom: usize,
) -> (Paragraph<'static>, usize, usize) {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(area.width);
    let position = total.saturating_sub(area.height as usize).saturating_sub(scroll_from_bottom);
    let vertical_scroll = paragraph_vertical_scroll(position, area.height);

    (paragraph.scroll((vertical_scroll, 0)), total, position)
}

fn render_log_controls(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    let level_width = level_controls_width();
    let target_width = target_controls_width();
    let middle_width = area.width.saturating_sub(level_width).saturating_sub(target_width);
    let occupancy = occupancy_text(model.log_occupancy(), middle_width);
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(level_width), Constraint::Min(0), Constraint::Length(target_width)])
        .split(area);

    let level_spans = LevelFilter::ALL
        .into_iter()
        .map(|filter| {
            let style = if filter == model.level_filter {
                style_for_level_filter(filter).add_modifier(Modifier::BOLD)
            } else {
                muted()
            };
            views.level_tabs.push((filter, Rect::default()));
            Span::styled(button_label(filter.label()), style)
        })
        .collect::<Vec<_>>();
    let levels = Paragraph::new(Line::from(level_spans)).alignment(Alignment::Left);
    frame.render_widget(levels, layout[0]);

    if let Some(occupancy) = occupancy {
        let usage = Paragraph::new(Line::from(Span::styled(occupancy, muted()))).alignment(Alignment::Center);
        frame.render_widget(usage, layout[1]);
    }

    let target_spans = TargetFilter::ALL
        .into_iter()
        .map(|filter| {
            let style = if filter == model.target_filter { emphasis_primary(model.interaction_mode) } else { muted() };
            views.target_tabs.push((filter, Rect::default()));
            Span::styled(button_label(filter.label()), style)
        })
        .collect::<Vec<_>>();
    let targets = Paragraph::new(Line::from(target_spans)).alignment(Alignment::Right);
    frame.render_widget(targets, layout[2]);

    let level_labels = LevelFilter::ALL.into_iter().map(|filter| button_label(filter.label())).collect::<Vec<_>>();
    let mut level_x = layout[0].x;
    for ((_, rect), label) in views.level_tabs.iter_mut().zip(level_labels.iter()) {
        *rect = Rect { x: level_x, y: layout[0].y, width: label.len() as u16, height: layout[0].height };
        level_x += label.len() as u16;
    }

    let target_labels = TargetFilter::ALL.into_iter().map(|filter| button_label(filter.label())).collect::<Vec<_>>();
    let mut target_x =
        layout[2].x + layout[2].width.saturating_sub(spans_width(target_labels.iter().map(|label| label.len() as u16)));
    for ((_, rect), label) in views.target_tabs.iter_mut().zip(target_labels.iter()) {
        *rect = Rect { x: target_x, y: layout[2].y, width: label.len() as u16, height: layout[2].height };
        target_x += label.len() as u16;
    }
}

const OCCUPANCY_PADDING: u16 = 4;

fn occupancy_percent(used: usize, budget: usize) -> u8 {
    let percent = used.saturating_mul(100).checked_div(budget).unwrap_or(0).min(100);
    u8::try_from(percent).unwrap_or(100)
}

fn occupancy_text(occupancy: [(RetentionTier, usize, usize); 4], available: u16) -> Option<String> {
    let percents = occupancy.map(|(tier, used, budget)| (tier, occupancy_percent(used, budget)));
    let labeled =
        percents.iter().map(|(tier, percent)| format!("{} {percent}%", tier.label())).collect::<Vec<_>>().join("  ");
    if labeled.len() as u16 + OCCUPANCY_PADDING <= available {
        return Some(labeled);
    }

    let mut compact = String::from("log %:");
    for (idx, (_, p)) in percents.iter().enumerate() {
        use std::fmt::Write;
        compact.push(if idx == 0 { ' ' } else { ',' });
        let _ = write!(&mut compact, "{p}");
    }
    (compact.len() as u16 + OCCUPANCY_PADDING <= available).then_some(compact)
}

fn log_toggle_label(model: &Model) -> &'static str {
    if model.log_pane_mode.is_maximized() { "-" } else { "+" }
}

fn log_title_spans(model: &Model) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled("Logs", emphasis_primary(model.interaction_mode))];
    if model.log_scrollbar_focused {
        spans.push(Span::styled("  scrub", emphasis_primary(model.interaction_mode)));
    }
    if !model.text_filter_pattern.is_empty() {
        spans.push(Span::styled(format!("  &{}", truncate_pattern(&model.text_filter_pattern)), muted()));
    }
    if !model.highlight_pattern.is_empty() {
        spans.push(Span::styled(format!("  /{}", truncate_pattern(&model.highlight_pattern)), muted()));
    }
    spans
}

fn truncate_pattern(pattern: &str) -> String {
    const MAX: usize = 24;
    if pattern.chars().count() <= MAX {
        pattern.to_string()
    } else {
        let truncated: String = pattern.chars().take(MAX.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

fn log_view_line(item: &LogViewItem, model: &Model, width: u16) -> Line<'static> {
    match item {
        LogViewItem::Record { record, .. } => log_record_line(record.as_ref(), model),
        LogViewItem::TierBoundary { tier } => tier_boundary_line(*tier, width, model.interaction_mode),
    }
}

fn tier_boundary_line(tier: RetentionTier, width: u16, mode: crate::model::InteractionMode) -> Line<'static> {
    let label = format!(" end of {} retention ", tier.label());
    let width = width as usize;
    let line = if width <= label.len() {
        label
    } else {
        let pad = width.saturating_sub(label.len());
        let left = pad / 2;
        let right = pad - left;
        format!("{}{label}{}", "─".repeat(left), "─".repeat(right))
    };
    Line::from(Span::styled(line, style_for_tier_boundary(mode))).alignment(Alignment::Left)
}

fn log_record_line(record: &TelemetryRecord, model: &Model) -> Line<'static> {
    let fields = crate::model::render_fields(record);
    let label = record.log_label();
    let mut spans = vec![
        Span::styled(format_log_wall_time(record.wall_time), muted()),
        Span::raw(" "),
        Span::styled(format!("{:>5}", record.level), style_for_level(record.level).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(record.target.clone(), style_for_target(&record.target).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(label, super::super::theme::emphasis_white()),
    ];

    if !fields.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(fields, muted()));
    }

    let mut line = Line::from(spans);
    if model.log_record_is_highlighted(record) {
        line = line.patch_style(style_for_log_match(model.log_record_is_cursor(record), model.interaction_mode));
    }
    line
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, widgets::Widget};

    use super::*;

    fn buffer_row(buffer: &Buffer, y: u16, width: u16) -> String {
        (0..width).map(|x| buffer.cell((x, y)).map(|cell| cell.symbol()).unwrap_or("").to_string()).collect()
    }

    #[test]
    fn keeps_the_newest_log_visible_when_an_older_log_wraps() {
        let area = Rect::new(0, 0, 10, 2);
        let lines = vec![Line::from("old-entry old-entry"), Line::from("new-entry")];
        let (paragraph, total, position) = log_paragraph(lines, area, 0);
        let mut buffer = Buffer::empty(area);

        paragraph.render(area, &mut buffer);

        assert_eq!(total, 3);
        assert_eq!(position, 1);
        assert_eq!(buffer.cell((0, 1)).map(|cell| cell.symbol()), Some("n"));
    }

    #[test]
    fn log_window_keeps_a_short_suffix_from_the_tail() {
        assert_eq!(log_window(0, 2, 0), LogWindow { start: 0, end: 0, scroll_from_bottom: 0 });
        assert_eq!(log_window(3, 2, 0), LogWindow { start: 1, end: 3, scroll_from_bottom: 0 });
        assert_eq!(log_window(10, 2, 3), LogWindow { start: 5, end: 10, scroll_from_bottom: 3 });
    }

    #[test]
    fn log_window_follows_the_tail_of_a_buffer_larger_than_u16_scroll() {
        let window = log_window(70_000, 2, 0);
        assert_eq!(window, LogWindow { start: 69_998, end: 70_000, scroll_from_bottom: 0 });
    }

    #[test]
    fn log_window_skips_newest_items_when_scroll_exceeds_the_safe_paragraph() {
        let scroll = MAX_LOG_WINDOW_ITEMS + 50;
        let window = log_window(70_000, 2, scroll);
        assert_eq!(window.end, 70_000 - scroll);
        assert_eq!(window.start, window.end - 2);
        assert_eq!(window.scroll_from_bottom, 0);
    }

    #[test]
    fn rendering_a_huge_log_view_shows_the_newest_lines_without_panicking() {
        let area = Rect::new(0, 0, 20, 2);
        let window = log_window(70_000, area.height, 0);
        let lines = (window.start..window.end).map(|index| Line::from(format!("line-{index}"))).collect();
        let (paragraph, total, position) = log_paragraph(lines, area, window.scroll_from_bottom);
        let mut buffer = Buffer::empty(area);

        paragraph.render(area, &mut buffer);

        assert_eq!(total, 2);
        assert_eq!(position, 0);
        assert!(buffer_row(&buffer, 1, area.width).starts_with("line-69999"), "{}", buffer_row(&buffer, 1, area.width));
    }

    #[test]
    fn paragraph_scroll_offset_fits_u16_even_for_huge_positions() {
        assert_eq!(paragraph_vertical_scroll(10, 20), 10);
        assert_eq!(paragraph_vertical_scroll(usize::MAX, 20), u16::MAX - 20);
        assert_eq!(paragraph_vertical_scroll(u16::MAX as usize, 20), u16::MAX - 20);
    }

    #[test]
    fn occupancy_text_picks_labeled_compact_or_none_from_available_width() {
        let occupancy = [
            (RetentionTier::DebugAndUp, 70, 100),
            (RetentionTier::InfoAndUp, 0, 100),
            (RetentionTier::WarnAndUp, 10, 100),
            (RetentionTier::Error, 100, 100),
        ];
        let labeled = occupancy_text(occupancy, 80).expect("labeled occupancy");
        assert!(labeled.contains("debug+ 70%"), "{labeled}");
        assert!(labeled.contains("error 100%"), "{labeled}");

        let compact = occupancy_text(occupancy, labeled.len() as u16).expect("compact occupancy");
        assert_eq!(compact, "log %: 70,0,10,100");
        assert!(occupancy_text(occupancy, 8).is_none());
    }
}
