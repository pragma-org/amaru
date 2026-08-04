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
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Gauge},
};

use super::super::theme::{accent_primary, block_title, border_secondary};
use crate::model::InteractionMode;

pub(in crate::ui) fn render_gauge_card(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    label: String,
    ratio: f64,
    detail: Option<String>,
    mode: InteractionMode,
) {
    let title = detail.map_or_else(|| title.to_string(), |detail| format!("{title} · {detail}"));
    let gauge = Gauge::default()
        .block(
            Block::default()
                .title(block_title(mode, &title))
                .borders(Borders::ALL)
                .border_style(border_secondary(mode)),
        )
        .gauge_style(Style::default().fg(accent_primary(mode)).bg(Color::Rgb(10, 22, 17)))
        .label(Span::raw(label))
        .ratio(ratio.clamp(0.0, 1.0))
        .use_unicode(true);
    frame.render_widget(gauge, area);
}
