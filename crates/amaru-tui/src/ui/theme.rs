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
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use tracing::Level;

use crate::model::LevelFilter;

pub(super) fn accent_primary() -> Color {
    Color::Rgb(110, 228, 150)
}

pub(super) fn block_title(title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("─ ", emphasis_primary()),
        Span::styled(title.to_string(), emphasis_primary()),
        Span::styled(" ─", emphasis_primary()),
    ])
}

pub(super) fn border_primary() -> Style {
    Style::default().fg(Color::Rgb(80, 156, 105))
}

pub(super) fn border_secondary() -> Style {
    Style::default().fg(Color::Rgb(57, 108, 75))
}

pub(super) fn emphasis_primary() -> Style {
    Style::default().fg(accent_primary()).add_modifier(Modifier::BOLD)
}

pub(super) fn emphasis_white() -> Style {
    Style::default().fg(emphasis_white_color()).add_modifier(Modifier::BOLD)
}

pub(super) fn emphasis_white_color() -> Color {
    Color::Rgb(235, 242, 248)
}

pub(super) fn label_style() -> Style {
    Style::default().fg(Color::Rgb(150, 170, 190)).add_modifier(Modifier::BOLD)
}

pub(super) fn muted() -> Style {
    Style::default().fg(muted_color())
}

pub(super) fn muted_color() -> Color {
    Color::Rgb(145, 160, 180)
}

pub(super) fn striped_row_style(index: usize) -> Style {
    let bg = if index.is_multiple_of(2) { Color::Rgb(8, 17, 14) } else { Color::Rgb(12, 24, 19) };
    Style::default().bg(bg)
}

pub(super) fn style_for_level(level: Level) -> Style {
    match level {
        Level::ERROR => Style::default().fg(Color::Rgb(244, 86, 86)),
        Level::WARN => Style::default().fg(Color::Rgb(242, 196, 72)),
        Level::INFO => Style::default().fg(Color::Rgb(96, 171, 255)),
        Level::DEBUG => Style::default().fg(Color::Rgb(184, 122, 255)),
        Level::TRACE => Style::default().fg(Color::Rgb(135, 145, 165)),
    }
}

pub(super) fn style_for_level_filter(filter: LevelFilter) -> Style {
    match filter {
        LevelFilter::All => emphasis_white(),
        LevelFilter::Error => style_for_level(Level::ERROR),
        LevelFilter::Warn => style_for_level(Level::WARN),
        LevelFilter::Info => style_for_level(Level::INFO),
        LevelFilter::Debug => style_for_level(Level::DEBUG),
    }
}

pub(super) fn style_for_target(_target: &str) -> Style {
    Style::default().fg(muted_color())
}

pub(super) fn table_header_style() -> Style {
    Style::default().fg(Color::Rgb(246, 250, 247)).bg(Color::Rgb(22, 48, 33)).add_modifier(Modifier::BOLD)
}
