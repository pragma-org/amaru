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
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Gauge, Paragraph},
};

use super::super::theme::{accent_primary, block_title, border_primary, border_secondary, muted};
use crate::{events::SystemSample, model::InteractionMode};

pub(in crate::ui) fn render_memory_card(
    frame: &mut Frame<'_>,
    area: Rect,
    sample: Option<&SystemSample>,
    mode: InteractionMode,
) {
    let title = memory_title(sample);
    let card =
        Block::default().title(block_title(mode, &title)).borders(Borders::ALL).border_style(border_secondary(mode));
    let inner = card.inner(area);
    frame.render_widget(card, area);

    if inner.height == 0 {
        return;
    }

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)]).split(inner);
    render_memory_row(
        frame,
        rows[0],
        "Footprint",
        sample.map(|sample| sample.process_memory_bytes),
        sample,
        mode,
        false,
    );
    render_memory_row(frame, rows[2], "RSS", sample.map(|sample| sample.rss_bytes), sample, mode, true);
}

fn render_memory_row(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    current_bytes: Option<u64>,
    sample: Option<&SystemSample>,
    mode: InteractionMode,
    dimmed: bool,
) {
    if area.height == 0 {
        return;
    }

    let current_bytes = current_bytes.unwrap_or(0);
    let total_bytes = sample.map_or(0, |sample| sample.memory_total_bytes);
    let ratio = memory_ratio(current_bytes, total_bytes);
    let value = match sample {
        Some(_) => format!("{} / {} MiB", bytes_to_mib(current_bytes), bytes_to_mib(total_bytes)),
        None => "—".into(),
    };
    let [label_area, gauge_area] = Layout::horizontal([Constraint::Length(11), Constraint::Fill(1)]).areas(area);

    frame.render_widget(
        Paragraph::new(label).style(if dimmed { muted() } else { Style::default() }).alignment(Alignment::Left),
        label_area,
    );

    let gauge_style = if dimmed {
        border_primary(mode).bg(Color::Rgb(10, 22, 17))
    } else {
        Style::default().fg(accent_primary(mode)).bg(Color::Rgb(10, 22, 17))
    };

    let gauge = Gauge::default()
        .gauge_style(gauge_style)
        .label(Span::styled(value, if dimmed { muted() } else { Style::default() }))
        .ratio(ratio)
        .use_unicode(true);
    frame.render_widget(gauge, gauge_area);
}

fn memory_ratio(current_bytes: u64, total_bytes: u64) -> f64 {
    if total_bytes == 0 { 0.0 } else { (current_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0) }
}

fn bytes_to_mib(bytes: u64) -> String {
    crate::ui::format::format_count(bytes.div_ceil(1_048_576))
}

fn memory_title(sample: Option<&SystemSample>) -> String {
    let Some(sample) = sample else {
        return "Memory".into();
    };

    format!(
        "Memory ({:.1}%, {:.1}%)",
        memory_ratio(sample.process_memory_bytes, sample.memory_total_bytes) * 100.0,
        memory_ratio(sample.rss_bytes, sample.memory_total_bytes) * 100.0,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn memory_ratio_keeps_bounds() {
        assert_eq!(memory_ratio(0, 1_000), 0.0);
        assert_eq!(memory_ratio(1_000, 1_000), 1.0);
        assert_eq!(memory_ratio(42, 300), 0.14);
    }

    #[test]
    fn bytes_to_mib_formats_counts() {
        let sample = SystemSample {
            at: Instant::now(),
            cpu_percent: 0.0,
            process_memory_bytes: 512 * 1_048_576,
            rss_bytes: 384 * 1_048_576,
            virtual_bytes: 0,
            memory_used_bytes: 0,
            memory_total_bytes: 2 * 1_024 * 1_048_576,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
            disk_live_read_bytes: 0,
            disk_live_write_bytes: 0,
            other_processes_live_read_bytes: 0,
            other_processes_live_write_bytes: 0,
        };

        assert_eq!(bytes_to_mib(sample.process_memory_bytes), "512");
        assert_eq!(bytes_to_mib(sample.rss_bytes), "384");
        assert_eq!(bytes_to_mib(sample.memory_total_bytes), "2,048");
    }
}
