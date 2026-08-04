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

use std::time::SystemTime;

use ratatui::{
    Frame,
    layout::Rect,
    text::Span,
    widgets::{Block, Borders},
};

use super::super::{
    common::{border_title_line, render_gradient_progress_bar},
    format::{format_count, format_duration, format_ratio},
    theme::{border_primary, emphasis_primary, emphasis_white},
};
use crate::model::Model;

pub(in crate::ui) fn render_epoch_progress(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let Some(tip) = &model.tip else {
        return;
    };

    let epoch_length = model.startup.epoch_length.max(1);
    let slot_in_epoch = tip.slot_in_epoch.min(epoch_length);
    let ratio = slot_in_epoch as f64 / epoch_length as f64;
    let wall_time = SystemTime::now();
    let epoch_title = match model.network_epoch_at(wall_time) {
        Some(network_epoch) if network_epoch != tip.epoch => {
            let eta = model
                .sync_eta_at(wall_time)
                .map(|duration| format!(" (ETA {})", format_duration(duration)))
                .unwrap_or_default();
            format!("Epoch {} / {}{eta}", format_count(tip.epoch), format_count(network_epoch))
        }
        _ => format!("Epoch {}", format_count(tip.epoch)),
    };
    let block = Block::default()
        .title_top(
            border_title_line(
                vec![Span::styled(epoch_title, emphasis_primary(model.interaction_mode))],
                model.interaction_mode,
                false,
            )
            .left_aligned(),
        )
        .title_top(
            border_title_line(
                vec![Span::styled(format!("{:.1}%", ratio * 100.0), emphasis_white())],
                model.interaction_mode,
                false,
            )
            .right_aligned(),
        )
        .borders(Borders::ALL)
        .border_style(border_primary(model.interaction_mode));
    let inner = block.inner(area);

    frame.render_widget(block, area);
    render_gradient_progress_bar(frame, inner, ratio, &format_ratio(slot_in_epoch, epoch_length));
}
