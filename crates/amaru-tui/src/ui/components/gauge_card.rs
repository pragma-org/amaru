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
    style::Color,
    text::Span,
    widgets::{Block, Borders},
};

use super::super::{
    common::{border_title_line, render_solid_progress_bar},
    theme::{accent_primary, block_title, border_primary, emphasis_white},
};
use crate::model::InteractionMode;

pub(in crate::ui) fn render_gauge_card(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    value: Option<String>,
    ratio: f64,
    percent: Option<String>,
    mode: InteractionMode,
) {
    let title = value.map_or_else(|| title.to_string(), |value| format!("{title} · {value}"));
    let mut card =
        Block::default().title(block_title(mode, &title)).borders(Borders::ALL).border_style(border_primary(mode));
    if let Some(percent) = percent {
        card = card
            .title_top(border_title_line(vec![Span::styled(percent, emphasis_white())], mode, false).right_aligned());
    }
    let inner = card.inner(area);
    frame.render_widget(card, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }
    render_solid_progress_bar(frame, inner, ratio, accent_primary(mode), Color::Rgb(10, 22, 17));
}
