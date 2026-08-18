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
    },
    format::format_log_wall_time,
    theme::{emphasis_primary, muted, style_for_level, style_for_level_filter, style_for_target},
};
use crate::{
    events::TelemetryRecord,
    model::{LevelFilter, Model, ScrollFocus, TargetFilter},
    ui::Views,
};

pub(in crate::ui) fn render_logs(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    views.logs_area = area;
    let focused = model.scroll_focus == ScrollFocus::Logs;
    let title = border_title_line(
        vec![Span::styled("Logs", emphasis_primary(model.interaction_mode))],
        model.interaction_mode,
        focused,
    );
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

    let lines = model.filtered_logs().iter().map(|record| log_record_line(record.as_ref())).collect::<Vec<_>>();
    let (paragraph, total, position) = log_paragraph(lines, layout[2], model.log_scroll);
    frame.render_widget(paragraph, layout[2]);
    render_scrollbar(frame, layout[2], total, layout[2].height as usize, position, model.interaction_mode);
}

fn log_paragraph(
    lines: Vec<Line<'static>>,
    area: Rect,
    scroll_from_bottom: usize,
) -> (Paragraph<'static>, usize, usize) {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(area.width);
    let max_position = total.saturating_sub(area.height as usize);
    let position = max_position.saturating_sub(scroll_from_bottom.min(max_position));
    let vertical_scroll = u16::try_from(position).unwrap_or(u16::MAX);

    (paragraph.scroll((vertical_scroll, 0)), total, position)
}

fn render_log_controls(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    let level_width = level_controls_width();
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(level_width), Constraint::Length(1), Constraint::Min(48)])
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

fn log_toggle_label(model: &Model) -> &'static str {
    if model.log_pane_mode.is_maximized() { "-" } else { "+" }
}

fn log_record_line(record: &TelemetryRecord) -> Line<'static> {
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

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, widgets::Widget};

    use super::*;

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
}
