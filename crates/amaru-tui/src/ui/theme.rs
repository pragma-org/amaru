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

use crate::model::{InteractionMode, LevelFilter};

pub(super) fn accent_primary(mode: InteractionMode) -> Color {
    match mode {
        InteractionMode::Normal => Color::Rgb(110, 228, 150),
        InteractionMode::Copy => Color::Rgb(96, 171, 255),
    }
}

pub(super) fn block_title(mode: InteractionMode, title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("─ ", border_secondary(mode)),
        Span::styled(title.to_string(), emphasis_primary(mode)),
        Span::styled(" ─", border_secondary(mode)),
    ])
}

pub(super) fn border_primary(mode: InteractionMode) -> Style {
    match mode {
        InteractionMode::Normal => Style::default().fg(Color::Rgb(80, 156, 105)),
        InteractionMode::Copy => Style::default().fg(Color::Rgb(71, 126, 186)),
    }
}

pub(super) fn border_secondary(mode: InteractionMode) -> Style {
    match mode {
        InteractionMode::Normal => Style::default().fg(Color::Rgb(57, 108, 75)),
        InteractionMode::Copy => Style::default().fg(Color::Rgb(50, 92, 141)),
    }
}

pub(super) fn emphasis_primary(mode: InteractionMode) -> Style {
    Style::default().fg(accent_primary(mode)).add_modifier(Modifier::BOLD)
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
        LevelFilter::Debug => style_for_level(Level::DEBUG),
        LevelFilter::Info => style_for_level(Level::INFO),
        LevelFilter::Warn => style_for_level(Level::WARN),
        LevelFilter::Error => style_for_level(Level::ERROR),
    }
}

pub(super) fn style_for_target(_target: &str) -> Style {
    Style::default().fg(muted_color())
}

pub(super) fn table_header_style(mode: InteractionMode) -> Style {
    let background = match mode {
        InteractionMode::Normal => Color::Rgb(22, 48, 33),
        InteractionMode::Copy => Color::Rgb(19, 39, 68),
    };

    Style::default().fg(Color::Rgb(246, 250, 247)).bg(background).add_modifier(Modifier::BOLD)
}
