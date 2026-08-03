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
    style::Style,
    widgets::{Block, Borders, Sparkline},
};

use super::super::{
    common::accent_primary,
    format::format_count,
    theme::{block_title, border_secondary},
};
use crate::model::InteractionMode;

pub(in crate::ui) fn render_series_card(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    data: Vec<u64>,
    unit: &str,
    detail: Option<String>,
    mode: InteractionMode,
) {
    let latest = data.last().copied().unwrap_or_default();
    let max = data.iter().copied().max().unwrap_or(1);
    let detail = detail.map(|detail| format!(" ({detail})")).unwrap_or_default();
    let block = Block::default()
        .title(block_title(mode, &format!("{title} · {} {unit}{detail}", format_count(latest))))
        .borders(Borders::ALL)
        .border_style(border_secondary(mode));
    let sparkline =
        Sparkline::default().data(&data).style(Style::default().fg(accent_primary(mode))).max(max).block(block);
    frame.render_widget(sparkline, area);
}
